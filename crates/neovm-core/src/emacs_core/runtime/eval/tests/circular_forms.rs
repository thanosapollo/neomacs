//! Evaluating a form whose argument list is circular signals `circular-list`
//! like GNU instead of looping.  GNU `eval_sub` runs `list_length` over every
//! call form's arguments (`src/eval.c`), `let` measures its varlist the same
//! way, `let*` walks it with `FOR_EACH_TAIL`, and `macroexpand` hands the
//! arguments to `apply`, whose `list_length` signals.  `circular-list`'s
//! datum is the cons where the walk met its tortoise.
//!
//! Each case evaluates to `(CONDITION DATUM-IS-EXPECTED-CONS ...)`; the
//! expected strings are GNU's output for the same forms on the emacs-31.1
//! `FOR_EACH_TAIL` Brent schedule (checked with GNU 30.2, whose lisp.h walk
//! and `let`/`let*` are unchanged in 31.1).  The cases run on a worker
//! thread so a regression fails here instead of hanging.

use crate::emacs_core::eval::{TierIEvent, TierIMode};
use crate::emacs_core::print::print_value;
use std::sync::mpsc;
use std::time::Duration;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(500);
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

const PRELUDE: &str = r#"
(defvar fix15-n 0)
(defalias 'fix15-f (lambda (&rest x) x))
(defalias 'fix15-g (make-byte-code 128 "\300\207" [nil] 1))
(defvar fix15-c nil)
(defvar fix15-form nil)
(defvar fix15-gc nil)
(defvar fix15-datum nil)
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
            "(circular-list t 2)",
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

/// Evaluate each `(SOURCE EXPECTED)` case as [`assert_cases`] does, once
/// under the tree walker and once each under Tier-I in `on` and `verify`
/// mode at threshold 1 (every body compiled at its first call), and assert
/// every printed result.  A cons a case leaves in `fix15-datum` must still
/// be an allocated cons: the printed result gets ` live` or ` FREED`.  In
/// the Tier-I modes compiled code must have run.
fn assert_tier_i_cases(cases: &[(&'static str, &'static str)]) {
    const MODES: [TierIMode; 3] = [TierIMode::Off, TierIMode::On, TierIMode::Verify];
    crate::test_utils::init_test_tracing();
    let sources: Vec<&'static str> = cases.iter().map(|(src, _)| *src).collect();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("circular-forms-tier-i".into())
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
            for mode in MODES {
                eval.tier_i.set_mode(mode);
                eval.tier_i.set_threshold(1);
                eval.tier_i.clear_for_test();
                for src in &sources {
                    let mut printed = match eval.eval_str(src) {
                        Ok(value) => print_value(&value),
                        Err(err) => format!("ERR {err:?}"),
                    };
                    let datum = eval.obarray.symbol_value("fix15-datum").copied();
                    if let Some(datum) = datum.filter(|datum| datum.is_cons()) {
                        let live = eval.tagged_heap.owns_heap_value_for_test(datum);
                        printed.push_str(if live { " live" } else { " FREED" });
                    }
                    let _ = eval.eval_str("(setq fix15-datum nil fix15-c nil fix15-form nil)");
                    if tx.send(printed).is_err() {
                        return;
                    }
                }
                let runs = eval.tier_i.stats().count(TierIEvent::Run);
                if tx.send(runs.to_string()).is_err() {
                    return;
                }
            }
        })
        .expect("spawn circular-forms-tier-i thread");
    let prelude = rx
        .recv_timeout(STARTUP_TIMEOUT)
        .expect("runtime startup and prelude should finish");
    assert_eq!(prelude, "");
    let mut mismatches = Vec::new();
    for mode in MODES {
        for (src, expected) in cases {
            let printed = rx.recv_timeout(CASE_TIMEOUT).unwrap_or_else(|_| {
                panic!("{mode:?}: did not finish within {CASE_TIMEOUT:?}: {src}")
            });
            if printed != *expected {
                mismatches.push(format!(
                    "{mode:?}: {src}\n  left: {printed}\n right: {expected}"
                ));
            }
        }
        let runs = rx.recv_timeout(CASE_TIMEOUT).expect("Tier-I run count");
        if mode != TierIMode::Off && runs == "0" {
            mismatches.push(format!("{mode:?}: no compiled body ran"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// GNU `Flet_star` walks the live VARLIST with `FOR_EACH_TAIL`, reading
/// each cdr after evaluating and binding the element before it: an init
/// form that breaks a cycle lets `let*' return, one that makes the varlist
/// circular or improper signals.  `CHECK_LIST_END (varlist, XCAR (args))`
/// reports an improper varlist itself, not its final cdr, even when it was
/// improper from the start.  The `fix15-ls' cases run the `let*' as a
/// function body, which Tier-I compiles while the varlist is still proper
/// and then runs over the mutated one.
#[test]
fn circular_form_let_star_walks_the_live_varlist() {
    let mut cases: Vec<(&'static str, &'static str)> = Vec::new();
    for (src, gnu) in [
        (
            "(progn
               (setq fix15-c (list (list 'a '(progn (setcdr fix15-c nil) 7))))
               (setcdr fix15-c fix15-c)
               (condition-case e (eval (list 'let* fix15-c 'a) LEX)
                 (error (list (car e) (eq (cadr e) fix15-c)))))",
            "7",
        ),
        (
            "(progn
               (setq fix15-c (list (list 'a '(progn (setcdr fix15-c fix15-c) 7))))
               (condition-case e (eval (list 'let* fix15-c 'a) LEX)
                 (error (list (car e) (eq (cadr e) fix15-c)))))",
            "(circular-list t)",
        ),
        (
            "(progn
               (setq fix15-c (list (list 'a '(progn (setcdr fix15-c 5) 7))))
               (condition-case e (eval (list 'let* fix15-c 'a) LEX)
                 (error (list (car e) (cadr e) (eq (nth 2 e) fix15-c)))))",
            "(wrong-type-argument listp t)",
        ),
        (
            "(progn
               (setq fix15-c (list (list 'a '(progn (setcdr (cdr fix15-c) 5) 7)) '(b 2)))
               (condition-case e (eval (list 'let* fix15-c 'a) LEX)
                 (error (list (car e) (cadr e) (eq (nth 2 e) fix15-c)))))",
            "(wrong-type-argument listp t)",
        ),
        (
            "(progn
               (setq fix15-c (cons '(a 1) 5))
               (condition-case e (eval (list 'let* fix15-c 'a) LEX)
                 (error (list (car e) (cadr e) (eq (nth 2 e) fix15-c)))))",
            "(wrong-type-argument listp t)",
        ),
    ] {
        for lex in ["nil", "t"] {
            let src: &'static str = src.replace("LEX", lex).leak();
            cases.push((src, gnu));
        }
    }
    cases.extend([
        (
            "(progn
               (setq fix15-gc nil)
               (setq fix15-c
                     (list (list 'a '(progn (when fix15-gc (setcdr fix15-c nil)) 7))))
               (defalias 'fix15-ls1
                 (eval (list 'function (list 'lambda nil (list 'let* fix15-c 'a))) t))
               (list (fix15-ls1)
                     (progn (setcdr fix15-c fix15-c) (setq fix15-gc t)
                            (condition-case e (fix15-ls1)
                              (error (list (car e) (eq (cadr e) fix15-c)))))))",
            "(7 7)",
        ),
        (
            "(progn
               (setq fix15-gc nil)
               (setq fix15-c
                     (list (list 'a '(progn (when fix15-gc (setcdr fix15-c fix15-c)) 7))))
               (defalias 'fix15-ls2
                 (eval (list 'function (list 'lambda nil (list 'let* fix15-c 'a))) t))
               (list (fix15-ls2)
                     (progn (setq fix15-gc t)
                            (condition-case e (fix15-ls2)
                              (error (list (car e) (eq (cadr e) fix15-c)))))))",
            "(7 (circular-list t))",
        ),
        (
            "(progn
               (setq fix15-gc nil)
               (setq fix15-c
                     (list (list 'a '(progn (when fix15-gc (setcdr fix15-c 5)) 7))))
               (defalias 'fix15-ls4
                 (eval (list 'function (list 'lambda nil (list 'let* fix15-c 'a))) t))
               (list (fix15-ls4)
                     (progn (setq fix15-gc t)
                            (condition-case e (fix15-ls4)
                              (error (list (car e) (cadr e) (eq (nth 2 e) fix15-c)))))))",
            "(7 (wrong-type-argument listp t))",
        ),
    ]);
    assert_tier_i_cases(&cases);
}

