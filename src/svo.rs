//! Sparse Voxel Octree over 8³ data-chunk leaves.
//!
//! All-air leaves store no 512-cell buffer: voxel payload is 0. Branch and
//! empty index nodes may exist so the tree can record a resolved empty region
//! without allocating voxel bytes. That index is not the payload.

use std::collections::HashMap;

use crate::world::{Block, DATA_CHUNK_SIZE, DataChunkKey, data_chunk_key, data_chunk_origin};

pub const LEAF_CELLS: usize = (DATA_CHUNK_SIZE * DATA_CHUNK_SIZE * DATA_CHUNK_SIZE) as usize;

/// Data-chunk span of one root cube. 16³ leaves = 128 voxels on a side.
const ROOT_LEVEL: u32 = 4;
const ROOT_SPAN: i32 = 1 << ROOT_LEVEL;

#[derive(Clone, Debug, Default)]
enum Node {
    /// Resolved all-air. No voxel buffer.
    #[default]
    Empty,
    /// Mixed or solid 8³. Exactly 512 block ids.
    Leaf(Box<[u8; LEAF_CELLS]>),
    Branch(Box<[Node; 8]>),
}

#[derive(Clone, Debug, Default)]
pub struct SparseVoxelOctree {
    roots: HashMap<[i32; 3], Node>,
}

impl SparseVoxelOctree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes of stored voxel ids for this 8³. Empty/absent leaves are 0.
    pub fn voxel_payload_bytes(&self, key: DataChunkKey) -> usize {
        match self.leaf_cells(key) {
            Some(cells) => std::mem::size_of_val(cells.as_ref()),
            None => 0,
        }
    }

    #[allow(dead_code)]
    pub fn get(&self, position: [i32; 3]) -> Option<Block> {
        let key = data_chunk_key(position);
        let cells = self.leaf_cells(key)?;
        Block::from_id(cells[leaf_index(position, key)])
    }

    /// Replace the 8³ payload. `None` stores an empty leaf (0 voxel bytes).
    pub fn store_leaf(&mut self, key: DataChunkKey, cells: Option<[u8; LEAF_CELLS]>) {
        match cells {
            None => self.write_node(key, Node::Empty),
            Some(cells) => {
                if cells.iter().all(|&id| id == 0) {
                    self.write_node(key, Node::Empty);
                } else {
                    self.write_node(key, Node::Leaf(Box::new(cells)));
                }
            }
        }
    }

    fn leaf_cells(&self, key: DataChunkKey) -> Option<&[u8; LEAF_CELLS]> {
        let root_key = root_key_of(key);
        let mut node = self.roots.get(&root_key)?;
        let mut origin = root_origin(root_key);
        let mut span = ROOT_SPAN;
        loop {
            match node {
                Node::Empty => return None,
                Node::Leaf(cells) => {
                    if span == 1 {
                        return Some(cells);
                    }
                    return None;
                }
                Node::Branch(children) => {
                    if span <= 1 {
                        return None;
                    }
                    let octant = octant_of(origin, span, key);
                    origin = child_origin(origin, span, octant);
                    span /= 2;
                    node = &children[octant];
                }
            }
        }
    }

    fn write_node(&mut self, key: DataChunkKey, value: Node) {
        let root_key = root_key_of(key);
        let root = self.roots.entry(root_key).or_insert(Node::Empty);
        insert_leaf(root, root_origin(root_key), ROOT_SPAN, key, value);
    }
}

fn root_key_of(key: DataChunkKey) -> [i32; 3] {
    [
        key.0.div_euclid(ROOT_SPAN),
        key.1.div_euclid(ROOT_SPAN),
        key.2.div_euclid(ROOT_SPAN),
    ]
}

fn root_origin(root_key: [i32; 3]) -> [i32; 3] {
    [
        root_key[0] * ROOT_SPAN,
        root_key[1] * ROOT_SPAN,
        root_key[2] * ROOT_SPAN,
    ]
}

fn octant_of(origin: [i32; 3], span: i32, key: DataChunkKey) -> usize {
    let half = span / 2;
    let mut octant = 0;
    if key.0 >= origin[0] + half {
        octant |= 1;
    }
    if key.1 >= origin[1] + half {
        octant |= 2;
    }
    if key.2 >= origin[2] + half {
        octant |= 4;
    }
    octant
}

fn child_origin(origin: [i32; 3], span: i32, octant: usize) -> [i32; 3] {
    let half = span / 2;
    [
        origin[0] + i32::from(octant & 1 != 0) * half,
        origin[1] + i32::from(octant & 2 != 0) * half,
        origin[2] + i32::from(octant & 4 != 0) * half,
    ]
}

fn insert_leaf(node: &mut Node, origin: [i32; 3], span: i32, key: DataChunkKey, value: Node) {
    if span == 1 {
        *node = value;
        return;
    }
    if matches!(node, Node::Empty | Node::Leaf(_)) {
        *node = Node::Branch(Box::new(std::array::from_fn(|_| Node::Empty)));
    }
    let Node::Branch(children) = node else {
        *node = value;
        return;
    };
    let octant = octant_of(origin, span, key);
    insert_leaf(
        &mut children[octant],
        child_origin(origin, span, octant),
        span / 2,
        key,
        value,
    );
}

#[allow(dead_code)]
fn leaf_index(position: [i32; 3], key: DataChunkKey) -> usize {
    let origin = data_chunk_origin(key);
    let x = (position[0] - origin[0]) as usize;
    let y = (position[1] - origin[1]) as usize;
    let z = (position[2] - origin[2]) as usize;
    x + (DATA_CHUNK_SIZE as usize) * (z + (DATA_CHUNK_SIZE as usize) * y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_leaf_has_zero_voxel_payload_and_solid_leaf_is_512_bytes() {
        let mut svo = SparseVoxelOctree::new();
        let air = (3, 40, -2);
        svo.store_leaf(air, None);
        assert_eq!(svo.voxel_payload_bytes(air), 0);
        assert!(svo.get(data_chunk_origin(air)).is_none());

        let mut cells = [Block::Stone as u8; LEAF_CELLS];
        cells[0] = Block::Dirt as u8;
        let filled = (3, 40, -1);
        svo.store_leaf(filled, Some(cells));
        assert_eq!(svo.voxel_payload_bytes(filled), LEAF_CELLS);
        let origin = data_chunk_origin(filled);
        assert_eq!(svo.get(origin), Some(Block::Dirt));
        assert_eq!(
            svo.get([origin[0] + 1, origin[1], origin[2]]),
            Some(Block::Stone)
        );
    }
}
