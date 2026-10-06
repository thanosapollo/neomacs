//! Native model journey on a real headless `WgpuRenderer` (review SHOULD 1):
//! a model created through the store and its loader, composited through both
//! surface resolver paths (root inline glyphs and child-frame content),
//! then freed, with the model budget back at its baseline.
//!
//! Skips (passes) cleanly where no GPU adapter is available, as the other
//! offscreen tests do, unless `NEOMACS_MODEL_REQUIRE_EVIDENCE` is set.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use neomacs_display_protocol::frame_glyphs::{FrameGlyphBuffer, GlyphRowRole};
use neomacs_display_protocol::model::{ModelMeshSpec, ModelView};
use neomacs_display_protocol::types::{Color, DisplayWindowId, SurfaceId};
use neomacs_display_protocol::{
    DeviceScale, GeometrySize, LogicalPixels, PresentMapping, PresentationExtent, SurfaceState,
};
use neomacs_renderer_wgpu::model::{ModelGpuHost, ModelStatus, ModelStore};
use neomacs_renderer_wgpu::{WgpuGlyphAtlas, WgpuRenderer};

const W: u32 = 96;
const H: u32 = 64;
const MODEL: u32 = 5;

fn try_renderer() -> Option<WgpuRenderer> {
    let renderer = WgpuRenderer::new(None, W, H).ok();
    assert!(
        renderer.is_some() || std::env::var_os("NEOMACS_MODEL_REQUIRE_EVIDENCE").is_none(),
        "GPU evidence required but no headless wgpu adapter"
    );
    renderer
}

fn mapping(frame: &FrameGlyphBuffer) -> PresentMapping {
    let SurfaceState::Drawable(surface) =
        SurfaceState::from_device_size(W, H, DeviceScale::new(1.0).unwrap()).unwrap()
    else {
        unreachable!("offscreen targets are drawable")
    };
    PresentMapping::top_left_clip(
        surface,
        PresentationExtent::new(
            frame.presentation_id,
            GeometrySize::<LogicalPixels>::from_px(frame.width, frame.height).unwrap(),
        ),
    )
}

fn frame(with_model: bool) -> FrameGlyphBuffer {
    let mut frame = FrameGlyphBuffer::with_size(W as f32, H as f32);
    frame.background = Color::BLACK;
    frame.set_draw_context(DisplayWindowId::new(1), GlyphRowRole::Text, None);
    if with_model {
        frame.add_surface(SurfaceId::new(MODEL), 8.0, 8.0, 32.0, 32.0);
    }
    frame
}

fn target(r: &WgpuRenderer) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = r.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("model-journey-target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

/// RGBA of pixel (x, y) of a BGRA target.
fn pixel(r: &WgpuRenderer, texture: &wgpu::Texture, x: u32, y: u32) -> [u8; 4] {
    let row =
        (W * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = r.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("model-journey-readback"),
        size: u64::from(row * H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = r.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    r.queue().submit(Some(encoder.finish()));
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.unwrap());
    r.device()
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(10)),
        })
        .unwrap();
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    let i = (y * row + x * 4) as usize;
    [mapped[i + 2], mapped[i + 1], mapped[i], mapped[i + 3]]
}

fn is_red(p: [u8; 4]) -> bool {
    p[0] > 200 && p[1] < 40 && p[2] < 40
}

fn is_black(p: [u8; 4]) -> bool {
    p[0] < 40 && p[1] < 40 && p[2] < 40
}

/// Root composite: the frame `frame(with_model)` drawn as a top-level frame.
fn render_root(
    r: &mut WgpuRenderer,
    atlas: &mut WgpuGlyphAtlas,
    with_model: bool,
) -> (wgpu::Texture, Option<HashSet<u32>>) {
    let (texture, view) = target(r);
    let frame = frame(with_model);
    atlas.set_current_frame_fonts(frame.font_bindings());
    r.render_frame_glyphs(
        &view,
        &frame,
        atlas,
        mapping(&frame),
        false,
        None,
        None,
        None,
        None,
        None,
    );
    (texture, r.take_drawn_surfaces())
}

/// Child composite: an empty root, then the model frame through the
/// child-frame content path at offset (16, 16).
fn render_child(
    r: &mut WgpuRenderer,
    atlas: &mut WgpuGlyphAtlas,
) -> (wgpu::Texture, Option<HashSet<u32>>) {
    let (texture, view) = target(r);
    let root = frame(false);
    atlas.set_current_frame_fonts(root.font_bindings());
    r.render_frame_glyphs(
        &view,
        &root,
        atlas,
        mapping(&root),
        false,
        None,
        None,
        None,
        None,
        None,
    );
    r.take_drawn_surfaces();
    r.take_referenced_surfaces();
    let child = frame(true);
    r.render_frame_content(
        &view, &child, atlas, W, H, 16.0, 16.0, false, None, 0.0, None, None, 1.0, 1.0, [0.0; 2],
    );
    (texture, r.take_drawn_surfaces())
}

