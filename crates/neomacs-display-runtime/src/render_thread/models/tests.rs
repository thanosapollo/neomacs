//! Model membership and presentation provenance through the render-thread
//! glue (design §6.2): the window events, presentations and pointer input
//! a native window delivers, observed on the ordered input channel; and
//! renderer loss published there at once (review MUST 9).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use neomacs_display_protocol::model::{ModelChange, ModelEvent, ModelMeshSpec, ModelView};
use neomacs_renderer_wgpu::model::store::{GlyphHit, ModelGpuHost, ModelStatus};
use neomacs_renderer_wgpu::model::{ModelDraw, ModelScene, PickPoll, PickRequest};

use super::RenderApp;
use crate::thread_comm::{EmacsComms, InputEvent, ThreadComms};

/// Records what the store asks of the GPU.
#[derive(Default)]
pub(super) struct FakeHost {
    pub draws: Vec<ModelDraw>,
    pub picks: Vec<PickRequest>,
}

impl ModelGpuHost for FakeHost {
    fn model_upload(
        &mut self,
        _id: u32,
        _scene: &ModelScene,
        logical: (u32, u32),
        scale: f32,
    ) -> Result<(u32, u32), String> {
        Ok((
            (logical.0 as f32 * scale) as u32,
            (logical.1 as f32 * scale) as u32,
        ))
    }
    fn model_resize(
        &mut self,
        _id: u32,
        logical: (u32, u32),
        scale: f32,
    ) -> Result<(u32, u32), String> {
        Ok((
            (logical.0 as f32 * scale) as u32,
            (logical.1 as f32 * scale) as u32,
        ))
    }
    fn model_render(&mut self, _id: u32, draw: &ModelDraw) -> Result<(), String> {
        self.draws.push(draw.clone());
        Ok(())
    }
    fn model_free(&mut self, _id: u32) {}
    fn model_pick_start(&mut self, _id: u32, request: &PickRequest) -> Result<(), String> {
        self.picks.push(*request);
        Ok(())
    }
    fn model_pick_poll(&mut self, _id: u32) -> PickPoll {
        PickPoll::Pending
    }
}

pub(super) const MODEL: u32 = 5;
/// Two top-level native windows.
pub(super) const A: u64 = 0xa;
pub(super) const B: u64 = 0xb;

pub(super) fn app_with_channel() -> (RenderApp, EmacsComms) {
    let comms = ThreadComms::new();
    let (emacs, render) = comms.split();
    let app = RenderApp::new(
        render,
        800,
        600,
        "test".to_string(),
        Arc::new(crate::render_thread::ImageRenderState::default()),
        Arc::new((Mutex::new(Vec::new()), std::sync::Condvar::new())),
        true,
        #[cfg(feature = "neo-term")]
        crate::terminal::new_shared_terminals(),
    );
    (app, emacs)
}

/// Model `MODEL` created from a real OBJ file and loaded through the real
/// loader thread, uploaded and drawn by `host`.
pub(super) fn with_ready_model(app: &mut RenderApp, host: &mut FakeHost) {
    create_model(app);
    finish_loading(app, host);
}

