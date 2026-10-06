//! Model membership and presentation provenance through the render-thread
//! glue (design §6.2): the window events, presentations and pointer input
//! a native window delivers, observed on the ordered input channel.

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
    let dir = std::env::temp_dir().join(format!(
        "neomacs-model-runtime-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("quad.obj");
    std::fs::write(&path, b"v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n").unwrap();
    app.models.create(
        MODEL,
        vec![ModelMeshSpec { path, sha256: None }],
        ModelView::default(),
        [0.0, 0.0, 0.0, 1.0],
        (64, 32),
    );
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
                ModelEvent::Readiness { current, rev, req } => format!("R{{{current},{rev},{req}}}"),
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

/// A frame of window `window` carrying the model glyph, presented with
/// the model composited (a full render that drew it).
pub(super) fn present(app: &mut RenderApp, window: u64) {
    let mut frame = crate::core::frame_glyphs::FrameGlyphBuffer::with_size(800.0, 600.0);
    frame.add_surface(
        neomacs_display_protocol::SurfaceId::new(MODEL),
        40.0,
        20.0,
        64.0,
        32.0,
    );
    app.note_models_composited(window, &std::collections::HashSet::from([MODEL]), &frame);
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
    assert!(delivered(&emacs).is_empty(), "exposure is not a presentation");
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
/// without any window-system occlusion event: the frame leaves at once,
/// keeps out while hidden, and rejoins by presenting once shown.
#[test]
fn hiding_or_iconifying_a_frame_leaves_until_shown_and_presented() {
    use crate::thread_comm::{FrameRef, WindowCommand, WindowVisibility};
    let (mut app, emacs) = app_with_channel();
    let mut host = FakeHost::default();
    with_ready_model(&mut app, &mut host);
    present(&mut app, A);
    present(&mut app, B);
    assert_eq!(delivered(&emacs), ["ready", "R{true,1,0}"]);
    for visibility in [WindowVisibility::Invisible, WindowVisibility::Iconified] {
        app.handle_window(WindowCommand::SetWindowVisibility {
            frame: FrameRef::Frame(A),
            visibility,
        });
        app.handle_window(WindowCommand::SetWindowVisibility {
            frame: FrameRef::Frame(B),
            visibility,
        });
        assert_eq!(delivered(&emacs), ["R{false,1,0}"]);
        present(&mut app, A);
        assert!(delivered(&emacs).is_empty(), "a hidden frame rejoined");
        app.handle_window(WindowCommand::SetWindowVisibility {
            frame: FrameRef::Frame(A),
            visibility: WindowVisibility::Visible,
        });
        assert!(delivered(&emacs).is_empty());
        present(&mut app, A);
        assert_eq!(delivered(&emacs), ["R{true,1,0}"]);
        app.handle_window(WindowCommand::SetWindowVisibility {
            frame: FrameRef::Frame(B),
            visibility: WindowVisibility::Visible,
        });
        present(&mut app, B);
    }
}
