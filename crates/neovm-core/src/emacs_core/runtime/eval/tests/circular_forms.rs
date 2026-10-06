//! Evaluating a form whose argument list is circular signals `circular-list`
//! like GNU instead of looping.  GNU `eval_sub` runs `list_length` over every
//! call form's arguments (`src/eval.c`), `let` measures its varlist the same
//! way, `let*` walks it with `FOR_EACH_TAIL`, and `macroexpand` hands the
//! arguments to `apply`, whose `list_length` signals.  `circular-list`'s
//! datum is the cons where the walk met its tortoise.
//!
//! Each case evaluates to `(CONDITION DATUM-IS-EXPECTED-CONS ...)`; the
//! expected strings are GNU 32.0.50's output for the same forms.  The cases
//! run on a worker thread so a regression fails here instead of hanging.

use crate::emacs_core::print::print_value;
use std::sync::mpsc;
use std::time::Duration;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(500);
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

const PRELUDE: &str = r#"
(defvar fix15-n 0)
(defalias 'fix15-f (lambda (&rest x) x))
(defalias 'fix15-g (make-byte-code 128 "\300\207" [nil] 1))
"#;

/// Evaluate each `(SOURCE EXPECTED)` case in one runtime-startup context on
/// a worker thread, bounding each case by [`CASE_TIMEOUT`], and assert
/// every printed result.  A case that never returns fails the test; the
/// worker dies with the test process.
fn assert_cases(cases: &[(&'static str, &'static str)]) {
    crate::test_utils::init_test_tracing();
    let sources: Vec<&'static str> = cases.iter().map(|(src, _)| *src).collect();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("circular-forms".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let mut eval = crate::test_utils::runtime_startup_context();
            let prelude = match eval.eval_str(PRELUDE) {
                Ok(_) => String::new(),
                Err(err) => format!("prelude: {err:?}"),
            };
            if tx.send(prelude).is_err() {
                return;
            }
            for src in sources {
                let printed = match eval.eval_str(src) {
                    Ok(value) => print_value(&value),
                    Err(err) => format!("ERR {err:?}"),
                };
                if tx.send(printed).is_err() {
                    return;
                }
            }
        })
        .expect("spawn circular-forms thread");
    let prelude = rx
        .recv_timeout(STARTUP_TIMEOUT)
        .expect("runtime startup and prelude should finish");
    assert_eq!(prelude, "");
    for (src, expected) in cases {
        let printed = rx
            .recv_timeout(CASE_TIMEOUT)
            .unwrap_or_else(|_| panic!("did not finish within {CASE_TIMEOUT:?}: {src}"));
        assert_eq!(printed, *expected, "{src}");
    }
}

#[test]
fn circular_form_progn_args_signal_circular_list() {
    assert_cases(&[(
        "(let ((c (list 'progn 1))) (setcdr (cdr c) (cdr c))
           (condition-case e (eval c) (error (list (car e) (eq (cadr e) (cdr c))))))",
        "(circular-list t)",
    )]);
}

