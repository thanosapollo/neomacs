use super::*;

#[test]
fn input_dequeue_combines_timed_idle_policy_with_background_terminal_events() {
    use crate::emacs_core::display_host::{TerminalCompletion, TerminalId};
    use neomacs_display_protocol::input_progress::InputDelivery;

    for idle_policy in [IdleTransitionPolicy::Manage, IdleTransitionPolicy::Preserve] {
        for wait_special in [false, true] {
            for from_host in [false, true] {
                for initially_idle in [false, true] {
                    for tracked in [false, true] {
                        for kind in 0..3 {
                            // Wait-special services bare background events;
                            // the read-char path owns tracked input consumption.
                            if wait_special && (kind == 2 || tracked) {
                                continue;
                            }
                            let mut eval = crate::emacs_core::Context::new();
                            let epoch = Instant::now() - Duration::from_secs(5);
                            let start = initially_idle.then_some(epoch);
                            eval.command_loop.idle_start_time = start;
                            eval.command_loop.last_idle_start_time = Some(epoch);
                            let event = match kind {
                                0 => InputEvent::TerminalDirectoryChanged {
                                    id: TerminalId::new(42).unwrap(),
                                    directory: "/home/α b".into(),
                                },
                                1 => InputEvent::TerminalSettled {
                                    id: TerminalId::new(42).unwrap(),
                                    completion: TerminalCompletion {
                                        exit_code: Some(0),
                                        signal: None,
                                        wait_error: None,
                                        output_drained: true,
                                        read_error: None,
                                    },
                                },
                                _ => InputEvent::key_press(KeyEvent::char('x')),
                            };
                            let delivery = InputDelivery::for_read();
                            let receipt = delivery.receipt();
                            let event = if tracked {
                                InputEvent::Tracked {
                                    receipt: delivery.clone(),
                                    event: Box::new(event),
                                }
                            } else {
                                event
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
                            let expected = if kind == 2
                                && idle_policy == IdleTransitionPolicy::Manage
                            {
                                None
                            } else {
                                start
                            };
                            if wait_special {
                                assert_eq!(
                                    eval.service_wait_request_special_input_events_with_idle_policy(
                                        idle_policy,
                                    )
                                        .unwrap(),
                                    SpecialInputServiceOutcome::default()
                                );
                            } else {
                                let event = eval
                                    .drain_ready_input_event_for_read_char(idle_policy)
                                    .unwrap();
                                assert_eq!(eval.command_loop.idle_start_time, expected);
                                let value = eval
                                    .handle_read_char_input_event_with_idle_policy(
                                        event,
                                        TtyInputDecoding::KeyboardCodingSystem,
                                        idle_policy,
                                    )
                                    .unwrap();
                                assert_eq!(value.is_some(), kind == 2);
                            }
                            assert_eq!(eval.command_loop.idle_start_time, expected);
                            assert_eq!(eval.command_loop.last_idle_start_time, Some(epoch));
                            assert!(eval.command_loop.keyboard.pending_input_events.is_empty());
                            if tracked {
                                assert!(receipt.consumed_or_cancelled());
                                assert!(!receipt.cancelled());
                            }
                            drop(tx);
                        }
                    }
                }
            }
        }
    }
}

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
                    let event = eval
                        .drain_ready_input_event_for_read_char(IdleTransitionPolicy::Manage)
                        .unwrap();
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
    assert!(
        eval.drain_ready_input_event_for_read_char(IdleTransitionPolicy::Manage)
            .is_some()
    );
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