/// Model `MODEL` created from a real OBJ file: its load is queued on the
/// real loader thread, and it is still loading.
pub(super) fn create_model(app: &mut RenderApp) {
    let dir = std::env::temp_dir().join(format!(
        "neomacs-model-runtime-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("quad.obj");
    std::fs::write(
        &path,
        b"v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n",
    )
    .unwrap();
    app.models.create(
        MODEL,
        vec![ModelMeshSpec { path, sha256: None }],
        ModelView::default(),
        [0.0, 0.0, 0.0, 1.0],
        (64, 32),
    );
}

/// Wait for `MODEL`'s load, then upload and draw it with `host`.
pub(super) fn finish_loading(app: &mut RenderApp, host: &mut FakeHost) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.models.status(MODEL) == Some(ModelStatus::Loading) {
        assert!(Instant::now() < deadline, "model load timed out");
        std::thread::sleep(Duration::from_millis(2));
        app.models.poll_loads(Some(host as &mut dyn ModelGpuHost));
    }
    assert_eq!(app.models.status(MODEL), Some(ModelStatus::Ready));
    app.models.render_dirty(host);
    app.flush_model_events();
}

/// A Lisp request, as `handle_model_update` applies it.
pub(super) fn update(app: &mut RenderApp, host: &mut FakeHost, req: u64, change: ModelChange) {
    app.models.update(MODEL, req, change, Some(host));
    app.flush_model_events();
}

/// Every model event delivered so far, rendered compactly.
pub(super) fn delivered(emacs: &EmacsComms) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(event) = emacs.input_rx.try_recv() {
        if let InputEvent::Model { event, .. } = event {
            out.push(match event {
                ModelEvent::Readiness { current, rev, req } => {
                    format!("R{{{current},{rev},{req}}}")
                }
                ModelEvent::View { rev, .. } => format!("View{{{rev}}}"),
                ModelEvent::Error { req, .. } => format!("Error{{{req:?}}}"),
                ModelEvent::Pick { rev, .. } => format!("Pick{{{rev}}}"),
                other => other.kind().to_owned(),
            });
        }
    }
    out
}

pub(super) fn hit() -> GlyphHit {
    GlyphHit {
        id: MODEL,
        u: 0.5,
        v: 0.5,
    }
}

/// Window `window` fully renders a frame whose glyph passes composite the
/// model (as `render_pass` reports a full composition).
pub(super) fn present(app: &mut RenderApp, window: u64) {
    let picture = super::picture_of(
        Some(std::collections::HashSet::from([MODEL])),
        &app.model_picture(),
    );
    app.note_models_presented(window, &picture);
}

fn yaw(yaw: f64) -> ModelChange {
    ModelChange::View(ModelView {
        yaw,
        pitch: 0.0,
        zoom: 1.0,
    })
}

// ----------------------------------------------------------------------
// Review MUST 4: native occlusion and hide end membership and capture.

/// Hiding the sole showing window (through the `WindowEvent::Occluded`
/// handler) drops readiness, ends that window's orbit capture with one
/// `View`, flushes both, and the window rejoins only by presenting after
/// its exposure.
#[test]
fn occluding_the_sole_showing_window_ends_membership_and_its_capture() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    assert!(app.models.press(A, hit(), (10.0, 10.0)));
    assert!(app.models.motion(A, (30.0, 10.0)), "orbiting");
    app.flush_model_events();
    assert_eq!(delivered(&emacs), ["R{false,2,0}"]);

    app.handle_window_occluded(A, true);
    // Flushed by the handler itself, ahead of any later input.
    assert_eq!(delivered(&emacs), ["View{2}"], "capture settled, A left");
    assert!(!app.models.motion(A, (40.0, 10.0)), "the capture ended");

    // A stray presentation while occluded does not rejoin; nor does the
    // exposure alone: only a presentation after it.
    app.models.render_dirty(&mut host);
    present(&mut app, A);
    assert!(delivered(&emacs).is_empty(), "occluded window rejoined");
    app.handle_window_occluded(A, false);
    assert!(
        delivered(&emacs).is_empty(),
        "exposure is not a presentation"
    );
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["R{true,2,0}"]);
}

/// Two windows show the model; B lags behind a new revision and is then
/// occluded: the visible A alone decides the fence, which comes up.
#[test]
fn occluding_a_lagging_second_window_releases_the_fence() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    present(&mut app, B);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    update(&mut app, &mut host, 1, yaw(30.0));
    assert_eq!(delivered(&emacs), ["R{false,2,1}"]);
    app.models.render_dirty(&mut host);
    present(&mut app, A);
    assert!(delivered(&emacs).is_empty(), "B still shows rev 1");
    app.handle_window_occluded(B, true);
    assert_eq!(delivered(&emacs), ["R{true,2,1}"]);
}