/// Unlike `let*`'s `CHECK_LIST_END (varlist, XCAR (args))`, every other
/// form reaches GNU's `list_length`, whose `CHECK_LIST_END (list, list)`
/// reports the final cdr itself, not the list it ends.
#[test]
fn improper_form_args_report_their_final_cdr() {
    let gnu = "(wrong-type-argument listp t)";
    let mut cases: Vec<(&'static str, &'static str)> = Vec::new();
    for form in [
        "(cons 'progn (cons 1 fix15-datum))",
        "(cons 'cond (cons '(nil 1) fix15-datum))",
        "(cons 'setq (cons 'fix15-n (cons 1 fix15-datum)))",
        "(list 'let (cons '(a 1) fix15-datum) 'a)",
        "(cons 'lambda (cons nil (cons 1 fix15-datum)))",
        "(cons 'list (cons 1 fix15-datum))",
        "(cons 'when (cons t (cons 1 fix15-datum)))",
    ] {
        for lex in ["nil", "t"] {
            let src: &'static str = format!(
                "(progn (setq fix15-datum (copy-sequence \"s\"))
                   (condition-case e (eval {form} {lex})
                     (error (list (car e) (cadr e) (eq (nth 2 e) fix15-datum)))))"
            )
            .leak();
            cases.push((src, gnu));
        }
    }
    for expander in ["macroexpand", "macroexpand-1"] {
        let src: &'static str = format!(
            "(progn (setq fix15-datum (copy-sequence \"s\"))
               (condition-case e ({expander} (cons 'when (cons t (cons 1 fix15-datum))))
                 (error (list (car e) (cadr e) (eq (nth 2 e) fix15-datum)))))"
        )
        .leak();
        cases.push((src, gnu));
    }
    assert_cases(&cases);
}

