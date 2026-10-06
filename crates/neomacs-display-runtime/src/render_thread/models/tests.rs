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