/// `make-frame-invisible` / `iconify-frame` hide the native window
/// without any window-system occlusion event. Where the platform reports
/// the window's visibility (X11, macOS, Windows), the frame leaves at once,
/// keeps out while hidden, and rejoins by presenting once shown.
#[test]
fn hiding_or_iconifying_a_frame_leaves_until_shown_and_presented() {
    use crate::thread_comm::WindowVisibility;
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    present(&mut app, B);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    for visibility in [WindowVisibility::Invisible, WindowVisibility::Iconified] {
        app.note_models_visibility(A, visibility, true);
        app.note_models_visibility(B, visibility, true);
        assert_eq!(delivered(&emacs), ["R{false,1,0}"]);
        present(&mut app, A);
        assert!(delivered(&emacs).is_empty(), "a hidden frame rejoined");
        app.note_models_visibility(A, WindowVisibility::Visible, true);
        assert!(delivered(&emacs).is_empty());
        present(&mut app, A);
        assert_eq!(delivered(&emacs), ["R{true,1,0}"]);
        app.note_models_visibility(B, WindowVisibility::Visible, true);
        present(&mut app, B);
    }
}

// ----------------------------------------------------------------------
// Review MUST 5/6: the presented picture, root and child, full and
// retained, decides membership and pick provenance.

/// A compositor-only frame of `window` through the retained-static
/// strategy, with the bookkeeping `retained_static::draw` does: rebuild
/// the retained raster when stale (the build composites `scene`, root and
/// child), blit it, run a filled-box cursor mini-pass compositing
/// `overlay`, and present. Answers whether the raster was rebuilt.
fn present_retained(
    app: &mut RenderApp,
    window: u64,
    retained: &mut Option<crate::render_thread::render_pass::retained_static::RetainedModels>,
    scene: &[u32],
    overlay: &[u32],
) -> bool {
    use crate::render_thread::render_pass::retained_static::{
        RetainedModels, retained_frame_picture, retained_scene_is_current,
    };
    let current = app.model_picture();
    let size = neomacs_renderer_wgpu::SnapshotSize::new(800, 600).unwrap();
    // No editor scene change between these frames: one generation.
    let rebuild = !retained
        .as_ref()
        .is_some_and(|built| retained_scene_is_current((1, size, built), (1, size, &current)));
    if rebuild {
        // The scene's model glyphs are referenced; the composite draws
        // those whose texture exists (the renderer's surface cache holds
        // exactly the drawable models).
        let referenced: std::collections::HashSet<u32> = scene.iter().copied().collect();
        let drawn = referenced
            .iter()
            .copied()
            .filter(|id| current.contains_key(id))
            .collect();
        *retained = Some(RetainedModels::record(Some(drawn), referenced, &current));
    }
    let picture = retained_frame_picture(
        retained.as_ref().map(|r| &r.picture),
        super::picture_of(Some(overlay.iter().copied().collect()), &current),
    );
    app.note_models_presented(window, &picture);
    rebuild
}

/// A retained scene holding a child frame's model keeps showing it across
/// a cursor-only frame whose filled-box overlay redraws no model.
#[test]
fn a_cursor_overlay_keeps_an_unchanged_retained_child_model() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    let mut retained = None;
    // The child frame's model is composited into the retained raster.
    assert!(present_retained(&mut app, A, &mut retained, &[MODEL], &[]));
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    assert!(!present_retained(&mut app, A, &mut retained, &[MODEL], &[]));
    assert!(
        delivered(&emacs).is_empty(),
        "the retained child model is still on screen"
    );
}

/// Two windows: A shows the model in a retained child frame, B in its
/// root. After a Lisp change only B fully renders; A's next frame is
/// cursor-only. A's raster pictures the old camera, so it is rebuilt and
/// A presents the new revision; A then still counts, so a later revision
/// B presents alone does not raise the fence.
#[test]
fn two_windows_a_retained_child_member_still_counts_for_readiness() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    let mut retained = None;
    present_retained(&mut app, A, &mut retained, &[MODEL], &[]);
    present(&mut app, B);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    update(&mut app, &mut host, 1, yaw(30.0));
    app.models.render_dirty(&mut host);
    present(&mut app, B);
    assert_eq!(
        delivered(&emacs),
        ["R{false,2,1}"],
        "A still pictures rev 1"
    );
    assert!(
        present_retained(&mut app, A, &mut retained, &[MODEL], &[]),
        "A's raster pictures the old camera"
    );
    assert_eq!(delivered(&emacs), ["R{true,2,1}"]);
    update(&mut app, &mut host, 2, yaw(60.0));
    app.models.render_dirty(&mut host);
    present(&mut app, B);
    assert_eq!(delivered(&emacs), ["R{false,3,2}"], "A still counts");
    present_retained(&mut app, A, &mut retained, &[MODEL], &[]);
    assert_eq!(delivered(&emacs), ["R{true,3,2}"]);
}

