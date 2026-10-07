//! Eager macro-expansion failures propagate like GNU's (P4.1 S1.0).
//!
//! GNU `readevalloop_eager_expand_eval` (src/lread.c:2135-2152) has no
//! fallback: the error `internal-macroexpand-for-load` raises
//! (lisp/emacs-lisp/macroexp.el:915-948) aborts the load or `eval-buffer` at
//! the failing form.  The expected strings are GNU 31.1's output for the same
//! forms (`emacs -Q --batch`); the oracle pins are
//! `neovm-oracle-tests/src/eager_expansion_failure_semantics.rs`.

use super::{
    EagerExpansionFailure, ImageConstructionExpansionScope, eager_expansion_failure_policy,
};
use crate::test_utils::runtime_startup_eval_one;

/// T1.1: a macro that signals while a `defun` body is eagerly expanded.
/// GNU: the load signals `Eager macro-expansion failure`, the form after the
/// failing one never runs, and neither does the failing `defun` itself; the
/// same holds for `eval-buffer` of a buffer (the `SourceFormEvaluation` path).
const BAD_EXPANSION_FORM: &str = r#"
(let* ((dir (make-temp-file "neo-eager-" t))
       (file (expand-file-name "bad-expand.el" dir)))
  (unwind-protect
      (progn
        (with-temp-file file
          (insert ";;; -*- lexical-binding: t; -*-\n"
                  "(defmacro bad-m (x) (if (eq x 'boom) (error \"boom in expander\") `(list ,x)))\n"
                  "(defvar bad-ran nil)\n"
                  "(defun bad-f () (bad-m boom))\n"
                  "(setq bad-ran t)\n"))
        (list (condition-case err (progn (load file nil t) 'loaded) (error err))
              (and (boundp 'bad-ran) bad-ran)
              (fboundp 'bad-f)
              (with-temp-buffer
                (insert "(defmacro bad-m2 (x) (if (eq x 'boom) (error \"boom2\") `(list ,x)))\n"
                        "(setq bad-ran2 'before)\n"
                        "(defun bad-f2 () (bad-m2 boom))\n"
                        "(setq bad-ran2 t)\n")
                (list (condition-case err (progn (eval-buffer) 'evaluated) (error err))
                      (and (boundp 'bad-ran2) bad-ran2)
                      (fboundp 'bad-f2)))))
    (delete-directory dir t)))
"#;

#[test]
fn eager_expansion_failure_aborts_load_and_eval_buffer_like_gnu() {
    crate::test_utils::init_test_tracing();
    assert_eq!(
        runtime_startup_eval_one(BAD_EXPANSION_FORM),
        r#"OK ((error "Eager macro-expansion failure: (error \"boom in expander\")") nil nil ((error "Eager macro-expansion failure: (error \"boom2\")") before nil))"#
    );
}

/// T1.2: a macro whose expander loads the file being expanded.  GNU detects
/// the cycle in the nested load (`load-file-name` is on
/// `macroexp--pending-eager-loads`), signals the cycle error there, and the
/// outer expansion wraps it in `Eager macro-expansion failure`.  The file
/// body ran once (the nested load stops at its first form), and the pending
/// list is empty again afterwards.
const EXPANSION_CYCLE_FORM: &str = r#"
(let* ((dir (make-temp-file "neo-eager-cycle-" t))
       (file (expand-file-name "cyc-a.el" dir)))
  (unwind-protect
      (progn
        (with-temp-file file
          (insert ";;; -*- lexical-binding: t; -*-\n"
                  "(defvar cyc-loads 0)\n"
                  "(setq cyc-loads (1+ cyc-loads))\n"
                  "(defmacro cyc-m () (load load-file-name nil t) 1)\n"
                  "(defvar cyc-x (cyc-m))\n"
                  "(setq cyc-done t)\n"))
        (defvar cyc-loads)
        (defvar cyc-done)
        (list (condition-case err (progn (load file nil t) 'loaded)
                (error (list (car err)
                             (replace-regexp-in-string (regexp-quote dir) "DIR" (cadr err)))))
              (and (boundp 'cyc-loads) cyc-loads)
              (and (boundp 'cyc-done) cyc-done)
              (and (boundp 'cyc-x) cyc-x)
              macroexp--pending-eager-loads))
    (delete-directory dir t)))
"#;

#[test]
fn eager_expansion_cycle_signals_like_gnu() {
    crate::test_utils::init_test_tracing();
    assert_eq!(
        runtime_startup_eval_one(EXPANSION_CYCLE_FORM),
        "OK ((error \"Eager macro-expansion failure: (error \\\"Eager macro-expansion skipped due to cycle:\n  … => (load \\\\\\\"cyc-a.el\\\\\\\") => (macroexpand-all (defvar cyc-x …)) => (load \\\\\\\"cyc-a.el\\\\\\\")\\\")\") 1 nil nil nil)"
    );
}

/// A `throw` out of an expander is not an error: GNU's `condition-case` in
/// `internal-macroexpand-for-load` does not see it, so it reaches the
/// enclosing `catch` and the rest of the file never runs.
#[test]
fn throw_out_of_an_eager_expander_reaches_the_enclosing_catch() {
    crate::test_utils::init_test_tracing();
    let form = r#"
(let* ((dir (make-temp-file "neo-eager-throw-" t))
       (file (expand-file-name "thr.el" dir)))
  (unwind-protect
      (progn
        (with-temp-file file
          (insert ";;; -*- lexical-binding: t; -*-\n"
                  "(defmacro thr-m () (throw 'neo-eager-tag 'thrown))\n"
                  "(defun thr-f () (thr-m))\n"
                  "(setq thr-after t)\n"))
        (defvar thr-after)
        (list (catch 'neo-eager-tag (load file nil t) 'loaded)
              (and (boundp 'thr-after) thr-after)
              (fboundp 'thr-f)))
    (delete-directory dir t)))
"#;
    assert_eq!(runtime_startup_eval_one(form), "OK (thrown nil nil)");
}

/// A Lisp tree with no `.elc`: `macroexp.el` is interpreted and `pcase` is
/// not defined yet, so `macroexp--expand-all`'s own `pcase` calls fail.  GNU
/// 32.0.50 signals `Eager macro-expansion failure: (void-function pcase)` for
/// a plain load of `pcase.el` in that state, and loadup.el:148-157 avoids it
/// by loading `pcase.el` with `macroexp--pending-eager-loads` = `(skip)` and
/// then reloading `macroexp.el`; the test runtime helpers must do the same.
#[test]
fn uncompiled_pcase_loads_only_with_loadup_eager_skip_like_gnu() {
    crate::test_utils::init_test_tracing();
    let lisp = crate::test_utils::workspace_root().join("lisp");
    let dir = tempfile::tempdir().expect("tempdir");
    let source_only_runtime = || {
        let mut eval = crate::emacs_core::eval::Context::new();
        eval.set_lexical_binding(true);
        eval.set_variable(
            "load-path",
            crate::emacs_core::value::Value::list(vec![crate::emacs_core::value::Value::string(
                dir.path().to_string_lossy().to_string(),
            )]),
        );
        for name in [
            "emacs-lisp/debug-early",
            "emacs-lisp/byte-run",
            "emacs-lisp/backquote",
            "subr",
        ] {
            super::load_file(&mut eval, &dir.path().join(format!("{name}.el")))
                .unwrap_or_else(|err| panic!("load {name}: {err:?}"));
        }
        eval
    };
    std::fs::create_dir_all(dir.path().join("emacs-lisp")).expect("mkdir emacs-lisp");
    for name in [
        "emacs-lisp/debug-early.el",
        "emacs-lisp/byte-run.el",
        "emacs-lisp/backquote.el",
        "subr.el",
        "emacs-lisp/macroexp.el",
        "emacs-lisp/pcase.el",
    ] {
        std::fs::copy(lisp.join(name), dir.path().join(name))
            .unwrap_or_else(|err| panic!("copy {name}: {err}"));
    }

    let mut plain = source_only_runtime();
    super::load_file(&mut plain, &dir.path().join("emacs-lisp/macroexp.el"))
        .expect("load macroexp.el");
    let plain_load = plain
        .eval_str(&format!(
            "(condition-case err (progn (load {:?} nil t t) 'ok) (error err))",
            dir.path().join("emacs-lisp/pcase.el").to_string_lossy()
        ))
        .expect("plain pcase.el load");
    assert_eq!(
        crate::emacs_core::print::print_value(&plain_load),
        r#"(error "Eager macro-expansion failure: (void-function pcase)")"#
    );

    let mut eval = source_only_runtime();
    crate::test_utils::load_gnu_macroexp_runtime(&mut eval);
    let state = eval
        .eval_str(
            "(list (compiled-function-p (symbol-function 'macroexpand-all))
                   (featurep 'pcase)
                   (pcase 1 (1 'one))
                   (pcase '(a . 2) (`(a . ,n) n))
                   macroexp--pending-eager-loads)",
        )
        .expect("pcase after loadup sequence");
    assert_eq!(
        crate::emacs_core::print::print_value(&state),
        "(nil t one 2 nil)"
    );
}

/// The fallback is scoped to image construction and restored on exit.
#[test]
fn image_construction_scope_is_the_only_fallback_and_nests() {
    crate::test_utils::init_test_tracing();
    assert_eq!(
        eager_expansion_failure_policy(),
        EagerExpansionFailure::Propagate
    );
    {
        let _outer = ImageConstructionExpansionScope::enter();
        assert_eq!(
            eager_expansion_failure_policy(),
            EagerExpansionFailure::FallBackWhileBuildingImage
        );
        {
            let _inner = ImageConstructionExpansionScope::enter();
        }
        assert_eq!(
            eager_expansion_failure_policy(),
            EagerExpansionFailure::FallBackWhileBuildingImage
        );
    }
    assert_eq!(
        eager_expansion_failure_policy(),
        EagerExpansionFailure::Propagate
    );
}
