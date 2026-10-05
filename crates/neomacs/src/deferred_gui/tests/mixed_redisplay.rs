//! The installed daemon callback, real PTY output and CPU GUI mailbox; no native loop.
use super::*;
use neovm_core::emacs_core::load::create_bootstrap_evaluator_cached_with_features;
use neovm_core::window::{FrameDisplayIdentity, FrameVisibility};
use std::fs::{File, OpenOptions};
use std::io::Read;

fn drain(master: &mut File) -> String {
    let mut bytes = Vec::new();
    let mut buf = [0; 4096];
    loop {
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => panic!("PTY drain: {e}"),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn invoke(eval: &mut Context) {
    let mut callback = eval.redisplay_fn.take().expect("installed daemon callback");
    callback(eval);
    eval.redisplay_fn = Some(callback);
}

fn opened_pty() -> (File, File, String) {
    let master = File::from(
        rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
            .unwrap(),
    );
    rustix::pty::grantpt(&master).unwrap();
    rustix::pty::unlockpt(&master).unwrap();
    rustix::fs::fcntl_setfl(&master, rustix::fs::OFlags::NONBLOCK).unwrap();
    let slave_name = rustix::pty::ptsname(&master, Vec::new())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let slave = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&slave_name)
        .unwrap();
    rustix::termios::tcsetwinsize(
        &slave,
        rustix::termios::Winsize {
            ws_row: 8,
            ws_col: 40,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .unwrap();
    (master, slave, slave_name)
}

#[test]
fn selected_tty_also_publishes_modified_visible_gui_without_selection_changes() {
    mixed(true);
}
#[test]
fn selected_initial_daemon_also_publishes_modified_visible_gui_without_selection_changes() {
    mixed(false);
}

fn mixed(select_tty: bool) {
    let mut eval = create_bootstrap_evaluator_cached_with_features(&["neomacs"]).unwrap();
    let startup =
        parse_startup_options(["neomacs", "-Q", "--fg-daemon"].map(str::to_owned)).unwrap();
    let _bootstrap = bootstrap_buffers(
        &mut eval,
        40,
        8,
        bootstrap_tty_display_config(Interactivity::Batch),
    );
    let initial = eval.frame_manager().selected_frame().unwrap().id;
    configure_gnu_startup_state(&mut eval, initial, &startup);
    let frame = eval.frame_manager().get(initial).unwrap();
    assert!(frame.initial);
    assert!(frame.effective_window_system().is_none());
    assert!(!frame.displays_chrome);

    let (mut master, _slave, slave_name) = opened_pty();
    let (mut other_master, _other_slave, other_slave_name) = opened_pty();
    let ttys = secondary_tty::SecondaryTtyRegistry::default();
    let (input_tx, _input_rx) = crossbeam_channel::bounded(32);
    eval.set_tty_frame_host_factory(Box::new(secondary_tty::SecondaryTtyFactory::new(
        ttys.clone(),
        input_tx,
        None,
        eval.quit_requested.clone(),
    )));
    eval.set_variable("test-tty-device", Value::string(slave_name));
    eval.eval_str("(setq test-tty-frame (make-terminal-frame (list (cons 'tty test-tty-device) (cons 'tty-type \"xterm-256color\"))))").unwrap();
    eval.set_variable("test-other-tty-device", Value::string(other_slave_name));
    eval.eval_str("(setq test-other-tty-frame (make-terminal-frame (list (cons 'tty test-other-tty-device) (cons 'tty-type \"xterm-256color\"))))").unwrap();
    let shared = eval.buffer_manager_mut().create_buffer("*mixed-redisplay*");
    eval.buffer_manager_mut().create_buffer("*other-tty*");
    let identity = GraphicalDisplayIdentity::named(
        neomacs_display_protocol::GraphicalBackend::Wayland,
        "cpu-mixed-redisplay",
    )
    .unwrap();
    let terminal =
        neovm_core::emacs_core::terminal::pure::register_graphical_terminal(identity.clone());
    let gui = eval
        .frame_manager_mut()
        .create_frame_on_terminal("cpu-gui", terminal, 480, 240, shared);
    {
        let frame = eval.frame_manager_mut().get_mut(gui).unwrap();
        frame.initial = false;
        frame.visibility = FrameVisibility::Visible;
        frame.set_window_system(Some(Value::symbol("neo")));
        frame.set_display_identity(FrameDisplayIdentity::Graphical(identity));
    }
    eval.set_variable("test-gui-frame", Value::make_frame(gui.0));
    eval.set_variable("test-initial-frame", Value::make_frame(initial.0));
    eval.eval_str("(setq test-shared-buffer (get-buffer \"*mixed-redisplay*\")) (set-window-buffer (frame-selected-window test-tty-frame) test-shared-buffer) (set-window-buffer (frame-selected-window test-other-tty-frame) (get-buffer \"*other-tty*\")) (set-window-buffer (frame-selected-window test-gui-frame) test-shared-buffer)").unwrap();
    eval.eval_str(if select_tty {
        "(select-frame test-tty-frame)"
    } else {
        "(select-frame test-initial-frame)"
    })
    .unwrap();
    eval.eval_str("(set-buffer test-shared-buffer)").unwrap();
    frame_layout::REDISPLAY_RUNTIME.with(|runtime| runtime.enable_cosmic_metrics());
    let comms = ThreadComms::new();
    install_redisplay_callback(&mut eval, ttys.clone(), comms.frame_tx, None);
    assert_eq!(ttys.render_selected(&mut eval), select_tty);
    drain(&mut master); // terminal-opening control bytes are not redisplay evidence.
    drain(&mut other_master);
    let selected = eval.frame_manager().selected_frame().unwrap().id;
    let window = eval
        .frame_manager()
        .selected_frame()
        .unwrap()
        .selected_window;
    let buffer = eval.buffer_manager().current_buffer_id();
    let mut previous = None;
    for text in ["OLDMARK", "NEWMARK"] {
        let other_text = format!("OTHER-{text}");
        eval.eval_str(&format!(
            "(with-current-buffer test-shared-buffer (erase-buffer) (insert \"{text}\\n\")) (with-current-buffer \"*other-tty*\" (erase-buffer) (insert \"{other_text}\\n\"))"
        ))
        .unwrap();
        invoke(&mut eval);
        let tty_output = drain(&mut master);
        let other_output = drain(&mut other_master);
        assert!(
            tty_output.contains(text),
            "first attached TTY output missing {text}: {tty_output:?}"
        );
        assert!(
            !tty_output.contains(&other_text),
            "first TTY received another terminal's frame"
        );
        assert!(
            other_output.contains(&other_text),
            "second attached TTY output missing {other_text}: {other_output:?}"
        );
        let state = comms
            .frame_rx
            .try_recv()
            .expect("actual installed callback must submit visible unselected GUI");
        assert_eq!(state.frame_placement.frame().get(), gui.0);
        assert!(
            state.render_text().contains(text),
            "fresh GUI content missing {text}"
        );
        assert_ne!(previous, Some(state.presentation_id.get()));
        previous = Some(state.presentation_id.get());
        assert!(
            comms.frame_rx.try_recv().is_err(),
            "TTY/initial frames are not native submissions"
        );
        assert_eq!(eval.frame_manager().selected_frame().unwrap().id, selected);
        assert_eq!(
            eval.frame_manager()
                .selected_frame()
                .unwrap()
                .selected_window,
            window
        );
        assert_eq!(eval.buffer_manager().current_buffer_id(), buffer);
        let presentation =
            neovm_core::window::geometry::PresentationId::new(state.presentation_id.get());
        eval.frame_manager_mut()
            .get_mut(gui)
            .unwrap()
            .activate_display_presentation(presentation)
            .unwrap();
    }
    ttys.close_all();
}