/// GNU `Flet_star` keeps the cons it is walking, and `FOR_EACH_TAIL` its
/// tortoise, in C locals across each init form, so a garbage collection
/// inside an init that dropped every other reference to the varlist leaves
/// them alive and `circular-list` reports the very cons.
#[test]
fn circular_form_let_star_keeps_its_varlist_alive_across_gc() {
    let mut cases: Vec<(&'static str, &'static str)> = Vec::new();
    for lex in ["nil", "t"] {
        let src: &'static str = "(progn
               (setq fix15-c
                     (list (list 'a '(progn (setcar (cdr fix15-form) nil)
                                            (setq fix15-c nil)
                                            (garbage-collect) (garbage-collect) 7))))
               (setcdr fix15-c fix15-c)
               (setq fix15-form (list 'let* fix15-c 'a))
               (identity nil)
               (condition-case e (eval fix15-form LEX)
                 (error (setq fix15-datum (cadr e))
                        (list (car e) (consp (cadr e)) (caar (cadr e))))))"
            .replace("LEX", lex)
            .leak();
        cases.push((src, "(circular-list t a) live"));
    }
    cases.push((
        "(progn
           (setq fix15-gc nil)
           (setq fix15-c
                 (list (list 'a '(progn (when fix15-gc
                                          (setcar (cdr fix15-form) nil)
                                          (setq fix15-c nil fix15-form nil)
                                          (garbage-collect) (garbage-collect))
                                        7))))
           (setq fix15-form (list 'let* fix15-c 'a))
           (defalias 'fix15-ls3 (eval (list 'function (list 'lambda nil fix15-form)) t))
           (list (fix15-ls3)
                 (progn (setcdr fix15-c fix15-c) (setq fix15-gc t)
                        (condition-case e (fix15-ls3)
                          (error (setq fix15-datum (cadr e))
                                 (list (car e) (consp (cadr e)) (caar (cadr e))))))))",
        "(7 (circular-list t a)) live",
    ));
    assert_tier_i_cases(&cases);
}
