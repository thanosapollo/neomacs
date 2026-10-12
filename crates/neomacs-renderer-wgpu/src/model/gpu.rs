//! GPU side of the native model view: upload a [`ModelScene`], draw it into
//! a sampled colour texture whenever the store asks (design §4.5), and
//! answer 1x1 picks (§5.4).
//!
//! The scene is drawn into a 4x MSAA colour target with a Depth32Float
//! buffer and resolved into `Rgba8UnormSrgb`, the format every surface
//! texture uses, so the existing surface composite samples it unchanged.
//!
//! Failures that wgpu reports through error scopes (validation and
//! allocation) come back as `Err`. Device loss is reported by wgpu's
//! device-lost callback instead, not by these methods.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::scene::{ModelScene, face_normal};

/// Format of the resolved texture the composite samples.
pub const MODEL_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Largest physical edge of a model texture (design §4.4).
pub const MAX_MODEL_EDGE_PX: u32 = 2048;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const PICK_IDS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;
const SAMPLES: u32 = 4;
/// One identity texel copy at a 256-byte aligned row.
const PICK_STAGING_BYTES: u64 = 256;

/// canvas-3d colours (`render.py`).
const BONE: [f32; 3] = [0.92, 0.77, 0.59];
const HIGHLIGHT: [f32; 3] = [1.0, 0.82, 0.2];

pub use neomacs_display_protocol::model::ModelView;

/// Per-axis clip scale: zoom * 0.8, with the longer axis shrunk so the unit
/// sphere fits the shorter one. `width` and `height` are the *logical* size,
/// so a capped or rounded physical target frames the model as its glyph does.
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

/// Highlight layers one draw paints at most (the uniform's layer array).
pub const MAX_GPU_LAYERS: usize = neomacs_display_protocol::model::MODEL_MAX_HIGHLIGHT_LAYERS;
/// Faces per region-mark word: each face has a 4-bit layer number.
const MARKS_PER_WORD: usize = 8;
// `model.wgsl` sizes its layer array as 8 (`MAX_LAYERS`), and a 4-bit
// region mark holds a layer number + 1.
const _: () = assert!(MAX_GPU_LAYERS == 8 && MAX_GPU_LAYERS < 16);

/// GPU bytes of a model's geometry and fixed resources (design §4.4):
/// positions 16 B·V, indices 12 B·T, face records 16 B·T, region marks
/// ⌈T/8⌉·4 B (4 bits per face), the two camera uniforms, the 1x1 pick
/// targets and staging.
pub fn geometry_bytes(vertices: u64, triangles: u64) -> u64 {
    16 * vertices
        + 12 * triangles
        + 16 * triangles
        + triangles.div_ceil(MARKS_PER_WORD as u64).max(1) * 4
        + 2 * std::mem::size_of::<CameraUniform>() as u64
        + (8 + 4)
        + PICK_STAGING_BYTES
}

/// GPU bytes of `width_px` x `height_px` render targets: resolved colour
/// 4 B, MSAA colour 16 B and MSAA depth 16 B per pixel.
pub fn target_bytes(width_px: u32, height_px: u32) -> u64 {
    let px = width_px as u64 * height_px as u64;
    px * (4 + 4 * SAMPLES as u64 + 4 * SAMPLES as u64)
}

/// What to paint, already resolved against the scene (design §2): a whole
/// mesh, a point disc on one mesh, and/or region faces (global numbers),
/// all in the highlight colour; then coloured `layers`, in order, a later
/// one winning where they overlap. At most [`MAX_GPU_LAYERS`] paints in
/// all (the store never combines the two forms).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuHighlight {
    pub mesh: Option<u16>,
    pub point: Option<GpuPoint>,
    pub faces: Option<Vec<u32>>,
    pub layers: Vec<GpuLayer>,
}

/// One coloured highlight layer resolved against the scene.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuLayer {
    /// sRGB components in 0..=1.
    pub color: [f32; 3],
    pub paint: GpuPaint,
}