/// A full frame whose model glyph is fully clipped composites nothing of
/// it: the window leaves even though the frame still carries the glyph.
#[test]
fn a_full_frame_with_the_model_fully_clipped_leaves() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    present(&mut app, B);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    app.note_models_presented(A, &super::ModelPicture::new());
    assert!(delivered(&emacs).is_empty(), "B still shows it");
    app.note_models_presented(B, &super::ModelPicture::new());
    assert_eq!(delivered(&emacs), ["R{false,1,0}"]);
}

/// Review MUST 5: a Lisp change, a full presentation, then a cursor-only
/// frame in the same native window with no redisplay in between. The
/// retained raster built before the change must not come back, and a
/// click picks the camera on screen.
#[test]
fn a_cursor_frame_after_a_model_change_shows_and_picks_the_new_camera() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    let mut retained = None;
    assert!(present_retained(&mut app, A, &mut retained, &[MODEL], &[]));
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    let before = app.models.rendered(MODEL).unwrap();

    update(&mut app, &mut host, 1, yaw(30.0));
    app.models.render_dirty(&mut host);
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["R{false,2,1}", "R{true,2,1}"]);

    // A cursor blink: compositor-only, same generation and size.
    assert!(
        present_retained(&mut app, A, &mut retained, &[MODEL], &[]),
        "the raster from before the change was blitted again"
    );
    assert!(delivered(&emacs).is_empty(), "A still shows rev 2");
    assert_eq!(retained.as_ref().unwrap().picture[&MODEL].view.yaw, 30.0);

    assert!(app.models.press(A, hit(), (10.0, 10.0)));
    app.models
        .release(A, Some(hit()), Instant::now(), &mut host);
    let pick = host.picks.last().expect("a click picks");
    assert_eq!(pick.view.yaw, 30.0);
    assert_ne!(pick.view, before.view);
}

// ----------------------------------------------------------------------
// Review SHOULD 3: fractional wheel input over a model accumulates.

fn pixels(y: f64) -> winit::event::MouseScrollDelta {
    winit::event::MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, y))
}

fn zoom(app: &RenderApp) -> f64 {
    app.models.view(MODEL).unwrap().zoom
}

/// Touchpad deltas too small to be a notch each add up: the event that
/// crosses a notch zooms once, and the burst settles with one View 150 ms
/// after its last event, not before.
#[test]
fn sub_notch_touchpad_events_add_up_to_a_zoom_that_settles_after_the_quiet_period() {
    use neomacs_renderer_wgpu::model::store::WHEEL_QUIET;
    use winit::event::{DeviceId, TouchPhase};
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    let pad = Some(DeviceId::from_raw(1));
    let t0 = Instant::now();
    let ms = |n: u64| t0 + Duration::from_millis(n);
    // 20 px is 0.4 notch: alone it rounds to nothing.
    app.wheel_over_model(pad, MODEL, pixels(20.0), TouchPhase::Started, ms(0));
    app.wheel_over_model(pad, MODEL, pixels(20.0), TouchPhase::Moved, ms(10));
    assert_eq!(zoom(&app), 1.0, "0.8 notch is not yet a notch");
    app.wheel_over_model(pad, MODEL, pixels(20.0), TouchPhase::Moved, ms(20));
    assert!((zoom(&app) - 1.1).abs() < 1e-9, "1.2 notches zoom once");
    let events = delivered(&emacs);
    assert!(!events.iter().any(|e| e.starts_with("View")), "{events:?}");

    app.models
        .tick(ms(20) + WHEEL_QUIET - Duration::from_millis(1), None);
    app.flush_model_events();
    assert!(delivered(&emacs).is_empty(), "the burst is not quiet yet");
    app.models.tick(ms(20) + WHEEL_QUIET, None);
    app.flush_model_events();
    assert_eq!(delivered(&emacs), ["View{2}"]);
}

