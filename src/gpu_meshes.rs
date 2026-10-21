//! One incomplete upload plus atomically replaceable resident meshes.
use std::{collections::HashMap, mem::size_of};

use glam::Vec3;

use crate::{
    upload::UploadCommand,
    world::{CHUNK_SIZE, ChunkKey, Vertex},
};

pub struct GpuChunk {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub index_count: u32,
    pub payload_bytes: usize,
    pub min: Vec3,
    pub max: Vec3,
}

struct Pending {
    key: ChunkKey,
    chunk: Option<GpuChunk>,
    vertex_bytes: usize,
    index_bytes: usize,
    vertices_written: usize,
    indices_written: usize,
}

#[derive(Default)]
pub struct GpuMeshes {
    pub resident: HashMap<ChunkKey, GpuChunk>,
    pending: Option<Pending>,
}

fn chunk_bounds(key: ChunkKey) -> (Vec3, Vec3) {
    // The world mesher emits block faces within this cube. Conservative bounds
    // avoid scanning every vertex again on the render thread.
    let min = Vec3::new(key.0 as f32, key.1 as f32, key.2 as f32) * CHUNK_SIZE as f32;
    (min, min + Vec3::splat(CHUNK_SIZE as f32))
}

impl GpuMeshes {
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        command: UploadCommand<'_>,
    ) -> Result<(), String> {
        match command {
            UploadCommand::Begin {
                key,
                vertex_bytes,
                index_count,
            } => {
                if self.pending.is_some() {
                    return Err("A mesh upload is already in progress".into());
                }
                let index_bytes = index_count
                    .checked_mul(size_of::<u32>())
                    .ok_or("Mesh index size overflow")?;
                let payload_bytes = vertex_bytes
                    .checked_add(index_bytes)
                    .ok_or("Mesh payload size overflow")?;
                let count = u32::try_from(index_count).map_err(|_| "Too many mesh indices")?;
                if !vertex_bytes.is_multiple_of(size_of::<Vertex>())
                    || !index_count.is_multiple_of(3)
                    || (vertex_bytes == 0) != (index_count == 0)
                {
                    return Err("Invalid mesh vertex/index layout".into());
                }
                let limit = device.limits().max_buffer_size;
                if vertex_bytes as u64 > limit || index_bytes as u64 > limit {
                    return Err("Mesh exceeds the graphics device buffer limit".into());
                }
                let chunk = (vertex_bytes != 0).then(|| {
                    let create = |label, size, usage| {
                        // Readback usage is test-only; production buffers only need drawing/copy-in.
                        let usage = usage | wgpu::BufferUsages::COPY_DST;
                        #[cfg(test)]
                        let usage = usage | wgpu::BufferUsages::COPY_SRC;
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some(label),
                            size: size as u64,
                            usage,
                            mapped_at_creation: false,
                        })
                    };
                    let (min, max) = chunk_bounds(key);
                    GpuChunk {
                        vertices: create(
                            "Pending chunk vertices",
                            vertex_bytes,
                            wgpu::BufferUsages::VERTEX,
                        ),
                        indices: create(
                            "Pending chunk indices",
                            index_bytes,
                            wgpu::BufferUsages::INDEX,
                        ),
                        index_count: count,
                        payload_bytes,
                        min,
                        max,
                    }
                });
                self.pending = Some(Pending {
                    key,
                    chunk,
                    vertex_bytes,
                    index_bytes,
                    vertices_written: 0,
                    indices_written: 0,
                });
            }
            UploadCommand::Vertices { offset, data } | UploadCommand::Indices { offset, data } => {
                let pending = self.pending.as_mut().ok_or("No mesh upload in progress")?;
                let chunk = pending.chunk.as_ref().ok_or("Cannot write an empty mesh")?;
                let (buffer, written, total) = if matches!(command, UploadCommand::Vertices { .. })
                {
                    (
                        &chunk.vertices,
                        &mut pending.vertices_written,
                        pending.vertex_bytes,
                    )
                } else {
                    if pending.vertices_written != pending.vertex_bytes {
                        return Err("Mesh vertices must be copied before indices".into());
                    }
                    (
                        &chunk.indices,
                        &mut pending.indices_written,
                        pending.index_bytes,
                    )
                };
                if data.is_empty()
                    || !offset.is_multiple_of(4)
                    || !data.len().is_multiple_of(4)
                    || offset != *written
                    || data.len() > total.saturating_sub(offset)
                {
                    return Err("Invalid or non-contiguous mesh copy range".into());
                }
                queue.write_buffer(buffer, offset as u64, data);
                *written += data.len();
            }
            UploadCommand::Commit => {
                let pending = self.pending.as_ref().ok_or("No mesh upload to commit")?;
                if pending.vertices_written != pending.vertex_bytes
                    || pending.indices_written != pending.index_bytes
                {
                    return Err("Cannot commit an incomplete mesh upload".into());
                }
                let pending = self.pending.take().expect("checked pending upload");
                if let Some(chunk) = pending.chunk {
                    self.resident.insert(pending.key, chunk);
                } else {
                    self.resident.remove(&pending.key);
                }
            }
            UploadCommand::Cancel => {
                self.pending.take();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    fn readback(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Buffer) -> Vec<u8> {
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Test readback"),
            size: source.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(source, 0, &staging, 0, source.size());
        let submission = queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        staging.map_async(wgpu::MapMode::Read, .., move |result| {
            sender.send(result).unwrap();
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(5)),
            })
            .expect("GPU readback deadline");
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let bytes = staging.get_mapped_range(..).unwrap().to_vec();
        staging.unmap();
        bytes
    }

    #[test]
    #[ignore = "requires a native GPU; run explicitly for upload/readback acceptance"]
    fn native_gpu_segmented_upload_preserves_old_mesh_until_commit_and_cancels_safely() {
        pollster::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .unwrap();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut meshes = GpuMeshes::default();
            let key = (-2, 1, 3);
            let vertices: Vec<u8> = (0..132).map(|i| (i * 7) as u8).collect();
            let indices = bytemuck::cast_slice::<u32, u8>(&[0, 1, 2]).to_vec();
            let begin = || UploadCommand::Begin {
                key,
                vertex_bytes: vertices.len(),
                index_count: 3,
            };
            meshes.upload(&device, &queue, begin()).unwrap();
            assert!(meshes.upload(&device, &queue, begin()).is_err());
            assert!(
                meshes
                    .upload(&device, &queue, UploadCommand::Commit)
                    .is_err()
            );
            assert!(
                meshes
                    .upload(
                        &device,
                        &queue,
                        UploadCommand::Vertices {
                            offset: 4,
                            data: &vertices[..4]
                        }
                    )
                    .is_err()
            );
            assert!(
                meshes
                    .upload(
                        &device,
                        &queue,
                        UploadCommand::Vertices {
                            offset: 0,
                            data: &vertices[..3]
                        }
                    )
                    .is_err()
            );
            assert!(
                meshes
                    .upload(
                        &device,
                        &queue,
                        UploadCommand::Indices {
                            offset: 0,
                            data: &indices
                        }
                    )
                    .is_err()
            );
            for (segment, data) in vertices.chunks(20).enumerate() {
                meshes
                    .upload(
                        &device,
                        &queue,
                        UploadCommand::Vertices {
                            offset: segment * 20,
                            data,
                        },
                    )
                    .unwrap();
                // Mirrors a skipped render's empty submit; no surface needed.
                queue.submit([]);
                assert!(!meshes.resident.contains_key(&key));
            }
            meshes
                .upload(
                    &device,
                    &queue,
                    UploadCommand::Indices {
                        offset: 0,
                        data: &indices,
                    },
                )
                .unwrap();
            meshes
                .upload(&device, &queue, UploadCommand::Commit)
                .unwrap();
            assert_eq!(
                readback(&device, &queue, &meshes.resident[&key].vertices),
                vertices
            );
            assert_eq!(
                readback(&device, &queue, &meshes.resident[&key].indices),
                indices
            );

            // Cancel already submitted writes, and then replace the same key.
            let replacement = vec![19; 132];
            meshes.upload(&device, &queue, begin()).unwrap();
            meshes
                .upload(
                    &device,
                    &queue,
                    UploadCommand::Vertices {
                        offset: 0,
                        data: &replacement[..20],
                    },
                )
                .unwrap();
            queue.submit([]);
            meshes
                .upload(&device, &queue, UploadCommand::Cancel)
                .unwrap();
            assert_eq!(
                readback(&device, &queue, &meshes.resident[&key].vertices),
                vertices
            );
            meshes.upload(&device, &queue, begin()).unwrap();
            meshes
                .upload(
                    &device,
                    &queue,
                    UploadCommand::Vertices {
                        offset: 0,
                        data: &replacement,
                    },
                )
                .unwrap();
            assert_eq!(
                readback(&device, &queue, &meshes.resident[&key].vertices),
                vertices
            );
            meshes
                .upload(
                    &device,
                    &queue,
                    UploadCommand::Indices {
                        offset: 0,
                        data: &indices,
                    },
                )
                .unwrap();
            meshes
                .upload(&device, &queue, UploadCommand::Commit)
                .unwrap();
            assert_eq!(
                readback(&device, &queue, &meshes.resident[&key].vertices),
                replacement
            );

            // Cancellation before submission must also be safe.
            meshes.upload(&device, &queue, begin()).unwrap();
            meshes
                .upload(
                    &device,
                    &queue,
                    UploadCommand::Vertices {
                        offset: 0,
                        data: &vertices[..20],
                    },
                )
                .unwrap();
            meshes
                .upload(&device, &queue, UploadCommand::Cancel)
                .unwrap();
            queue.submit([]);
            assert_eq!(
                readback(&device, &queue, &meshes.resident[&key].vertices),
                replacement
            );
            meshes
                .upload(
                    &device,
                    &queue,
                    UploadCommand::Begin {
                        key,
                        vertex_bytes: 0,
                        index_count: 0,
                    },
                )
                .unwrap();
            assert!(meshes.resident.contains_key(&key));
            meshes
                .upload(&device, &queue, UploadCommand::Commit)
                .unwrap();
            assert!(!meshes.resident.contains_key(&key));
            assert!(
                meshes
                    .upload(
                        &device,
                        &queue,
                        UploadCommand::Begin {
                            key,
                            vertex_bytes: 44,
                            index_count: 0
                        }
                    )
                    .is_err()
            );
            assert!(
                meshes
                    .upload(
                        &device,
                        &queue,
                        UploadCommand::Begin {
                            key,
                            vertex_bytes: 44,
                            index_count: 1
                        }
                    )
                    .is_err()
            );
            let oversized = (device.limits().max_buffer_size as usize / 44 + 1) * 44;
            assert!(
                meshes
                    .upload(
                        &device,
                        &queue,
                        UploadCommand::Begin {
                            key,
                            vertex_bytes: oversized,
                            index_count: 3
                        }
                    )
                    .is_err()
            );
            assert!(
                scope.pop().await.is_none(),
                "GPU rejected segmented uploads"
            );
        });
    }

    #[test]
    fn generated_vertices_stay_inside_conservative_chunk_bounds() {
        for seed in [1, 396, 728_491] {
            let world = World::new(seed);
            for key in [(-2, 1, -3), (0, 0, 0), (2, 1, -2), (0, 4, 0)] {
                let (min, max) = chunk_bounds(key);
                for vertex in world.mesh_chunk(key).vertices {
                    let position = Vec3::from_array(vertex.position);
                    assert!(
                        position.cmpge(min).all() && position.cmple(max).all(),
                        "{key:?}: {position:?}"
                    );
                }
            }
        }
    }
}