/// What one layer paints.
#[derive(Debug, Clone, PartialEq)]
pub enum GpuPaint {
    Mesh(u16),
    Point(GpuPoint),
    /// Global face numbers.
    Faces(Vec<u32>),
}

impl GpuHighlight {
    /// Every paint in drawing order: the single-colour fields first (as
    /// layers in the highlight colour), then `layers`; at most
    /// [`MAX_GPU_LAYERS`].
    fn paints(&self) -> Vec<(&[f32; 3], PaintRef<'_>)> {
        let mut out = Vec::new();
        if let Some(mesh) = self.mesh {
            out.push((&HIGHLIGHT, PaintRef::Mesh(mesh)));
        }
        if let Some(point) = &self.point {
            out.push((&HIGHLIGHT, PaintRef::Point(point)));
        }
        if let Some(faces) = &self.faces {
            out.push((&HIGHLIGHT, PaintRef::Faces(faces)));
        }
        for layer in &self.layers {
            let paint = match &layer.paint {
                GpuPaint::Mesh(mesh) => PaintRef::Mesh(*mesh),
                GpuPaint::Point(point) => PaintRef::Point(point),
                GpuPaint::Faces(faces) => PaintRef::Faces(faces),
            };
            out.push((&layer.color, paint));
        }
        // Unreachable: the store sends either at most 3 single-colour
        // paints or at most MODEL_MAX_HIGHLIGHT_LAYERS layers, never both.
        debug_assert!(out.len() <= MAX_GPU_LAYERS, "too many highlight paints");
        out.truncate(MAX_GPU_LAYERS);
        out
    }
}

#[derive(Clone, Copy)]
enum PaintRef<'a> {
    Mesh(u16),
    Point(&'a GpuPoint),
    Faces(&'a [u32]),
}

/// A point target in the normalised frame the GPU draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuPoint {
    pub mesh: u16,
    pub centre: [f32; 3],
    pub radius: f32,
}

/// One draw request.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDraw {
    pub view: ModelView,
    /// Logical size, for the camera fit.
    pub logical: (u32, u32),
    /// An Emacs (sRGB) colour, like a face colour.
    pub background: [f32; 4],
    pub highlight: GpuHighlight,
}

/// One pick request: the camera and size of the picture the user clicked,
/// and the clicked texel in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickRequest {
    pub view: ModelView,
    pub logical: (u32, u32),
    pub size_px: (u32, u32),
    pub texel: (u32, u32),
}

/// A finished pick, in GPU terms: the global face and the barycentric
/// weights (b0, b1, b2) of the covering fragment, or `None` for background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawPick {
    pub hit: Option<(u32, [f64; 3])>,
}

/// State of the one outstanding pick.
#[derive(Debug, Clone, PartialEq)]
pub enum PickPoll {
    Idle,
    Pending,
    Done(RawPick),
    Failed(String),
}

/// One highlight layer as `model.wgsl`'s `Layer` reads it.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    colour: [f32; 4],
    /// x: painted whole mesh + 1; y: point mesh + 1; z: 1 for region marks.
    paint: [f32; 4],
    /// Point centre (normalised frame) and radius.
    point: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    r0: [f32; 4],
    r1: [f32; 4],
    r2: [f32; 4],
    scale: [f32; 4],
    pick: [f32; 4],
    base: [f32; 4],
    /// x: number of layers in use.
    count: [f32; 4],
    layers: [LayerUniform; MAX_GPU_LAYERS],
}

impl CameraUniform {
    fn new(view: &ModelView, logical: (u32, u32)) -> Self {
        let r = view.rotation();
        let row = |i: usize| [r[i][0] as f32, r[i][1] as f32, r[i][2] as f32, 0.0];
        let scale = clip_scale(view, logical.0, logical.1);
        Self {
            r0: row(0),
            r1: row(1),
            r2: row(2),
            scale: [scale[0], scale[1], 0.0, 0.0],
            pick: [0.0, 0.0, 1.0, 1.0],
            base: [BONE[0], BONE[1], BONE[2], 1.0],
            count: [0.0; 4],
            layers: [LayerUniform::default(); MAX_GPU_LAYERS],
        }
    }
}

