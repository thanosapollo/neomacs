use crate::core::frame_glyphs::{CursorStyle, DisplaySlotId, FrameGlyphBuffer, WindowCursor};
use crate::render_thread::state::{PointerAppearanceState, PresentedAppearanceKey};
use neomacs_display_protocol::frame_chrome::PresentationId;
use neomacs_display_protocol::{
    Color, DisplayWindowId, EffectsConfig, FrameRate, PointerAppearanceId,
};

fn filled_box_frame(effects: EffectsConfig) -> (FrameGlyphBuffer, DisplayWindowId) {
    let window_id = DisplayWindowId::new(1);
    let mut frame = FrameGlyphBuffer::with_size(20.0, 20.0);
    frame.window_cursors.push(WindowCursor {
        window_id,
        slot_id: DisplaySlotId::ZERO,
        x: 1.0,
        y: 2.0,
        width: 3.0,
        height: 4.0,
        style: CursorStyle::FilledBox,
        color: Color::WHITE,
        cursor_fg: Color::BLACK,
        ascent: 0.0,
        active: true,
    });
    frame.set_window_cursor_effects(window_id, effects);
    (frame, window_id)
}

// If this claim is false, a hovered or pressed pointer appearance is composited
// from a retained scene captured before the appearance changed: the cell under
// the pointer keeps its old look until the next scene commit, and the cursor
// cells retained for that scene redraw the stale appearance over the composite.
#[test]
fn a_hovered_or_pressed_pointer_appearance_forces_the_full_render_and_retains_no_cursor_cells() {
    let mut pointer = PointerAppearanceState::default();
    let mut frame = FrameGlyphBuffer::with_size(20.0, 20.0);
    frame.window_cursors.push(WindowCursor {
        window_id: DisplayWindowId::new(1),
        slot_id: DisplaySlotId::ZERO,
        x: 1.0,
        y: 2.0,
        width: 3.0,
        height: 4.0,
        style: CursorStyle::FilledBox,
        color: Color::WHITE,
        cursor_fg: Color::BLACK,
        ascent: 0.0,
        active: true,
    });
    assert!(super::pointer_appearance_allowed(&pointer));
    assert_eq!(
        super::build_filled_box_cursor_cells(&frame, 1.0, &pointer).len(),
        1
    );

    let key = PresentedAppearanceKey::new(
        PresentationId::new(7),
        PointerAppearanceId::try_from(0usize).unwrap(),
    );
    pointer.hover(Some(key));
    assert!(!super::pointer_appearance_allowed(&pointer));
    assert!(super::build_filled_box_cursor_cells(&frame, 1.0, &pointer).is_empty());

    pointer.press();
    assert!(!super::pointer_appearance_allowed(&pointer));
    assert!(super::build_filled_box_cursor_cells(&frame, 1.0, &pointer).is_empty());
}

// If this claim is false, a window with its own cursor-effect profile loses it
// the moment the frame takes the retained-static path: the mini-frame falls back
// to the global profile, so the cursor changes colour-cycle behaviour depending
// on which composition strategy the frame happened to pick.
#[test]
fn a_retained_filled_box_cursor_cell_resolves_the_same_local_effect_profile_as_a_full_render() {
    let pointer = PointerAppearanceState::default();

    for local_enabled in [false, true] {
        let mut local = EffectsConfig::cursor_profile_baseline();
        local.cursor_color_cycle.enabled = local_enabled;
        local.cursor_color_cycle.fps = FrameRate::new(12).unwrap();
        let (frame, window_id) = filled_box_frame(local.clone());

        let mut global = EffectsConfig::default();
        global.cursor_color_cycle.enabled = !local_enabled;
        global.cursor_color_cycle.fps = FrameRate::new(60).unwrap();

        let cells = super::build_filled_box_cursor_cells(&frame, 1.0, &pointer);
        assert_eq!(cells.len(), 1);
        assert_eq!(
            cells[0]
                .mini
                .effective_window_cursor_effects(window_id, &global),
            &local,
            "the retained cursor-only path must resolve the same local profile as the full renderer"
        );
    }
}