/// A remainder belongs to one device, one model and one gesture: it does
/// not carry across a quiet period, a new gesture, another device or
/// another model.
#[test]
fn a_wheel_remainder_resets_on_quiet_gesture_device_and_model_changes() {
    use neomacs_renderer_wgpu::model::store::WHEEL_QUIET;
    use winit::event::{DeviceId, TouchPhase};
    let (mut app, _emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    let pad = Some(DeviceId::from_raw(1));
    let other = Some(DeviceId::from_raw(2));
    let t0 = Instant::now();
    let ms = |n: u64| t0 + Duration::from_millis(n);
    let wheel = |app: &mut RenderApp, device, id, y, phase, at| {
        app.model_wheel.notches(device, id, pixels(y), phase, at)
    };
    // 0.6 then 0.6 within one gesture: one notch.
    assert_eq!(
        wheel(&mut app, pad, MODEL, 30.0, TouchPhase::Started, ms(0)),
        0
    );
    assert_eq!(
        wheel(&mut app, pad, MODEL, 30.0, TouchPhase::Moved, ms(10)),
        1
    );
    // 0.2 carried; after a quiet period it is gone.
    assert_eq!(
        wheel(
            &mut app,
            pad,
            MODEL,
            40.0,
            TouchPhase::Moved,
            ms(10) + WHEEL_QUIET
        ),
        0,
        "a stale remainder joined a later burst"
    );
    // 0.8 carried; a new gesture starts from zero.
    assert_eq!(
        wheel(&mut app, pad, MODEL, 30.0, TouchPhase::Started, ms(200)),
        0
    );
    // 0.6 carried; another device does not inherit it.
    assert_eq!(
        wheel(&mut app, other, MODEL, 30.0, TouchPhase::Moved, ms(210)),
        0
    );
    // 0.6 carried for `other`; another model does not inherit it.
    assert_eq!(
        wheel(&mut app, other, MODEL + 1, 30.0, TouchPhase::Moved, ms(220)),
        0
    );
    // Negative deltas accumulate the same way, toward zoom-out.
    assert_eq!(
        wheel(&mut app, pad, MODEL, -30.0, TouchPhase::Started, ms(300)),
        0
    );
    assert_eq!(
        wheel(&mut app, pad, MODEL, -30.0, TouchPhase::Moved, ms(310)),
        -1
    );
    // A whole notch from a mouse wheel is still exactly one notch.
    assert_eq!(
        app.model_wheel.notches(
            None,
            MODEL,
            winit::event::MouseScrollDelta::LineDelta(0.0, 1.0),
            TouchPhase::Moved,
            ms(400)
        ),
        1
    );
}

// ----------------------------------------------------------------------
// Review MUST 9: renderer loss reaches the ordered input channel at once.

fn model_events(emacs: &EmacsComms) -> Vec<InputEvent> {
    emacs.input_rx.try_iter().collect()
}

/// current -> renderer loss -> a key typed afterwards: the evaluator sees
/// `Readiness{current: nil}` before that key, with no replacement renderer.
/// Runs on the fake GPU host: the ordering is the render thread's, not the
/// device's, so no adapter is needed (OPUS-REVIEW SHOULD 4).
#[test]
fn renderer_loss_reaches_the_input_channel_before_a_later_key() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    let current = model_events(&emacs);
    assert!(
        matches!(
            current.last(),
            Some(InputEvent::Model {
                id: MODEL,
                event: ModelEvent::Readiness { current: true, .. }
            })
        ),
        "current before the loss, got {current:?}"
    );

    app.renderer = None;
    app.note_models_renderer_lost();
    app.comms.send_input(InputEvent::Key {
        key: neovm_core::keyboard::FrontendKey::Character('x'),
        modifiers: 0,
        pressed: true,
        emacs_frame_id: A,
    });
    let after = model_events(&emacs);
    assert!(
        matches!(
            after.as_slice(),
            [
                InputEvent::Model {
                    id: MODEL,
                    event: ModelEvent::Readiness { current: false, .. }
                },
                InputEvent::Key { .. } | InputEvent::Observed { .. } | InputEvent::Tracked { .. }
            ]
        ),
        "R{{nil}} must precede the later key, got {after:?}"
    );
}