/// Colour, MSAA and depth targets at one physical size.
struct Targets {
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width_px: u32,
    height_px: u32,
}

struct PickTargets {
    ids: wgpu::Texture,
    ids_view: wgpu::TextureView,
    depth: wgpu::TextureView,
    staging: wgpu::Buffer,
    /// 0 idle, 1 mapping, 2 mapped, 3 map failed.
    state: Arc<AtomicU8>,
}

const PICK_IDLE: u8 = 0;
const PICK_MAPPING: u8 = 1;
const PICK_MAPPED: u8 = 2;
const PICK_MAP_FAILED: u8 = 3;

/// Render targets created by [`ModelGpu::stage_targets`] and not yet
/// committed.
pub struct StagedTargets(Targets);

impl StagedTargets {
    /// The resolved colour texture and its view (for the composite).
    pub fn texture(&self) -> (wgpu::Texture, wgpu::TextureView) {
        (self.0.texture.clone(), self.0.view.clone())
    }
}

/// One uploaded model with its render and pick targets.
pub struct ModelGpu {
    pipeline: wgpu::RenderPipeline,
    pick_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    pick_bind_group: wgpu::BindGroup,
    camera: wgpu::Buffer,
    pick_camera: wgpu::Buffer,
    marks: wgpu::Buffer,
    marks_words: usize,
    /// Region layers currently in `marks`, as (layer number, global
    /// faces), so unchanged regions are not rewritten on every draw.
    marks_written: Option<Vec<(u32, Vec<u32>)>>,
    vertex_count: u32,
    /// Original f64 geometry in the normalised frame, in global face order.
    /// CPU-owned (72 bytes per triangle), not part of GPU byte accounting.
    pick_triangles: Vec<[[f64; 3]; 3]>,
    pick_request: Option<PickRequest>,
    targets: Targets,
    pick: PickTargets,
    /// Geometry and fixed-resource bytes ([`geometry_bytes`]).
    pub geometry_bytes: u64,
}

fn target(
    device: &wgpu::Device,
    label: &str,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
    samples: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
        view_formats: &[],
    })
}

/// Run `body` inside OutOfMemory and Validation error scopes; any captured
/// error becomes `Err("{what}: {error}")`. The scopes are pushed and popped
/// around `body` with no early return between them.
fn scoped<T>(device: &wgpu::Device, what: &str, body: impl FnOnce() -> T) -> Result<T, String> {
    let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let value = body();
    let rejected = pollster::block_on(validation.pop());
    let exhausted = pollster::block_on(oom.pop());
    match rejected.or(exhausted) {
        Some(error) => Err(format!("{what}: {error}")),
        None => Ok(value),
    }
}

fn check_target_size(device: &wgpu::Device, (w, h): (u32, u32)) -> Result<(), String> {
    let max_edge = device.limits().max_texture_dimension_2d;
    if w == 0 || h == 0 || w > max_edge || h > max_edge {
        return Err(format!("model target {w}x{h} outside 1..={max_edge}"));
    }
    Ok(())
}

impl Targets {
    fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let sampled = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
        let texture = target(
            device,
            "model colour",
            size,
            MODEL_TEXTURE_FORMAT,
            1,
            sampled,
        );
        let msaa = target(
            device,
            "model msaa",
            size,
            MODEL_TEXTURE_FORMAT,
            SAMPLES,
            wgpu::TextureUsages::empty(),
        );
        let depth = target(
            device,
            "model depth",
            size,
            DEPTH_FORMAT,
            SAMPLES,
            wgpu::TextureUsages::empty(),
        );
        Self {
            msaa: msaa.create_view(&Default::default()),
            depth: depth.create_view(&Default::default()),
            view: texture.create_view(&Default::default()),
            texture,
            width_px: size.0,
            height_px: size.1,
        }
    }
}

