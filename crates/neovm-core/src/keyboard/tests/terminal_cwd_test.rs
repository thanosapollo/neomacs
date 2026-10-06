use super::*;

// Source-only matrix: both actual dequeues, pending FIFO and host channel,
// active/no-op owner hooks, active and previously-stopped idle epochs.
fn assert_cwd_dequeue_preserves_idle(wait_special: bool) {
    for from_host in [false, true] {
        for active_owner in [false, true] {
            for initially_idle in [false, true] {
                let mut eval = crate::emacs_core::Context::new();
                eval.eval_str(if active_owner {
                    "(setq neo-term-directory-changed-functions (list (lambda (id directory) (setq cwd-seen (list id directory)))))"
                } else {
                    // Model an ignored stale owner at the hook boundary.
                    "(setq neo-term-directory-changed-functions (list (lambda (id directory) nil)))"
                }).unwrap();
                eval.eval_str(
                    "(setq timer-idle-list (list [nil 0 0 0 nil ignore nil idle 0 nil]))",
                )
                .unwrap();
                let epoch = Instant::now() - Duration::from_secs(5);
                let start = initially_idle.then_some(epoch);
                eval.command_loop.idle_start_time = start;
                // A stopped prior epoch must not be resumed accidentally.
                eval.command_loop.last_idle_start_time = Some(epoch);
                let before = eval.current_idle_duration();
                assert_eq!(
                    eval.next_idle_gnu_timer_timeout(),
                    initially_idle.then_some(Duration::ZERO)
                );
                let event = InputEvent::TerminalDirectoryChanged {
                    id: crate::emacs_core::display_host::TerminalId::new(42).unwrap(),
                    directory: "/home/α b".into(),
                };
                let (tx, rx) = crossbeam_channel::unbounded();
                eval.input_rx = Some(rx);
                if from_host {
                    tx.send(event).unwrap();
                } else {
                    eval.command_loop
                        .keyboard
                        .pending_input_events
                        .push_back(event);
                }
                if wait_special {
                    let outcome = eval.service_wait_request_special_input_events().unwrap();
                    assert_eq!(outcome, SpecialInputServiceOutcome::default());
                } else {
                    let event = eval.drain_ready_input_event_for_read_char().unwrap();
                    // Check before dispatch: direct handler tests miss the
                    // unconditional idle stop at the actual dequeue.
                    assert_eq!(eval.command_loop.idle_start_time, start);
                    assert!(
                        eval.handle_read_char_input_event(
                            event,
                            TtyInputDecoding::KeyboardCodingSystem
                        )
                        .unwrap()
                        .is_none()
                    );
                }
                assert_eq!(eval.command_loop.idle_start_time, start);
                assert_eq!(eval.command_loop.last_idle_start_time, Some(epoch));
                assert_eq!(
                    eval.next_idle_gnu_timer_timeout(),
                    initially_idle.then_some(Duration::ZERO)
                );
                assert_eq!(eval.current_idle_time_value().is_nil(), !initially_idle);
                if let Some(before) = before {
                    assert!(eval.current_idle_duration().unwrap() >= before);
                }
                assert!(eval.command_loop.keyboard.pending_input_events.is_empty());
                assert_eq!(
                    eval.eval_str(if active_owner {
                        "(equal cwd-seen '(42 \"/home/α b\"))"
                    } else {
                        "(null (boundp 'cwd-seen))"
                    })
                    .unwrap(),
                    Value::T
                );
                // Keep the sender live until servicing finishes to exclude
                // channel disconnect/quit from the cwd regression.
                drop(tx);
            }
        }
    }
}

#[test]
fn terminal_cwd_wait_special_dequeue_preserves_existing_idle_epoch() {
    assert_cwd_dequeue_preserves_idle(true);
}

#[test]
fn terminal_cwd_read_char_dequeue_preserves_existing_idle_epoch() {
    assert_cwd_dequeue_preserves_idle(false);
    let mut eval = crate::emacs_core::Context::new();
    let epoch = Instant::now();
    eval.command_loop.idle_start_time = Some(epoch);
    eval.command_loop
        .keyboard
        .pending_input_events
        .push_back(InputEvent::key_press(KeyEvent::char('x')));
    assert!(eval.drain_ready_input_event_for_read_char().is_some());
    assert_eq!(eval.command_loop.idle_start_time, None);
    assert_eq!(eval.command_loop.last_idle_start_time, Some(epoch));
}

#[test]
fn terminal_cwd_notification_is_wait_service_not_command_input() {
    let event = InputEvent::TerminalDirectoryChanged {
        id: crate::emacs_core::display_host::TerminalId::new(42).unwrap(),
        directory: "/home/α b".into(),
    };
    assert!(!crate::frontend_events::interrupts(&event));
    assert!(crate::frontend_events::is_wait_special(&event, false));
    assert!(crate::frontend_events::is_wait_special(&event, true));
}

#[test]
fn terminal_cwd_production_read_char_dispatch_calls_typed_hook() {
    let mut eval = crate::emacs_core::Context::new();
    eval.eval_str("(setq neo-term-directory-changed-functions (list (lambda (id directory) (setq cwd-seen (list id directory)))))").unwrap();
    let event = InputEvent::TerminalDirectoryChanged {
        id: crate::emacs_core::display_host::TerminalId::new(42).unwrap(),
        directory: "/home/α b".into(),
    };
    assert!(
        eval.handle_read_char_input_event(event, TtyInputDecoding::KeyboardCodingSystem)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        eval.eval_str("(equal cwd-seen '(42 \"/home/α b\"))")
            .unwrap(),
        Value::T
    );
}