fn host(r: &mut WgpuRenderer) -> Option<&mut dyn ModelGpuHost> {
    Some(r)
}

#[test]
fn a_model_composites_through_both_resolvers_and_free_restores_the_budget() {
    let Some(mut r) = try_renderer() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let mut atlas = WgpuGlyphAtlas::new(r.device());
    let baseline = r.model_budget_used();

    let dir = std::env::temp_dir().join(format!("neomacs-model-journey-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("quad.obj");
    std::fs::write(
        &path,
        b"v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n",
    )
    .unwrap();

    // Create through the store and its loader thread; the background shows
    // in the texture's corners (the quad covers only its centre).
    let mut store = ModelStore::new();
    store.create(
        MODEL,
        vec![ModelMeshSpec { path, sha256: None }],
        ModelView::default(),
        [1.0, 0.0, 0.0, 1.0],
        (32, 32),
    );
    // Still loading (the store moves on only when polled): both resolvers
    // reach the glyph and composite nothing for it. A raster built now
    // depends on the model without picturing it (SOL-REVIEW-2 MUST 3).
    assert_eq!(store.status(MODEL), Some(ModelStatus::Loading));
    let (texture, drawn) = render_root(&mut r, &mut atlas, true);
    assert_eq!(drawn, Some(HashSet::new()), "nothing to draw while loading");
    assert_eq!(
        r.take_referenced_surfaces(),
        HashSet::from([MODEL]),
        "the root resolver reached the loading model's glyph"
    );
    assert!(is_black(pixel(&r, &texture, 10, 10)), "no model pixels yet");
    let (texture, drawn) = render_child(&mut r, &mut atlas);
    assert!(!drawn.unwrap_or_default().contains(&MODEL));
    assert_eq!(
        r.take_referenced_surfaces(),
        HashSet::from([MODEL]),
        "the child resolver reached the loading model's glyph"
    );
    assert!(is_black(pixel(&r, &texture, 26, 26)));

    let deadline = Instant::now() + Duration::from_secs(20);
    while store.status(MODEL) == Some(ModelStatus::Loading) {
        assert!(Instant::now() < deadline, "the load never finished");
        std::thread::sleep(Duration::from_millis(5));
        store.poll_loads(host(&mut r));
    }
    assert_eq!(store.status(MODEL), Some(ModelStatus::Ready));
    assert!(r.model_budget_used() > baseline, "the upload is accounted");
    store.render_dirty(&mut r);

    // Root inline composite (layer_media / draw_inline_surfaces).
    let (texture, drawn) = render_root(&mut r, &mut atlas, true);
    assert_eq!(drawn, Some(HashSet::from([MODEL])), "root resolver drew it");
    assert_eq!(r.take_referenced_surfaces(), HashSet::from([MODEL]));
    assert!(
        is_red(pixel(&r, &texture, 10, 10)),
        "model pixels in the glyph"
    );
    assert!(
        is_black(pixel(&r, &texture, 4, 4)),
        "frame outside the glyph"
    );

    // Child-frame content composite (content.rs inline surfaces).
    let (texture, drawn) = render_child(&mut r, &mut atlas);
    assert_eq!(
        drawn,
        Some(HashSet::from([MODEL])),
        "child resolver drew it"
    );
    assert!(
        is_red(pixel(&r, &texture, 26, 26)),
        "model pixels at the child offset"
    );
    assert!(
        is_black(pixel(&r, &texture, 10, 10)),
        "root left as cleared"
    );

    // Free: every reservation returns and nothing composites it any more.
    store.free(MODEL, host(&mut r));
    assert_eq!(
        r.model_budget_used(),
        baseline,
        "budget back at its baseline"
    );
    let (texture, drawn) = render_root(&mut r, &mut atlas, true);
    assert_eq!(
        drawn,
        Some(HashSet::new()),
        "a freed model is not composited"
    );
    assert!(is_black(pixel(&r, &texture, 10, 10)));
    let (texture, drawn) = render_child(&mut r, &mut atlas);
    assert!(!drawn.unwrap_or_default().contains(&MODEL));
    assert!(is_black(pixel(&r, &texture, 26, 26)));
    std::fs::remove_dir_all(dir).unwrap();
}
