use super::*;
use std::{cell::Cell, rc::Rc};

#[test]
fn gui_service_keeps_deadline_and_pending_input_order() {
    let mut eval = crate::emacs_core::Context::new();
    eval.eval_str("(setq inhibit-redisplay nil)").unwrap();
    let captures = Rc::new(Cell::new(0));
    let observed = captures.clone();
    eval.display_idle_maintenance_fn = Some(Box::new(move |_| {
        observed.set(observed.get() + 1);
        (None, true)
    }));
    let paints = Rc::new(Cell::new(0));
    let observed = paints.clone();
    eval.redisplay_fn = Some(Box::new(move |_| observed.set(observed.get() + 1)));
    for ch in ['a', 'b'] {
        eval.command_loop.unread_key(KeyEvent::char(ch));
    }
    let now = Instant::now();
    eval.service_gui_command_boundary_at(now).unwrap();
    eval.service_gui_command_boundary_at(now + Duration::from_millis(8))
        .unwrap();
    assert_eq!(captures.get(), 0);
    eval.service_gui_command_boundary_at(now + Duration::from_millis(20))
        .unwrap();
    assert_eq!(captures.get(), 1);
    assert_eq!(paints.get(), 1);
    eval.service_gui_command_boundary_at(now + Duration::from_millis(21))
        .unwrap();
    assert_eq!(captures.get(), 1);
    assert_eq!(
        eval.command_loop.read_key_event(),
        Some(Value::fixnum('a' as i64))
    );
    assert_eq!(
        eval.command_loop.read_key_event(),
        Some(Value::fixnum('b' as i64))
    );
    eval.service_gui_command_boundary_at(now + Duration::from_millis(50))
        .unwrap();
    assert_eq!(captures.get(), 1, "idle wait owns idle maintenance");
    assert!(eval.command_loop.gui_display_deadline.is_none());
}

#[test]
fn gui_service_never_paints_inside_a_keyboard_macro() {
    let mut eval = crate::emacs_core::Context::new();
    eval.eval_str("(setq inhibit-redisplay nil)").unwrap();
    eval.display_idle_maintenance_fn = Some(Box::new(|_| (None, true)));
    let paints = Rc::new(Cell::new(0));
    let observed = paints.clone();
    eval.redisplay_fn = Some(Box::new(move |_| observed.set(observed.get() + 1)));
    eval.command_loop.unread_key(KeyEvent::char('a'));
    eval.command_loop
        .keyboard
        .begin_executing_kbd_macro(vec![Value::fixnum('b' as i64)]);
    let now = Instant::now();
    for ms in [0, 20, 40, 60] {
        eval.service_gui_command_boundary_at(now + Duration::from_millis(ms))
            .unwrap();
    }
    assert_eq!(paints.get(), 0);
    assert!(eval.command_loop.gui_display_deadline.is_none());
    eval.command_loop.keyboard.finish_executing_kbd_macro();
    eval.service_gui_command_boundary_at(now + Duration::from_millis(80))
        .unwrap();
    eval.service_gui_command_boundary_at(now + Duration::from_millis(100))
        .unwrap();
    assert_eq!(
        paints.get(),
        1,
        "pending input still paints after the macro"
    );
}

/// The same rule through the real command loop: `execute-kbd-macro` runs
/// each macro command through `command_loop_1`, whose boundary service
/// would otherwise paint (typed input pending, frame deadline overdue).
#[test]
fn keyboard_macro_commands_run_without_automatic_repaints() {
    let mut eval = crate::emacs_core::Context::new();
    eval.eval_str("(setq inhibit-redisplay nil)").unwrap();
    eval.display_idle_maintenance_fn = Some(Box::new(|_| (None, true)));
    let paints = Rc::new(Cell::new(0));
    let observed = paints.clone();
    eval.redisplay_fn = Some(Box::new(move |_| observed.set(observed.get() + 1)));
    // Typed input waiting in the frontend transport: macro events are
    // read before it, so it stays pending for the whole macro.
    let (tx, rx) = crossbeam_channel::unbounded();
    eval.input_rx = Some(rx);
    tx.send(crate::keyboard::InputEvent::KeyPress {
        key: KeyEvent::char('z'),
        emacs_frame_id: 0,
    })
    .unwrap();
    eval.command_loop.gui_display_deadline = Some(Instant::now() - Duration::from_secs(1));
    let steps = eval
        .eval_str(
            r#"(progn
                 (setq neo-kmacro-steps 0)
                 (fset 'command-execute
                       (lambda (command &optional _record _keys _special)
                         (funcall command)))
                 (fset 'neo-kmacro-step
                       (lambda () (interactive)
                         (setq neo-kmacro-steps (1+ neo-kmacro-steps))))
                 (let ((global (make-sparse-keymap)))
                   (use-global-map global)
                   (define-key global "a" 'neo-kmacro-step)
                   (execute-kbd-macro "aaaaaaaa"))
                 neo-kmacro-steps)"#,
        )
        .unwrap();
    assert_eq!(steps, Value::fixnum(8));
    assert_eq!(paints.get(), 0);
    assert!(eval.command_loop.gui_display_deadline.is_none());
}
