//! Synchronous render-thread protocol. Only Commit changes visible residency.
use crate::world::ChunkKey;

#[derive(Debug)]
pub enum UploadCommand<'a> {
    Begin {
        key: ChunkKey,
        vertex_bytes: usize,
        index_count: usize,
    },
    Vertices {
        offset: usize,
        data: &'a [u8],
    },
    Indices {
        offset: usize,
        data: &'a [u8],
    },
    Commit,
    Cancel,
}
