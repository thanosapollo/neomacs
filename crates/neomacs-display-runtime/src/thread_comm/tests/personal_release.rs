//! Combined transport contracts; no native window, PTY or GPU fixture.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[test]
fn cancelled_gui_lease_and_opacity_refresh_survive_startup_transport() {
    let (emacs, render) = ThreadComms::new().split();
    assert!(Arc::ptr_eq(&emacs.frame_opacity, &render.frame_opacity));
    emacs.frame_opacity.lock().unwrap().accept(7, [0.5; 2], 0.2);
    let live = Arc::new(AtomicBool::new(true));
    let (reply, _response) = bounded(1);
    emacs
        .cmd_tx
        .try_send(RenderCommand::Window(WindowCommand::RealizeFrame {
            frame: FrameRef::Frame(7),
            width: 800,
            height: 600,
            title: "combined transport".to_owned(),
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
            reply,
            live: Arc::clone(&live),
            deadline: Instant::now() + Duration::from_secs(1),
        }))
        .unwrap();
    emacs
        .cmd_tx
        .try_send(RenderCommand::Window(WindowCommand::RefreshFrameOpacity))
        .unwrap();
    // Cancellation occurs after admission but before startup extraction.
    live.store(false, Ordering::Release);
    let RenderCommand::Window(WindowCommand::RealizeFrame {
        frame,
        live: received_live,
        ..
    }) = render.cmd_rx.try_recv_startup().unwrap()
    else {
        panic!("realization must remain ahead of the opacity refresh");
    };
    assert_eq!(frame.raw_id(), 7);
    assert!(Arc::ptr_eq(&live, &received_live));
    assert!(!received_live.load(Ordering::Acquire));
    assert_eq!(render.frame_opacity.lock().unwrap().applied(7), Some(0.5));
    assert!(matches!(
        render.cmd_rx.try_recv_startup().unwrap(),
        RenderCommand::Window(WindowCommand::RefreshFrameOpacity)
    ));
    assert!(render.cmd_rx.try_recv().is_err());
}

#[cfg(feature = "neo-term")]
#[test]
fn terminal_cwd_delivery_does_not_consume_the_following_repeat_receipt() {
    let (emacs, render) = ThreadComms::new().split();
    let id = crate::terminal::TerminalId::new(17).unwrap();
    render.send_input(InputEvent::TerminalDirectoryChanged {
        id,
        directory: "/home/α b".to_owned(),
    });
    let (read, completion, _) = render.send_key_input_with_receipt(InputEvent::Key {
        key: neovm_core::keyboard::FrontendKey::Character('p'),
        modifiers: 0,
        pressed: true,
        emacs_frame_id: 7,
    });
    let read = read.unwrap();
    assert!(completion.is_none());
    assert!(matches!(
        emacs.input_rx.try_recv().unwrap(),
        InputEvent::TerminalDirectoryChanged { id: actual, directory }
            if actual == id && directory == "/home/α b"
    ));
    assert!(!read.consumed_or_cancelled());
    let InputEvent::Tracked { receipt, event } = emacs.input_rx.try_recv().unwrap() else {
        panic!("native repeat must retain read ownership after terminal metadata");
    };
    assert!(matches!(
        *event,
        InputEvent::Key {
            key: neovm_core::keyboard::FrontendKey::Character('p'),
            emacs_frame_id: 7,
            ..
        }
    ));
    let mut progress = neomacs_display_protocol::input_progress::InputProgress::default();
    progress.consumed(receipt);
    assert!(read.consumed_or_cancelled());
    assert!(progress.current_command_receipts().is_empty());
}
