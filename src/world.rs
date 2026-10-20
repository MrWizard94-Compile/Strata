//! Deterministic procedural terrain, sparse edits, DDA selection, and greedy meshes.
//!
//! Generation is a pure function of seed and integer coordinates. Each mesh samples
//! a one-cell halo so visibility and ambient occlusion agree across chunk borders.
//! Trees are anchored in a global grid and are never owned by the first chunk that
//! happens to load. No cache survives an edit; worker jobs should own a World clone.

use std::collections::HashMap;

pub const CHUNK_SIZE: i32 = 32;
pub const SEA_LEVEL: i32 = 28;
pub type ChunkKey = (i32, i32, i32);

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
            _ => return None,
        })
    }
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
    moisture: f64,
    warm: f64,
    river: f64,
}

#[derive(Clone, Copy)]
struct Tree {
    x: i32,
    z: i32,
    ground: i32,
    height: i32,
    pine: bool,
}

impl Tree {
    fn block(self, x: i32, y: i32, z: i32) -> Block {
        let dx = (i64::from(x) - i64::from(self.x)).abs();
        let dz = (i64::from(z) - i64::from(self.z)).abs();
        let dy = y - self.ground;
        if dx == 0 && dz == 0 && dy > 0 && dy <= self.height {
            return Block::Wood;
        }
        if self.pine {
            if dy >= 3 && dy <= self.height + 2 {
                let radius = ((self.height + 3 - dy) / 2).clamp(0, 3) as i64;
                if dx.max(dz) <= radius && dx + dz <= radius + 1 {
                    return Block::Leaves;
                }
            }
        } else {
            let crown_y = dy - self.height;
            if (-2..=2).contains(&crown_y) && dx * dx + dz * dz + i64::from(crown_y * crown_y) <= 11
            {
                return Block::Leaves;
            }
        }
        Block::Air
    }
}

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

    fn column(&self, x: i32, z: i32) -> Column {
        let x = f64::from(x);
        let z = f64::from(z);
        let moisture = fbm2(self.seed ^ 0x1959, x / 175.0, z / 175.0, 3);
        let warm = noise2(self.seed ^ 0xA713, x / 240.0, z / 240.0);
        let broad = fbm2(self.seed ^ 0x72F8, x / 135.0, z / 135.0, 4);
        let ridge = 1.0 - noise2(self.seed ^ 0x9137, x / 92.0, z / 92.0).abs();
        let mountain_mask = smoothstep(0.0, 0.6, broad);
        let mountains = ridge.powi(3) * mountain_mask * 68.0;
        let hills = fbm2(self.seed ^ 0x82A7, x / 43.0, z / 43.0, 3) * 8.0;
        let base = 40.0 + broad * 19.0 + hills + mountains;

        // A continuous meandering river is readable at the starting vista. Noise
        // supplies tributaries beyond it without chunk-local discontinuities.
        let center = (z / 115.0).sin() * 29.0 + noise2(self.seed ^ 0xC171, z / 170.0, 1.7) * 24.0;
        let river_distance = (x - center).abs();
        let river = 1.0 - smoothstep(6.0, 23.0, river_distance);
        let riverbed = 23.0 + noise2(self.seed ^ 0xD441, x / 19.0, z / 19.0) * 2.0;
        let height = (base * (1.0 - river) + riverbed * river)
            .round()
            .clamp(12.0, 117.0) as i32;
        Column {
            height,
            moisture,
            warm,
            river,
        }
    }

    fn terrain_block(&self, x: i32, y: i32, z: i32, column: Column) -> Block {
        if y > column.height {
            return if y <= SEA_LEVEL {
                Block::Water
            } else {
                Block::Air
            };
        }
        if y < 0 {
            return Block::Basalt;
        }
        if y > 4 && y < column.height - 4 {
            let nx = f64::from(x) / 27.0;
            let ny = f64::from(y) / 21.0;
            let nz = f64::from(z) / 27.0;
            let tunnel = noise3(self.seed ^ 0xBE93, nx, ny, nz);
            if tunnel.abs() < 0.105 {
                let cross = noise3(self.seed ^ 0x51F1, nx * 1.3, ny * 0.75, nz * 1.3);
                if cross.abs() < 0.19 {
                    return Block::Air;
                }
            }
        }
        let depth = column.height - y;
        if depth == 0 {
            if column.height >= 85 {
                Block::Snow
            } else if column.height <= SEA_LEVEL + 1 || column.river > 0.72 {
                Block::Sand
            } else if column.warm > 0.35 && column.moisture < -0.08 {
                Block::Clay
            } else if column.height >= 68 {
                Block::Stone
            } else {
                Block::Grass
            }
        } else if depth < 4 {
            if column.height <= SEA_LEVEL + 1 {
                Block::Sand
            } else if column.warm > 0.35 && column.moisture < -0.08 {
                Block::Clay
            } else {
                Block::Dirt
            }
        } else if y < 26 && hash3(self.seed ^ 0xCA77, x, y, z).is_multiple_of(179) {
            Block::Crystal
        } else if y < 10 || (y / 7).rem_euclid(8) == 0 {
            Block::Basalt
        } else {
            Block::Stone
        }
    }

    fn tree(&self, grid_x: i32, grid_z: i32) -> Option<Tree> {
        let hash = hash3(self.seed ^ 0x7AEE, grid_x, 0, grid_z);
        let x = grid_x.checked_mul(12)?.checked_add(2 + (hash % 8) as i32)?;
        let z = grid_z
            .checked_mul(12)?
            .checked_add(2 + ((hash >> 8) % 8) as i32)?;
        let column = self.column(x, z);
        if column.height <= SEA_LEVEL + 3
            || column.height > 89
            || column.river > 0.3
            || column.moisture < -0.25
            || hash % 100 > if column.moisture > 0.1 { 78 } else { 43 }
        {
            return None;
        }
        let beside = self.column(x.saturating_add(2), z.saturating_add(2));
        if (beside.height - column.height).abs() > 3 {
            return None;
        }
        Some(Tree {
            x,
            z,
            ground: column.height,
            height: 5 + ((hash >> 16) % 5) as i32,
            pine: column.height > 60 || column.warm < -0.2,
        })
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
        if terrain != Block::Air || y <= column.height {
            return terrain;
        }
        let mut foliage = Block::Air;
        for gz in z.div_euclid(12) - 1..=z.div_euclid(12) + 1 {
            for gx in x.div_euclid(12) - 1..=x.div_euclid(12) + 1 {
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
        let min_x = (origin[0] - 4).div_euclid(12);
        let max_x = (origin[0] + CHUNK_SIZE + 3).div_euclid(12);
        let min_z = (origin[2] - 4).div_euclid(12);
        let max_z = (origin[2] + CHUNK_SIZE + 3).div_euclid(12);
        for gz in min_z..=max_z {
            for gx in min_x..=max_x {
                if origin[1] > 128 || origin[1] + CHUNK_SIZE < 0 {
                    continue;
                }
                let Some(tree) = self.tree(gx, gz) else {
                    continue;
                };
                let start_y = (tree.ground + 1 - origin[1]).max(-1);
                let end_y = (tree.ground + tree.height + 2 - origin[1]).min(CHUNK_SIZE);
                for z in (tree.z - 3 - origin[2]).max(-1)..=(tree.z + 3 - origin[2]).min(CHUNK_SIZE)
                {
                    for x in
                        (tree.x - 3 - origin[0]).max(-1)..=(tree.x + 3 - origin[0]).min(CHUNK_SIZE)
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
    let ix = x.floor() as i32;
    let iz = z.floor() as i32;
    let fx = fade(x - x.floor());
    let fz = fade(z - z.floor());
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
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let iz = z.floor() as i32;
    let fx = fade(x - x.floor());
    let fy = fade(y - y.floor());
    let fz = fade(z - z.floor());
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
        result += noise2(seed.wrapping_add(u64::from(octave) * 7919), x, z) * amplitude;
        amplitude *= 0.5;
        x *= 2.03;
        z *= 2.03;
    }
    result
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
    fn all_material_ids_round_trip_and_unknown_ids_are_rejected() {
        for id in 0..=11 {
            let block = Block::from_id(id).expect("known material");
            assert_eq!(block as u8, id);
            assert!(!block.name().is_empty());
        }
        assert_eq!(Block::from_id(12), None);
        assert_eq!(Block::from_id(255), None);
        assert!(!Block::Water.is_solid());
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
                assert!((12..=117).contains(&a.surface_height(x, z)));
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
        let tree = (-16..16)
            .find_map(|gz| {
                (-16..16).find_map(|gx| {
                    world
                        .tree(gx, gz)
                        .filter(|tree| tree.x.rem_euclid(CHUNK_SIZE) <= 2)
                })
            })
            .expect("seeded forest includes a boundary tree");
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
        let mut foliage = 0;
        for y in right_origin[1]..right_origin[1] + CHUNK_SIZE {
            for z in right_origin[2]..right_origin[2] + CHUNK_SIZE {
                let block = world.block_at(boundary, y, z);
                foliage += usize::from(block == Block::Leaves || block == Block::Wood);
                assert_eq!(
                    right[halo_index([0, y - right_origin[1], z - right_origin[2]])],
                    block
                );
                assert_eq!(
                    left[halo_index([CHUNK_SIZE, y - left_origin[1], z - left_origin[2]])],
                    block
                );
            }
        }
        assert!(foliage > 0, "boundary must contain actual tree geometry");
    }

    #[test]
    fn cave_cells_and_river_water_exist_in_generated_world() {
        let world = World::new(31415);
        let mut caves = 0;
        let mut river = 0;
        for x in (-64..64).step_by(4) {
            for z in (-64..64).step_by(4) {
                let column = world.column(x, z);
                river += usize::from(world.block_at(x, SEA_LEVEL, z) == Block::Water);
                for y in (5..column.height - 4).step_by(3) {
                    caves += usize::from(world.block_at(x, y, z) == Block::Air);
                }
            }
        }
        assert!(
            caves > 20,
            "seed must produce explorable underground volume"
        );
        assert!(river > 50, "seed must produce river surface");
    }
}
