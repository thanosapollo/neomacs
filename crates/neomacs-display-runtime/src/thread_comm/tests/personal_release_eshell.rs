//! Source-only combined Eshell, deferred GUI and repeat transport regressions.
//! No PTY, native window, GPU or socket is created by these definitions.

use super::*;
use neovm_core::emacs_core::display_host::{TerminalCompletion, TerminalInvocation};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[cfg(feature = "neo-term")]
#[test]
fn eshell_exact_invocation_survives_cancelled_gui_and_opacity_startup_extraction() {
    let (emacs, render) = ThreadComms::new().split();
    let id = crate::terminal::TerminalId::new(17).unwrap();
    let invocation = TerminalInvocation {
        executable: "/bin/echo".into(),
        argv: vec!["".into(), "space λ".into(), "; $()".into()],
        directory: "/literal directory/".into(),
        environment: vec![
            "SHELL=/bin/sh".into(),
            "A=first=tail".into(),
            "UNSET".into(),
        ],
    };
    emacs
        .cmd_tx
        .try_send(RenderCommand::Terminal(TerminalCommand::TerminalCreate {
            id,
            size: crate::terminal::TerminalGridSize::new(80, 24).unwrap(),
            target: crate::terminal::TerminalDisplayTarget::Floating,
            shell: None,
            invocation: Some(invocation.clone()),
        }))
        .unwrap();
    emacs.frame_opacity.lock().unwrap().accept(7, [0.5; 2], 0.2);
    let live = Arc::new(AtomicBool::new(true));
    let (reply, _response) = bounded(1);
    emacs
        .cmd_tx
        .try_send(RenderCommand::Window(WindowCommand::RealizeFrame {
            frame: FrameRef::Frame(7),
            width: 800,
            height: 600,
            title: "combined Eshell transport".into(),
            geometry_hints: GuiFrameGeometryHints {
                base_width: 0,
                base_height: 0,
                min_width: 1,
                min_height: 1,
                width_inc: 1,
                height_inc: 1,
            },
            fullscreen: None,
            visual: None,
            adopt_primary: false,
            reply: Some(reply),
            live: Arc::clone(&live),
            deadline: Instant::now() + Duration::from_secs(1),
        }))
        .unwrap();
    emacs
        .cmd_tx
        .try_send(RenderCommand::Window(WindowCommand::RefreshFrameOpacity))
        .unwrap();
    live.store(false, Ordering::Release);
    let RenderCommand::Window(WindowCommand::RealizeFrame { live: received, .. }) =
        render.cmd_rx.try_recv_startup().unwrap()
    else {
        panic!("CPU startup must bypass the staged terminal without consuming it");
    };
    assert!(Arc::ptr_eq(&live, &received));
    assert!(!received.load(Ordering::Acquire));
    assert!(matches!(
        render.cmd_rx.try_recv_startup().unwrap(),
        RenderCommand::Window(WindowCommand::RefreshFrameOpacity)
    ));
    assert!(render.cmd_rx.try_recv_startup().is_err());
    assert_eq!(render.frame_opacity.lock().unwrap().applied(7), Some(0.5));
    assert!(matches!(render.cmd_rx.try_recv().unwrap(),
        RenderCommand::Terminal(TerminalCommand::TerminalCreate { id: actual, shell: None, invocation: Some(received), .. })
            if actual == id && received == invocation));
    assert!(render.cmd_rx.try_recv().is_err());
}

#[cfg(feature = "neo-term")]
#[test]
fn eshell_settlement_keeps_following_page_key_read_and_completion_receipts_independent() {
    let (emacs, render) = ThreadComms::new().split();
    let id = crate::terminal::TerminalId::new(17).unwrap();
    let settled = TerminalCompletion {
        exit_code: Some(17),
        signal: None,
        wait_error: None,
        output_drained: true,
        read_error: None,
    };
    render.send_input(InputEvent::TerminalSettled {
        id,
        completion: settled.clone(),
    });
    let (read, completion, _) = render.send_key_input_with_receipt(InputEvent::Key {
        key: neovm_core::keyboard::FrontendKey::Keysym(0xff55),
        modifiers: 0,
        pressed: true,
        emacs_frame_id: 7,
    });
    let read = read.unwrap();
    let completion = completion.unwrap();
    assert!(!read.same_input(&completion));
    assert!(matches!(emacs.input_rx.try_recv().unwrap(),
        InputEvent::TerminalSettled { id: actual, completion: received }
            if actual == id && received == settled));
    assert!(!read.consumed_or_cancelled());
    let mut progress = neomacs_display_protocol::input_progress::InputProgress::default();
    let command = progress.begin_command();
    let InputEvent::Tracked { receipt, event } = emacs.input_rx.try_recv().unwrap() else {
        panic!("settlement must not consume the following read receipt");
    };
    progress.consumed(receipt);
    let InputEvent::Tracked { receipt, event } = *event else {
        panic!("page key must retain its separate command completion receipt");
    };
    assert!(matches!(
        *event,
        InputEvent::Key {
            key: neovm_core::keyboard::FrontendKey::Keysym(0xff55),
            emacs_frame_id: 7,
            ..
        }
    ));
    progress.consumed(receipt);
    assert!(read.consumed_or_cancelled());
    assert!(!completion.acknowledged_by(&progress.checkpoint()));
    assert_eq!(progress.current_command_receipts().len(), 1);
    assert!(progress.current_command_receipts()[0].same_input(&completion));
    drop(command);
    assert!(completion.acknowledged_by(&progress.checkpoint()));
    assert!(emacs.input_rx.try_recv().is_err());
}
