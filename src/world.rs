//! Deterministic procedural terrain, sparse edits, DDA selection, and greedy meshes.
//!
//! Generation is a pure function of the world seed and integer coordinates. Chunk
//! order and thread scheduling do not change a block. Each mesh samples a one-cell
//! halo so visibility and ambient occlusion agree across chunk borders. Trees and
//! ore veins are anchored on global grids, so a feature that crosses a chunk is
//! identical from either side. No generation cache is stored on `World`.

use std::collections::HashMap;

pub const CHUNK_SIZE: i32 = 32;
pub const SEA_LEVEL: i32 = 28;
/// Inclusive surface clamp. Terrain, including tree crowns, stays below the
/// y=192 benchmark fixtures.
pub const MIN_SURFACE: i32 = 12;
pub const MAX_SURFACE: i32 = 96;
/// Save-file terrain version. Generator 1 is rejected so older files keep
/// their bytes instead of being reinterpreted under these rules.
pub const GENERATOR_VERSION: u32 = 2;
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
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// IDs are part of the save format. Never reorder or recycle them.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Block {
    Air = 0,
    Grass = 1,
    Dirt = 2,
    Stone = 3,
    Sand = 4,
    Water = 5,
    Wood = 6,
    Leaves = 7,
    Snow = 8,
    Clay = 9,
    Crystal = 10,
    Basalt = 11,
    CoalOre = 12,
    IronOre = 13,
    GoldOre = 14,
    DiamondOre = 15,
}

impl Block {
    pub fn color(self) -> [f32; 4] {
        match self {
            Self::Air => [0.0, 0.0, 0.0, 0.0],
            Self::Grass => [0.38, 0.58, 0.22, 1.0],
            Self::Dirt => [0.39, 0.26, 0.16, 1.0],
            Self::Stone => [0.51, 0.54, 0.56, 1.0],
            Self::Sand => [0.80, 0.72, 0.48, 1.0],
            Self::Water => [0.10, 0.44, 0.53, 0.72],
            Self::Wood => [0.38, 0.24, 0.14, 1.0],
            Self::Leaves => [0.22, 0.43, 0.22, 1.0],
            Self::Snow => [0.87, 0.91, 0.91, 1.0],
            Self::Clay => [0.66, 0.34, 0.22, 1.0],
            Self::Crystal => [0.28, 0.86, 0.89, 1.0],
            Self::Basalt => [0.24, 0.28, 0.31, 1.0],
            Self::CoalOre => [0.16, 0.16, 0.17, 1.0],
            Self::IronOre => [0.66, 0.48, 0.40, 1.0],
            Self::GoldOre => [0.86, 0.72, 0.28, 1.0],
            Self::DiamondOre => [0.45, 0.86, 0.84, 1.0],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Air => "Air",
            Self::Grass => "Meadow grass",
            Self::Dirt => "Earth",
            Self::Stone => "Limestone",
            Self::Sand => "River sand",
            Self::Water => "Water",
            Self::Wood => "Timber",
            Self::Leaves => "Canopy",
            Self::Snow => "Alpine snow",
            Self::Clay => "Terracotta",
            Self::Crystal => "Lumen crystal",
            Self::Basalt => "Basalt",
            Self::CoalOre => "Coal ore",
            Self::IronOre => "Iron ore",
            Self::GoldOre => "Gold ore",
            Self::DiamondOre => "Diamond ore",
        }
    }

    pub fn is_solid(self) -> bool {
        !matches!(self, Self::Air | Self::Water)
    }

    pub fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::Air,
            1 => Self::Grass,
            2 => Self::Dirt,
            3 => Self::Stone,
            4 => Self::Sand,
            5 => Self::Water,
            6 => Self::Wood,
            7 => Self::Leaves,
            8 => Self::Snow,
            9 => Self::Clay,
            10 => Self::Crystal,
            11 => Self::Basalt,
            12 => Self::CoalOre,
            13 => Self::IronOre,
            14 => Self::GoldOre,
            15 => Self::DiamondOre,
            _ => return None,
        })
    }
}

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
}

#[derive(Clone, Copy)]
struct Tree {
    x: i32,
    z: i32,
    ground: i32,
    height: i32,
}

impl Tree {
    fn block(self, x: i32, y: i32, z: i32) -> Block {
        let dx = (i64::from(x) - i64::from(self.x)).abs();
        let dz = (i64::from(z) - i64::from(self.z)).abs();
        let dy = y - self.ground;
        if !(0..=i64::from(TREE_CANOPY)).contains(&dx.max(dz)) {
            return Block::Air;
        }
        if dx == 0 && dz == 0 && dy > 0 && dy <= self.height {
            return Block::Wood;
        }
        let crown = dy - self.height;
        if (-2..=TREE_CROWN_ABOVE).contains(&crown)
            && dx * dx + dz * dz + i64::from(crown * crown) <= 6
        {
            return Block::Leaves;
        }
        Block::Air
    }
}

