use super::*;
use crate::emacs_core::eval::Context;

fn two_windows() -> Context {
    let mut eval = Context::new();
    eval.eval_str(
        r#"(progn
          (set-buffer (get-buffer-create "hook-points"))
          (set-window-buffer (selected-window) (current-buffer))
          (insert (make-string 2000 ?λ))
          (setq hook-first (selected-window))
          (setq hook-second (split-window-internal hook-first 40 t nil))
          (make-local-variable 'window-configuration-change-hook)
          (make-local-variable 'window-state-change-functions)
          (set-window-point hook-first 100)
          (set-window-point hook-second 500))"#,
    )
    .expect("two same-buffer windows");
    eval
}

fn assert_true(eval: &mut Context, source: &str) {
    assert_eq!(eval.eval_str(source).expect(source), Value::T);
}

#[test]
fn window_hook_context_configuration_adopts_each_point_and_preserves_it() {
    let mut eval = two_windows();
    assert_true(
        &mut eval,
        r#"(progn
      (setq hook-seen nil)
      (setq window-configuration-change-hook
        (list (lambda () (setq hook-seen (cons (point) hook-seen)))))
      (run-window-configuration-change-hook)
      (and (equal hook-seen '(500 100))
           (= (point) 100) (= (window-point hook-first) 100)
           (= (window-point hook-second) 500)))"#,
    );
}

#[test]
fn window_hook_context_same_selection_keeps_live_point_and_restriction() {
    let mut eval = two_windows();
    assert_true(
        &mut eval,
        r#"(progn
      (narrow-to-region 50 1000) (goto-char 123)
      (setq window-configuration-change-hook
        (list (lambda ()
          (if (null (= (point) (if (eq (selected-window) hook-first) 123 500)))
            (signal 'error '("wrong narrowed callback point"))))))
      (run-window-configuration-change-hook)
      (and (= (point) 123) (= (point-min) 50) (= (point-max) 1000)
           (= (window-point hook-first) 123) (= (window-point hook-second) 500)))"#,
    );
}

#[test]
fn window_hook_context_configuration_restores_distinct_current_buffer() {
    let mut eval = two_windows();
    assert_true(
        &mut eval,
        r#"(progn
      (setq window-configuration-change-hook
        (list (lambda ()
          (if (null (= (point) (if (eq (selected-window) hook-first) 100 500)))
            (signal 'error '("wrong point with distinct caller buffer"))))))
      (setq hook-other (get-buffer-create "hook-caller"))
      (set-buffer hook-other) (insert "independent caller")
      (narrow-to-region 3 10) (goto-char 7)
      (setq hook-order (buffer-list))
      (run-window-configuration-change-hook)
      (and (eq (current-buffer) hook-other) (eq (selected-window) hook-first)
           (= (point) 7) (= (point-min) 3) (= (point-max) 10)
           (equal (buffer-list) hook-order)
           (= (window-point hook-first) 100) (= (window-point hook-second) 500)))"#,
    );
}

#[test]
fn window_hook_context_configuration_keeps_callback_point_changes() {
    let mut eval = two_windows();
    assert_true(
        &mut eval,
        r#"(progn
      (setq window-configuration-change-hook
        (list (lambda () (goto-char (+ (point) 1)))))
      (run-window-configuration-change-hook)
      (and (= (point) 101) (= (window-point hook-first) 101)
           (= (window-point hook-second) 501)))"#,
    );
}

#[test]
fn window_hook_context_configuration_restores_on_signal_and_throw() {
    for callback in ["(signal 'error '(hook-signal))", "(throw 'hook-exit 17)"] {
        let mut eval = two_windows();
        eval.eval_str(&format!(
            r#"(setq window-configuration-change-hook
          (list (lambda () (if (eq (selected-window) hook-second)
            (progn (goto-char 501) {callback})))))"#
        ))
        .unwrap();
        let depth = eval.specpdl.len();
        if callback.starts_with("(signal") {
            assert!(
                eval.eval_str("(run-window-configuration-change-hook)")
                    .is_err()
            );
        } else {
            assert_eq!(
                eval.eval_str("(catch 'hook-exit (run-window-configuration-change-hook))")
                    .unwrap(),
                Value::fixnum(17)
            );
        }
        assert_eq!(eval.specpdl.len(), depth);
        assert_true(
            &mut eval,
            "(and (eq (selected-window) hook-first) (= (point) 100) (= (window-point hook-second) 501))",
        );
        assert_eq!(eval.eval_str("(+ 20 22)").unwrap(), Value::fixnum(42));
    }
}

