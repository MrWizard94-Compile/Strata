//! Deterministic procedural terrain, sparse edits, DDA selection, and greedy meshes.
//!
//! Generation is a pure function of the world seed and integer coordinates. Chunk
//! order and thread scheduling do not change a block. Each mesh samples a one-cell
//! halo so visibility and ambient occlusion agree across chunk borders. Trees and
//! ore veins are anchored on global grids, so a feature that crosses a chunk is
//! identical from either side. No generation cache is stored on `World`.

use crate::registry::{self, BlockId, RenderClass, WorldgenBlocks};
use std::{collections::HashMap, sync::Arc};

pub const CHUNK_SIZE: i32 = 32;
pub const SEA_LEVEL: i32 = 28;
/// Inclusive surface clamp. Terrain, including tree crowns, stays below the
/// y=192 benchmark fixtures.
pub const MIN_SURFACE: i32 = 12;
pub const MAX_SURFACE: i32 = 96;
/// Save-file terrain version. Generators 1 and 2 are pre-release and are
/// rejected so older files keep their bytes. Generator 2 had the biome model
/// without snow caps, clay beds, crystal clusters, or deep basalt.
pub const GENERATOR_VERSION: u32 = 3;
/// Cool grass wears snow from this surface height up.
///
/// On seed `20261021`, 231_361 columns (`x` and `z` in `-960..=960`, step 4)
/// have a 97th-percentile height of 41 and a maximum of 46. About 3.4% are
/// at least 41. A cutoff of 47 is above every column in that sample. The
/// height-46 columns found on a step-1 pass of the same square are warm
/// forest, so the cool-temperature gate still leaves them bare. The highest
/// columns that are only cool are height 45.
const HIGH_GROUND_SNOW: i32 = 41;
pub type ChunkKey = (i32, i32, i32);

const TREE_GRID: i32 = 9;
const TREE_CANOPY: i32 = 2;
const TREE_CROWN_ABOVE: i32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
    pub texture_layer: u32,
    /// Linear shader multiply. Identity white leaves the texture unchanged.
    pub tint: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// Runtime block id. Definitions live in the registry, not in this enum.
pub type Block = BlockId;

pub use registry::{
    AIR, BASALT, CLAY, COAL_ORE, CRYSTAL, DIAMOND_ORE, DIRT, GOLD_ORE, GRASS, IRON_ORE, LEAVES,
    SAND, SNOW, STONE, WATER, WOOD,
};

/// Climate region at a column. Transitions are continuous in height and dithered
/// in cover; this label is the dominant region after that blend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Biome {
    Plains,
    Forest,
    Desert,
    Ocean,
    Beach,
}

#[derive(Clone)]
pub struct World {
    seed: u64,
    edits: HashMap<[i32; 3], Block>,
    /// Builtin ids resolved once from names. Terrain reads these, not strings.
    worldgen: WorldgenBlocks,
    /// Parallel to [`ORE_RULES`], in the same order, copied from [`Self::worldgen`].
    ore_ids: [BlockId; 4],
    registry: Arc<registry::Registry>,
    /// Loaded name-to-number table. Saves append names; they do not reorder it.
    palette: Vec<String>,
    /// Original ids for blocks the frozen registry does not know.
    placeholders: Vec<String>,
    placeholder_ids: HashMap<String, BlockId>,
    /// Per-id solid, replaceable, render class, layer, and tint. Built once.
    /// Each mesh copies the flag byte onto the halo and the inner loops read that.
    mesh_lookup: MeshLookup,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub block: [i32; 3],
    /// Last empty cell; equal to `block` when the ray starts in a solid.
    pub previous: [i32; 3],
    /// Distance in blocks, independent of the supplied direction's magnitude.
    pub distance: f32,
}

#[derive(Clone, Copy)]
struct Column {
    height: i32,
    biome: Biome,
    /// Desert influence in `0.0..=1.0`, used to dither sand onto neighboring land.
    sand: f64,
    /// Climate sample already used for biome weights. Snow reads it; no second noise.
    temperature: f64,
}

#[derive(Clone, Copy)]
struct Tree {
    x: i32,
    z: i32,
    ground: i32,
    height: i32,
    /// Captured from [`WorldgenBlocks`] when the anchor is accepted.
    wood: BlockId,
    leaves: BlockId,
}

impl Tree {
    fn block(self, x: i32, y: i32, z: i32) -> Block {
        let dx = (i64::from(x) - i64::from(self.x)).abs();
        let dz = (i64::from(z) - i64::from(self.z)).abs();
        let dy = y - self.ground;
        if !(0..=i64::from(TREE_CANOPY)).contains(&dx.max(dz)) {
            return AIR;
        }
        if dx == 0 && dz == 0 && dy > 0 && dy <= self.height {
            return self.wood;
        }
        let crown = dy - self.height;
        if (-2..=TREE_CROWN_ABOVE).contains(&crown)
            && dx * dx + dz * dz + i64::from(crown * crown) <= 6
        {
            return self.leaves;
        }
        AIR
    }
}

struct OreRule {
    salt: u64,
    cell: i32,
    min_y: i32,
    max_y: i32,
    chance: u64,
    radius_sq: i32,
}

struct Cluster {
    salt: u64,
    cell: i32,
    min_y: i32,
    max_y: i32,
    chance: u64,
    radius_sq: i32,
}

const CRYSTAL_CLUSTER: Cluster = Cluster {
    salt: 0xC757,
    cell: 12,
    min_y: 2,
    max_y: 14,
    chance: 11,
    radius_sq: 2,
};

const BASALT_CLUSTER: Cluster = Cluster {
    salt: 0xBA5A,
    cell: 8,
    min_y: 1,
    max_y: 8,
    chance: 16,
    radius_sq: 3,
};

const ORE_RULES: [OreRule; 4] = [
    OreRule {
        salt: 0xD1A0,
        cell: 13,
        min_y: 1,
        max_y: 12,
        chance: 8,
        radius_sq: 1,
    },
    OreRule {
        salt: 0x60D0,
        cell: 11,
        min_y: 1,
        max_y: 22,
        chance: 14,
        radius_sq: 1,
    },
    OreRule {
        salt: 0x1A0,
        cell: 9,
        min_y: 1,
        max_y: 46,
        chance: 34,
        radius_sq: 2,
    },
    OreRule {
        salt: 0xC0A1,
        cell: 8,
        min_y: 6,
        max_y: 70,
        chance: 46,
        radius_sq: 2,
    },
];