impl PickTargets {
    fn new(device: &wgpu::Device) -> Self {
        let one = (1, 1);
        let ids = target(
            device,
            "model pick ids",
            one,
            PICK_IDS_FORMAT,
            1,
            wgpu::TextureUsages::COPY_SRC,
        );
        let depth = target(
            device,
            "model pick depth",
            one,
            DEPTH_FORMAT,
            1,
            wgpu::TextureUsages::empty(),
        );
        Self {
            ids_view: ids.create_view(&Default::default()),
            ids,
            depth: depth.create_view(&Default::default()),
            staging: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("model pick staging"),
                size: PICK_STAGING_BYTES,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            state: Arc::new(AtomicU8::new(PICK_IDLE)),
        }
    }
}

impl ModelGpu {
    /// Upload `scene` and create `size_px` render targets plus the pick
    /// resources.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &ModelScene,
        size_px: (u32, u32),
    ) -> Result<Self, String> {
        let mut positions: Vec<[f32; 4]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut faces: Vec<[f32; 4]> = Vec::new();
        let mut pick_triangles = Vec::new();
        for (mesh_index, mesh) in scene.meshes().iter().enumerate() {
            let base = u32::try_from(positions.len()).map_err(|_| "model too large")?;
            positions.extend(mesh.vertices.iter().map(|v| {
                let p = scene.normalise(*v);
                [p[0], p[1], p[2], 1.0]
            }));
            for tri in &mesh.triangles {
                indices.extend(tri.iter().map(|i| base + i));
                pick_triangles.push(tri.map(|i| {
                    let v = mesh.vertices[i as usize];
                    [0, 1, 2].map(|axis| (v[axis] - scene.centre()[axis]) / scene.radius())
                }));
                let n = face_normal(tri.map(|i| mesh.vertices[i as usize]))
                    .ok_or("degenerate triangle")?;
                faces.push([n[0] as f32, n[1] as f32, n[2] as f32, mesh_index as f32]);
            }
        }
        let marks_words = faces.len().div_ceil(MARKS_PER_WORD).max(1);
        let limits = device.limits();
        for (name, bytes) in [
            ("positions", positions.len() * 16),
            ("indices", indices.len() * 4),
            ("faces", faces.len() * 16),
            ("marks", marks_words * 4),
        ] {
            if bytes as u64 > limits.max_storage_buffer_binding_size
                || bytes as u64 > limits.max_buffer_size
            {
                return Err(format!("model {name} buffer exceeds device limits"));
            }
        }
        check_target_size(device, size_px)?;
        let vertex_count = u32::try_from(indices.len()).map_err(|_| "model too large")?;
        // Everything below is checked by wgpu against the adapter (vertex
        // storage buffers need DownlevelFlags::VERTEX_STORAGE, MSAA needs
        // format support); capture rejections as an error instead of the
        // fatal uncaptured-error handler, as surface pipelines do.
        scoped(device, "model GPU resources rejected", || {
            // Not `create_buffer_init`: it maps the new buffer with an
            // infallible `expect`, which unwinds when the allocation failed.
            // A queue write of an invalid buffer is a scoped error instead.
            let storage = |label, contents: &[u8]| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: contents.len().max(4) as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&buffer, 0, contents);
                buffer
            };
            let positions_buf = storage("model positions", bytemuck::cast_slice(&positions));
            let indices_buf = storage("model indices", bytemuck::cast_slice(&indices));
            let faces_buf = storage("model faces", bytemuck::cast_slice(&faces));
            let marks = storage("model marks", &vec![0u8; marks_words * 4]);
            let uniform = |label| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: std::mem::size_of::<CameraUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            let camera = uniform("model camera");
            let pick_camera = uniform("model pick camera");

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
                    entry(4, read_only),
                ],
            });
            let bind = |label, camera: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
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
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: marks.as_entire_binding(),
                        },
                    ],
                })
            };
            let bind_group = bind("model bind group", &camera);
            let pick_bind_group = bind("model pick bind group", &pick_camera);
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("model shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("model.wgsl").into()),
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("model pipeline layout"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline =
                |label, fragment, targets: &[Option<wgpu::ColorTargetState>], samples| {
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(label),
                        layout: Some(&pipeline_layout),
                        vertex: wgpu::VertexState {
                            module: &shader,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &[],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &shader,
                            entry_point: Some(fragment),
                            compilation_options: Default::default(),
                            targets,
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
                            count: samples,
                            ..Default::default()
                        },
                        multiview_mask: None,
                        cache: None,
                    })
                };
            let colour = |format| {
                Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })
            };
            let main = pipeline(
                "model pipeline",
                "fs_main",
                &[colour(MODEL_TEXTURE_FORMAT)],
                SAMPLES,
            );
            let pick_pipeline = pipeline(
                "model pick pipeline",
                "fs_pick",
                &[colour(PICK_IDS_FORMAT)],
                1,
            );
            Self {
                pipeline: main,
                pick_pipeline,
                bind_group,
                pick_bind_group,
                camera,
                pick_camera,
                marks,
                marks_words,
                marks_written: None,
                vertex_count,
                pick_triangles,
                pick_request: None,
                targets: Targets::new(device, size_px),
                pick: PickTargets::new(device),
                geometry_bytes: geometry_bytes(positions.len() as u64, faces.len() as u64),
            }
        })
    }

    pub fn size_px(&self) -> (u32, u32) {
        (self.targets.width_px, self.targets.height_px)
    }

    /// The resolved colour texture and its view (shared with the surface
    /// cache, which composites them).
    pub fn texture(&self) -> (wgpu::Texture, wgpu::TextureView) {
        (self.targets.texture.clone(), self.targets.view.clone())
    }

    /// Replace the render targets with `size_px` ones. The old targets stay
    /// until the new ones exist, so a failure leaves the model as it was.
    pub fn resize(&mut self, device: &wgpu::Device, size_px: (u32, u32)) -> Result<(), String> {
        let staged = Self::stage_targets(device, size_px)?;
        self.commit_targets(staged);
        Ok(())
    }

    /// Create `size_px` render targets without installing them, so a caller
    /// can prepare everything else that depends on them (the composite bind
    /// group) before committing any owner.
    pub fn stage_targets(
        device: &wgpu::Device,
        size_px: (u32, u32),
    ) -> Result<StagedTargets, String> {
        check_target_size(device, size_px)?;
        scoped(device, "model resize rejected", || {
            StagedTargets(Targets::new(device, size_px))
        })
    }

    /// Install staged targets; the old ones are dropped. Infallible.
    pub fn commit_targets(&mut self, staged: StagedTargets) {
        self.targets = staged.0;
    }

    /// Draw the scene into the colour texture.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draw: &ModelDraw,
    ) -> Result<(), String> {
        let background = clear_color(draw.background);
        let mut uniform = CameraUniform::new(&draw.view, draw.logical);
        let paints = draw.highlight.paints();
        uniform.count[0] = paints.len() as f32;
        // Region layers, as (layer number, faces): each face's mark is the
        // number + 1 of the last region layer listing it, so the shader,
        // trying layers last first, finds the one that wins.
        let mut regions: Vec<(u32, &[u32])> = Vec::new();
        for (slot, (number, (color, paint))) in
            uniform.layers.iter_mut().zip(paints.iter().enumerate())
        {
            slot.colour = [color[0], color[1], color[2], 1.0];
            match *paint {
                PaintRef::Mesh(mesh) => slot.paint[0] = mesh as f32 + 1.0,
                PaintRef::Point(point) => {
                    slot.paint[1] = point.mesh as f32 + 1.0;
                    slot.point = [
                        point.centre[0],
                        point.centre[1],
                        point.centre[2],
                        point.radius,
                    ];
                }
                PaintRef::Faces(faces) => {
                    slot.paint[2] = 1.0;
                    regions.push((number as u32, faces));
                }
            }
        }
        let unchanged = self.marks_written.as_ref().is_some_and(|written| {
            written.len() == regions.len()
                && written
                    .iter()
                    .zip(&regions)
                    .all(|((n, f), (m, g))| n == m && f.as_slice() == *g)
        });
        let marks = (!regions.is_empty() && !unchanged).then_some(&regions);
        // Both writes allocate staging memory: scope them with the pass.
        // The marks count as written only once the scope reports success.
        let rendered = scoped(device, "model render rejected", || {
            if let Some(regions) = marks {
                let mut words = vec![0u32; self.marks_words];
                for (number, faces) in regions {
                    for &face in *faces {
                        let shift = (face as usize % MARKS_PER_WORD) * 4;
                        if let Some(word) = words.get_mut(face as usize / MARKS_PER_WORD) {
                            *word = (*word & !(0xf << shift)) | ((number + 1) << shift);
                        }
                    }
                }
                queue.write_buffer(&self.marks, 0, bytemuck::cast_slice(&words));
            }
            queue.write_buffer(&self.camera, 0, bytemuck::bytes_of(&uniform));
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("model render"),
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("model pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.msaa,
                        depth_slice: None,
                        resolve_target: Some(&self.targets.view),
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(background),
                            store: wgpu::StoreOp::Discard,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.targets.depth,
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
        });
        match rendered {
            Ok(()) => {
                if let Some(regions) = marks {
                    self.marks_written = Some(
                        regions
                            .iter()
                            .map(|(number, faces)| (*number, faces.to_vec()))
                            .collect(),
                    );
                }
            }
            Err(_) => self.marks_written = None,
        }
        rendered
    }

    /// Start a pick of `request`'s texel. One pick at a time: the staging
    /// buffer is reused only after its mapping has returned.
    pub fn pick_start(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        request: &PickRequest,
    ) -> Result<(), String> {
        if self.pick.state.load(Ordering::Acquire) != PICK_IDLE {
            return Err("model pick already in flight".to_owned());
        }
        let (w, h) = (request.size_px.0.max(1), request.size_px.1.max(1));
        let (x, y) = (request.texel.0.min(w - 1), request.texel.1.min(h - 1));
        let mut uniform = CameraUniform::new(&request.view, request.logical);
        // The texel centre in NDC (y up), magnified so one texel covers the
        // whole 1x1 target and its centre lands on the target's sample.
        uniform.pick = [
            (x as f32 + 0.5) / w as f32 * 2.0 - 1.0,
            1.0 - (y as f32 + 0.5) / h as f32 * 2.0,
            w as f32,
            h as f32,
        ];
        scoped(device, "model pick rejected", || {
            queue.write_buffer(&self.pick_camera, 0, bytemuck::bytes_of(&uniform));
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("model pick"),
            });
            {
                let attachment = |view| {
                    Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })
                };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("model pick pass"),
                    color_attachments: &[attachment(&self.pick.ids_view)],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.pick.depth,
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
                pass.set_pipeline(&self.pick_pipeline);
                pass.set_bind_group(0, &self.pick_bind_group, &[]);
                pass.draw(0..self.vertex_count, 0..1);
            }
            {
                let texture = &self.pick.ids;
                let offset = 0;
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &self.pick.staging,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(1),
                        },
                    },
                    wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
            }
            queue.submit(Some(encoder.finish()));
        })?;
        self.pick_request = Some(*request);
        let state = self.pick.state.clone();
        state.store(PICK_MAPPING, Ordering::Release);
        self.pick
            .staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                state.store(
                    if result.is_ok() {
                        PICK_MAPPED
                    } else {
                        PICK_MAP_FAILED
                    },
                    Ordering::Release,
                );
            });
        Ok(())
    }

    /// Advance the outstanding pick without blocking.
    pub fn pick_poll(&mut self, device: &wgpu::Device) -> PickPoll {
        match self.pick.state.load(Ordering::Acquire) {
            PICK_IDLE => return PickPoll::Idle,
            PICK_MAPPING => {
                let _ = device.poll(wgpu::PollType::Poll);
            }
            _ => {}
        }
        match self.pick.state.load(Ordering::Acquire) {
            PICK_MAPPING => PickPoll::Pending,
            PICK_MAPPED => {
                let raw = {
                    let Ok(mapped) = self.pick.staging.slice(..).get_mapped_range() else {
                        self.pick.staging.unmap();
                        self.pick.state.store(PICK_IDLE, Ordering::Release);
                        return PickPoll::Failed("model pick readback unavailable".to_owned());
                    };
                    let ids: [u32; 2] = bytemuck::pod_read_unaligned(&mapped[0..8]);
                    if ids[1] == 0 {
                        Ok(RawPick { hit: None })
                    } else {
                        self.pick_request
                            .as_ref()
                            .and_then(|request| {
                                self.pick_triangles
                                    .get((ids[1] - 1) as usize)
                                    .and_then(|triangle| pick_weights(*triangle, request))
                            })
                            .map(|weights| RawPick {
                                hit: Some((ids[1] - 1, weights)),
                            })
                            .ok_or_else(|| "model pick intersection unavailable".to_owned())
                    }
                };
                self.pick.staging.unmap();
                self.pick.state.store(PICK_IDLE, Ordering::Release);
                self.pick_request = None;
                match raw {
                    Ok(raw) => PickPoll::Done(raw),
                    Err(error) => PickPoll::Failed(error),
                }
            }
            PICK_MAP_FAILED => {
                self.pick.state.store(PICK_IDLE, Ordering::Release);
                PickPoll::Failed("model pick readback failed".to_owned())
            }
            _ => PickPoll::Idle,
        }
    }
}