// ----------------------------------------------------------------------
// Round 2.

/// Pre-review MUST 1: a model freed while loading leaves the store empty;
/// `service_models` still drains its load result, so the loop stops its
/// 16 ms poll and the parsed scene is not kept in the channel.
#[test]
fn a_model_freed_while_loading_stops_the_poll_once_its_result_arrives() {
    let (mut app, emacs) = app_with_channel();
    let dir = std::env::temp_dir().join(format!(
        "neomacs-model-runtime-freed-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("quad.obj");
    std::fs::write(
        &path,
        b"v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n",
    )
    .unwrap();
    app.handle_model_create(
        MODEL,
        vec![ModelMeshSpec { path, sha256: None }],
        ModelView::default(),
        [0.0, 0.0, 0.0, 1.0],
        (64, 32),
    );
    assert!(app.handle_model_free(MODEL));
    assert!(app.models.is_empty());
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.model_deadline(Instant::now()).is_some() {
        assert!(
            Instant::now() < deadline,
            "an empty store still polls: the freed load was never drained"
        );
        std::thread::sleep(Duration::from_millis(2));
        app.service_models(neomacs_display_protocol::frame_time::observe_platform_now());
    }
    assert!(!app.models.has_pending_loads());
    assert!(
        delivered(&emacs).is_empty(),
        "a freed model reports nothing"
    );
}

/// Display-lane open item: touchpad input too small to complete a notch
/// still extends the burst it belongs to; the burst settles a quiet
/// period after the last input, not after the last whole notch.
#[test]
fn sub_notch_input_keeps_the_wheel_burst_active() {
    use neomacs_renderer_wgpu::model::store::WHEEL_QUIET;
    use winit::event::{DeviceId, TouchPhase};
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    let pad = Some(DeviceId::from_raw(1));
    let t0 = Instant::now();
    let ms = |n: u64| t0 + Duration::from_millis(n);
    // 50 px is one notch; 20 px (0.4 notch) completes none.
    app.wheel_over_model(pad, MODEL, pixels(50.0), TouchPhase::Started, ms(0));
    app.wheel_over_model(pad, MODEL, pixels(20.0), TouchPhase::Moved, ms(100));
    assert!((zoom(&app) - 1.1).abs() < 1e-9);
    assert_eq!(delivered(&emacs), ["R{false,2,0}"]);
    assert_eq!(app.model_deadline(ms(100)), Some(ms(100) + WHEEL_QUIET));
    app.models.tick(ms(0) + WHEEL_QUIET, None);
    app.flush_model_events();
    assert!(delivered(&emacs).is_empty(), "input kept arriving");
    app.models.tick(ms(100) + WHEEL_QUIET, None);
    app.flush_model_events();
    assert_eq!(delivered(&emacs), ["View{2}"]);
}

// ----------------------------------------------------------------------
// Round 3.

/// SOL-REVIEW-2 MUST 3 / OPUS-REVIEW MUST 1: a retained raster built while
/// a model it references is still loading pictures nothing of it. Once the
/// model uploads and a full frame shows it, the next cursor-only frame at
/// the same editor generation and size must rebuild the raster rather than
/// blit the one without the model; the window stays a member.
#[test]
fn a_raster_built_while_the_model_loads_is_rebuilt_once_the_model_is_drawable() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    create_model(&mut app);
    let mut retained = None;
    // Inserted while loading: the full frame and the first cursor blink
    // composite nothing of it, and the raster is built that way.
    present(&mut app, A);
    assert!(present_retained(&mut app, A, &mut retained, &[MODEL], &[]));
    finish_loading(&mut app, &mut host);
    assert_eq!(delivered(&emacs), ["ready"]);
    // The upload's full frame shows it.
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["R{true,1,0}"]);
    // The next blink: same generation, same size, no redisplay.
    assert!(
        present_retained(&mut app, A, &mut retained, &[MODEL], &[]),
        "the raster from before the upload was blitted again"
    );
    assert!(
        delivered(&emacs).is_empty(),
        "A still shows the model at rev 1"
    );
}