#[test]
fn circular_form_cond_clauses_signal_circular_list() {
    assert_cases(&[(
        "(let ((c (list '(nil 1)))) (setcdr c c)
           (condition-case e (eval (cons 'cond c)) (error (list (car e) (eq (cadr e) c)))))",
        "(circular-list t)",
    )]);
}

#[test]
fn circular_form_let_varlist_signals_circular_list() {
    assert_cases(&[(
        "(let ((c (list '(a 1)))) (setcdr c c)
           (condition-case e (eval (list 'let c 'a)) (error (list (car e) (eq (cadr e) c)))))",
        "(circular-list t)",
    )]);
}

#[test]
fn circular_form_macroexpand_all_signals_circular_list() {
    assert_cases(&[(
        "(let ((c (list 'when t 1))) (setcdr (nthcdr 2 c) (nthcdr 2 c))
           (condition-case e (macroexpand-all c)
             (error (list (car e) (eq (cadr e) (nthcdr 2 c))))))",
        "(circular-list t)",
    )]);
}

/// Special forms: the up-front `list_length` in `eval_sub` signals before
/// the form runs, reporting the argument list for a cycle through it and
/// the looping cons for a prefixed cycle.
#[test]
fn circular_form_special_form_args_signal_circular_list() {
    let gnu = "(circular-list t)";
    assert_cases(&[
        (
            "(let ((c (list 'progn 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'progn 1 2 3))) (setcdr (nthcdr 3 c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'progn 1 2 3))) (setcdr (nthcdr 3 c) (nthcdr 3 c))
               (condition-case e (eval c t)
                 (error (list (car e) (eq (cadr e) (nthcdr 3 c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'and 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'or nil))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'if nil 1 2))) (setcdr (nthcdr 3 c) (nthcdr 3 c))
               (condition-case e (eval c t)
                 (error (list (car e) (eq (cadr e) (nthcdr 3 c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'setq 'x 1))) (setcdr (nthcdr 2 c) c)
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'quote 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'function 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'while nil 2))) (setcdr (nthcdr 2 c) (nthcdr 2 c))
               (condition-case e (eval c t)
                 (error (list (car e) (eq (cadr e) (nthcdr 2 c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'prog1 1 2))) (setcdr (nthcdr 2 c) (nthcdr 2 c))
               (condition-case e (eval c t)
                 (error (list (car e) (eq (cadr e) (nthcdr 2 c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'unwind-protect 1 2))) (setcdr (nthcdr 2 c) (nthcdr 2 c))
               (condition-case e (eval c t)
                 (error (list (car e) (eq (cadr e) (nthcdr 2 c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'condition-case nil 1 '(error 2))))
               (setcdr (nthcdr 3 c) (nthcdr 3 c))
               (condition-case e (eval c t)
                 (error (list (car e) (eq (cadr e) (nthcdr 3 c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'save-excursion 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'cond '(nil 1)))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
    ]);
}

/// `let` measures its varlist before evaluating any init form (so an
/// improper one signals first too); `let*` walks it with `FOR_EACH_TAIL`,
/// evaluating and binding each element once before the walk comes back
/// round.  The third element counts the init forms evaluated.
#[test]
fn circular_form_let_and_let_star_varlists_signal_circular_list() {
    assert_cases(&[
        (
            "(let ((c (list '(a 1)))) (setcdr c c)
               (condition-case e (eval (list 'let c 'a) t)
                 (error (list (car e) (eq (cadr e) c)))))",
            "(circular-list t)",
        ),
        (
            "(let ((c (list '(a 1)))) (setcdr c c)
               (condition-case e (eval (list 'let* c 'a))
                 (error (list (car e) (eq (cadr e) c)))))",
            "(circular-list t)",
        ),
        (
            "(let ((c (list '(a 1)))) (setcdr c c)
               (condition-case e (eval (list 'let* c 'a) t)
                 (error (list (car e) (eq (cadr e) c)))))",
            "(circular-list t)",
        ),
        (
            "(let ((c (list '(a (setq fix15-n (1+ fix15-n))) '(b 2))))
               (setcdr (cdr c) c) (setq fix15-n 0)
               (condition-case e (eval (list 'let c 'a) t)
                 (error (list (car e) (eq (cadr e) c) fix15-n))))",
            "(circular-list t 0)",
        ),
        (
            "(let ((c (list '(a (setq fix15-n (1+ fix15-n))) '(b 2))))
               (setcdr (cdr c) c) (setq fix15-n 0)
               (condition-case e (eval (list 'let* c 'a) t)
                 (error (list (car e) (eq (cadr e) c) fix15-n))))",
            "(circular-list t 1)",
        ),
        (
            "(condition-case e (eval '(let ((a (error \"init\")) . 3) a) t) (error e))",
            "(wrong-type-argument listp 3)",
        ),
    ]);
}

/// Calls of subrs, interpreted closures, byte-code and macros (`lambda`
/// included).
#[test]
fn circular_form_call_and_macro_args_signal_circular_list() {
    let gnu = "(circular-list t)";
    assert_cases(&[
        (
            "(let ((c (list 'car nil))) (setcdr (cdr c) c)
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'list 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list '(lambda (&rest x) x) 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'fix15-f 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'fix15-g 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'progn 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (funcall (eval (list 'function (list 'lambda nil c)) t))
                 (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'progn 1))) (setcdr (cdr c) (cdr c))
               (condition-case e (funcall (eval (list 'function (list 'lambda nil c)) nil))
                 (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'lambda nil 1))) (setcdr (nthcdr 2 c) (nthcdr 2 c))
               (condition-case e (eval c t)
                 (error (list (car e) (eq (cadr e) (nthcdr 2 c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'lambda nil 1))) (setcdr (nthcdr 2 c) (nthcdr 2 c))
               (condition-case e (eval c nil)
                 (error (list (car e) (eq (cadr e) (nthcdr 2 c))))))",
            gnu,
        ),
        (
            "(condition-case e (eval '(lambda nil 1 . 3) t) (error e))",
            "(wrong-type-argument listp 3)",
        ),
        (
            "(let ((c (list 'when t 1))) (setcdr (nthcdr 2 c) (nthcdr 2 c))
               (condition-case e (eval c t)
                 (error (list (car e) (eq (cadr e) (nthcdr 2 c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'when t 1))) (setcdr (nthcdr 2 c) c)
               (condition-case e (eval c t) (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
    ]);
}

/// `macroexpand` and `macroexpand-1` hand a macro call's arguments to
/// `apply`, whose `list_length` signals.
#[test]
fn circular_form_macroexpand_args_signal_circular_list() {
    let gnu = "(circular-list t)";
    assert_cases(&[
        (
            "(let ((c (list 'when t 1))) (setcdr (nthcdr 2 c) (nthcdr 2 c))
               (condition-case e (macroexpand c)
                 (error (list (car e) (eq (cadr e) (nthcdr 2 c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'when t 1))) (setcdr (nthcdr 2 c) c)
               (condition-case e (macroexpand c)
                 (error (list (car e) (eq (cadr e) (cdr c))))))",
            gnu,
        ),
        (
            "(let ((c (list 'when t 1))) (setcdr (nthcdr 2 c) (nthcdr 2 c))
               (condition-case e (macroexpand-1 c)
                 (error (list (car e) (eq (cadr e) (nthcdr 2 c))))))",
            gnu,
        ),
    ]);
}