fn snapshot(rev: u64, yaw: f64) -> neomacs_renderer_wgpu::model::store::Snapshot {
    neomacs_renderer_wgpu::model::store::Snapshot {
        rev,
        req: 0,
        view: neomacs_display_protocol::model::ModelView {
            yaw,
            pitch: 0.0,
            zoom: 1.0,
        },
        logical: (64, 32),
        size_px: (64, 32),
    }
}

fn record(
    drawn: &[u32],
    referenced: &[u32],
    current: &crate::render_thread::models::ModelPicture,
) -> super::RetainedModels {
    super::RetainedModels::record(
        Some(drawn.iter().copied().collect()),
        referenced.iter().copied().collect(),
        current,
    )
}

// Review MUST 5: a retained scene pictures each model it composited at the
// snapshot its texture showed then. Once any of them is redrawn (an orbit,
// a zoom, a Lisp request), blitting the retained raster would put the old
// camera back on screen with no editor scene change, so it must rebuild.
#[test]
fn a_retained_scene_is_stale_once_a_model_it_pictures_is_redrawn() {
    use crate::render_thread::models::ModelPicture;
    let size = neomacs_renderer_wgpu::SnapshotSize::new(800, 600).unwrap();
    let current = ModelPicture::from([(5, snapshot(1, 0.0)), (7, snapshot(4, 10.0))]);
    let built = record(&[5, 7], &[5, 7], &current);
    assert_eq!(built.picture, current);
    assert!(super::retained_scene_is_current(
        (3, size, &built),
        (3, size, &current)
    ));
    // Model 7 orbited and redrew: same scene generation, same size.
    let now = ModelPicture::from([(5, snapshot(1, 0.0)), (7, snapshot(5, 30.0))]);
    assert!(
        !super::retained_scene_is_current((3, size, &built), (3, size, &now)),
        "the retained raster still shows model 7's old camera"
    );
    // Model 7 was freed: its pixels are still in the raster.
    let freed = ModelPicture::from([(5, snapshot(1, 0.0))]);
    assert!(!super::retained_scene_is_current(
        (3, size, &built),
        (3, size, &freed)
    ));
}

// SOL-REVIEW-2 MUST 3 / OPUS-REVIEW MUST 1: a raster whose glyphs reference
// a model that was not drawable at the build (still loading, or absent)
// composited nothing for it. When that model becomes drawable the raster
// is missing its pixels, so it must rebuild; a model the raster's glyphs do
// not reference stays irrelevant to it, loading or not.
#[test]
fn a_retained_scene_is_stale_once_a_model_it_references_becomes_drawable() {
    use crate::render_thread::models::ModelPicture;
    let size = neomacs_renderer_wgpu::SnapshotSize::new(800, 600).unwrap();
    // Model 7 is referenced but loading: only 5 is drawn.
    let loading = ModelPicture::from([(5, snapshot(1, 0.0))]);
    let built = record(&[5], &[5, 7], &loading);
    assert_eq!(built.picture, loading, "the raster pictures only model 5");
    assert!(super::retained_scene_is_current(
        (3, size, &built),
        (3, size, &loading)
    ));
    // Model 7 uploaded: same scene generation, same size.
    let uploaded = ModelPicture::from([(5, snapshot(1, 0.0)), (7, snapshot(1, 0.0))]);
    assert!(
        !super::retained_scene_is_current((3, size, &built), (3, size, &uploaded)),
        "the retained raster lacks model 7's pixels"
    );
    // Model 9, which the raster does not reference, uploads or redraws:
    // the raster still pictures its scene.
    let unrelated = ModelPicture::from([(5, snapshot(1, 0.0)), (9, snapshot(2, 0.0))]);
    assert!(super::retained_scene_is_current(
        (3, size, &built),
        (3, size, &unrelated)
    ));
    // A referenced surface that is no model (a shader surface) never
    // appears among the models' snapshots and changes nothing.
    let built = record(&[5], &[5, 11], &loading);
    assert!(super::retained_scene_is_current(
        (3, size, &built),
        (3, size, &loading)
    ));
}