/// OPUS-REVIEW SHOULD 1: on Wayland neither hiding nor minimizing can be
/// observed (winit's `is_visible`/`is_minimized` answer `None`, and no
/// `Occluded` is sent), and a compositor may ignore the request outright.
/// Such a request leaves at once, but the window rejoins at its next
/// presentation instead of staying out until a focus or exposure event
/// that may never come.
#[test]
fn a_hide_the_platform_cannot_confirm_leaves_only_until_the_next_presentation() {
    use crate::thread_comm::WindowVisibility;
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    for visibility in [WindowVisibility::Invisible, WindowVisibility::Iconified] {
        app.note_models_visibility(A, visibility, false);
        assert_eq!(delivered(&emacs), ["R{false,1,0}"], "{visibility:?}");
        // The compositor ignored it: the window still presents.
        present(&mut app, A);
        assert_eq!(delivered(&emacs), ["R{true,1,0}"], "{visibility:?}");
    }
    // The command handler: with no native window to ask, nothing confirms
    // the hiding either.
    app.handle_window(crate::thread_comm::WindowCommand::SetWindowVisibility {
        frame: crate::thread_comm::FrameRef::Frame(A),
        visibility: WindowVisibility::Invisible,
    });
    assert_eq!(delivered(&emacs), ["R{false,1,0}"]);
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["R{true,1,0}"]);
    // Where the platform does report hiding, the window stays out until
    // shown, as before.
    app.note_models_visibility(A, WindowVisibility::Iconified, true);
    assert_eq!(delivered(&emacs), ["R{false,1,0}"]);
    present(&mut app, A);
    assert!(delivered(&emacs).is_empty());
}

/// SOL-REVIEW-2 SHOULD 3: a fraction left over a model does not join a
/// later wheel event after the pointer left the window or the window lost
/// focus, even within the quiet period and with no wheel event between.
#[test]
fn a_wheel_remainder_does_not_survive_leaving_the_window_or_losing_focus() {
    use winit::event::{DeviceId, TouchPhase};
    let (mut app, _emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    let pad = Some(DeviceId::from_raw(1));
    let t0 = Instant::now();
    let ms = |n: u64| t0 + Duration::from_millis(n);
    let wheel = |app: &mut RenderApp, y, phase, at| {
        app.model_wheel.notches(pad, MODEL, pixels(y), phase, at)
    };
    // 0.6 notch left over the model, then the pointer leaves the window.
    assert_eq!(wheel(&mut app, 30.0, TouchPhase::Started, ms(0)), 0);
    app.handle_cursor_left(winit::window::WindowId::from_raw(1));
    assert_eq!(
        wheel(&mut app, 30.0, TouchPhase::Moved, ms(10)),
        0,
        "the remainder survived leaving the window"
    );
    // 0.6 left again; the window loses focus.
    assert_eq!(wheel(&mut app, 30.0, TouchPhase::Started, ms(20)), 0);
    app.handle_window_focus_models(A, false);
    assert_eq!(
        wheel(&mut app, 30.0, TouchPhase::Moved, ms(30)),
        0,
        "the remainder survived focus loss"
    );
    // 0.6 left; the pointer moves within the model's glyph: the gesture
    // goes on.
    assert_eq!(wheel(&mut app, 30.0, TouchPhase::Started, ms(40)), 0);
    app.model_wheel.pointer_over(Some(MODEL));
    assert_eq!(wheel(&mut app, 30.0, TouchPhase::Moved, ms(50)), 1);
    // 0.2 left; the pointer moves off the model and back (what
    // `handle_cursor_moved` reports through `pointer_over`).
    app.model_wheel.pointer_over(None);
    app.model_wheel.pointer_over(Some(MODEL));
    assert_eq!(
        wheel(&mut app, 40.0, TouchPhase::Moved, ms(60)),
        0,
        "the remainder survived the pointer leaving the model"
    );
}

/// OPUS-REVIEW NIT 1: a window hidden while no model exists, then deleted,
/// leaves no hidden mark behind.
#[test]
fn a_window_hidden_while_no_model_exists_is_forgotten_when_it_goes() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    app.note_models_window_hidden(A);
    app.note_models_window_gone(A);
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
}