/// Intersect the orthographic texel-centre ray with the GPU-selected face.
/// GPU depth/tie/coverage decides identity; original f64 geometry decides
/// coordinates. Solving in camera XY is the ray/triangle intersection for
/// this orthographic camera, without an artificial ray-origin distance.
fn pick_weights(triangle: [[f64; 3]; 3], request: &PickRequest) -> Option<[f64; 3]> {
    let rotation = request.view.rotation();
    let q = triangle.map(|p| {
        [0, 1].map(|i| rotation[i][0] * p[0] + rotation[i][1] * p[1] + rotation[i][2] * p[2])
    });
    let (w, h) = (request.size_px.0.max(1), request.size_px.1.max(1));
    let (x, y) = (request.texel.0.min(w - 1), request.texel.1.min(h - 1));
    let (lw, lh) = (
        request.logical.0.max(1) as f64,
        request.logical.1.max(1) as f64,
    );
    let scale = request.view.zoom * 0.8;
    let sx = scale * (lh / lw).min(1.0);
    let sy = scale * (lw / lh).min(1.0);
    let p = [
        ((x as f64 + 0.5) / w as f64 * 2.0 - 1.0) / sx,
        (1.0 - (y as f64 + 0.5) / h as f64 * 2.0) / sy,
    ];
    let a = [q[1][0] - q[0][0], q[1][1] - q[0][1]];
    let b = [q[2][0] - q[0][0], q[2][1] - q[0][1]];
    let d = [p[0] - q[0][0], p[1] - q[0][1]];
    let det = a[0] * b[1] - a[1] * b[0];
    let b1 = (d[0] * b[1] - d[1] * b[0]) / det;
    let b2 = (a[0] * d[1] - a[1] * d[0]) / det;
    let weights = [1.0 - b1 - b2, b1, b2];
    if !weights
        .iter()
        .all(|v| v.is_finite() && *v >= -1e-5 && *v <= 1.0 + 1e-5)
    {
        return None;
    }
    // A GPU-covered boundary may lie just outside the f64 triangle because
    // rasterisation uses f32 geometry. Snap only that rounding fringe; keep
    // public point-target weights nonnegative and summing to one.
    let weights = weights.map(|v| v.clamp(0.0, 1.0));
    let sum: f64 = weights.iter().sum();
    Some(weights.map(|v| v / sum))
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
