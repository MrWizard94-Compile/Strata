use std::{collections::HashMap, sync::Arc};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::{
    materials,
    world::{ChunkKey, Mesh, Vertex},
};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Uint32];

pub enum PresentOutcome {
    Presented,
    Skipped,
}

pub struct RenderCamera {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub time: f32,
    pub fog_distance: f32,
    pub selected: Option<[i32; 3]>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
    inverse_view_projection: [[f32; 4]; 4],
    eye_time: [f32; 4],
    sun_fog: [f32; 4],
    selected: [f32; 4],
}

struct GpuChunk {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    min: Vec3,
    max: Vec3,
}

struct Pipelines {
    terrain: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    selection: wgpu::RenderPipeline,
    camera_layout: wgpu::BindGroupLayout,
}

/// The surface owns an `Arc<Window>`, so its lifetime is independent of callers.
/// Chunk buffers are replaced atomically on the render thread after CPU meshing.
pub struct Renderer {
    instance: wgpu::Instance,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub format: wgpu::TextureFormat,
    pub adapter_name: String,
    pub visible_chunks: usize,
    pub visible_triangles: usize,
    pipelines: Pipelines,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    depth_view: wgpu::TextureView,
    multisample_view: Option<wgpu::TextureView>,
    sample_count: u32,
    chunks: HashMap<ChunkKey, GpuChunk>,
    suspended: bool,
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|error| format!("Could not create the game window surface: {error}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("No compatible graphics adapter is available: {error}"))?;
        let info = adapter.get_info();
        let adapter_name = format!("{} / {:?}", info.name, info.backend);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("STRATA graphics device"),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("Could not initialize the graphics device: {error}"))?;
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("The graphics adapter cannot present to this window")?;
        let capabilities = surface.get_capabilities(&adapter);
        config.format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(config.format);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &config);

        // Shader validation is a startup failure with an actionable message,
        // instead of an uncaptured GPU panic after the window has opened.
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let sample_count = if [config.format, DEPTH_FORMAT].iter().all(|format| {
            adapter
                .get_texture_format_features(*format)
                .flags
                .sample_count_supported(4)
        }) {
            4
        } else {
            1
        };
        let pipelines = create_pipelines(&device, config.format, sample_count);
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera and atmosphere parameters"),
            contents: bytemuck::bytes_of(&CameraUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let (material_view, material_sampler) = create_material_texture(&device, &queue);
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Camera bind group"),
            layout: &pipelines.camera_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&material_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&material_sampler),
                },
            ],
        });
        let depth_view = create_attachment_view(&device, &config, DEPTH_FORMAT, sample_count);
        let multisample_view = (sample_count > 1)
            .then(|| create_attachment_view(&device, &config, config.format, sample_count));
        if let Some(error) = validation.pop().await {
            return Err(format!("The world renderer failed GPU validation: {error}"));
        }
        Ok(Self {
            instance,
            window,
            surface,
            device,
            queue,
            format: config.format,
            config,
            adapter_name,
            visible_chunks: 0,
            visible_triangles: 0,
            pipelines,
            camera_buffer,
            camera_bind_group,
            depth_view,
            multisample_view,
            sample_count,
            chunks: HashMap::new(),
            suspended: size.width == 0 || size.height == 0,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.suspended = width == 0 || height == 0;
        if self.suspended {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view =
            create_attachment_view(&self.device, &self.config, DEPTH_FORMAT, self.sample_count);
        self.multisample_view = (self.sample_count > 1).then(|| {
            create_attachment_view(
                &self.device,
                &self.config,
                self.config.format,
                self.sample_count,
            )
        });
    }

    pub fn upload_chunk(&mut self, key: ChunkKey, mesh: &Mesh) {
        if mesh.vertices.is_empty() || mesh.indices.is_empty() {
            self.remove_chunk(key);
            return;
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for vertex in &mesh.vertices {
            let position = Vec3::from_array(vertex.position);
            min = min.min(position);
            max = max.max(position);
        }
        let vertices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Chunk vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let indices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Chunk triangle indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        self.chunks.insert(
            key,
            GpuChunk {
                vertices,
                indices,
                index_count: mesh.indices.len() as u32,
                min,
                max,
            },
        );
    }

    pub fn remove_chunk(&mut self, key: ChunkKey) {
        self.chunks.remove(&key);
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.chunks
            .values()
            .map(|chunk| chunk.index_count as usize / 3)
            .sum()
    }

    pub fn render(
        &mut self,
        camera: RenderCamera,
        egui_renderer: &mut egui_wgpu::Renderer,
        paint_jobs: &[egui::ClippedPrimitive],
        textures_delta: &egui::TexturesDelta,
        pixels_per_point: f32,
    ) -> Result<PresentOutcome, String> {
        for (id, deltas) in &textures_delta.set {
            for delta in deltas {
                egui_renderer.update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        let result = self.render_frame(camera, egui_renderer, paint_jobs, pixels_per_point);
        // Delta application is independent of successful presentation: font and
        // UI texture updates must not disappear during a surface resize/loss.
        for id in &textures_delta.free {
            egui_renderer.free_texture(id);
        }
        result
    }

    fn render_frame(
        &mut self,
        camera: RenderCamera,
        egui_renderer: &mut egui_wgpu::Renderer,
        paint_jobs: &[egui::ClippedPrimitive],
        pixels_per_point: f32,
    ) -> Result<PresentOutcome, String> {
        if self.suspended {
            return Ok(PresentOutcome::Skipped);
        }
        let (frame, reconfigure) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => (frame, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(PresentOutcome::Skipped);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(PresentOutcome::Skipped);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(Arc::clone(&self.window))
                    .map_err(|e| format!("Cannot restore window surface: {e}"))?;
                self.surface.configure(&self.device, &self.config);
                return Ok(PresentOutcome::Skipped);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("GPU rejected surface acquisition".into());
            }
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let eye = Vec3::from_array(camera.eye);
        let view =
            glam::camera::rh::view::look_at_mat4(eye, Vec3::from_array(camera.target), Vec3::Y);
        let projection = glam::camera::rh::proj::directx::perspective(
            70.0_f32.to_radians(),
            self.config.width as f32 / self.config.height as f32,
            0.08,
            camera.fog_distance.max(64.0) * 1.4,
        );
        let view_projection = projection * view;
        let sun = Vec3::new(-0.38, 0.64, -0.64).normalize();
        let selected = camera
            .selected
            .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32, 1.0])
            .unwrap_or([0.0; 4]);
        let uniform = CameraUniform {
            view_projection: view_projection.to_cols_array_2d(),
            inverse_view_projection: view_projection.inverse().to_cols_array_2d(),
            eye_time: [eye.x, eye.y, eye.z, camera.time],
            sun_fog: [sun.x, sun.y, sun.z, camera.fog_distance],
            selected,
        };
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));
        let frustum = Frustum::new(view_projection);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("STRATA frame"),
            });
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point,
        };
        let callback_buffers = egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            paint_jobs,
            &screen,
        );
        self.visible_chunks = 0;
        self.visible_triangles = 0;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Atmosphere and voxel world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.multisample_view.as_ref().unwrap_or(&target),
                    depth_slice: None,
                    resolve_target: self.multisample_view.as_ref().map(|_| &target),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_pipeline(&self.pipelines.sky);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.pipelines.terrain);
            for chunk in self.chunks.values() {
                if !frustum.intersects(chunk.min, chunk.max) {
                    continue;
                }
                pass.set_vertex_buffer(0, chunk.vertices.slice(..));
                pass.set_index_buffer(chunk.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..chunk.index_count, 0, 0..1);
                self.visible_chunks += 1;
                self.visible_triangles += chunk.index_count as usize / 3;
            }
            if camera.selected.is_some() {
                pass.set_pipeline(&self.pipelines.selection);
                pass.draw(0..24, 0..1);
            }
        }
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Interface"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            // egui owns callback resources internally; this pass is dropped
            // before encoder.finish(), as required by forget_lifetime().
            egui_renderer.render(&mut pass.forget_lifetime(), paint_jobs, &screen);
        }
        self.queue.submit(
            callback_buffers
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        self.queue.present(frame);
        if reconfigure {
            self.surface.configure(&self.device, &self.config);
        }
        Ok(PresentOutcome::Presented)
    }
}