#[test]
fn window_hook_context_configuration_global_snapshot_survives_gc() {
    let mut eval = two_windows();
    assert_true(
        &mut eval,
        r#"(progn
      (setq hook-global-point nil)
      (set-default 'window-configuration-change-hook
        (list (lambda () (setq hook-global-point (point)))))
      (setq window-configuration-change-hook
        (list (lambda ()
          (set-default 'window-configuration-change-hook nil)
          (garbage-collect))))
      (run-window-configuration-change-hook)
      (and (= hook-global-point 100) (= (point) 100)
           (= (window-point hook-second) 500)))"#,
    );
}

#[test]
fn window_hook_context_shared_local_runner_adopts_points_and_restores() {
    let mut eval = two_windows();
    eval.eval_str(
        r#"(progn (setq hook-seen nil)
      (setq window-state-change-functions
        (list (lambda (window)
          (setq hook-seen (cons (list (eq window (selected-window)) (point)) hook-seen))))))"#,
    )
    .unwrap();
    let fid = eval.frames.selected_frame().unwrap().id;
    let windows = eval.frames.get(fid).unwrap().window_list();
    let sym = hook_runtime::hook_symbol_by_name(&mut eval, "window-state-change-functions");
    run_window_local_hook_values(
        &mut eval,
        fid,
        &windows,
        "window-state-change-functions",
        sym,
    )
    .unwrap();
    assert_true(
        &mut eval,
        "(and (equal hook-seen '((t 500) (t 100))) (= (point) 100) (= (window-point hook-second) 500))",
    );
}

#[test]
fn window_hook_context_shared_local_signal_restores_and_keeps_point() {
    let mut eval = two_windows();
    eval.eval_str(r#"(setq window-state-change-functions
      (list (lambda (window) (if (eq window hook-second) (progn (goto-char 501) (signal 'error '(hook-signal)))))))"#).unwrap();
    let fid = eval.frames.selected_frame().unwrap().id;
    let windows = eval.frames.get(fid).unwrap().window_list();
    let sym = hook_runtime::hook_symbol_by_name(&mut eval, "window-state-change-functions");
    run_window_local_hook_values(
        &mut eval,
        fid,
        &windows,
        "window-state-change-functions",
        sym,
    )
    .unwrap();
    assert_true(
        &mut eval,
        "(and (eq (selected-window) hook-first) (= (point) 100) (= (window-point hook-second) 501))",
    );
}

#[test]
fn window_hook_context_callback_edits_keep_window_markers() {
    let mut eval = two_windows();
    assert_true(
        &mut eval,
        r#"(progn
      (setq window-configuration-change-hook
        (list (lambda () (if (eq (selected-window) hook-second) (insert "λ")))))
      (run-window-configuration-change-hook)
      (and (= (point) 100) (= (window-point hook-first) 100)
           (= (window-point hook-second) 501) (= (buffer-size) 2001)))"#,
    );
}

#[test]
fn window_hook_context_deleted_window_and_killed_caller_are_safe() {
    let mut eval = two_windows();
    assert_true(
        &mut eval,
        r#"(progn
      (setq window-configuration-change-hook
        (list (lambda () (if (eq (selected-window) hook-second)
          (progn (delete-window-internal hook-first) (kill-buffer hook-other))))))
      (setq hook-other (get-buffer-create "doomed-hook-caller"))
      (set-buffer hook-other)
      (run-window-configuration-change-hook)
      (and (eq (selected-window) hook-second)
           (null (window-live-p hook-first))
           (null (buffer-live-p hook-other))
           (= (window-point hook-second) 500)))"#,
    );
    assert_eq!(eval.eval_str("(+ 20 22)").unwrap(), Value::fixnum(42));
}

#[test]
fn window_hook_context_shared_default_runner_restores_current_buffer() {
    let mut eval = two_windows();
    eval.eval_str(
        r#"(progn
      (set-default 'window-state-change-functions
        (list (lambda (frame) (setq hook-default-point (point)))))
      (setq hook-other (get-buffer-create "hook-default-caller"))
      (set-buffer hook-other) (insert "caller") (goto-char 3))"#,
    )
    .unwrap();
    let fid = eval.frames.selected_frame().unwrap().id;
    let sym = hook_runtime::hook_symbol_by_name(&mut eval, "window-state-change-functions");
    run_window_default_hook_value(&mut eval, fid, true, sym).unwrap();
    assert_true(
        &mut eval,
        "(and (= hook-default-point 100) (eq (current-buffer) hook-other) (= (point) 3) (eq (selected-window) hook-first))",
    );
}
