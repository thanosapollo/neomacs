//! Render-thread model glue on a real headless renderer: renderer loss is
//! published on the ordered input channel at once (review MUST 9).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use neomacs_display_protocol::model::{ModelEvent, ModelMeshSpec, ModelView};
use neomacs_renderer_wgpu::model::ModelStatus;

use super::super::RenderApp;
use crate::thread_comm::{EmacsComms, InputEvent, ThreadComms};

fn make_app() -> (RenderApp, EmacsComms) {
    let comms = ThreadComms::new();
    let (emacs, render) = comms.split();
    let app = RenderApp::new(
        render,
        64,
        64,
        "test".to_string(),
        Arc::new(crate::render_thread::ImageRenderState::default()),
        Arc::new((Mutex::new(Vec::new()), std::sync::Condvar::new())),
        true,
        #[cfg(feature = "neo-term")]
        crate::terminal::new_shared_terminals(),
    );
    (app, emacs)
}

fn model_events(emacs: &EmacsComms) -> Vec<InputEvent> {
    emacs.input_rx.try_iter().collect()
}

/// current -> renderer loss -> a key typed afterwards: the evaluator sees
/// `Readiness{current: nil}` before that key, with no replacement renderer.
#[test]
fn renderer_loss_reaches_the_input_channel_before_a_later_key() {
    let Ok(renderer) = neomacs_renderer_wgpu::WgpuRenderer::new(None, 64, 64) else {
        assert!(
            std::env::var_os("NEOMACS_MODEL_REQUIRE_EVIDENCE").is_none(),
            "GPU evidence required but no headless wgpu adapter"
        );
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let (mut app, emacs) = make_app();
    app.renderer = Some(renderer);
    let dir = std::env::temp_dir().join(format!("neomacs-runtime-model-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("triangle.obj");
    std::fs::write(&path, b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();

    app.handle_model_create(
        5,
        vec![ModelMeshSpec { path, sha256: None }],
        ModelView::default(),
        [0.0, 0.0, 0.0, 1.0],
        (32, 32),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.models.status(5) == Some(ModelStatus::Loading) {
        assert!(Instant::now() < deadline, "the load never finished");
        std::thread::sleep(Duration::from_millis(5));
        app.service_models();
    }
    app.render_dirty_models();
    app.models
        .presented(1, &HashSet::from([5]), &HashSet::from([5]));
    app.flush_model_events();
    let current = model_events(&emacs);
    assert!(
        matches!(
            current.last(),
            Some(InputEvent::Model {
                id: 5,
                event: ModelEvent::Readiness { current: true, .. }
            })
        ),
        "current before the loss"
    );

    app.renderer = None;
    app.note_models_renderer_lost();
    app.comms.send_input(InputEvent::Key {
        key: neovm_core::keyboard::FrontendKey::Character('x'),
        modifiers: 0,
        pressed: true,
        emacs_frame_id: 1,
    });
    let after = model_events(&emacs);
    assert!(
        matches!(
            after.as_slice(),
            [
                InputEvent::Model {
                    id: 5,
                    event: ModelEvent::Readiness { current: false, .. }
                },
                InputEvent::Key { .. } | InputEvent::Observed { .. } | InputEvent::Tracked { .. }
            ]
        ),
        "R{{nil}} must precede the later key, got {after:?}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