/// OPUS-REVIEW NIT 2: focus leaving one window does not end an orbit held
/// in another; focus leaving the capture's own window does.
#[test]
fn focus_loss_ends_only_the_capture_of_that_window() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    assert!(app.models.press(A, hit(), (10.0, 10.0)));
    assert!(app.models.motion(A, (30.0, 10.0)), "orbiting");
    app.flush_model_events();
    assert_eq!(delivered(&emacs), ["R{false,2,0}"]);
    app.handle_window_focus_models(B, false);
    assert!(delivered(&emacs).is_empty(), "A's orbit was ended by B");
    assert!(app.models.motion(A, (40.0, 10.0)), "A's orbit goes on");
    app.handle_window_focus_models(A, false);
    assert_eq!(delivered(&emacs), ["View{3}"]);
    assert!(!app.models.motion(A, (50.0, 10.0)), "the capture ended");
}

// SOL-REVIEW-2 MUST 2: a frame whose composition also shows an older
// picture (pane morph, frame/theme transition, child resize crossfade).

/// A presentation through a composition that blends an older picture in,
/// whose fresh glyph passes composited the models in `scene`.
fn present_blended(app: &mut RenderApp, window: u64, scene: &[u32]) {
    let picture = super::picture_of(Some(scene.iter().copied().collect()), &app.model_picture());
    app.note_models_presented_blended(window, &picture);
}

/// A theme crossfade starting in the frame where the camera moves from X
/// to Y shows X (and later a blend of both) while the fresh passes draw Y:
/// neither the fence nor a pick may treat the window as showing Y until a
/// frame without the older picture presents.
#[test]
fn a_transition_blending_an_older_camera_neither_raises_the_fence_nor_picks() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    update(&mut app, &mut host, 1, yaw(30.0));
    app.models.render_dirty(&mut host);
    app.flush_model_events();
    assert_eq!(delivered(&emacs), ["R{false,2,1}"]);

    present_blended(&mut app, A, &[MODEL]);
    assert!(
        delivered(&emacs).is_empty(),
        "the fence rose for Y while the crossfade shows X"
    );
    assert!(app.models.press(A, hit(), (10.0, 10.0)));
    app.models
        .release(A, Some(hit()), Instant::now(), &mut host);
    assert!(host.picks.is_empty(), "a pick resolved against Y over X");

    // The transition ends: the window shows Y alone.
    present(&mut app, A);
    assert_eq!(delivered(&emacs), ["R{true,2,1}"]);
    assert!(app.models.press(A, hit(), (10.0, 10.0)));
    app.models
        .release(A, Some(hit()), Instant::now(), &mut host);
    assert_eq!(host.picks.last().expect("picks again").view.yaw, 30.0);
}

/// Switching a window away from a model under a crossfade: the fresh
/// passes no longer draw it but the outgoing picture still shows it, so
/// the window stays a member (holding the fence down, since the blend is
/// no one snapshot) until a frame without the older picture presents.
#[test]
fn an_outgoing_crossfade_keeps_the_window_a_member_until_it_ends() {
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    present(&mut app, B);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);

    present_blended(&mut app, A, &[]);
    assert_eq!(
        delivered(&emacs),
        ["R{false,1,0}"],
        "A left while its crossfade still shows the model"
    );
    assert!(app.models.press(A, hit(), (10.0, 10.0)));
    app.models
        .release(A, Some(hit()), Instant::now(), &mut host);
    assert!(host.picks.is_empty(), "a pick resolved against a blend");

    // The crossfade ends without the model: A leaves, B alone counts.
    app.note_models_presented(A, &super::ModelPicture::new());
    assert_eq!(delivered(&emacs), ["R{true,1,0}"]);
}
