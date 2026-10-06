use super::blends_older_pictures;
use crate::core::frame_glyphs::FrameGlyphBuffer;
use crate::render_thread::frame_windows::GuiFrameRenderState;
use neomacs_display_protocol::frame_time::{self, FrameSample};
use neomacs_display_protocol::motion_spec::{MotionDuration, MotionSpec, TweenSpec};
use neomacs_display_protocol::scroll_animation::TransitionEasing;
use neomacs_renderer_wgpu::{SnapshotSize, WgpuRenderer};

const ROOT: u64 = 1;
const CHILD: u64 = 2;

fn tween_100ms() -> MotionSpec {
    MotionSpec::Tween(TweenSpec {
        duration: MotionDuration::new(std::time::Duration::from_millis(100)).expect("positive"),
        easing: TransitionEasing::Linear,
        bezier: None,
    })
}

fn frame(id: u64, parent: u64, size: f32) -> FrameGlyphBuffer {
    let mut buf = FrameGlyphBuffer::with_size(size, size);
    buf.set_frame_identity(
        neomacs_display_protocol::DisplayFrameId::new(id),
        neomacs_display_protocol::DisplayFrameId::new(parent),
        0.0,
        0.0,
        if parent == 0 { i32::MIN } else { 1 },
        false,
        0.0,
        neomacs_display_protocol::Color::BLACK,
        false,
        1.0,
    );
    buf
}

fn sample_at(origin: frame_time::EventTime, millis: u64) -> FrameSample {
    FrameSample::new(
        origin.plus(std::time::Duration::from_millis(millis)),
        std::time::Duration::ZERO,
    )
}

/// SOL-REVIEW-2 MUST 2: each composition that shows an older picture
/// besides the fresh glyph passes is recognised, through the state the
/// composition itself reads (pane placements, the active transitions, the
/// child crossfades), and only while it runs.
#[test]
fn compositions_showing_an_older_picture_are_recognised_while_they_run() {
    let Ok(mut renderer) = WgpuRenderer::new(None, 80, 40) else {
        assert!(std::env::var_os("NEOMACS_REQUIRE_GPU_TESTS").is_none());
        return;
    };
    let origin = frame_time::observe_platform_now();
    let mut render = GuiFrameRenderState::new(ROOT, renderer.device(), 1.0, false, origin);
    let size = SnapshotSize::new(80, 40).unwrap();
    let at = |ms| sample_at(origin, ms);

    // A plain frame shows only its fresh passes.
    assert!(!blends_older_pictures(&render, false, at(0)));
    // A pane morph places ring pictures.
    assert!(blends_older_pictures(&render, true, at(0)), "pane morph");

    // A theme crossfade blends the previous composition in.
    let old = renderer.acquire_snapshot(size).unwrap();
    render
        .compositor
        .transitions
        .start_theme_crossfade_for_test(old, tween_100ms(), origin);
    assert!(
        blends_older_pictures(&render, false, at(10)),
        "theme crossfade"
    );
    render.compositor.transitions.clear_active_for_test();
    assert!(!blends_older_pictures(&render, false, at(10)));

    // A child frame's resize crossfade layers its previous presentation.
    let root = frame(ROOT, 0, 400.0);
    render.compositor.child_frames.set_root_frame(Some(&root));
    render
        .compositor
        .child_frames
        .update_frame(frame(CHILD, ROOT, 40.0));
    let old = renderer.acquire_snapshot(size).unwrap();
    render.compositor.child_frames.begin_resize_crossfade(
        CHILD,
        old,
        40.0,
        40.0,
        tween_100ms(),
        origin,
    );
    assert!(
        blends_older_pictures(&render, false, at(10)),
        "child crossfade"
    );
    // Once its mix finished, the child shows only its fresh payload.
    assert!(!blends_older_pictures(&render, false, at(200)));
}