fn create_material_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::TextureView, wgpu::Sampler) {
    let mips = materials::mip_chain();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Original 32x32 material tiles"),
        size: wgpu::Extent3d {
            width: materials::TILE_SIZE,
            height: materials::TILE_SIZE,
            depth_or_array_layers: materials::LAYER_COUNT,
        },
        mip_level_count: mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, bytes) in mips.iter().enumerate() {
        let side = materials::TILE_SIZE >> level;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(side * 4),
                rows_per_image: Some(side),
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: materials::LAYER_COUNT,
            },
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Crisp close materials, filtered distance mips"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    (view, sampler)
}

fn create_attachment_view(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("World render attachment"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn create_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> Pipelines {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("STRATA terrain, atmosphere, water and selection"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/world.wgsl").into()),
    });
    let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Camera layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<CameraUniform>() as u64
                    ),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("World pipeline layout"),
        bind_group_layouts: &[Some(&camera_layout)],
        immediate_size: 0,
    });
    let targets = [Some(wgpu::ColorTargetState {
        format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let vertex_layout = [Some(wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRIBUTES,
    })];
    let make_pipeline =
        |label: &str, vertex: &str, fragment: &str, terrain: bool, selection: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers: if terrain { &vertex_layout } else { &[] },
                },
                primitive: wgpu::PrimitiveState {
                    topology: if selection {
                        wgpu::PrimitiveTopology::LineList
                    } else {
                        wgpu::PrimitiveTopology::TriangleList
                    },
                    cull_mode: if terrain {
                        Some(wgpu::Face::Back)
                    } else {
                        None
                    },
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(terrain),
                    depth_compare: Some(if terrain || selection {
                        wgpu::CompareFunction::LessEqual
                    } else {
                        wgpu::CompareFunction::Always
                    }),
                    stencil: Default::default(),
                    // The selection shader expands the cube slightly; depth
                    // bias is not legal for a line-list pipeline in wgpu.
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
    Pipelines {
        terrain: make_pipeline("Voxel world", "vs_terrain", "fs_terrain", true, false),
        sky: make_pipeline("Atmospheric sky", "vs_sky", "fs_sky", false, false),
        selection: make_pipeline(
            "Selected block outline",
            "vs_selection",
            "fs_selection",
            false,
            true,
        ),
        camera_layout,
    }
}

struct Frustum([Vec4; 6]);

impl Frustum {
    fn new(matrix: Mat4) -> Self {
        let rows = matrix.transpose();
        // wgpu uses 0 <= clip.z <= clip.w, unlike OpenGL's -w <= z.
        Self([
            rows.w_axis + rows.x_axis,
            rows.w_axis - rows.x_axis,
            rows.w_axis + rows.y_axis,
            rows.w_axis - rows.y_axis,
            rows.z_axis,
            rows.w_axis - rows.z_axis,
        ])
    }

    fn intersects(&self, min: Vec3, max: Vec3) -> bool {
        self.0.iter().all(|plane| {
            let positive = Vec3::new(
                if plane.x >= 0.0 { max.x } else { min.x },
                if plane.y >= 0.0 { max.y } else { min.y },
                if plane.z >= 0.0 { max.z } else { min.z },
            );
            plane.truncate().dot(positive) + plane.w >= 0.0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frustum_preserves_visible_and_intersecting_chunks() {
        let projection =
            glam::camera::rh::proj::directx::perspective(90.0_f32.to_radians(), 1.0, 0.1, 100.0);
        let frustum = Frustum::new(projection);
        assert!(frustum.intersects(Vec3::new(-1.0, -1.0, -8.0), Vec3::new(1.0, 1.0, -5.0)));
        assert!(frustum.intersects(Vec3::new(4.0, -1.0, -8.0), Vec3::new(20.0, 1.0, -5.0)));
        assert!(!frustum.intersects(Vec3::new(-1.0, -1.0, 2.0), Vec3::new(1.0, 1.0, 4.0)));
        assert!(!frustum.intersects(Vec3::new(20.0, -1.0, -8.0), Vec3::new(30.0, 1.0, -5.0)));
        assert!(!frustum.intersects(Vec3::new(-1.0, -1.0, -120.0), Vec3::new(1.0, 1.0, -110.0)));
    }

    #[test]
    fn camera_uniform_and_textured_vertices_match_gpu_layout() {
        assert_eq!(std::mem::size_of::<CameraUniform>(), 176);
        assert_eq!(std::mem::size_of::<Vertex>(), 44);
        assert_eq!(std::mem::offset_of!(Vertex, normal), 12);
        assert_eq!(std::mem::offset_of!(Vertex, color), 24);
        assert_eq!(std::mem::offset_of!(Vertex, texture_layer), 40);
        assert_eq!(VERTEX_ATTRIBUTES[3].offset, 40);
        assert_eq!(VERTEX_ATTRIBUTES[3].format, wgpu::VertexFormat::Uint32);
    }

    #[test]
    #[ignore = "requires a native GPU; run explicitly for graphics acceptance"]
    fn native_gpu_validates_all_world_shader_pipelines() {
        pollster::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("native GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .expect("native GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _pipelines = create_pipelines(&device, wgpu::TextureFormat::Bgra8UnormSrgb, 4);
            let _materials = create_material_texture(&device, &queue);
            queue.submit([]);
            let result = scope.pop().await;
            assert!(
                result.is_none(),
                "world shader pipeline validation failed: {result:?}"
            );
        });
    }
}
