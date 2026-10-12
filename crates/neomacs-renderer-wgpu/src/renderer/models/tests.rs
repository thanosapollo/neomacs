//! The renderer's model cache on a real headless device: resize is
//! transactional across the GPU targets, the composite surface entry and the
//! budget (review MUST 3).

use super::*;
use crate::model::gpu::{GpuHighlight, MODEL_TEXTURE_FORMAT};
use crate::model::obj::parse_obj;
use crate::model::scene::remaining_limits;
use neomacs_display_protocol::model::ModelView;

/// A headless renderer, or `None` to skip; `NEOMACS_MODEL_REQUIRE_EVIDENCE=1`
/// makes a missing adapter a failure instead of a silent pass.
fn try_renderer() -> Option<WgpuRenderer> {
    let renderer = WgpuRenderer::new(None, 64, 64).ok();
    assert!(
        renderer.is_some() || std::env::var_os("NEOMACS_MODEL_REQUIRE_EVIDENCE").is_none(),
        "GPU evidence required but no headless wgpu adapter"
    );
    renderer
}

fn quad_scene() -> ModelScene {
    let mesh = parse_obj(
        b"v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n",
        remaining_limits(&[]),
    )
    .unwrap();
    ModelScene::new(vec![mesh]).unwrap()
}

fn draw(background: [f32; 4]) -> ModelDraw {
    ModelDraw {
        view: ModelView::default(),
        logical: (64, 32),
        background,
        highlight: GpuHighlight::default(),
    }
}

/// Tightly packed RGBA8 of `texture`.
fn read_texture(r: &WgpuRenderer, texture: &wgpu::Texture) -> Vec<u8> {
    assert_eq!(texture.format(), MODEL_TEXTURE_FORMAT);
    let (w, h) = (texture.width(), texture.height());
    let row =
        (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = r.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model test readback"),
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = r.device.create_command_encoder(&Default::default());
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
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    r.queue.submit(Some(encoder.finish()));
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.unwrap());
    r.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .unwrap();
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let start = (y * row) as usize;
        out.extend_from_slice(&mapped[start..start + (w * 4) as usize]);
    }
    out
}

/// What the model draws into and what the composite samples.
fn model_pixels(r: &WgpuRenderer, id: u32) -> Vec<u8> {
    read_texture(r, &r.models.models[&id].texture().0)
}

fn composite_pixels(r: &WgpuRenderer, id: u32) -> Vec<u8> {
    read_texture(r, &r.caches.surface.get(id).unwrap().texture)
}

/// Review MUST 3: a composite installation failing after the new targets
/// exist leaves the model exactly as it was: GPU targets, the composited
/// texture, sizes and budget; and a later draw still reaches the
/// composited texture.
#[test]
fn a_resize_failing_at_composite_installation_changes_nothing() {
    let Some(mut r) = try_renderer() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let baseline = r.model_budget_used();
    let scene = quad_scene();
    assert_eq!(r.model_upload(5, &scene, (64, 32), 1.0), Ok((64, 32)));
    let red = [1.0, 0.0, 0.0, 1.0];
    r.model_render(5, &draw(red)).unwrap();
    let used = r.model_budget_used();
    let before = model_pixels(&r, 5);
    assert_eq!(composite_pixels(&r, 5), before);

    r.models.fail_next_composite = true;
    assert!(r.model_resize(5, (128, 128), 1.0).is_err());
    assert_eq!(r.model_budget_used(), used, "accounting unchanged");
    assert_eq!(r.models.models[&5].size_px(), (64, 32), "old targets kept");
    assert_eq!(
        r.caches.surface.sizes(5),
        Some(((64, 32), (64, 32))),
        "composite entry unchanged"
    );
    assert_eq!(model_pixels(&r, 5), before, "old pixels survive");
    assert_eq!(composite_pixels(&r, 5), before);
    // The model still draws into the texture the composite samples.
    let blue = [0.0, 0.0, 1.0, 1.0];
    r.model_render(5, &draw(blue)).unwrap();
    let after = model_pixels(&r, 5);
    assert_ne!(after, before);
    assert_eq!(composite_pixels(&r, 5), after, "one texture, one owner");

    // The same resize, uninjected, commits everything together.
    assert_eq!(r.model_resize(5, (128, 128), 1.0), Ok((128, 128)));
    assert_eq!(r.models.models[&5].size_px(), (128, 128));
    assert_eq!(r.caches.surface.sizes(5), Some(((128, 128), (128, 128))));
    assert_eq!(
        r.model_budget_used(),
        used - target_bytes(64, 32) + target_bytes(128, 128)
    );
    r.model_free(5);
    assert_eq!(r.model_budget_used(), baseline);
}

/// A logical-only change (same physical size) whose composite installation
/// fails keeps the old logical size too.
#[test]
fn a_logical_only_resize_failing_at_installation_keeps_the_entry() {
    let Some(mut r) = try_renderer() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let scene = quad_scene();
    assert_eq!(r.model_upload(5, &scene, (64, 32), 1.0), Ok((64, 32)));
    let used = r.model_budget_used();
    r.models.fail_next_composite = true;
    // 64x32 at scale 1 vs 32x16 at scale 2: the same physical targets.
    assert!(r.model_resize(5, (32, 16), 2.0).is_err());
    assert_eq!(r.caches.surface.sizes(5), Some(((64, 32), (64, 32))));
    assert_eq!(r.model_budget_used(), used);
    r.model_free(5);
}
