use super::*;
use crate::emacs_core::display_host::{TerminalCompletion, TerminalId};

#[test]
fn native_settlement_actual_dequeues_preserve_idle_and_dispatch_status_before_legacy_hook() {
    for wait_special in [false, true] {
        for from_host in [false, true] {
            for initially_idle in [false, true] {
                let mut eval = crate::emacs_core::Context::new();
                eval.eval_str("(setq settled-seen nil neo-term-settled-functions (list (lambda (&rest args) (setq settled-seen args))) neo-term-exit-functions (list (lambda (id) (setq legacy-seen (list id settled-seen)))))").unwrap();
                let epoch = Instant::now() - Duration::from_secs(5);
                let start = initially_idle.then_some(epoch);
                eval.command_loop.idle_start_time = start;
                eval.command_loop.last_idle_start_time = Some(epoch);
                let event = InputEvent::TerminalSettled {
                    id: TerminalId::new(42).unwrap(),
                    completion: TerminalCompletion {
                        exit_code: None,
                        signal: Some("Terminated".into()),
                        wait_error: None,
                        output_drained: false,
                        read_error: Some("cancelled".into()),
                    },
                };
                assert!(!crate::frontend_events::interrupts(&event));
                assert!(crate::frontend_events::is_wait_special(&event, true));
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
                    assert_eq!(
                        eval.service_wait_request_special_input_events().unwrap(),
                        SpecialInputServiceOutcome::default()
                    );
                } else {
                    let event = eval.drain_ready_input_event_for_read_char().unwrap();
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
                    eval.eval_str(
                        "(equal legacy-seen '(42 (42 nil \"Terminated\" nil nil \"cancelled\")))"
                    )
                    .unwrap(),
                    Value::T
                );
                drop(tx);
            }
        }
    }
}