struct OreRule {
    salt: u64,
    cell: i32,
    min_y: i32,
    max_y: i32,
    chance: u64,
    radius_sq: i32,
    block: Block,
}

const ORE_RULES: [OreRule; 4] = [
    OreRule {
        salt: 0xD1A0,
        cell: 13,
        min_y: 1,
        max_y: 12,
        chance: 8,
        radius_sq: 1,
        block: Block::DiamondOre,
    },
    OreRule {
        salt: 0x60D0,
        cell: 11,
        min_y: 1,
        max_y: 22,
        chance: 14,
        radius_sq: 1,
        block: Block::GoldOre,
    },
    OreRule {
        salt: 0x1A0,
        cell: 9,
        min_y: 1,
        max_y: 46,
        chance: 34,
        radius_sq: 2,
        block: Block::IronOre,
    },
    OreRule {
        salt: 0xC0A1,
        cell: 8,
        min_y: 6,
        max_y: 70,
        chance: 46,
        radius_sq: 2,
        block: Block::CoalOre,
    },
];

impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            edits: HashMap::new(),
        }
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

    pub fn edits(&self) -> Vec<([i32; 3], u8)> {
        let mut edits: Vec<_> = self
            .edits
            .iter()
            .map(|(&position, &block)| (position, block as u8))
            .collect();
        edits.sort_unstable_by_key(|&(position, _)| position);
        edits
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
        }
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
        for rule in &ORE_RULES {
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
                            return Some(rule.block);
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
        if y > column.height {
            return if y <= SEA_LEVEL {
                Block::Water
            } else {
                Block::Air
            };
        }
        if y < 1 {
            return Block::Basalt;
        }
        if self.carves(x, y, z, column) {
            return Block::Air;
        }
        let depth = column.height - y;
        let sandy = self.sandy(x, z, column);
        if depth == 0 {
            if sandy { Block::Sand } else { Block::Grass }
        } else if depth < 4 {
            if sandy { Block::Sand } else { Block::Dirt }
        } else {
            self.ore_at(x, y, z).unwrap_or(Block::Stone)
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
        })
    }

    fn foliage_at(&self, x: i32, y: i32, z: i32) -> Block {
        let mut foliage = Block::Air;
        let min_z = (z - TREE_CANOPY).div_euclid(TREE_GRID);
        let max_z = (z + TREE_CANOPY).div_euclid(TREE_GRID);
        let min_x = (x - TREE_CANOPY).div_euclid(TREE_GRID);
        let max_x = (x + TREE_CANOPY).div_euclid(TREE_GRID);
        for gz in min_z..=max_z {
            for gx in min_x..=max_x {
                if let Some(tree) = self.tree(gx, gz) {
                    let block = tree.block(x, y, z);
                    if block == Block::Wood {
                        return block;
                    }
                    if block == Block::Leaves {
                        foliage = block;
                    }
                }
            }
        }
        foliage
    }

    fn generated_block(&self, x: i32, y: i32, z: i32) -> Block {
        if y < 0 {
            return Block::Basalt;
        }
        if y > 128 {
            return Block::Air;
        }
        let column = self.column(x, z);
        let terrain = self.terrain_block(x, y, z, column);
        if y <= column.height || terrain != Block::Air {
            return terrain;
        }
        self.foliage_at(x, y, z)
    }

    /// Mesh positions are world-space f32. Camera-relative large-world rendering
    /// is a later renderer contract; avoid claiming precision beyond 2^24 blocks.
    pub fn mesh_chunk(&self, key: ChunkKey) -> Mesh {
        let Some(origin) = chunk_origin(key) else {
            return Mesh::default();
        };
        let halo = self.sample_halo(origin);
        mesh_halo(origin, &halo)
    }

    fn sample_halo(&self, origin: [i32; 3]) -> Vec<Block> {
        let mut blocks = vec![Block::Air; HALO_VOLUME];
        let mut heights = vec![0; HALO_SIDE * HALO_SIDE];
        // Compute a column once, rather than running fBm for every voxel or face.
        for z in -1..=CHUNK_SIZE {
            for x in -1..=CHUNK_SIZE {
                let wx = origin[0] + x;
                let wz = origin[2] + z;
                let column = self.column(wx, wz);
                heights[(x + 1) as usize + (z + 1) as usize * HALO_SIDE] = column.height;
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
                            if blocks[index] == Block::Air
                                || (blocks[index] == Block::Leaves && block == Block::Wood)
                            {
                                blocks[index] = block;
                            }
                        }
                    }
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

fn mesh_halo(origin: [i32; 3], halo: &[Block]) -> Mesh {
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
                        let block = halo[halo_index(p)];
                        let mut outside = p;
                        outside[axis] += sign;
                        let neighbor = halo[halo_index(outside)];
                        let visible = if block == Block::Water {
                            neighbor == Block::Air
                        } else {
                            block.is_solid() && !neighbor.is_solid()
                        };
                        mask[i as usize + j as usize * side] = if visible {
                            let ao = corner_ao(halo, outside, u, v);
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

fn corner_ao(halo: &[Block], outside: [i32; 3], u: usize, v: usize) -> [u8; 4] {
    let mut ao = [3; 4];
    for (corner, (du, dv)) in [(-1, -1), (1, -1), (1, 1), (-1, 1)].into_iter().enumerate() {
        let mut side_u = outside;
        let mut side_v = outside;
        let mut diagonal = outside;
        side_u[u] += du;
        side_v[v] += dv;
        diagonal[u] += du;
        diagonal[v] += dv;
        let a = u8::from(halo[halo_index(side_u)].is_solid());
        let b = u8::from(halo[halo_index(side_v)].is_solid());
        let c = u8::from(halo[halo_index(diagonal)].is_solid());
        ao[corner] = if a == 1 && b == 1 { 0 } else { 3 - a - b - c };
    }
    ao
}

fn emit_quad(mesh: &mut Mesh, p: [i32; 3], axis: usize, sign: i32, size: [f32; 2], face: Face) {
    let base = mesh.vertices.len() as u32;
    let u = (axis + 1) % 3;
    let v = (axis + 2) % 3;
    let mut normal = [0.0; 3];
    normal[axis] = sign as f32;
    let texture_layer = crate::materials::layer_for(face.block, normal.map(|v| v as i32));
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
            // Texture owns albedo. Vertex RGB carries only linear AO/variation.
            color: [brightness, brightness, brightness, face.block.color()[3]],
            texture_layer,
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
        let mut halo = vec![Block::Air; HALO_VOLUME];
        for &(p, block) in cells {
            halo[halo_index(p)] = block;
        }
        mesh_halo([0; 3], &halo)
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
        ((-1, 0, -8), 0xbdec_8b26_2b9a_9845, "ocean"),
        ((20, 1, -25), 0x26f9_ca54_ab95_53c9, "desert surface"),
        ((-2, 1, -2), 0xc802_ddd2_84ce_1d27, "forest canopy"),
        ((-5, 0, -6), 0xf385_23ea_2448_a242, "caves and ores"),
        (
            (-25, 0, -27),
            0x7ec8_79e1_b84f_eb60,
            "far negative caves and ores",
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
                assert!(count(Block::Water) > 1000, "{key:?} should be ocean");
                assert_eq!(count(Block::Grass), 0, "{key:?} ocean cover");
            }
            "desert surface" => {
                assert!(count(Block::Sand) > 1000, "{key:?} should be desert");
                assert_eq!(count(Block::Grass), 0, "{key:?} desert cover");
                assert_eq!(count(Block::Wood), 0, "{key:?} desert trees");
            }
            "forest canopy" => {
                assert!(count(Block::Wood) > 0, "{key:?} should contain a trunk");
                assert!(count(Block::Leaves) > 0, "{key:?} should contain leaves");
            }
            "caves and ores" | "far negative caves and ores" => {
                assert!(
                    key.0 < 0 && key.2 < 0,
                    "{label} should use negative coordinates"
                );
                assert!(count(Block::CoalOre) > 0, "{key:?} coal");
                assert!(count(Block::IronOre) > 0, "{key:?} iron");
                assert!(count(Block::GoldOre) > 0, "{key:?} gold");
                assert!(count(Block::DiamondOre) > 0, "{key:?} diamond");
                let deep_air = blocks.iter().enumerate().any(|(index, block)| {
                    let y = (index / (CHUNK_SIZE * CHUNK_SIZE) as usize) as i32;
                    y <= 12 && *block == Block::Air
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
        bytes.extend(blocks.iter().map(|block| *block as u8));
        bytes
    }

    #[test]
    fn all_material_ids_round_trip_and_unknown_ids_are_rejected() {
        let mut id = 0_u8;
        while let Some(block) = Block::from_id(id) {
            assert_eq!(block as u8, id);
            assert!(!block.name().is_empty());
            assert!(block.color().iter().all(|channel| channel.is_finite()));
            id += 1;
        }
        assert!(id > 15, "ore ids must stay registered");
        assert_eq!(Block::from_id(id), None);
        assert_eq!(Block::from_id(255), None);
        assert!(!Block::Water.is_solid());
        assert!(Block::DiamondOre.is_solid());
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
            world.set_block(p, Block::Crystal);
            assert_eq!(world.block_at(p[0], p[1], p[2]), Block::Crystal);
        }
        let snapshot = world.clone();
        world.set_block(positions[0], Block::Air);
        assert_eq!(world.edits().len(), 2);
        assert_eq!(snapshot.edits().len(), 3);
        assert_eq!(world.edits()[0].0, [-1, 150, 32]);
        assert_eq!(world.edits()[1].0, [0, 150, 0]);
    }

    #[test]
    fn empty_single_and_adjacent_voxels_have_expected_surface_area() {
        assert!(artificial_mesh(&[]).indices.is_empty());
        let single = artificial_mesh(&[([8, 8, 8], Block::Stone)]);
        assert_eq!(single.indices.len(), 36);
        assert_eq!(triangle_area(&single), 6.0);
        let pair = artificial_mesh(&[([8, 8, 8], Block::Stone), ([9, 8, 8], Block::Stone)]);
        assert_eq!(triangle_area(&pair), 10.0);
        assert_eq!(pair.indices.len(), 36, "coplanar equal-lit faces merge");
    }

    #[test]
    fn chunk_halo_culls_internal_border_faces() {
        let edge = artificial_mesh(&[([31, 8, 8], Block::Stone), ([32, 8, 8], Block::Stone)]);
        assert_eq!(triangle_area(&edge), 5.0);
        assert!(
            edge.vertices
                .iter()
                .all(|vertex| vertex.normal != [1.0, 0.0, 0.0])
        );
        let negative = artificial_mesh(&[([0, 8, 8], Block::Stone), ([-1, 8, 8], Block::Stone)]);
        assert_eq!(triangle_area(&negative), 5.0);
    }

    #[test]
    fn ambient_occlusion_darkens_a_concave_corner() {
        let mut halo = vec![Block::Air; HALO_VOLUME];
        // Sample the top of a voxel at [8,8,8], with two adjacent walls.
        halo[halo_index([7, 9, 8])] = Block::Stone;
        halo[halo_index([8, 9, 7])] = Block::Stone;
        let ao = corner_ao(&halo, [8, 9, 8], 0, 2);
        assert_eq!(ao[0], 0, "two occluding side cells close the corner");
        assert_eq!(ao[2], 3, "opposite open corner receives full ambient light");
    }

    #[test]
    fn emitted_grass_and_timber_faces_select_their_distinct_material_tiles() {
        for (block, top, side, bottom) in [(Block::Grass, 0, 1, 2), (Block::Wood, 7, 6, 7)] {
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
        let mesh = artificial_mesh(&[([8, 8, 8], Block::Stone), ([9, 9, 8], Block::Wood)]);
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
        let mesh = artificial_mesh(&[([8, 8, 8], Block::Water), ([9, 8, 8], Block::Water)]);
        assert_eq!(triangle_area(&mesh), 10.0);
        let mut world = World::new(1);
        world.set_block([1, 150, 0], Block::Water);
        world.set_block([2, 150, 0], Block::Stone);
        let hit = world
            .raycast([0.5, 150.5, 0.5], [1.0, 0.0, 0.0], 5.0)
            .unwrap();
        assert_eq!(hit.block, [2, 150, 0]);
    }

    #[test]
    fn raycast_handles_negative_cells_scaled_direction_and_range() {
        let mut world = World::new(1);
        world.set_block([-3, 150, -1], Block::Stone);
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
        world.set_block([1, 151, 1], Block::Stone);
        world.set_block([1, 150, 0], Block::Wood);
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
        world.set_block([-1, 42, -32], Block::Crystal);
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
                Block::Wood,
                "trunk must be continuous"
            );
        }
        assert_eq!(
            world.block_at(tree.x, tree.ground + tree.height + 1, tree.z),
            Block::Leaves
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
                    if block == Block::Leaves || block == Block::Wood {
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
                        assert_eq!(world.block_at(x, SEA_LEVEL, z), Block::Water);
                        assert_eq!(surface, Block::Sand);
                        assert_eq!(world.block_at(x, height - 1, z), Block::Sand);
                        for dy in 0..6 {
                            let block = world.block_at(x, height - dy, z);
                            assert!(block.is_solid(), "ocean floor opened at y={}", height - dy);
                        }
                        for y in height + 1..=SEA_LEVEL {
                            assert_eq!(world.block_at(x, y, z), Block::Water);
                            for [nx, nz] in [[1, 0], [-1, 0], [0, 1], [0, -1]] {
                                let neighbor = world.block_at(x + nx, y, z + nz);
                                if neighbor == Block::Air {
                                    flooded += 1;
                                }
                            }
                            assert_ne!(world.block_at(x, y - 1, z), Block::Air);
                        }
                    }
                    Biome::Beach | Biome::Desert => {
                        desert_seen += usize::from(biome == Biome::Desert);
                        desert_sand +=
                            usize::from(biome == Biome::Desert && surface == Block::Sand);
                        assert_eq!(surface, Block::Sand);
                        assert!(height >= SEA_LEVEL);
                    }
                    Biome::Forest => {
                        forest_seen += 1;
                        forest_grass += usize::from(surface == Block::Grass);
                        assert!(height > SEA_LEVEL);
                    }
                    Biome::Plains => {
                        plains_seen += 1;
                        plains_grass += usize::from(surface == Block::Grass);
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
                assert_eq!(world.block_at(tree.x, tree.ground, tree.z), Block::Grass);
                for dy in 1..=tree.height {
                    assert_eq!(
                        world.block_at(tree.x, tree.ground + dy, tree.z),
                        Block::Wood
                    );
                }
                assert_eq!(
                    world.block_at(tree.x + 1, tree.ground + tree.height, tree.z),
                    Block::Leaves
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
                            Block::CoalOre => {
                                assert!((6..=70).contains(&y));
                                coal += 1;
                            }
                            Block::IronOre => {
                                assert!((1..=46).contains(&y));
                                iron += 1;
                            }
                            Block::GoldOre => {
                                assert!((1..=22).contains(&y));
                                gold += 1;
                            }
                            Block::DiamondOre => {
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
                assert_ne!(world.block_at(x, height, z), Block::Air);
                for y in 2..height {
                    if world.block_at(x, y, z) == Block::Air {
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
        let keys: Vec<ChunkKey> = (-1..=1)
            .flat_map(|z| (-1..=1).map(move |x| (x, 0, z)))
            .collect();
        for key in &keys {
            std::hint::black_box(world.generate_chunk(*key));
        }
        let rounds = 2;
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            for key in &keys {
                std::hint::black_box(world.generate_chunk(*key));
            }
        }
        let average_us = start.elapsed().as_secs_f64() * 1_000_000.0 / (keys.len() * rounds) as f64;
        eprintln!(
            "STRATA_CHUNK_GEN_AVG_US {average_us:.1} chunks={} rounds={rounds}",
            keys.len()
        );
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
                let biome = biome_rgb(world.biome_at(wx, wz), height);
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
            for x in (origin_x..origin_x + PREVIEW_SPAN).step_by(4) {
                match world.biome_at(x, z) {
                    Biome::Ocean => saw_ocean = true,
                    Biome::Beach => {}
                    _ => saw_land = true,
                }
                let height = world.surface_height(x, z).min(72);
                for y in 1..height {
                    match world.block_at(x, y, z) {
                        Block::Air => caves += 1,
                        Block::CoalOre => coal += 1,
                        Block::IronOre => iron += 1,
                        Block::GoldOre => gold += 1,
                        Block::DiamondOre => diamond += 1,
                        _ => {}
                    }
                }
            }
            if saw_ocean && saw_land && caves > 0 && coal > 0 && iron > 0 && gold > 0 && diamond > 0
            {
                let score = caves.min(40) + coal.min(20) + iron.min(20) + gold * 3 + diamond * 5;
                if score > best.1 {
                    best = (z, score);
                }
            }
        }
        assert!(best.1 > 0, "preview slice missed caves or an ore band");
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
            Block::Air => {
                if y > SEA_LEVEL {
                    [186, 214, 232]
                } else {
                    [28, 22, 36]
                }
            }
            Block::Grass => [74, 150, 58],
            Block::Dirt => [122, 84, 52],
            Block::Stone => [132, 136, 134],
            Block::Sand => [214, 196, 132],
            Block::Water => [42, 112, 168],
            Block::Wood => [112, 74, 42],
            Block::Leaves => [48, 124, 54],
            Block::Snow => [230, 236, 238],
            Block::Clay => [168, 86, 56],
            Block::Crystal => [72, 210, 214],
            Block::Basalt => [48, 52, 58],
            Block::CoalOre => [42, 42, 46],
            Block::IronOre => [176, 110, 78],
            Block::GoldOre => [214, 176, 64],
            Block::DiamondOre => [80, 220, 210],
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
