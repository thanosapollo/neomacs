//! GPU side of the native model view: upload a [`ModelScene`] and draw it
//! into a sampled colour texture (design §4.5).
//!
//! The scene is drawn into a 4x MSAA colour target with a Depth32Float
//! buffer and resolved into `Rgba8UnormSrgb`, the format every surface
//! texture uses, so the existing surface composite samples it unchanged.

use super::scene::{ModelScene, face_normal};

/// Format of the resolved texture the composite samples.
pub const MODEL_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Largest physical edge of a model texture (design §4.4).
pub const MAX_MODEL_EDGE_PX: u32 = 2048;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;

/// canvas-3d colours (`render.py`).
const BONE: [f32; 3] = [0.92, 0.77, 0.59];
const HIGHLIGHT: [f32; 3] = [1.0, 0.82, 0.2];

pub use neomacs_display_protocol::model::ModelView;

/// Per-axis clip scale: zoom * 0.8, with the longer axis shrunk so the unit
/// sphere fits the shorter one.
pub fn clip_scale(view: &ModelView, width: u32, height: u32) -> [f32; 2] {
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);
    let s = view.zoom * 0.8;
    if w >= h {
        [(s * h / w) as f32, s as f32]
    } else {
        [s as f32, (s * w / h) as f32]
    }
}

/// Physical target size for a `width` x `height` logical model at `scale`:
/// one common factor brings the longer edge within `max_edge`, so the
/// texture keeps the logical aspect ratio it is composited into.
pub fn model_target_px(width: u32, height: u32, scale: f32, max_edge: u32) -> (u32, u32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale as f64
    } else {
        1.0
    };
    let (w, h) = (width.max(1) as f64 * scale, height.max(1) as f64 * scale);
    let fit = (max_edge.max(1) as f64 / w.max(h)).min(1.0);
    let px = |v: f64| ((v * fit).round() as u32).clamp(1, max_edge.max(1));
    (px(w), px(h))
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    r0: [f32; 4],
    r1: [f32; 4],
    r2: [f32; 4],
    scale: [f32; 4],
    base: [f32; 4],
    marked: [f32; 4],
}

/// One uploaded model with its render targets.
pub struct ModelGpu {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    camera: wgpu::Buffer,
    vertex_count: u32,
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width_px: u32,
    pub height_px: u32,
    /// Bytes of GPU memory this model holds (design §4.4).
    pub bytes: u64,
}