/// Schema 1 numbers, in order. `strata:missing` is not part of a migrated palette.
fn legacy_palette() -> Vec<String> {
    [
        "strata:air",
        "strata:grass",
        "strata:dirt",
        "strata:stone",
        "strata:sand",
        "strata:water",
        "strata:wood",
        "strata:leaves",
        "strata:snow",
        "strata:clay",
        "strata:crystal",
        "strata:basalt",
        "strata:coal_ore",
        "strata:iron_ore",
        "strata:gold_ore",
        "strata:diamond_ore",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

impl World {
    pub fn new(seed: u64) -> Self {
        let registry = registry::registry_handle();
        let worldgen = WorldgenBlocks::resolve(&registry);
        let mesh_lookup = MeshLookup::from_registry(&registry);
        Self {
            seed,
            edits: HashMap::new(),
            ore_ids: [
                worldgen.diamond_ore,
                worldgen.gold_ore,
                worldgen.iron_ore,
                worldgen.coal_ore,
            ],
            worldgen,
            registry,
            palette: legacy_palette(),
            placeholders: Vec::new(),
            placeholder_ids: HashMap::new(),
            mesh_lookup,
        }
        .checked()
    }

    /// The resolved ids must match the builtin constants. A mismatch means the
    /// load order drifted from the schema-1 numbers. These checks stay on in
    /// release builds: schema 1 stores the same numbers. Registry data that a
    /// file can change (grass top, water tint, stone rotation, air's name key)
    /// is rejected by the loader instead of asserted here.
    fn checked(self) -> Self {
        assert_eq!(self.worldgen.air, AIR);
        assert_eq!(AIR.raw(), 0);
        assert_eq!(self.worldgen.grass, GRASS);
        assert_eq!(self.worldgen.dirt, DIRT);
        assert_eq!(self.worldgen.stone, STONE);
        assert_eq!(self.worldgen.sand, SAND);
        assert_eq!(self.worldgen.water, WATER);
        assert_eq!(self.worldgen.wood, WOOD);
        assert_eq!(self.worldgen.leaves, LEAVES);
        assert_eq!(self.worldgen.snow, SNOW);
        assert_eq!(self.worldgen.clay, CLAY);
        assert_eq!(self.worldgen.crystal, CRYSTAL);
        assert_eq!(self.worldgen.basalt, BASALT);
        assert_eq!(self.worldgen.coal_ore, COAL_ORE);
        assert_eq!(self.worldgen.iron_ore, IRON_ORE);
        assert_eq!(self.worldgen.gold_ore, GOLD_ORE);
        assert_eq!(self.worldgen.diamond_ore, DIAMOND_ORE);
        assert_eq!(DIAMOND_ORE.raw(), 15);
        assert_eq!(self.registry.get("strata:missing"), Some(registry::MISSING));
        assert_eq!(registry::MISSING.raw(), 16);
        assert_eq!(self.registry.name(STONE), "strata:stone");
        let grass_top = self
            .registry
            .texture_index("strata:block/grass_top")
            .expect("grass top tile");
        assert_eq!(crate::materials::layer_for(GRASS, [0, 1, 0]), grass_top);
        assert_eq!(WATER.render_class(), RenderClass::Fluid);
        assert_eq!(registry::render_class(LEAVES), RenderClass::Cutout);
        assert_eq!(self.registry.builtin_content_hash().len(), 64);
        assert!(
            self.registry
                .builtin_content_hash()
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
        self
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Block {
        self.edits
            .get(&[x, y, z])
            .copied()
            .unwrap_or_else(|| self.generated_block(x, y, z))
    }

    /// Restoring the generated block removes the override, keeping saves sparse.
    pub fn set_block(&mut self, position: [i32; 3], block: Block) {
        if self.generated_block(position[0], position[1], position[2]) == block {
            self.edits.remove(&position);
        } else {
            self.edits.insert(position, block);
        }
    }

    pub fn edits(&self) -> Vec<([i32; 3], u16)> {
        self.saved_palette_and_edits().1
    }

    /// Palette for the next schema 2 save, then edits as indexes into it.
    /// The stored palette is not reordered. Names that are not in it yet are appended.
    pub fn saved_palette_and_edits(&self) -> (Vec<String>, Vec<([i32; 3], u16)>) {
        let mut palette = self.palette.clone();
        let mut index_of: HashMap<String, u16> = palette
            .iter()
            .enumerate()
            .map(|(index, name)| (name.clone(), index as u16))
            .collect();
        let mut edits = Vec::with_capacity(self.edits.len());
        for (&position, &block) in &self.edits {
            let name = self.block_key(block).to_string();
            let index = if let Some(index) = index_of.get(&name) {
                *index
            } else {
                let index = u16::try_from(palette.len()).expect("palette stays within u16");
                palette.push(name.clone());
                index_of.insert(name, index);
                index
            };
            edits.push((position, index));
        }
        edits.sort_unstable_by_key(|&(position, _)| position);
        (palette, edits)
    }

    /// English label for a builtin, or the original id string for a placeholder.
    pub fn display_name(&self, id: Block) -> &str {
        self.placeholder_name(id)
            .unwrap_or_else(|| self.registry.display(id))
    }

    /// Namespaced id for a block, including a placeholder's original string.
    pub fn block_key(&self, id: Block) -> &str {
        self.placeholder_name(id)
            .unwrap_or_else(|| self.registry.name(id))
    }

    /// Replace the save palette and allocate placeholders for unknown names.
    /// `names` must already be valid and free of duplicates.
    pub fn install_palette(&mut self, names: Vec<String>) -> Result<Vec<Block>, String> {
        let mut ids = Vec::with_capacity(names.len());
        for name in &names {
            ids.push(self.intern_block(name)?);
        }
        self.palette = names;
        Ok(ids)
    }

    pub fn intern_block(&mut self, name: &str) -> Result<Block, String> {
        if let Some(id) = self.registry.get(name) {
            return Ok(id);
        }
        if let Some(id) = self.placeholder_ids.get(name) {
            return Ok(*id);
        }
        if !registry::is_block_id(name) {
            return Err(format!("{name}: block id does not match the id regex"));
        }
        let index = self
            .registry
            .builtin_count()
            .checked_add(self.placeholders.len())
            .filter(|index| *index < 65_535)
            .ok_or_else(|| "block registry refuses the 65536th block".to_string())?;
        let id = BlockId::from_index(u16::try_from(index).expect("index fits u16"));
        self.placeholders.push(name.to_string());
        self.placeholder_ids.insert(name.to_string(), id);
        Ok(id)
    }

    fn placeholder_name(&self, id: Block) -> Option<&str> {
        let index = id.index().checked_sub(self.registry.builtin_count())?;
        self.placeholders.get(index).map(String::as_str)
    }

    /// Highest base-terrain cell, excluding caves, trees, and player edits.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        self.column(x, z).height
    }

    /// Dominant climate label. The runtime samples terrain through `block_at`;
    /// this is the tested query for maps and biome checks.
    #[allow(dead_code)]
    pub fn biome_at(&self, x: i32, z: i32) -> Biome {
        self.column(x, z).biome
    }

    /// Interior blocks of one chunk, in `x + CHUNK_SIZE * (z + CHUNK_SIZE * y)` order.
    /// `None` when the chunk origin cannot be represented. Edits are included, matching
    /// `block_at`. Generation itself does not read or write shared mutable state.
    /// Meshing still samples a halo; this is the timed, order-independent chunk API.
    #[allow(dead_code)]
    pub fn generate_chunk(&self, key: ChunkKey) -> Option<Vec<Block>> {
        let origin = chunk_origin(key)?;
        let halo = self.sample_halo(origin);
        let mut blocks = Vec::with_capacity((CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize);
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    blocks.push(halo[halo_index([x, y, z])]);
                }
            }
        }
        Some(blocks)
    }

    fn column(&self, x: i32, z: i32) -> Column {
        let xf = f64::from(x);
        let zf = f64::from(z);
        let continental = fbm2(self.seed ^ 0xC0A7, xf / 220.0, zf / 220.0, 4);
        let temperature = fbm2(self.seed ^ 0x7E11, xf / 240.0, zf / 240.0, 3);
        let humidity = fbm2(self.seed ^ 0x111D, xf / 210.0, zf / 210.0, 3);
        let hills = fbm2(self.seed ^ 0x1111, xf / 46.0, zf / 46.0, 4);
        let roll = fbm2(self.seed ^ 0x4011, xf / 130.0, zf / 130.0, 3);
        let detail = fbm2(self.seed ^ 0x0D37, xf / 17.0, zf / 17.0, 2);
        let ocean = smoothstep(0.10, -0.28, continental);
        let shore = smoothstep(-0.34, -0.10, continental) * smoothstep(0.18, -0.02, continental);
        let desert_w = smoothstep(0.0, 0.24, temperature) * smoothstep(0.04, -0.24, humidity);
        let forest_w = smoothstep(-0.04, 0.28, humidity) * (1.0 - desert_w);
        let plains_w = (1.0 - desert_w - forest_w).max(0.0);

        let sea = f64::from(SEA_LEVEL);
        let ocean_h = sea - 11.0 + hills * 2.4 + detail * 0.5;
        let plains_h = sea + 9.0 + roll * 4.5 + hills * 7.0 + detail * 1.2;
        let forest_h = sea + 10.0 + roll * 4.0 + hills * 8.0 + detail * 1.4;
        let desert_h = sea + 7.0 + roll * 2.0 + hills * 2.6 + detail * 0.6;
        let land = 1.0 - ocean;
        let land_h = plains_h * plains_w + forest_h * forest_w + desert_h * desert_w;
        let mut height = ocean_h * ocean + land_h * land;
        let beach_h = sea + 1.0 + detail * 0.45;
        height = lerp(height, beach_h, shore);
        let height = round_half_away(height).clamp(MIN_SURFACE, MAX_SURFACE);

        let biome = if height < SEA_LEVEL {
            Biome::Ocean
        } else if shore > 0.35 && height <= SEA_LEVEL + 2 {
            Biome::Beach
        } else if desert_w >= 0.42 && desert_w >= forest_w {
            Biome::Desert
        } else if forest_w >= 0.46 {
            Biome::Forest
        } else {
            Biome::Plains
        };
        Column {
            height,
            biome,
            sand: desert_w.clamp(0.0, 1.0),
            temperature,
        }
    }

    /// One block of snow on cold grass, and on high ground that is merely cool.
    /// Sand, beaches, deserts, and ocean stay bare. The cap is applied after trees
    /// so a trunk still occupies the column.
    fn wears_snow(&self, x: i32, z: i32, column: Column) -> bool {
        match column.biome {
            Biome::Ocean | Biome::Beach | Biome::Desert => false,
            Biome::Plains | Biome::Forest => {
                if self.sandy(x, z, column) {
                    return false;
                }
                column.temperature < -0.16
                    || (column.temperature < 0.02 && column.height >= HIGH_GROUND_SNOW)
            }
        }
    }

    /// Clay patches in beach sand and shallow seabeds. Deep ocean and desert stay sand.
    fn clay_patch(&self, x: i32, z: i32, column: Column) -> bool {
        if !matches!(column.biome, Biome::Ocean | Biome::Beach) {
            return false;
        }
        if !(SEA_LEVEL - 3..=SEA_LEVEL + 2).contains(&column.height) {
            return false;
        }
        hash3(self.seed ^ 0xC1A7, x.div_euclid(5), 2, z.div_euclid(5)) % 100 < 28
    }

    /// One hash per cell. Clusters stay inside that cell, so a chunk border cannot
    /// change them and the deep path does not grow a neighbor search.
    fn cell_cluster(&self, at: [i32; 3], cluster: Cluster) -> bool {
        let [x, y, z] = at;
        if !(cluster.min_y..=cluster.max_y).contains(&y) {
            return false;
        }
        let cx = x.div_euclid(cluster.cell);
        let cy = y.div_euclid(cluster.cell);
        let cz = z.div_euclid(cluster.cell);
        let hash = hash3(self.seed ^ cluster.salt, cx, cy, cz);
        if hash % 100 >= cluster.chance {
            return false;
        }
        let span = u64::from(cluster.cell as u32);
        let ox = cx * cluster.cell + (hash % span) as i32;
        let oy = cy * cluster.cell + ((hash >> 8) % span) as i32;
        let oz = cz * cluster.cell + ((hash >> 16) % span) as i32;
        if !(cluster.min_y..=cluster.max_y).contains(&oy) {
            return false;
        }
        let dx = x - ox;
        let dy = y - oy;
        let dz = z - oz;
        dx * dx + dy * dy + dz * dz <= cluster.radius_sq
    }

    fn sandy(&self, x: i32, z: i32, column: Column) -> bool {
        match column.biome {
            Biome::Ocean | Biome::Beach | Biome::Desert => true,
            Biome::Plains | Biome::Forest => {
                if column.sand <= 0.08 {
                    false
                } else {
                    let roll = (hash3(self.seed ^ 0x5AAD, x, 1, z) % 1024) as f64 / 1024.0;
                    roll < column.sand
                }
            }
        }
    }

    fn carves(&self, x: i32, y: i32, z: i32, column: Column) -> bool {
        let seal = match column.biome {
            Biome::Ocean | Biome::Beach => 6,
            _ if column.height <= SEA_LEVEL + 3 => 6,
            _ => 4,
        };
        if y < 2 || y > column.height - seal {
            return false;
        }
        // Keep tunnels from climbing into the sea along a coast. Inland hills
        // may still open dry caves above this line.
        if y >= SEA_LEVEL - 3 && column.height < SEA_LEVEL + 12 {
            return false;
        }
        let xf = f64::from(x);
        let yf = f64::from(y);
        let zf = f64::from(z);
        let warp = noise3(self.seed ^ 0x0A11, xf / 31.0, yf / 27.0, zf / 31.0) * 5.0;
        let sheet = noise3(self.seed ^ 0xCA7E, (xf + warp) / 18.0, yf / 12.0, zf / 18.0);
        // The tube sample cannot open a cave once the sheet misses. Skipping it
        // keeps the same boolean result.
        if sheet.abs() >= 0.085 {
            return false;
        }
        let tube = noise3(
            self.seed ^ 0x71BE,
            xf / 18.0,
            (yf - warp) / 14.0,
            (zf + warp) / 18.0,
        );
        tube.abs() < 0.15
    }

    fn ore_at(&self, x: i32, y: i32, z: i32) -> Option<Block> {
        if !(1..=70).contains(&y) {
            return None;
        }
        for (rule_index, rule) in ORE_RULES.iter().enumerate() {
            if !(rule.min_y..=rule.max_y).contains(&y) {
                continue;
            }
            let min_cx = x.saturating_sub(1).div_euclid(rule.cell);
            let max_cx = x.saturating_add(1).div_euclid(rule.cell);
            let min_cy = y.saturating_sub(1).div_euclid(rule.cell);
            let max_cy = y.saturating_add(1).div_euclid(rule.cell);
            let min_cz = z.saturating_sub(1).div_euclid(rule.cell);
            let max_cz = z.saturating_add(1).div_euclid(rule.cell);
            for cz in min_cz..=max_cz {
                for cy in min_cy..=max_cy {
                    for cx in min_cx..=max_cx {
                        if self.vein_hits(rule, [cx, cy, cz], [x, y, z]) {
                            return Some(self.ore_ids[rule_index]);
                        }
                    }
                }
            }
        }
        None
    }

    fn vein_hits(&self, rule: &OreRule, cell: [i32; 3], at: [i32; 3]) -> bool {
        let [cx, cy, cz] = cell;
        let [x, y, z] = at;
        let hash = hash3(self.seed ^ rule.salt, cx, cy, cz);
        if hash % 100 >= rule.chance {
            return false;
        }
        let span = u64::from(rule.cell as u32);
        let ox = cx * rule.cell + (hash % span) as i32;
        let oy = cy * rule.cell + ((hash >> 8) % span) as i32;
        let oz = cz * rule.cell + ((hash >> 16) % span) as i32;
        if !(rule.min_y..=rule.max_y).contains(&oy) {
            return false;
        }
        let dx = x - ox;
        let dy = y - oy;
        let dz = z - oz;
        dx * dx + dy * dy + dz * dz <= rule.radius_sq
    }

    fn terrain_block(&self, x: i32, y: i32, z: i32, column: Column) -> Block {
        let blocks = self.worldgen;
        if y > column.height {
            return if y <= SEA_LEVEL {
                blocks.water
            } else {
                blocks.air
            };
        }
        if y < 1 {
            return blocks.basalt;
        }
        if self.carves(x, y, z, column) {
            return blocks.air;
        }
        let depth = column.height - y;
        let sandy = self.sandy(x, z, column);
        if depth == 0 {
            if sandy {
                if self.clay_patch(x, z, column) {
                    blocks.clay
                } else {
                    blocks.sand
                }
            } else {
                blocks.grass
            }
        } else if depth < 4 {
            if sandy {
                if depth == 1 && self.clay_patch(x, z, column) {
                    blocks.clay
                } else {
                    blocks.sand
                }
            } else {
                blocks.dirt
            }
        } else if let Some(ore) = self.ore_at(x, y, z) {
            ore
        } else if self.cell_cluster([x, y, z], CRYSTAL_CLUSTER) {
            blocks.crystal
        } else if self.cell_cluster([x, y, z], BASALT_CLUSTER) {
            blocks.basalt
        } else {
            blocks.stone
        }
    }

    fn tree(&self, grid_x: i32, grid_z: i32) -> Option<Tree> {
        let hash = hash3(self.seed ^ 0x7AEE, grid_x, 0, grid_z);
        let x = grid_x
            .checked_mul(TREE_GRID)?
            .checked_add(2 + (hash % 5) as i32)?;
        let z = grid_z
            .checked_mul(TREE_GRID)?
            .checked_add(2 + ((hash >> 8) % 5) as i32)?;
        let column = self.column(x, z);
        let chance = match column.biome {
            Biome::Forest => 68,
            Biome::Plains => 11,
            Biome::Desert | Biome::Ocean | Biome::Beach => return None,
        };
        if hash % 100 >= chance || self.sandy(x, z, column) {
            return None;
        }
        let east = self.column(x.saturating_add(2), z);
        let south = self.column(x, z.saturating_add(2));
        if (east.height - column.height).abs() > 2 || (south.height - column.height).abs() > 2 {
            return None;
        }
        Some(Tree {
            x,
            z,
            ground: column.height,
            height: 4 + ((hash >> 16) % 3) as i32,
            wood: self.worldgen.wood,
            leaves: self.worldgen.leaves,
        })
    }

    fn foliage_at(&self, x: i32, y: i32, z: i32) -> Block {
        let mut foliage = self.worldgen.air;
        let min_z = (z - TREE_CANOPY).div_euclid(TREE_GRID);
        let max_z = (z + TREE_CANOPY).div_euclid(TREE_GRID);
        let min_x = (x - TREE_CANOPY).div_euclid(TREE_GRID);
        let max_x = (x + TREE_CANOPY).div_euclid(TREE_GRID);
        for gz in min_z..=max_z {
            for gx in min_x..=max_x {
                if let Some(tree) = self.tree(gx, gz) {
                    let block = tree.block(x, y, z);
                    if block == self.worldgen.wood {
                        return block;
                    }
                    if block == self.worldgen.leaves {
                        foliage = block;
                    }
                }
            }
        }
        foliage
    }

    fn generated_block(&self, x: i32, y: i32, z: i32) -> Block {
        if y < 0 {
            return self.worldgen.basalt;
        }
        if y > 128 {
            return self.worldgen.air;
        }
        let column = self.column(x, z);
        let terrain = self.terrain_block(x, y, z, column);
        if y <= column.height || terrain != self.worldgen.air {
            return terrain;
        }
        let foliage = self.foliage_at(x, y, z);
        if foliage != self.worldgen.air {
            return foliage;
        }
        if y == column.height + 1 && y > SEA_LEVEL && self.wears_snow(x, z, column) {
            self.worldgen.snow
        } else {
            self.worldgen.air
        }
    }

    /// Mesh positions are world-space f32. Camera-relative large-world rendering
    /// is a later renderer contract; avoid claiming precision beyond 2^24 blocks.
    pub fn mesh_chunk(&self, key: ChunkKey) -> Mesh {
        let Some(origin) = chunk_origin(key) else {
            return Mesh::default();
        };
        let halo = self.sample_halo(origin);
        mesh_halo(origin, &halo, &self.mesh_lookup)
    }

    fn sample_halo(&self, origin: [i32; 3]) -> Vec<Block> {
        let mut blocks = vec![self.worldgen.air; HALO_VOLUME];
        let mut heights = vec![0; HALO_SIDE * HALO_SIDE];
        let mut snow_caps = vec![false; HALO_SIDE * HALO_SIDE];
        // Compute a column once, rather than running fBm for every voxel or face.
        for z in -1..=CHUNK_SIZE {
            for x in -1..=CHUNK_SIZE {
                let wx = origin[0] + x;
                let wz = origin[2] + z;
                let column = self.column(wx, wz);
                let column_index = (x + 1) as usize + (z + 1) as usize * HALO_SIDE;
                heights[column_index] = column.height;
                snow_caps[column_index] = self.wears_snow(wx, wz, column);
                for y in -1..=CHUNK_SIZE {
                    let wy = origin[1] + y;
                    blocks[halo_index([x, y, z])] = self.terrain_block(wx, wy, wz, column);
                }
            }
        }
        // Apply all trees that can intersect this chunk's halo. The same anchors
        // and trunk-before-leaf rule are used by point queries and meshing.
        let min_x = (origin[0] - TREE_CANOPY).div_euclid(TREE_GRID);
        let max_x = (origin[0] + CHUNK_SIZE + TREE_CANOPY).div_euclid(TREE_GRID);
        let min_z = (origin[2] - TREE_CANOPY).div_euclid(TREE_GRID);
        let max_z = (origin[2] + CHUNK_SIZE + TREE_CANOPY).div_euclid(TREE_GRID);
        for gz in min_z..=max_z {
            for gx in min_x..=max_x {
                if origin[1] > 128 || origin[1] + CHUNK_SIZE < 0 {
                    continue;
                }
                let Some(tree) = self.tree(gx, gz) else {
                    continue;
                };
                let start_y = (tree.ground + 1 - origin[1]).max(-1);
                let end_y =
                    (tree.ground + tree.height + TREE_CROWN_ABOVE - origin[1]).min(CHUNK_SIZE);
                for z in (tree.z - TREE_CANOPY - origin[2]).max(-1)
                    ..=(tree.z + TREE_CANOPY - origin[2]).min(CHUNK_SIZE)
                {
                    for x in (tree.x - TREE_CANOPY - origin[0]).max(-1)
                        ..=(tree.x + TREE_CANOPY - origin[0]).min(CHUNK_SIZE)
                    {
                        for y in start_y..=end_y {
                            if origin[1] + y
                                <= heights[(x + 1) as usize + (z + 1) as usize * HALO_SIDE]
                            {
                                continue;
                            }
                            let index = halo_index([x, y, z]);
                            let block = tree.block(origin[0] + x, origin[1] + y, origin[2] + z);
                            // Snow and leaves overwrite only replaceable cells (air).
                            // A trunk still replaces leaves so an overlapping crown
                            // cannot erase the stem. That is the pre-registry rule.
                            let existing = blocks[index];
                            let trunk_over_leaves =
                                existing == self.worldgen.leaves && block == self.worldgen.wood;
                            if self.mesh_lookup.replaceable(existing) || trunk_over_leaves {
                                blocks[index] = block;
                            }
                        }
                    }
                }
            }
        }
        for z in -1..=CHUNK_SIZE {
            for x in -1..=CHUNK_SIZE {
                let column_index = (x + 1) as usize + (z + 1) as usize * HALO_SIDE;
                if !snow_caps[column_index] {
                    continue;
                }
                let wy = heights[column_index] + 1;
                if wy <= SEA_LEVEL {
                    continue;
                }
                let y = wy - origin[1];
                if !(-1..=CHUNK_SIZE).contains(&y) {
                    continue;
                }
                let index = halo_index([x, y, z]);
                if self.mesh_lookup.replaceable(blocks[index]) {
                    blocks[index] = self.worldgen.snow;
                }
            }
        }
        for (&position, &block) in &self.edits {
            let local = [
                i64::from(position[0]) - i64::from(origin[0]),
                i64::from(position[1]) - i64::from(origin[1]),
                i64::from(position[2]) - i64::from(origin[2]),
            ];
            if local
                .iter()
                .all(|&v| (-1..=i64::from(CHUNK_SIZE)).contains(&v))
            {
                blocks[halo_index(local.map(|v| v as i32))] = block;
            }
        }
        blocks
    }

    /// Amanatides-Woo grid traversal. All tied axes advance together so a ray
    /// crossing an edge does not select a voxel it only touches at a point.
    pub fn raycast(
        &self,
        origin: [f32; 3],
        direction: [f32; 3],
        max_distance: f32,
    ) -> Option<RayHit> {
        if !origin.iter().chain(&direction).all(|v| v.is_finite())
            || !max_distance.is_finite()
            || max_distance < 0.0
        {
            return None;
        }
        let magnitude = direction
            .iter()
            .map(|&v| f64::from(v).powi(2))
            .sum::<f64>()
            .sqrt();
        if magnitude < f64::EPSILON {
            return None;
        }
        let direction = direction.map(|v| f64::from(v) / magnitude);
        let origin = origin.map(f64::from);
        if origin
            .iter()
            .any(|&v| v <= f64::from(i32::MIN) + 1.0 || v >= f64::from(i32::MAX) - 1.0)
        {
            return None;
        }
        let mut cell = origin.map(|v| v.floor() as i32);
        let mut previous = cell;
        let step = direction.map(|v| {
            if v > 0.0 {
                1
            } else if v < 0.0 {
                -1
            } else {
                0
            }
        });
        let delta = direction.map(|v| {
            if v == 0.0 {
                f64::INFINITY
            } else {
                1.0 / v.abs()
            }
        });
        let mut next = [0.0; 3];
        for axis in 0..3 {
            next[axis] = if step[axis] > 0 {
                (f64::from(cell[axis]) + 1.0 - origin[axis]) / direction[axis]
            } else if step[axis] < 0 {
                (f64::from(cell[axis]) - origin[axis]) / direction[axis]
            } else {
                f64::INFINITY
            };
        }
        let mut distance = 0.0;
        loop {
            if self.block_at(cell[0], cell[1], cell[2]).is_solid() {
                return Some(RayHit {
                    block: cell,
                    previous,
                    distance: distance as f32,
                });
            }
            let minimum = next[0].min(next[1]).min(next[2]);
            if minimum > f64::from(max_distance) {
                return None;
            }
            previous = cell;
            for axis in 0..3 {
                if (next[axis] - minimum).abs() < 1e-10 {
                    cell[axis] = cell[axis].checked_add(step[axis])?;
                    next[axis] += delta[axis];
                }
            }
            distance = minimum;
        }
    }
}

const HALO_SIDE: usize = CHUNK_SIZE as usize + 2;
const HALO_VOLUME: usize = HALO_SIDE * HALO_SIDE * HALO_SIDE;

fn chunk_origin(key: ChunkKey) -> Option<[i32; 3]> {
    let origin = [
        key.0.checked_mul(CHUNK_SIZE)?,
        key.1.checked_mul(CHUNK_SIZE)?,
        key.2.checked_mul(CHUNK_SIZE)?,
    ];
    // Reserve space for tree bounds as well as the meshing halo.
    if origin
        .iter()
        .any(|&v| !(i32::MIN + 8..=i32::MAX - CHUNK_SIZE - 8).contains(&v))
    {
        None
    } else {
        Some(origin)
    }
}

fn halo_index(p: [i32; 3]) -> usize {
    (p[0] + 1) as usize + HALO_SIDE * ((p[1] + 1) as usize + HALO_SIDE * (p[2] + 1) as usize)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Face {
    block: Block,
    ao: [u8; 4],
    variation: u8,
}

impl Face {
    fn can_merge(self) -> bool {
        self.ao.iter().all(|&value| value == self.ao[0])
    }
}

/// Dense per-id facts for one mesh. Bit 0 is solid. Bit 1 is replaceable.
/// Bits 2.. store [`RenderClass`].
#[derive(Clone)]
struct MeshLookup {
    flags: Vec<u8>,
    layers: Vec<u32>,
    tints: Vec<[f32; 4]>,
    fallback_flags: u8,
    fallback_layers: [u32; 6],
    fallback_tint: [f32; 4],
}

const FLAG_SOLID: u8 = 1 << 0;
const FLAG_REPLACEABLE: u8 = 1 << 1;

fn pack_flags(solid: bool, replaceable: bool, render: RenderClass) -> u8 {
    (u8::from(solid) * FLAG_SOLID)
        | (u8::from(replaceable) * FLAG_REPLACEABLE)
        | ((render as u8) << 2)
}

fn unpack_render(flags: u8) -> RenderClass {
    match flags >> 2 {
        0 => RenderClass::Opaque,
        1 => RenderClass::Cutout,
        2 => RenderClass::Fluid,
        _ => RenderClass::Invisible,
    }
}

impl MeshLookup {
    fn from_registry(registry: &registry::Registry) -> Self {
        let rows = registry.flag_rows();
        let mut flags = Vec::with_capacity(rows.len());
        let mut layers = Vec::with_capacity(rows.len() * 6);
        let mut tints = Vec::with_capacity(rows.len());
        for row in rows {
            flags.push(pack_flags(row.solid, row.replaceable, row.render));
            tints.push(row.tint);
            // `BlockFlags.layers` is the first variant of each face, written at freeze.
            layers.extend_from_slice(&row.layers);
        }
        let unknown = BlockId::from_index(u16::MAX);
        let placeholder = registry.flags(unknown);
        let fallback_layers = placeholder.layers;
        Self {
            flags,
            layers,
            tints,
            fallback_flags: pack_flags(
                placeholder.solid,
                placeholder.replaceable,
                placeholder.render,
            ),
            fallback_layers,
            fallback_tint: placeholder.tint,
        }
    }

    fn flags_of(&self, block: Block) -> u8 {
        self.flags
            .get(block.index())
            .copied()
            .unwrap_or(self.fallback_flags)
    }

    fn layer_of(&self, block: Block, normal: [i32; 3]) -> u32 {
        let face = registry::BlockFace::from_normal(normal).index();
        self.layers
            .get(block.index() * 6 + face)
            .copied()
            .unwrap_or(self.fallback_layers[face])
    }

    fn tint_of(&self, block: Block) -> [f32; 4] {
        self.tints
            .get(block.index())
            .copied()
            .unwrap_or(self.fallback_tint)
    }

    fn replaceable(&self, block: Block) -> bool {
        self.flags_of(block) & FLAG_REPLACEABLE == FLAG_REPLACEABLE
    }
}

/// Fluid emits only into invisible cells (air). Cutout is culled as opaque
/// until the cutout pipeline exists, so leaves keep today's solid faces.
/// `flags` is one byte per halo cell. The inner loops do not index by block id.
fn face_visible(flags: &[u8], cell: usize, neighbor: usize) -> bool {
    let block_flags = flags[cell];
    let neighbor_flags = flags[neighbor];
    match unpack_render(block_flags) {
        RenderClass::Invisible => false,
        RenderClass::Fluid => unpack_render(neighbor_flags) == RenderClass::Invisible,
        RenderClass::Cutout | RenderClass::Opaque => {
            block_flags & FLAG_SOLID == FLAG_SOLID && neighbor_flags & FLAG_SOLID == 0
        }
    }
}

fn halo_flags(halo: &[Block], tables: &MeshLookup) -> Vec<u8> {
    halo.iter().map(|block| tables.flags_of(*block)).collect()
}

fn mesh_halo(origin: [i32; 3], halo: &[Block], tables: &MeshLookup) -> Mesh {
    let cell_flags = halo_flags(halo, tables);
    let mut mesh = Mesh::default();
    let side = CHUNK_SIZE as usize;
    let mut mask = vec![None; side * side];
    for axis in 0..3 {
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        for sign in [-1, 1] {
            for slice in 0..CHUNK_SIZE {
                for j in 0..CHUNK_SIZE {
                    for i in 0..CHUNK_SIZE {
                        let mut p = [0; 3];
                        p[axis] = slice;
                        p[u] = i;
                        p[v] = j;
                        let mut outside = p;
                        outside[axis] += sign;
                        let cell = halo_index(p);
                        let visible = face_visible(&cell_flags, cell, halo_index(outside));
                        mask[i as usize + j as usize * side] = if visible {
                            let block = halo[cell];
                            let ao = corner_ao(&cell_flags, outside, u, v);
                            let world = [origin[0] + p[0], origin[1] + p[1], origin[2] + p[2]];
                            Some(Face {
                                block,
                                ao,
                                variation: (hash3(
                                    0xFC70,
                                    world[0].div_euclid(4),
                                    world[1].div_euclid(4),
                                    world[2].div_euclid(4),
                                ) % 7) as u8,
                            })
                        } else {
                            None
                        };
                    }
                }
                for j in 0..side {
                    let mut i = 0;
                    while i < side {
                        let Some(face) = mask[i + j * side] else {
                            i += 1;
                            continue;
                        };
                        let mut width = 1;
                        let mut height = 1;
                        if face.can_merge() {
                            while i + width < side && mask[i + width + j * side] == Some(face) {
                                width += 1;
                            }
                            'grow: while j + height < side {
                                for k in 0..width {
                                    if mask[i + k + (j + height) * side] != Some(face) {
                                        break 'grow;
                                    }
                                }
                                height += 1;
                            }
                        }
                        let mut p = origin;
                        p[axis] += slice + i32::from(sign > 0);
                        p[u] += i as i32;
                        p[v] += j as i32;
                        emit_quad(
                            &mut mesh,
                            tables,
                            p,
                            axis,
                            sign,
                            [width as f32, height as f32],
                            face,
                        );
                        for row in j..j + height {
                            for col in i..i + width {
                                mask[col + row * side] = None;
                            }
                        }
                        i += width;
                    }
                }
            }
        }
    }
    mesh
}

fn corner_ao(flags: &[u8], outside: [i32; 3], u: usize, v: usize) -> [u8; 4] {
    let mut ao = [3; 4];
    for (corner, (du, dv)) in [(-1, -1), (1, -1), (1, 1), (-1, 1)].into_iter().enumerate() {
        let mut side_u = outside;
        let mut side_v = outside;
        let mut diagonal = outside;
        side_u[u] += du;
        side_v[v] += dv;
        diagonal[u] += du;
        diagonal[v] += dv;
        let a = u8::from(flags[halo_index(side_u)] & FLAG_SOLID == FLAG_SOLID);
        let b = u8::from(flags[halo_index(side_v)] & FLAG_SOLID == FLAG_SOLID);
        let c = u8::from(flags[halo_index(diagonal)] & FLAG_SOLID == FLAG_SOLID);
        ao[corner] = if a == 1 && b == 1 { 0 } else { 3 - a - b - c };
    }
    ao
}

fn emit_quad(
    mesh: &mut Mesh,
    tables: &MeshLookup,
    p: [i32; 3],
    axis: usize,
    sign: i32,
    size: [f32; 2],
    face: Face,
) {
    let base = mesh.vertices.len() as u32;
    let u = (axis + 1) % 3;
    let v = (axis + 2) % 3;
    let mut normal = [0.0; 3];
    normal[axis] = sign as f32;
    let mut grid_normal = [0; 3];
    grid_normal[axis] = sign;
    let layer = tables.layer_of(face.block, grid_normal);
    let tint = tables.tint_of(face.block);
    let fluid = unpack_render(tables.flags_of(face.block)) == RenderClass::Fluid;
    let variation = 0.94 + f32::from(face.variation) * 0.02;
    let directional = if axis == 1 && sign < 0 {
        0.80
    } else if axis == 1 {
        1.0
    } else {
        0.94
    };
    for (corner, uv) in [[0.0, 0.0], [size[0], 0.0], size, [0.0, size[1]]]
        .into_iter()
        .enumerate()
    {
        let mut position = p.map(|value| value as f32);
        position[u] += uv[0];
        position[v] += uv[1];
        let brightness = (0.55 + f32::from(face.ao[corner]) * 0.15) * variation * directional;
        mesh.vertices.push(Vertex {
            position,
            normal,
            // Texture owns albedo. Vertex RGB is lighting only. Fluid alpha is the
            // development shader's water signal (`color.a < 0.99`), not tint[3].
            color: [
                brightness,
                brightness,
                brightness,
                if fluid { 0.72 } else { 1.0 },
            ],
            texture_layer: layer,
            tint,
        });
    }
    // Pick the diagonal with the lower AO discontinuity. Reverse negative faces
    // so every triangle's cross product points along its declared normal.
    let diagonal = if face.ao[0] + face.ao[2] > face.ao[1] + face.ao[3] {
        [0, 1, 3, 1, 2, 3]
    } else {
        [0, 1, 2, 0, 2, 3]
    };
    for triangle in diagonal.chunks_exact(3) {
        mesh.indices.push(base + triangle[0]);
        mesh.indices
            .push(base + triangle[if sign > 0 { 1 } else { 2 }]);
        mesh.indices
            .push(base + triangle[if sign > 0 { 2 } else { 1 }]);
    }
}

fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn hash3(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    mix64(
        seed ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (y as u64).wrapping_mul(0xD1B5_4A32_D192_ED03)
            ^ (z as u64).wrapping_mul(0xABC9_8388_FB8F_AC03),
    )
}

fn random_signed(seed: u64, x: i32, y: i32, z: i32) -> f64 {
    (hash3(seed, x, y, z) >> 11) as f64 * (2.0 / ((1_u64 << 53) as f64)) - 1.0
}

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn smoothstep(low: f64, high: f64, value: f64) -> f64 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn noise2(seed: u64, x: f64, z: f64) -> f64 {
    let (ix, fx) = floor_parts(x);
    let (iz, fz) = floor_parts(z);
    let fx = fade(fx);
    let fz = fade(fz);
    lerp(
        lerp(
            random_signed(seed, ix, 0, iz),
            random_signed(seed, ix + 1, 0, iz),
            fx,
        ),
        lerp(
            random_signed(seed, ix, 0, iz + 1),
            random_signed(seed, ix + 1, 0, iz + 1),
            fx,
        ),
        fz,
    )
}

fn noise3(seed: u64, x: f64, y: f64, z: f64) -> f64 {
    let (ix, fx) = floor_parts(x);
    let (iy, fy) = floor_parts(y);
    let (iz, fz) = floor_parts(z);
    let fx = fade(fx);
    let fy = fade(fy);
    let fz = fade(fz);
    let mut planes = [0.0; 2];
    for (index, plane) in planes.iter_mut().enumerate() {
        let z = iz + index as i32;
        *plane = lerp(
            lerp(
                random_signed(seed, ix, iy, z),
                random_signed(seed, ix + 1, iy, z),
                fx,
            ),
            lerp(
                random_signed(seed, ix, iy + 1, z),
                random_signed(seed, ix + 1, iy + 1, z),
                fx,
            ),
            fy,
        );
    }
    lerp(planes[0], planes[1], fz)
}

fn fbm2(seed: u64, mut x: f64, mut z: f64, octaves: u32) -> f64 {
    let mut result = 0.0;
    let mut amplitude = 0.57;
    for octave in 0..octaves {
        // Separate multiply and add. Rust does not contract these into FMA.
        result += noise2(seed.wrapping_add(u64::from(octave) * 7919), x, z) * amplitude;
        amplitude *= 0.5;
        x *= 2.03;
        z *= 2.03;
    }
    result
}

/// Truncate toward zero. `f64 as i32` saturates outside the `i32` range.
fn trunc_to_i32(value: f64) -> i32 {
    value as i32
}

/// Floor without libm. Finite inputs that fit in `i32` match `f64::floor` cast to `i32`.
fn floor_to_i32(value: f64) -> i32 {
    let truncated = trunc_to_i32(value);
    if value >= 0.0 || value == f64::from(truncated) {
        truncated
    } else {
        truncated.saturating_sub(1)
    }
}

fn floor_parts(value: f64) -> (i32, f64) {
    let index = floor_to_i32(value);
    (index, value - f64::from(index))
}

/// Round half away from zero without libm. Matches `f64::round` for finite values in `i32`.
fn round_half_away(value: f64) -> i32 {
    if value >= 0.0 {
        trunc_to_i32(value + 0.5)
    } else {
        trunc_to_i32(value - 0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artificial_mesh(cells: &[([i32; 3], Block)]) -> Mesh {
        let mut halo = vec![AIR; HALO_VOLUME];
        for &(p, block) in cells {
            halo[halo_index(p)] = block;
        }
        mesh_halo(
            [0; 3],
            &halo,
            &MeshLookup::from_registry(registry::registry()),
        )
    }

    fn triangle_area(mesh: &Mesh) -> f32 {
        mesh.indices
            .chunks_exact(3)
            .map(|tri| {
                let a = glam::Vec3::from(mesh.vertices[tri[0] as usize].position);
                let b = glam::Vec3::from(mesh.vertices[tri[1] as usize].position);
                let c = glam::Vec3::from(mesh.vertices[tri[2] as usize].position);
                (b - a).cross(c - a).length() * 0.5
            })
            .sum()
    }

    #[test]
    fn floor_and_round_match_half_away_without_libm() {
        for (value, floor, rounded) in [
            (0.0, 0, 0),
            (0.5, 0, 1),
            (1.2, 1, 1),
            (1.5, 1, 2),
            (-0.2, -1, 0),
            (-0.5, -1, -1),
            (-1.5, -2, -2),
            (-2.0, -2, -2),
            (11.6, 11, 12),
        ] {
            assert_eq!(floor_to_i32(value), floor, "floor {value}");
            assert_eq!(round_half_away(value), rounded, "round {value}");
            let (index, fraction) = floor_parts(value);
            assert_eq!(index, floor);
            assert!((0.0..1.0).contains(&fraction), "{value} -> {fraction}");
        }
    }

    /// Seed `20261021`. Chunk bytes are three little-endian `i32`s followed by
    /// 32³ block ids in `generate_chunk` order. The digest is FNV-1a 64
    /// (offset `0xCBF29CE484222325`, prime `0x100000001B3`), not `DefaultHasher`.
    ///
    /// Replace these digests only after an intentional, reviewed generator
    /// change, using the actual values printed by
    /// `golden_chunks_match_platform_independent_hashes`. A mismatch on
    /// unchanged generator code is cross-platform drift; do not refresh the
    /// table to hide it.
    const GOLDEN_SEED: u64 = 20261021;
    const GOLDEN_CHUNK_HASHES: &[((i32, i32, i32), u64, &str)] = &[
        ((-1, 0, -8), 0x24f4_9364_cb10_5b1b, "ocean"),
        ((20, 1, -25), 0x26f9_ca54_ab95_53c9, "desert surface"),
        ((-2, 1, -2), 0x939a_5de1_ffa4_4b37, "forest canopy"),
        ((-5, 0, -6), 0x26cc_513b_7122_963a, "caves and ores"),
        (
            (-25, 0, -27),
            0xe417_9e0a_b5c3_f4d0,
            "far negative caves and ores",
        ),
        ((-11, 1, -15), 0x9c71_9975_76c7_986c, "snow cap"),
        (
            (-15, 0, -15),
            0x9cc4_e3bc_280b_83a0,
            "clay, crystal, and deep basalt",
        ),
    ];

    #[test]
    fn golden_chunks_match_platform_independent_hashes() {
        let world = World::new(GOLDEN_SEED);
        let mut report = String::new();
        let mut mismatched = false;
        for &(key, expected, label) in GOLDEN_CHUNK_HASHES {
            let blocks = world
                .generate_chunk(key)
                .unwrap_or_else(|| panic!("chunk {key:?} ({label}) did not generate"));
            assert_role(&blocks, key, label);
            let actual = fnv1a64(&chunk_canonical_bytes(key, &blocks));
            let mark = if actual == expected { "ok" } else { "MISMATCH" };
            mismatched |= actual != expected;
            report.push_str(&format!(
                "  ({:>3},{:>2},{:>3}) {label:<28} expected {expected:016x} actual {actual:016x} {mark}\n",
                key.0, key.1, key.2
            ));
        }
        assert!(
            !mismatched,
            "World generation for seed {GOLDEN_SEED} no longer matches the golden chunk hashes.\n\
             A mismatch means the world generator changed, or the same seed is not deterministic \
             across platforms (noise or floating-point drift).\n\
             If the generator change is intentional and reviewed, replace GOLDEN_CHUNK_HASHES in \
             src/world.rs with the actual digests below. Do not update them merely to silence a \
             cross-platform failure.\n{report}"
        );
    }

    fn assert_role(blocks: &[Block], key: ChunkKey, label: &str) {
        let count = |block: Block| {
            blocks
                .iter()
                .filter(|candidate| **candidate == block)
                .count()
        };
        match label {
            "ocean" => {
                assert!(count(WATER) > 1000, "{key:?} should be ocean");
                assert_eq!(count(GRASS), 0, "{key:?} ocean cover");
            }
            "desert surface" => {
                assert!(count(SAND) > 1000, "{key:?} should be desert");
                assert_eq!(count(GRASS), 0, "{key:?} desert cover");
                assert_eq!(count(WOOD), 0, "{key:?} desert trees");
            }
            "forest canopy" => {
                assert!(count(WOOD) > 0, "{key:?} should contain a trunk");
                assert!(count(LEAVES) > 0, "{key:?} should contain leaves");
            }
            "snow cap" => {
                assert!(count(SNOW) > 0, "{key:?} should contain snow");
            }
            "clay, crystal, and deep basalt" => {
                assert!(count(CLAY) > 0, "{key:?} clay");
                assert!(count(CRYSTAL) > 0, "{key:?} crystal");
                assert!(count(BASALT) > 1024, "{key:?} deep basalt above the floor");
            }
            "caves and ores" | "far negative caves and ores" => {
                assert!(
                    key.0 < 0 && key.2 < 0,
                    "{label} should use negative coordinates"
                );
                assert!(count(COAL_ORE) > 0, "{key:?} coal");
                assert!(count(IRON_ORE) > 0, "{key:?} iron");
                assert!(count(GOLD_ORE) > 0, "{key:?} gold");
                assert!(count(DIAMOND_ORE) > 0, "{key:?} diamond");
                let deep_air = blocks.iter().enumerate().any(|(index, block)| {
                    let y = (index / (CHUNK_SIZE * CHUNK_SIZE) as usize) as i32;
                    y <= 12 && *block == AIR
                });
                assert!(deep_air, "{key:?} should carve cave air at y <= 12");
            }
            _ => panic!("unknown golden role {label}"),
        }
    }

    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut hash = 0xCBF2_9CE4_8422_2325_u64;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
        }
        hash
    }

    fn chunk_canonical_bytes(key: ChunkKey, blocks: &[Block]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(12 + blocks.len());
        for coord in [key.0, key.1, key.2] {
            bytes.extend_from_slice(&coord.to_le_bytes());
        }
        bytes.extend(blocks.iter().map(|block| {
            u8::try_from(block.raw()).expect("golden chunks only contain builtin ids 0..=15")
        }));
        bytes
    }

    #[test]
    fn all_material_ids_round_trip_and_unknown_ids_are_rejected() {
        let mut id = 0_u8;
        while let Some(block) = Block::from_id(id) {
            assert_eq!(block.legacy_u8(), Some(id));
            assert!(!block.name().is_empty());
            assert!(block.color().iter().all(|channel| channel.is_finite()));
            id += 1;
        }
        assert!(id > 15, "ore ids must stay registered");
        assert_eq!(Block::from_id(id), None);
        assert_eq!(Block::from_id(255), None);
        assert!(!WATER.is_solid());
        assert!(DIAMOND_ORE.is_solid());
    }

    #[test]
    fn generation_is_repeatable_and_seed_affects_terrain() {
        let a = World::new(41);
        let b = World::new(41);
        let c = World::new(42);
        let mut differences = 0;
        for x in (-192..192).step_by(11) {
            for z in (-192..192).step_by(13) {
                assert_eq!(a.surface_height(x, z), b.surface_height(x, z));
                assert_eq!(a.block_at(x, 35, z), b.block_at(x, 35, z));
                differences += usize::from(a.surface_height(x, z) != c.surface_height(x, z));
                assert!((MIN_SURFACE..=MAX_SURFACE).contains(&a.surface_height(x, z)));
            }
        }
        assert!(differences > 300);
    }

    #[test]
    fn edits_override_generation_restore_and_serialize_in_coordinate_order() {
        let mut world = World::new(16);
        let positions = [[32, 150, -1], [-1, 150, 32], [0, 150, 0]];
        for p in positions {
            world.set_block(p, CRYSTAL);
            assert_eq!(world.block_at(p[0], p[1], p[2]), CRYSTAL);
        }
        let snapshot = world.clone();
        world.set_block(positions[0], AIR);
        assert_eq!(world.edits().len(), 2);
        assert_eq!(snapshot.edits().len(), 3);
        assert_eq!(world.edits()[0].0, [-1, 150, 32]);
        assert_eq!(world.edits()[1].0, [0, 150, 0]);
    }

    #[test]
    fn empty_single_and_adjacent_voxels_have_expected_surface_area() {
        assert!(artificial_mesh(&[]).indices.is_empty());
        let single = artificial_mesh(&[([8, 8, 8], STONE)]);
        assert_eq!(single.indices.len(), 36);
        assert_eq!(triangle_area(&single), 6.0);
        let pair = artificial_mesh(&[([8, 8, 8], STONE), ([9, 8, 8], STONE)]);
        assert_eq!(triangle_area(&pair), 10.0);
        assert_eq!(pair.indices.len(), 36, "coplanar equal-lit faces merge");
    }

    #[test]
    fn chunk_halo_culls_internal_border_faces() {
        let edge = artificial_mesh(&[([31, 8, 8], STONE), ([32, 8, 8], STONE)]);
        assert_eq!(triangle_area(&edge), 5.0);
        assert!(
            edge.vertices
                .iter()
                .all(|vertex| vertex.normal != [1.0, 0.0, 0.0])
        );
        let negative = artificial_mesh(&[([0, 8, 8], STONE), ([-1, 8, 8], STONE)]);
        assert_eq!(triangle_area(&negative), 5.0);
    }

    #[test]
    fn ambient_occlusion_darkens_a_concave_corner() {
        let mut halo = vec![AIR; HALO_VOLUME];
        // Sample the top of a voxel at [8,8,8], with two adjacent walls.
        halo[halo_index([7, 9, 8])] = STONE;
        halo[halo_index([8, 9, 7])] = STONE;
        let tables = MeshLookup::from_registry(registry::registry());
        let ao = corner_ao(&halo_flags(&halo, &tables), [8, 9, 8], 0, 2);
        assert_eq!(ao[0], 0, "two occluding side cells close the corner");
        assert_eq!(ao[2], 3, "opposite open corner receives full ambient light");
    }

    #[test]
    fn mesh_vertices_carry_the_block_tint() {
        let mut tables = MeshLookup::from_registry(registry::registry());
        let tint = [0.25, 0.5, 0.75, 1.0];
        tables.tints[WATER.index()] = tint;
        let mut halo = vec![AIR; HALO_VOLUME];
        halo[halo_index([8, 8, 8])] = WATER;
        let mesh = mesh_halo([0; 3], &halo, &tables);
        assert!(
            !mesh.vertices.is_empty(),
            "water beside air must emit faces"
        );
        assert!(
            mesh.vertices.iter().all(|vertex| vertex.tint == tint),
            "each water vertex must carry the block tint"
        );
    }

    #[test]
    fn emitted_grass_and_timber_faces_select_their_distinct_material_tiles() {
        let registry = registry::registry();
        let layer = |name: &str| registry.texture_index(name).unwrap();
        for (block, top, side, bottom) in [
            (
                GRASS,
                layer("strata:block/grass_top"),
                layer("strata:block/grass_side"),
                layer("strata:block/soil"),
            ),
            (
                WOOD,
                layer("strata:block/endgrain"),
                layer("strata:block/bark"),
                layer("strata:block/endgrain"),
            ),
        ] {
            let mesh = artificial_mesh(&[([8, 8, 8], block)]);
            assert_eq!(mesh.vertices.len(), 24);
            for vertex in mesh.vertices {
                let expected = if vertex.normal[1] > 0.0 {
                    top
                } else if vertex.normal[1] < 0.0 {
                    bottom
                } else {
                    side
                };
                assert_eq!(vertex.texture_layer, expected);
                assert_eq!(vertex.tint, [1.0, 1.0, 1.0, 1.0]);
                assert_eq!(
                    vertex.color[0], vertex.color[1],
                    "albedo must come from the texture"
                );
                assert_eq!(vertex.color[1], vertex.color[2]);
            }
        }
    }

    #[test]
    fn all_triangles_have_outward_winding_and_valid_indices() {
        let mesh = artificial_mesh(&[([8, 8, 8], STONE), ([9, 9, 8], WOOD)]);
        for tri in mesh.indices.chunks_exact(3) {
            let vertices = [
                mesh.vertices[tri[0] as usize],
                mesh.vertices[tri[1] as usize],
                mesh.vertices[tri[2] as usize],
            ];
            let a = glam::Vec3::from(vertices[0].position);
            let b = glam::Vec3::from(vertices[1].position);
            let c = glam::Vec3::from(vertices[2].position);
            assert!(
                (b - a)
                    .cross(c - a)
                    .dot(glam::Vec3::from(vertices[0].normal))
                    > 0.0
            );
            assert!(
                vertices
                    .iter()
                    .all(|v| v.color.iter().all(|channel| channel.is_finite()))
            );
        }
    }

    #[test]
    fn water_has_surface_without_internal_water_faces_and_is_not_pickable() {
        let mesh = artificial_mesh(&[([8, 8, 8], WATER), ([9, 8, 8], WATER)]);
        assert_eq!(triangle_area(&mesh), 10.0);
        assert!(mesh.vertices.iter().all(|vertex| vertex.color[3] == 0.72));
        assert!(
            mesh.vertices
                .iter()
                .all(|vertex| vertex.tint == [1.0, 1.0, 1.0, 1.0])
        );
        let mut world = World::new(1);
        world.set_block([1, 150, 0], WATER);
        world.set_block([2, 150, 0], STONE);
        let hit = world
            .raycast([0.5, 150.5, 0.5], [1.0, 0.0, 0.0], 5.0)
            .unwrap();
        assert_eq!(hit.block, [2, 150, 0]);
    }

    #[test]
    fn raycast_handles_negative_cells_scaled_direction_and_range() {
        let mut world = World::new(1);
        world.set_block([-3, 150, -1], STONE);
        let hit = world
            .raycast([0.5, 150.5, -0.5], [-4.0, 0.0, 0.0], 3.0)
            .unwrap();
        assert_eq!(hit.block, [-3, 150, -1]);
        assert_eq!(hit.previous, [-2, 150, -1]);
        assert!((hit.distance - 2.5).abs() < 0.0001);
        assert!(
            world
                .raycast([0.5, 150.5, -0.5], [-1.0, 0.0, 0.0], 2.49)
                .is_none()
        );
    }

    #[test]
    fn raycast_rejects_invalid_input_and_handles_start_inside_and_ties() {
        let mut world = World::new(1);
        world.set_block([1, 151, 1], STONE);
        world.set_block([1, 150, 0], WOOD);
        let hit = world
            .raycast([0.5, 150.5, 0.5], [1.0, 1.0, 1.0], 5.0)
            .unwrap();
        assert_eq!(hit.block, [1, 151, 1], "corner-only contact is not a hit");
        assert_eq!(
            world
                .raycast([1.5, 151.5, 1.5], [1.0, 0.0, 0.0], 0.0)
                .unwrap()
                .distance,
            0.0
        );
        assert!(
            world
                .raycast([f32::NAN, 150.0, 0.0], [1.0; 3], 2.0)
                .is_none()
        );
        assert!(world.raycast([0.0, 150.0, 0.0], [0.0; 3], 2.0).is_none());
        assert!(world.raycast([0.0, 150.0, 0.0], [1.0; 3], -1.0).is_none());
    }

    #[test]
    fn meshing_halo_matches_point_queries_across_negative_chunk_borders() {
        let mut world = World::new(31415);
        world.set_block([-1, 42, -32], CRYSTAL);
        for key in [(-1, 1, -1), (0, 1, -1), (-1, 2, 0)] {
            let origin = chunk_origin(key).unwrap();
            let halo = world.sample_halo(origin);
            for z in -1..=CHUNK_SIZE {
                for x in -1..=CHUNK_SIZE {
                    for y in (-1..=CHUNK_SIZE).step_by(3) {
                        assert_eq!(
                            halo[halo_index([x, y, z])],
                            world.block_at(origin[0] + x, origin[1] + y, origin[2] + z),
                            "mismatch at chunk {key:?}, local {:?}",
                            [x, y, z]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_tree_crossing_a_chunk_boundary_is_identical_on_both_sides() {
        let world = World::new(728491);
        let tree = (-48..48)
            .find_map(|gz| {
                (-48..48).find_map(|gx| {
                    world
                        .tree(gx, gz)
                        .filter(|tree| tree.x.rem_euclid(CHUNK_SIZE) <= 1)
                })
            })
            .expect("seeded forest includes a boundary tree");
        for dy in 1..=tree.height {
            assert_eq!(
                world.block_at(tree.x, tree.ground + dy, tree.z),
                WOOD,
                "trunk must be continuous"
            );
        }
        assert_eq!(
            world.block_at(tree.x, tree.ground + tree.height + 1, tree.z),
            LEAVES
        );
        let boundary = tree.x.div_euclid(CHUNK_SIZE) * CHUNK_SIZE;
        let key = (
            tree.x.div_euclid(CHUNK_SIZE),
            (tree.ground + tree.height - 1).div_euclid(CHUNK_SIZE),
            tree.z.div_euclid(CHUNK_SIZE),
        );
        let right_origin = chunk_origin(key).unwrap();
        let left_origin = chunk_origin((key.0 - 1, key.1, key.2)).unwrap();
        let right = world.sample_halo(right_origin);
        let left = world.sample_halo(left_origin);
        let mut left_leaves = 0;
        let mut right_leaves = 0;
        for y in tree.ground..=tree.ground + tree.height + TREE_CROWN_ABOVE {
            for dz in -TREE_CANOPY..=TREE_CANOPY {
                for dx in -TREE_CANOPY..=TREE_CANOPY {
                    let x = tree.x + dx;
                    let z = tree.z + dz;
                    let block = world.block_at(x, y, z);
                    if block == LEAVES || block == WOOD {
                        if x < boundary {
                            left_leaves += 1;
                        } else {
                            right_leaves += 1;
                        }
                    }
                    if (left_origin[1]..left_origin[1] + CHUNK_SIZE).contains(&y)
                        && (left_origin[2]..left_origin[2] + CHUNK_SIZE).contains(&z)
                    {
                        let local_x = x - left_origin[0];
                        if (0..CHUNK_SIZE).contains(&local_x) {
                            assert_eq!(
                                left[halo_index([local_x, y - left_origin[1], z - left_origin[2]])],
                                block
                            );
                        }
                    }
                    if (right_origin[1]..right_origin[1] + CHUNK_SIZE).contains(&y)
                        && (right_origin[2]..right_origin[2] + CHUNK_SIZE).contains(&z)
                    {
                        let local_x = x - right_origin[0];
                        if (0..CHUNK_SIZE).contains(&local_x) {
                            assert_eq!(
                                right[halo_index([
                                    local_x,
                                    y - right_origin[1],
                                    z - right_origin[2]
                                ])],
                                block
                            );
                        }
                    }
                }
            }
        }
        assert!(
            left_leaves > 0,
            "canopy must enter the chunk before the trunk"
        );
        assert!(
            right_leaves > 0,
            "canopy must continue on the trunk's chunk"
        );
        let boundary_block = world.block_at(boundary, tree.ground + tree.height, tree.z);
        assert_eq!(
            right[halo_index([
                0,
                tree.ground + tree.height - right_origin[1],
                tree.z - right_origin[2]
            ])],
            boundary_block
        );
        assert_eq!(
            left[halo_index([
                CHUNK_SIZE,
                tree.ground + tree.height - left_origin[1],
                tree.z - left_origin[2]
            ])],
            boundary_block
        );
    }

    #[test]
    fn same_seed_regenerates_identical_chunks_independent_of_order() {
        let seed = 0x000A_11CE;
        let keys = [(0, 0, 0), (1, 0, -1), (-2, 0, 3), (0, 1, 0), (-1, 0, -1)];
        let world = World::new(seed);
        let serial: Vec<_> = keys
            .iter()
            .map(|key| world.generate_chunk(*key).expect("chunk origin"))
            .collect();
        let other = World::new(seed);
        for key in keys.into_iter().rev() {
            let chunk = other.generate_chunk(key).unwrap();
            let index = keys.iter().position(|candidate| *candidate == key).unwrap();
            assert_eq!(chunk, serial[index]);
        }
        let mut handles = Vec::new();
        for key in keys {
            handles.push(std::thread::spawn(move || {
                let world = World::new(seed);
                (key, world.generate_chunk(key).unwrap())
            }));
        }
        for handle in handles {
            let (key, chunk) = handle.join().expect("worker");
            let index = keys.iter().position(|candidate| *candidate == key).unwrap();
            assert_eq!(
                chunk, serial[index],
                "parallel generation diverged at {key:?}"
            );
        }
        let origin = chunk_origin(keys[0]).unwrap();
        let chunk = &serial[0];
        for y in 0..CHUNK_SIZE {
            for z in (0..CHUNK_SIZE).step_by(3) {
                for x in (0..CHUNK_SIZE).step_by(3) {
                    let index = (x + CHUNK_SIZE * (z + CHUNK_SIZE * y)) as usize;
                    assert_eq!(
                        chunk[index],
                        world.block_at(origin[0] + x, origin[1] + y, origin[2] + z)
                    );
                }
            }
        }
    }

    #[test]
    fn oceans_beaches_and_land_biomes_have_the_expected_cover() {
        let world = World::new(20261021);
        let mut counts = [0_u32; 5];
        let mut desert_sand = 0;
        let mut desert_seen = 0;
        let mut forest_grass = 0;
        let mut forest_seen = 0;
        let mut plains_grass = 0;
        let mut plains_seen = 0;
        let mut oceans = 0;
        let mut flooded = 0;
        for x in (-512..512).step_by(8) {
            for z in (-512..512).step_by(8) {
                let biome = world.biome_at(x, z);
                counts[biome_index(biome)] += 1;
                let height = world.surface_height(x, z);
                let surface = world.block_at(x, height, z);
                match biome {
                    Biome::Ocean => {
                        oceans += 1;
                        assert!(height < SEA_LEVEL);
                        assert_eq!(world.block_at(x, SEA_LEVEL, z), WATER);
                        let bed = world.block_at(x, height - 1, z);
                        assert!(matches!(surface, SAND | CLAY), "ocean bed {surface:?}");
                        assert!(matches!(bed, SAND | CLAY), "ocean subsurface {bed:?}");
                        if surface == CLAY || bed == CLAY {
                            assert!(
                                height >= SEA_LEVEL - 3,
                                "clay in deep ocean at height {height}"
                            );
                        }
                        for dy in 0..6 {
                            let block = world.block_at(x, height - dy, z);
                            assert!(block.is_solid(), "ocean floor opened at y={}", height - dy);
                        }
                        for y in height + 1..=SEA_LEVEL {
                            assert_eq!(world.block_at(x, y, z), WATER);
                            for [nx, nz] in [[1, 0], [-1, 0], [0, 1], [0, -1]] {
                                let neighbor = world.block_at(x + nx, y, z + nz);
                                if neighbor == AIR {
                                    flooded += 1;
                                }
                            }
                            assert_ne!(world.block_at(x, y - 1, z), AIR);
                        }
                    }
                    Biome::Beach | Biome::Desert => {
                        desert_seen += usize::from(biome == Biome::Desert);
                        desert_sand += usize::from(biome == Biome::Desert && surface == SAND);
                        if biome == Biome::Desert {
                            assert_eq!(surface, SAND);
                        } else {
                            assert!(matches!(surface, SAND | CLAY), "beach cover {surface:?}");
                        }
                        assert!(height >= SEA_LEVEL);
                    }
                    Biome::Forest => {
                        forest_seen += 1;
                        forest_grass += usize::from(surface == GRASS);
                        assert!(height > SEA_LEVEL);
                    }
                    Biome::Plains => {
                        plains_seen += 1;
                        plains_grass += usize::from(surface == GRASS);
                        assert!(height > SEA_LEVEL);
                    }
                }
            }
        }
        assert!(
            counts.iter().all(|count| *count > 20),
            "biome counts {counts:?}"
        );
        assert!(oceans > 40);
        assert_eq!(flooded, 0, "cave air met ocean water");
        assert_eq!(desert_sand, desert_seen);
        assert!(forest_seen > 0 && forest_grass * 100 / forest_seen > 70);
        assert!(plains_seen > 0 && plains_grass * 100 / plains_seen > 70);
    }

    #[test]
    fn trees_are_dense_in_forest_sparse_on_plains_and_absent_elsewhere() {
        let world = World::new(20261021);
        let mut forest = 0;
        let mut plains = 0;
        let mut elsewhere = 0;
        for gz in -30..30 {
            for gx in -30..30 {
                let Some(tree) = world.tree(gx, gz) else {
                    continue;
                };
                match world.biome_at(tree.x, tree.z) {
                    Biome::Forest => forest += 1,
                    Biome::Plains => plains += 1,
                    _ => elsewhere += 1,
                }
                assert!(world.surface_height(tree.x, tree.z) > SEA_LEVEL);
                assert_eq!(world.block_at(tree.x, tree.ground, tree.z), GRASS);
                for dy in 1..=tree.height {
                    assert_eq!(world.block_at(tree.x, tree.ground + dy, tree.z), WOOD);
                }
                assert_eq!(
                    world.block_at(tree.x + 1, tree.ground + tree.height, tree.z),
                    LEAVES
                );
            }
        }
        assert_eq!(elsewhere, 0);
        assert!(
            forest > plains && plains > 0,
            "forest {forest}, plains {plains}"
        );
    }

    #[test]
    fn high_ground_snow_uses_the_height_clause_on_a_cool_peak() {
        let world = World::new(20261021);
        let (x, z) = (280, -772);
        let column = world.column(x, z);
        assert_eq!(column.height, 45);
        assert!(column.height >= HIGH_GROUND_SNOW);
        assert!(
            (-0.16..0.02).contains(&column.temperature),
            "temperature {} must miss the cold clause and hit the cool clause",
            column.temperature
        );
        assert!(matches!(column.biome, Biome::Forest));
        assert!(!world.sandy(x, z, column));
        assert!(world.edits().is_empty());
        assert_eq!(world.block_at(x, column.height, z), GRASS);
        assert_eq!(world.block_at(x, column.height + 1, z), SNOW);
        // The sample maximum is warmer than the cool gate, so it stays bare.
        let warm = world.column(884, -460);
        assert_eq!(warm.height, 46);
        assert!(warm.temperature >= 0.02, "{}", warm.temperature);
        assert_ne!(world.block_at(884, warm.height + 1, -460), SNOW);
    }

    #[test]
    fn snow_clay_crystal_and_deep_basalt_generate_at_fixed_columns() {
        let world = World::new(20261021);
        assert!(world.edits().is_empty());
        assert_eq!(world.surface_height(-340, -480), 31);
        assert_eq!(world.block_at(-340, 31, -480), GRASS);
        assert_eq!(world.block_at(-340, 32, -480), SNOW);
        assert!(matches!(
            world.biome_at(-340, -480),
            Biome::Plains | Biome::Forest
        ));
        assert_eq!(world.block_at(-480, 29, -480), CLAY);
        assert!((SEA_LEVEL - 3..=SEA_LEVEL + 2).contains(&world.surface_height(-480, -480)));
        assert_eq!(world.block_at(-12, 6, -96), CRYSTAL);
        assert_eq!(world.block_at(-92, 0, -96), BASALT);
        assert_eq!(world.block_at(-92, 1, -96), BASALT);
    }

    #[test]
    fn ores_stay_inside_depth_bands_and_each_band_is_populated() {
        let world = World::new(20261021);
        let mut coal = 0;
        let mut iron = 0;
        let mut gold = 0;
        let mut diamond = 0;
        for cy in 0..=2 {
            for cz in -2..=2 {
                for cx in -2..=2 {
                    let chunk = world.generate_chunk((cx, cy, cz)).unwrap();
                    let origin_y = cy * CHUNK_SIZE;
                    for (index, block) in chunk.iter().copied().enumerate() {
                        let y = origin_y + (index / (CHUNK_SIZE * CHUNK_SIZE) as usize) as i32;
                        match block {
                            COAL_ORE => {
                                assert!((6..=70).contains(&y));
                                coal += 1;
                            }
                            IRON_ORE => {
                                assert!((1..=46).contains(&y));
                                iron += 1;
                            }
                            GOLD_ORE => {
                                assert!((1..=22).contains(&y));
                                gold += 1;
                            }
                            DIAMOND_ORE => {
                                assert!((1..=12).contains(&y));
                                diamond += 1;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        assert!(
            coal > iron && iron > gold && gold > 0,
            "coal {coal} iron {iron} gold {gold}"
        );
        assert!(diamond > 0, "diamond band was empty in the sample");
        assert!(
            coal > diamond * 4,
            "coal should be much more common than diamond"
        );
    }

    #[test]
    fn caves_exist_underground_without_opening_the_sky() {
        let world = World::new(31415);
        let mut caves = 0;
        for x in (-80..80).step_by(5) {
            for z in (-80..80).step_by(5) {
                let height = world.surface_height(x, z);
                assert_ne!(world.block_at(x, height, z), AIR);
                for y in 2..height {
                    if world.block_at(x, y, z) == AIR {
                        caves += 1;
                        assert!(y <= height - 4);
                    }
                }
            }
        }
        assert!(caves > 30, "sample contained {caves} cave cells");
    }

    #[test]
    fn chunk_generation_reports_average_time() {
        let world = World::new(20261021);
        let keys = [
            (-1, 0, -8),
            (20, 1, -25),
            (-2, 1, -2),
            (-5, 0, -6),
            (-25, 0, -27),
            (-11, 1, -15),
            (-15, 0, -15),
        ];
        for _ in 0..3 {
            for key in keys {
                std::hint::black_box(world.generate_chunk(key));
            }
        }
        let rounds = 30;
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            for key in keys {
                std::hint::black_box(world.generate_chunk(key));
            }
        }
        let total = rounds * keys.len();
        let average_us = start.elapsed().as_secs_f64() * 1_000_000.0 / total as f64;
        eprintln!("STRATA_CHUNK_GEN_AVG_US {average_us:.1} chunks={total}");
        assert!(
            average_us < 50_000.0,
            "chunk generation averaged {average_us:.1} µs"
        );
    }

    fn biome_index(biome: Biome) -> usize {
        match biome {
            Biome::Plains => 0,
            Biome::Forest => 1,
            Biome::Desert => 2,
            Biome::Ocean => 3,
            Biome::Beach => 4,
        }
    }

    #[test]
    fn export_worldgen_previews_when_requested() {
        let Ok(directory) = std::env::var("STRATA_PREVIEW_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let world = World::new(PREVIEW_SEED);
        let (origin_x, origin_z) = preview_origin(&world);
        let slice_z = preview_slice_z(&world, origin_x, origin_z);
        let span = PREVIEW_SPAN;
        let mut map = vec![0_u8; (span * (span * 2 + 8) * 3) as usize];
        for z in 0..span {
            for x in 0..span {
                let wx = origin_x + x;
                let wz = origin_z + z;
                let height = world.surface_height(wx, wz);
                let mut biome = biome_rgb(world.biome_at(wx, wz), height);
                if world.block_at(wx, height + 1, wz) == SNOW {
                    biome = [230, 236, 238];
                }
                let height_shade = height_rgb(height, world.biome_at(wx, wz));
                put_rgb(&mut map, span * 2 + 8, x, z, biome);
                put_rgb(&mut map, span * 2 + 8, span + 8 + x, z, height_shade);
            }
        }
        write_rgb(
            &directory.join("biome-height.png"),
            (span * 2 + 8) as u32,
            span as u32,
            &map,
        );
        let section_top = 72;
        let depth = section_top + 1;
        let mut section = vec![0_u8; (span * depth * 3) as usize];
        for x in 0..span {
            for y in 0..=section_top {
                let block = world.block_at(origin_x + x, y, slice_z);
                let row = section_top - y;
                put_rgb(&mut section, span, x, row, block_rgb(block, y));
            }
        }
        write_rgb(
            &directory.join("cross-section.png"),
            span as u32,
            depth as u32,
            &section,
        );
        eprintln!(
            "STRATA_PREVIEW seed={PREVIEW_SEED} origin={origin_x},{origin_z} slice_z={slice_z} span={span}"
        );
    }

    const PREVIEW_SEED: u64 = 20261021;
    const PREVIEW_SPAN: i32 = 320;

    fn preview_origin(world: &World) -> (i32, i32) {
        let mut best = (0, 0, -1_i32);
        for origin_z in (-640..640).step_by(80) {
            for origin_x in (-640..640).step_by(80) {
                let mut counts = [0_i32; 5];
                for z in (0..PREVIEW_SPAN).step_by(16) {
                    for x in (0..PREVIEW_SPAN).step_by(16) {
                        counts[biome_index(world.biome_at(origin_x + x, origin_z + z))] += 1;
                    }
                }
                let Some(least) = counts.iter().copied().min() else {
                    continue;
                };
                if least < 4 {
                    continue;
                }
                let score =
                    least * 1_000 + counts.iter().map(|count| (*count).min(30)).sum::<i32>();
                if score > best.2 {
                    best = (origin_x, origin_z, score);
                }
            }
        }
        assert!(best.2 > 0, "preview window did not contain every biome");
        (best.0, best.1)
    }

    fn preview_slice_z(world: &World, origin_x: i32, origin_z: i32) -> i32 {
        let mut best = (origin_z + PREVIEW_SPAN / 2, 0_i32);
        for z in (origin_z..origin_z + PREVIEW_SPAN).step_by(2) {
            let mut saw_ocean = false;
            let mut saw_land = false;
            let mut caves = 0;
            let mut coal = 0;
            let mut iron = 0;
            let mut gold = 0;
            let mut diamond = 0;
            let mut restored = 0;
            for x in (origin_x..origin_x + PREVIEW_SPAN).step_by(4) {
                match world.biome_at(x, z) {
                    Biome::Ocean => saw_ocean = true,
                    Biome::Beach => {}
                    _ => saw_land = true,
                }
                let height = world.surface_height(x, z).min(72);
                if world.block_at(x, height + 1, z) == SNOW {
                    restored += 1;
                }
                for y in 1..height {
                    match world.block_at(x, y, z) {
                        AIR => caves += 1,
                        COAL_ORE => coal += 1,
                        IRON_ORE => iron += 1,
                        GOLD_ORE => gold += 1,
                        DIAMOND_ORE => diamond += 1,
                        CLAY | CRYSTAL | BASALT | SNOW => {
                            restored += 1;
                        }
                        _ => {}
                    }
                }
            }
            if saw_ocean
                && saw_land
                && caves > 0
                && coal > 0
                && iron > 0
                && gold > 0
                && diamond > 0
                && restored > 0
            {
                let score = caves.min(40)
                    + coal.min(20)
                    + iron.min(20)
                    + gold * 3
                    + diamond * 5
                    + restored.min(30) * 4;
                if score > best.1 {
                    best = (z, score);
                }
            }
        }
        assert!(
            best.1 > 0,
            "preview slice missed caves, an ore band, or a restored block"
        );
        best.0
    }

    fn biome_rgb(biome: Biome, height: i32) -> [u8; 3] {
        let shade = (0.55 + f64::from(height - MIN_SURFACE) / 80.0).clamp(0.45, 1.15);
        let base = match biome {
            Biome::Plains => [92, 164, 70],
            Biome::Forest => [34, 108, 52],
            Biome::Desert => [214, 186, 112],
            Biome::Ocean => [30, 96, 156],
            Biome::Beach => [222, 204, 150],
        };
        base.map(|channel| (f64::from(channel) * shade).round() as u8)
    }

    fn height_rgb(height: i32, biome: Biome) -> [u8; 3] {
        if biome == Biome::Ocean {
            let depth = (SEA_LEVEL - height).clamp(0, 24);
            return [20, (70 + depth * 3) as u8, (120 + depth * 4).min(220) as u8];
        }
        let shade =
            ((height - MIN_SURFACE) * 255 / (MAX_SURFACE - MIN_SURFACE)).clamp(0, 255) as u8;
        [shade, shade, shade]
    }

    fn block_rgb(block: Block, y: i32) -> [u8; 3] {
        match block {
            AIR => {
                if y > SEA_LEVEL {
                    [186, 214, 232]
                } else {
                    [28, 22, 36]
                }
            }
            GRASS => [74, 150, 58],
            DIRT => [122, 84, 52],
            STONE => [132, 136, 134],
            SAND => [214, 196, 132],
            WATER => [42, 112, 168],
            WOOD => [112, 74, 42],
            LEAVES => [48, 124, 54],
            SNOW => [230, 236, 238],
            CLAY => [168, 86, 56],
            CRYSTAL => [72, 210, 214],
            BASALT => [48, 52, 58],
            COAL_ORE => [42, 42, 46],
            IRON_ORE => [176, 110, 78],
            GOLD_ORE => [214, 176, 64],
            DIAMOND_ORE => [80, 220, 210],
            _ => [255, 0, 255],
        }
    }

    fn put_rgb(image: &mut [u8], width: i32, x: i32, y: i32, color: [u8; 3]) {
        let index = ((y * width + x) * 3) as usize;
        image[index..index + 3].copy_from_slice(&color);
    }

    fn write_rgb(path: &std::path::Path, width: u32, height: u32, rgb: &[u8]) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgb).unwrap();
    }
}
