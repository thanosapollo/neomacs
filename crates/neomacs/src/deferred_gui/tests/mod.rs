use super::*;
use neomacs_display_protocol::GraphicalBackend;

#[test]
fn deferred_gui_selection_uses_lisp_runtime_and_absolute_socket() {
    let mut eval = Context::new();
    eval.set_variable(
        "process-environment",
        Value::list(vec![Value::string("XDG_RUNTIME_DIR=/chosen/lisp/runtime")]),
    );
    let runtime = lisp_environment(&mut eval, "XDG_RUNTIME_DIR").unwrap();
    assert_eq!(runtime.as_deref(), Some("/chosen/lisp/runtime"));
    #[cfg(target_os = "linux")]
    {
        assert_eq!(
            DisplaySelection {
                name: Some("selected".into()),
                runtime
            }
            .socket()
            .unwrap(),
            std::path::PathBuf::from("/chosen/lisp/runtime/selected")
        );
        assert_eq!(
            DisplaySelection {
                name: Some("/absolute/socket".into()),
                runtime: None
            }
            .socket()
            .unwrap(),
            std::path::PathBuf::from("/absolute/socket")
        );
        assert!(
            DisplaySelection {
                name: Some("selected".into()),
                runtime: None
            }
            .socket()
            .is_err()
        );
    }
}

#[test]
fn deferred_gui_cancelled_disposition_survives_later_success() {
    use neomacs_display_runtime::render_thread::{RenderLoopError, RenderLoopExit};
    let mut disposition = NativeDisposition::default();
    disposition.observe(&Ok(RenderLoopExit::Finished));
    assert!(!disposition.bypass_finalizers);
    disposition.observe(&Ok(RenderLoopExit::GpuStartupCancelled));
    disposition.observe(&Ok(RenderLoopExit::Finished));
    assert!(disposition.bypass_finalizers);
    let mut error = NativeDisposition::default();
    error.observe(&Err(RenderLoopError::StartupInterrupted(
        "controlled startup failure".into(),
    )));
    assert!(error.bypass_finalizers);
}

#[test]
fn deferred_gui_failed_input_spawn_does_not_publish_terminal() {
    let mut eval = Context::new();
    let before = eval.eval_str("(terminal-list)").unwrap();
    let identity =
        GraphicalDisplayIdentity::named(GraphicalBackend::Wayland, "failed-bridge").unwrap();
    assert!(
        admit_input_bridge(identity, || Err(std::io::Error::other(
            "injected spawn failure"
        )))
        .is_err()
    );
    let after = eval.eval_str("(terminal-list)").unwrap();
    eval.set_variable("terminals-before", before);
    eval.set_variable("terminals-after", after);
    assert!(
        eval.eval_str("(equal terminals-before terminals-after)")
            .unwrap()
            .is_truthy()
    );
}

#[test]
fn deferred_gui_successful_input_spawn_publishes_terminal() {
    let mut eval = Context::new();
    let identity =
        GraphicalDisplayIdentity::named(GraphicalBackend::Wayland, "ready-bridge").unwrap();
    let terminal =
        admit_input_bridge(identity, || std::thread::Builder::new().spawn(|| {})).unwrap();
    assert!(terminal > 0);
    assert!(
        eval.eval_str("(let ((terminals (terminal-list)) found) (while terminals (if (equal (terminal-name (car terminals)) \"ready-bridge\") (setq found (terminal-live-p (car terminals)))) (setq terminals (cdr terminals))) found)")
            .unwrap()
            .is_truthy()
    );
}