fn target(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    samples: u32,
    sampled: bool,
) -> wgpu::Texture {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if sampled {
        usage |= wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
    }
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

impl ModelGpu {
    /// Upload `scene` and create `width_px` x `height_px` render targets.
    /// Every GPU failure (capability, validation, allocation) is an `Err`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &ModelScene,
        width_px: u32,
        height_px: u32,
    ) -> Result<Self, String> {
        let mut positions: Vec<[f32; 4]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut faces: Vec<[f32; 4]> = Vec::new();
        for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
            let base = positions.len() as u32;
            positions.extend(mesh.vertices.iter().map(|v| {
                let p = scene.normalise(*v);
                [p[0], p[1], p[2], 1.0]
            }));
            for tri in &mesh.triangles {
                indices.extend(tri.iter().map(|i| base + i));
                let n = face_normal(tri.map(|i| mesh.vertices[i as usize]))
                    .ok_or("degenerate triangle")?;
                faces.push([n[0] as f32, n[1] as f32, n[2] as f32, mesh_index as f32]);
            }
        }
        let limits = device.limits();
        for (name, bytes) in [
            ("positions", positions.len() * 16),
            ("indices", indices.len() * 4),
            ("faces", faces.len() * 16),
        ] {
            if bytes as u64 > limits.max_storage_buffer_binding_size
                || bytes as u64 > limits.max_buffer_size
            {
                return Err(format!("model {name} buffer exceeds device limits"));
            }
        }
        let max_edge = limits.max_texture_dimension_2d;
        if width_px == 0 || height_px == 0 || width_px > max_edge || height_px > max_edge {
            return Err(format!(
                "model target {width_px}x{height_px} outside 1..={max_edge}"
            ));
        }
        // Everything below is checked by wgpu against the adapter (vertex
        // storage buffers need DownlevelFlags::VERTEX_STORAGE, MSAA needs
        // format support); capture rejections as an error instead of the
        // fatal uncaptured-error handler, as surface pipelines do.
        let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        // Not `create_buffer_init`: it maps the new buffer with an
        // infallible `expect`, which unwinds when the allocation failed.
        // A queue write of an invalid buffer is a scoped error instead.
        let storage = |label, contents: &[u8]| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: contents.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buffer, 0, contents);
            buffer
        };
        let positions_buf = storage("model positions", bytemuck::cast_slice(&positions));
        let indices_buf = storage("model indices", bytemuck::cast_slice(&indices));
        let faces_buf = storage("model faces", bytemuck::cast_slice(&faces));
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let read_only = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model bind group layout"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(1, read_only),
                entry(2, read_only),
                entry(3, read_only),
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: positions_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: indices_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: faces_buf.as_entire_binding(),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("model.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("model pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: MODEL_TEXTURE_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });

        let px = width_px as u64 * height_px as u64;
        let bytes = (positions.len() * 16 + indices.len() * 4 + faces.len() * 16) as u64
            + std::mem::size_of::<CameraUniform>() as u64
            + px * 4 // resolved colour
            + px * 4 * SAMPLES as u64 // MSAA colour
            + px * 4 * SAMPLES as u64; // MSAA depth
        let texture = target(
            device,
            "model colour",
            width_px,
            height_px,
            MODEL_TEXTURE_FORMAT,
            1,
            true,
        );
        let msaa = target(
            device,
            "model msaa",
            width_px,
            height_px,
            MODEL_TEXTURE_FORMAT,
            SAMPLES,
            false,
        );
        let depth = target(
            device,
            "model depth",
            width_px,
            height_px,
            DEPTH_FORMAT,
            SAMPLES,
            false,
        );
        let msaa = msaa.create_view(&Default::default());
        let depth = depth.create_view(&Default::default());
        let view = texture.create_view(&Default::default());
        let rejected = pollster::block_on(validation.pop());
        let exhausted = pollster::block_on(oom.pop());
        if let Some(error) = rejected.or(exhausted) {
            return Err(format!("model GPU resources rejected: {error}"));
        }
        Ok(Self {
            pipeline,
            bind_group,
            camera,
            vertex_count: indices.len() as u32,
            msaa,
            depth,
            view,
            texture,
            width_px,
            height_px,
            bytes,
        })
    }

    /// Draw the scene with `view`, `selected` mesh (none when `None`) and
    /// `background` (an Emacs sRGB colour), into [`Self::texture`].
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &ModelView,
        selected: Option<u16>,
        background: [f32; 4],
    ) -> Result<(), String> {
        let background = clear_color(background);
        // The camera write allocates staging memory: scope both kinds.
        let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let r = view.rotation();
        let row = |i: usize| [r[i][0] as f32, r[i][1] as f32, r[i][2] as f32, 0.0];
        let scale = clip_scale(view, self.width_px, self.height_px);
        let uniform = CameraUniform {
            r0: row(0),
            r1: row(1),
            r2: row(2),
            scale: [
                scale[0],
                scale[1],
                selected.map_or(0.0, |m| m as f32 + 1.0),
                0.0,
            ],
            base: [BONE[0], BONE[1], BONE[2], 1.0],
            marked: [HIGHLIGHT[0], HIGHLIGHT[1], HIGHLIGHT[2], 1.0],
        };
        queue.write_buffer(&self.camera, 0, bytemuck::bytes_of(&uniform));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("model render"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("model pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa,
                    depth_slice: None,
                    resolve_target: Some(&self.view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(background),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..self.vertex_count, 0..1);
        }
        queue.submit(Some(encoder.finish()));
        let rejected = pollster::block_on(validation.pop());
        let exhausted = pollster::block_on(oom.pop());
        match rejected.or(exhausted) {
            Some(error) => Err(format!("model render rejected: {error}")),
            None => Ok(()),
        }
    }

    /// Keep only the resolved texture (static picture); the geometry and
    /// attachments are released once the GPU has finished with them.
    pub fn into_texture(self) -> (wgpu::Texture, wgpu::TextureView) {
        (self.texture, self.view)
    }
}

/// The clear colour for an Emacs (sRGB) `background`: the target is an
/// `*Srgb` format, so the clear value is linear and the GPU encodes it back
/// to the caller's bytes (like the shader's decode of the bone colour).
fn clear_color(background: [f32; 4]) -> wgpu::Color {
    let [r, g, b, a] = background;
    let c = neomacs_display_protocol::types::Color { r, g, b, a }.srgb_to_linear();
    wgpu::Color {
        r: f64::from(c.r),
        g: f64::from(c.g),
        b: f64::from(c.b),
        a: f64::from(c.a),
    }
}

#[cfg(test)]
#[path = "gpu/tests.rs"]
mod tests;
