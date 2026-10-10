//! GNU parity of the backtracker's fail stack (P3.3 section 12, D5-D7): the
//! cycle check of a non-greedy loop over a nullable body, and the "Stack
//! overflow in regexp matcher" that the Pike fallback used to mask.
//!
//! Every expected value below is GNU 31.1's `(string-match RE S)` followed by
//! `(match-data t)` (probe: `tmp/probes/nasty.el` in the U0.5 worktree).

use super::*;

/// GNU `(and (string-match PATTERN TEXT) (match-data t))` over ASCII text,
/// plus whether the search stopped on the fail-stack limit.
fn string_match(pattern: &str, text: &str) -> (Option<Vec<i64>>, bool) {
    let compiled = regex_compile(pattern, false, false).expect("pattern compiles");
    let syntax = DefaultSyntaxLookup;
    let _ = take_matcher_overflow();
    let found = re_search(
        &compiled,
        text.as_bytes(),
        0,
        text.len() as isize,
        &syntax,
        0,
    );
    let overflow = take_matcher_overflow();
    (found.map(|(_, registers)| match_data(&registers)), overflow)
}

/// Registers as GNU's `match-data` lists them: trailing unset groups dropped.
fn match_data(registers: &MatchRegisters) -> Vec<i64> {
    let mut data: Vec<i64> = registers
        .start
        .iter()
        .zip(&registers.end)
        .flat_map(|(&start, &end)| [start, end])
        .collect();
    while data.len() > 2 && data[data.len() - 1] < 0 && data[data.len() - 2] < 0 {
        data.truncate(data.len() - 2);
    }
    data
}

#[test]
fn nongreedy_loops_over_nullable_bodies_match_like_gnu() {
    crate::test_utils::init_test_tracing();
    let cases: &[(&str, &str, Option<&[i64]>)] = &[
        // D5: GNU answers at once; the loop without GNU's marker frames
        // re-entered the empty iteration forever.
        ("\\(?:a?\\)*?b", "aaaac", None),
        ("\\(?:a?\\)*?b", "c", None),
        ("\\(?:a?\\)*?b", "", None),
        ("\\(?:a?\\)*?b", "aaab", Some(&[0, 4])),
        // D6: each empty iteration left a register save behind, so the
        // search ended in a spurious fail-stack overflow.
        ("\\(a\\|\\)+?x", "aaaay", None),
        ("\\(a\\|\\)+?x", "aaaax", Some(&[0, 5, 3, 4])),
        ("\\(a\\|\\)+?x", "x", Some(&[0, 1, 0, 0])),
        // regex-emacs.c: "We want (x?)*?y\1z to match both xxyz and xxyxz."
        ("\\(x?\\)*?y\\1z", "xxyz", Some(&[0, 4, 2, 2])),
        ("\\(x?\\)*?y\\1z", "xxyxz", Some(&[0, 5, 1, 2])),
        ("\\(?:a\\|\\)*?c", "aab", None),
        ("\\(\\(?:ab\\)?\\)*?\\(?:ab\\)c", "ababab", None),
        (
            "\\(\\(?:ab\\)?\\)*?\\(?:ab\\)c",
            "abababc",
            Some(&[0, 7, 2, 4]),
        ),
        ("\\(?:\\(a\\)?\\|b\\)+?\\'", "abba", Some(&[0, 4, 3, 4])),
        ("\\(?:\\(a\\)?\\|b\\)+?\\'", "abbac", Some(&[5, 5])),
        ("x\\(?:a*?\\)*?y", "xaaaz", None),
        ("x\\(?:a*?\\)*?y", "xaay", Some(&[0, 4])),
        ("\\(?:a?b?\\)*?c", "ababx", None),
    ];
    for &(pattern, text, expected) in cases {
        let compiled = regex_compile(pattern, false, false).expect("pattern compiles");
        assert!(
            compiled
                .buffer
                .contains(&(RegexOp::OnFailureJumpNastyloop as u8)),
            "{pattern:?} must exercise on_failure_jump_nastyloop"
        );
        let (found, overflow) = string_match(pattern, text);
        assert!(!overflow, "{pattern:?} on {text:?}: no fail-stack overflow");
        assert_eq!(
            found.as_deref(),
            expected,
            "{pattern:?} on {text:?}: GNU's match data"
        );
    }
}

/// A long text: GNU answers `nil` and `(0 1001)` (its time grows with the
/// square of the length, as each candidate walks the rest of the text).
#[test]
fn nongreedy_loop_over_nullable_body_walks_a_long_text_like_gnu() {
    let text = format!("{}c", "a".repeat(1_000));
    let (found, overflow) = string_match("\\(?:a?\\)*?b", &text);
    assert_eq!(found, None);
    assert!(!overflow);
    let (found, overflow) = string_match("\\(?:a?\\)*?c", &text);
    assert_eq!(found, Some(vec![0, 1_001]));
    assert!(!overflow);
}

/// D7: a candidate the budgeted backtracker hands to the Pike VM still ends
/// in GNU's "Stack overflow in regexp matcher" when GNU's backtracker would
/// run out of fail stack there, and in the Pike VM's answer otherwise.
#[test]
fn pike_fallback_keeps_gnu_fail_stack_overflow() {
    crate::test_utils::init_test_tracing();
    let pairs = |n: usize| "ab".repeat(n);

    // One candidate (only `x` starts a match): 200K characters overflow in GNU.
    let (found, overflow) = string_match("x\\(?:a\\|b\\)*c", &format!("x{}", pairs(100_000)));
    assert_eq!(found, None);
    assert!(overflow, "GNU signals the fail-stack overflow");

    // The 20K-character twin stays under GNU's limit: a plain failure.
    let (found, overflow) = string_match("x\\(?:a\\|b\\)*c", &format!("x{}", pairs(10_000)));
    assert_eq!(found, None);
    assert!(!overflow);

    // The design's D7 case: GNU stops at the first candidate.
    let (found, overflow) = string_match("\\(?:a\\|b\\)*c", &pairs(150_000));
    assert_eq!(found, None);
    assert!(overflow, "GNU signals the fail-stack overflow");

    // A long candidate that matches before the stack fills keeps the Pike
    // VM's (linear, exact) answer.
    let text = format!("x{}c", pairs(3_000));
    let (found, overflow) = string_match("x\\(?:a\\|b\\)*c", &text);
    assert_eq!(found, Some(vec![0, text.len() as i64]));
    assert!(!overflow);
}

#[test]
fn nullable_nongreedy_loop_answers_nil_from_lisp() {
    let mut ev = crate::emacs_core::eval::Context::new();
    let caught = ev
        .eval_str(
            "(list (condition-case err (string-match \"\\\\(a\\\\|\\\\)+?x\" \"aaaay\") \
                     (error err)) \
                   (string-match \"\\\\(?:a?\\\\)*?b\" \"aaaac\"))",
        )
        .expect("condition-case evaluates");
    assert_eq!(crate::emacs_core::print::print_value(&caught), "(nil nil)");
}

#[test]
fn pike_fallback_overflow_signals_the_gnu_error_from_lisp() {
    let mut ev = crate::emacs_core::eval::Context::new();
    let caught = ev
        .eval_str(
            "(condition-case err \
                 (string-match \"x\\\\(?:a\\\\|b\\\\)*c\" \
                               (concat \"x\" (apply #'concat (make-list 100000 \"ab\")))) \
               (error err))",
        )
        .expect("condition-case evaluates");
    assert_eq!(
        crate::emacs_core::print::print_value(&caught),
        "(error \"Stack overflow in regexp matcher\")"
    );
}

/// The bound behind the D7 fix: a backtracker path's fail stack never holds
/// more than `2 * push sites` entries per position it pushed at.  Measured
/// over random patterns with nested and nullable loops, including the
/// patterns the Pike VM cannot run (POSIX patterns are the C4 test's).
#[test]
fn fail_stack_depth_stays_within_the_overflow_bound() {
    crate::test_utils::init_test_tracing();
    let syntax = DefaultSyntaxLookup;
    let mut rng = BoundRng(0x5eed_f00d);
    let mut measured = 0usize;
    let mut worst_ratio = 0.0f64;
    for case in 0..20_000 {
        let pattern = bound_pattern(&mut rng, 2);
        let Ok(compiled) = regex_compile(&pattern, false, false) else {
            continue;
        };
        if compiled.buffer.iter().any(|&b| {
            matches!(
                RegexOp::from_byte(b),
                Some(RegexOp::SucceedN | RegexOp::JumpN | RegexOp::SetNumberAt)
            )
        }) {
            continue;
        }
        let text = bound_text(&mut rng, 24);
        let sites = fail_stack_push_sites(&compiled.buffer);
        for pos in 0..=text.len() {
            let _ = take_fail_stack_probe();
            let _ = take_matcher_overflow();
            // The backtrack budget caps the exponential shapes (it only
            // stops the run early; the depth bound holds at every step).
            let mut scratch = MatchScratch::default();
            let mut registers = MatchRegisters::default();
            let _ = re_match_internal(
                &mut scratch,
                &compiled,
                &text,
                pos,
                text.len(),
                &syntax,
                pos,
                true,
                &mut registers,
            );
            let _ = take_pike_fallback();
            assert!(!take_matcher_overflow());
            let probe = take_fail_stack_probe();
            if probe.max_depth == 0 {
                continue;
            }
            measured += 1;
            let consumed = probe.max_push_pos - pos;
            let bound = (consumed + 1) * 2 * sites;
            worst_ratio = worst_ratio.max(probe.max_depth as f64 / ((consumed + 1) * sites) as f64);
            assert!(
                probe.max_depth <= bound,
                "case {case}: {pattern:?} at {pos} in {text:?}: depth {} > bound {bound} \
                 (consumed {consumed}, sites {sites})",
                probe.max_depth
            );
        }
    }
    tracing::info!(
        measured,
        worst_ratio,
        "fail-stack depth per push site and position"
    );
    assert!(measured > 50_000, "too few measured runs: {measured}");
}

struct BoundRng(u64);

impl BoundRng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Random patterns over `a`/`b` with groups, alternations, empty arms and
/// every quantifier nested freely, so nullable loops inside loops appear.
fn bound_pattern(rng: &mut BoundRng, depth: usize) -> String {
    let mut out = String::new();
    let arms = 1 + rng.below(3);
    for arm in 0..arms {
        if arm > 0 {
            out.push_str("\\|");
        }
        for _ in 0..rng.below(4) {
            let atom = match rng.below(if depth == 0 { 4 } else { 7 }) {
                0 => "a".to_string(),
                1 => "b".to_string(),
                2 => ".".to_string(),
                3 => "\\b".to_string(),
                4 => format!("\\(?:{}\\)", bound_pattern(rng, depth - 1)),
                5 => format!("\\({}\\)", bound_pattern(rng, depth - 1)),
                _ => format!("[ab]{}", ["", "*"][rng.below(2)]),
            };
            out.push_str(&atom);
            out.push_str(["", "", "*", "+", "?", "*?", "+?", "??"][rng.below(8)]);
        }
    }
    out
}

fn bound_text(rng: &mut BoundRng, max_len: usize) -> Vec<u8> {
    (0..rng.below(max_len))
        .map(|_| b"aab c"[rng.below(5)])
        .collect()
}

/// Exercise the native cached evaluator with an explicitly selected owned
/// measurement form.  Ignored in ordinary semantic test selections.
#[test]
#[ignore = "private matched optimized measurement, not a semantic acceptance test"]
fn r26_native_matched_measurement() {
    let path = std::env::var("NEOMACS_REGEX_MEASURE_FORM").expect("explicit owned form");
    let form = std::fs::read_to_string(path).expect("read owned form");
    let observed = crate::test_utils::runtime_startup_eval_one(&form);
    println!("R26-MEASURE {observed}");
    assert!(observed.starts_with("OK ("), "{observed}");
}

/// #74: one-character greedy loops whose exit is the pattern's final
/// non-POSIX `succeed`, with nothing in between.  GNU proves them safe in
/// `mutually_exclusive_one`'s `case succeed` (regex-emacs.c:3938-3951,
/// `unconstrained` still true) and rewrites them to one keep-string failure
/// point for the whole loop (regex-emacs.c:4896-4906).
const UNCONSTRAINED_TERMINAL_LOOPS: &[&str] = &[
    "[ \t\n\r]+",
    " +",
    "x* *",
    "[ ]+",
    "[^a]+",
    ".+",
    "[ \t]*",
    "[ A]+",
];

/// Structural: the loop right before the final `Succeed` is GNU's
/// keep-string shape `OFKSJ exit; P; Jump P; exit: Succeed`, not the
/// per-character backtracking loop.
#[test]
fn unconstrained_terminal_loops_resolve_to_one_keep_string_frame_like_gnu() {
    for &pattern in UNCONSTRAINED_TERMINAL_LOOPS {
        for case_fold in [false, true] {
            let compiled = regex_compile(pattern, false, case_fold).expect("pattern compiles");
            let bc = &compiled.buffer;
            let n = bc.len();
            assert_eq!(bc[n - 1], RegexOp::Succeed as u8, "{pattern:?}");
            assert!(
                !bc.contains(&(RegexOp::OnFailureJumpSmart as u8)),
                "{pattern:?}: no unresolved smart jump"
            );
            let jump_at = n - 4;
            assert_eq!(
                bc[jump_at],
                RegexOp::Jump as u8,
                "{pattern:?} (fold {case_fold})"
            );
            let body = (jump_at as i64 + 3 + extract_number(bc, jump_at + 1) as i64) as usize;
            let ofksj = body - 3;
            assert_eq!(
                bc[ofksj],
                RegexOp::OnFailureKeepStringJump as u8,
                "{pattern:?} (fold {case_fold}): the jump back targets the body of a keep-string loop"
            );
            let exit = (ofksj as i64 + 3 + extract_number(bc, ofksj + 1) as i64) as usize;
            assert_eq!(
                exit,
                n - 1,
                "{pattern:?}: the loop exits straight to Succeed"
            );
        }
    }
}

/// Instrumented: a long run holds one failure point for the whole loop, as
/// in GNU, instead of one per character.
#[test]
fn unconstrained_terminal_loops_hold_one_fail_frame_per_run() {
    let syntax = DefaultSyntaxLookup;
    let text = vec![b' '; 1_000];
    for &pattern in UNCONSTRAINED_TERMINAL_LOOPS {
        let compiled = regex_compile(pattern, false, false).expect("pattern compiles");
        let _ = take_fail_stack_probe();
        let _ = take_matcher_overflow();
        let mut scratch = MatchScratch::default();
        let mut registers = MatchRegisters::default();
        let end = re_match_internal(
            &mut scratch,
            &compiled,
            &text,
            0,
            text.len(),
            &syntax,
            0,
            false,
            &mut registers,
        );
        assert_eq!(end, Some(text.len()), "{pattern:?}: greedy run to the end");
        assert!(!take_matcher_overflow());
        let probe = take_fail_stack_probe();
        assert!(
            probe.max_depth <= 2,
            "{pattern:?}: fail stack reached {} entries over a {}-char run",
            probe.max_depth,
            text.len()
        );
    }
}

/// GNU 32.0.50 (commit b28750b822dc45ffdceda3ea44a3c8b93f4e5a6b)
/// `(string-match RE S)` + `(match-data t)` over 400,000 characters, past the fail-stack limit for a per-character loop: GNU
/// answers `(0 400000)` for every unconstrained terminal loop.
#[test]
fn unconstrained_terminal_loops_match_long_runs_like_gnu() {
    let spaces = " ".repeat(400_000);
    let mixed = " \t\n\r".repeat(100_000);
    for (pattern, text) in [
        ("[ \t\n\r]+", &mixed),
        (" +", &spaces),
        ("x* *", &spaces),
        ("[ ]+", &spaces),
        ("[^a]+", &spaces),
        (".+", &spaces),
    ] {
        let (found, overflow) = string_match(pattern, text);
        assert!(!overflow, "{pattern:?}: GNU does not overflow");
        assert_eq!(found, Some(vec![0, 400_000]), "{pattern:?}");
    }
}

/// A pending quit must still interrupt the keep-string loop, without becoming
/// an overflow or consuming the request.  Clearing it permits the full match.
#[test]
fn terminal_success_refinement_preserves_quit_polling() {
    let text = vec![b' '; 400_000];
    for &pattern in UNCONSTRAINED_TERMINAL_LOOPS {
        let compiled = regex_compile(pattern, false, false).expect("pattern compiles");
        let mut scratch = MatchScratch::default();
        let mut registers = MatchRegisters::default();
        let _ = take_matcher_overflow();
        let flag = crate::emacs_core::eval::install_quit_requested_for_test(true);
        let interrupted = re_match_internal(
            &mut scratch,
            &compiled,
            &text,
            0,
            text.len(),
            &DefaultSyntaxLookup,
            0,
            false,
            &mut registers,
        );
        let pending = crate::emacs_core::eval::tls_quit_pending();
        crate::emacs_core::eval::clear_quit_requested_for_test();
        drop(flag);
        assert_eq!(interrupted, None, "{pattern:?}: quit must interrupt");
        assert!(pending, "{pattern:?}: the matcher must not consume quit");
        assert!(
            !take_matcher_overflow(),
            "{pattern:?}: quit is not overflow"
        );
        let completed = re_match_internal(
            &mut scratch,
            &compiled,
            &text,
            0,
            text.len(),
            &DefaultSyntaxLookup,
            0,
            false,
            &mut registers,
        );
        assert_eq!(completed, Some(text.len()), "{pattern:?}: clear then retry");
        assert_eq!(match_data(&registers), vec![0, 400_000], "{pattern:?}");
        assert!(!take_matcher_overflow());
    }
}

/// Negative controls that keep the per-character loop: an assertion
/// between the loop and `succeed` clears GNU's `unconstrained`
/// (RETURN_CONSTRAIN for `wordbound`), and POSIX patterns stay out of this
/// rewrite by scope.  The POSIX exclusion follows the cited GNU source
/// (45b0f7699d, `forall_firstchar` returns false at the pattern end,
/// regex-emacs.c:2847-2853); it is not a parity claim against the 32.0.50
/// oracle, whose POSIX first-character walk differs.
#[test]
fn constrained_or_posix_terminal_loops_keep_the_backtracking_loop() {
    for (pattern, posix) in [("[ ]+\\b", false), ("[ ]+\\B", false), ("[ ]+", true)] {
        let compiled = regex_compile(pattern, posix, false).expect("pattern compiles");
        assert!(
            !compiled
                .buffer
                .contains(&(RegexOp::OnFailureKeepStringJump as u8)),
            "{pattern:?} (posix {posix}) must stay a backtracking loop"
        );
    }
    // GNU: (error "Stack overflow in regexp matcher").
    let (found, overflow) = string_match("[ ]+\\b", &" ".repeat(400_000));
    assert_eq!(found, None);
    assert!(overflow, "GNU signals the fail-stack overflow");
}

/// The same answers through the Lisp front end, against GNU 32.0.50
/// (commit b28750b822dc45ffdceda3ea44a3c8b93f4e5a6b) running this exact form (`-Q --batch`): folding, custom case tables, unibyte
/// high bytes, multibyte, START, buffer BOUND, `looking-at`, and
/// `string-match-p` leaving the match data alone.
#[test]
fn unconstrained_terminal_loops_answer_like_gnu_from_lisp() {
    let observed = crate::test_utils::runtime_startup_eval_one(
        r##"(progn
  (require 'case-table)
  (let ((sp (make-string 400000 ?\s))
        (ws (apply #'concat (make-list 100000 " \t\n\r")))
        (case-fold-search nil)
        (probe (lambda (re s fold)
                 (let ((case-fold-search fold))
                   (condition-case err (and (string-match re s) (match-data t))
                     (error err)))))
        out)
    (push (funcall probe "[ \t\n\r]+" ws nil) out)
    (push (funcall probe " +" sp nil) out)
    (push (funcall probe "x* *" sp nil) out)
    (push (funcall probe "[ ]+" sp nil) out)
    (push (funcall probe "[^a]+" sp nil) out)
    (push (funcall probe ".+" sp nil) out)
    (push (funcall probe "[ A]+" sp t) out)
    (push (funcall probe "[ ]+\\b" sp nil) out)
    (push (funcall probe "[ \t\n\r]+" "ab \t\ncd" nil) out)
    (push (funcall probe "[ \t\n\r]+" "a  b   c" nil) out)
    (push (funcall probe "[ \t\n\r]+" "abc" nil) out)
    (push (funcall probe "[ \t\n\r]+" "" nil) out)
    (push (funcall probe "[ \t]*" "abc" nil) out)
    (push (funcall probe "[a-c]+" "xxABCabcD" t) out)
    (push (funcall probe "[a-c]+" "xxABCabcD" nil) out)
    (push (funcall probe "[\200-\377]+" (string-to-unibyte "a\200\377\201b") nil) out)
    (push (funcall probe "[αβ]+" "xαββαy" nil) out)
    (push (and (string-match "[ ]+" "  a   b" 3) (match-data t)) out)
    (with-temp-buffer
      (let ((tbl (copy-case-table (standard-case-table))))
        (set-case-syntax-pair ?! ?\s tbl)
        (set-case-table tbl)
        (push (funcall probe "[ ]+" "x! !y" t) out)
        (push (funcall probe "[ ]+" "x! !y" nil) out)
        (push (funcall probe "[ ]+" (apply #'concat (make-list 200000 "! ")) t) out)))
    (with-temp-buffer
      (insert "a     b")
      (goto-char 1)
      (push (list (re-search-forward "[ ]+" 4 t) (point) (match-beginning 0) (match-end 0))
            out))
    (with-temp-buffer
      (insert sp)
      (goto-char 1)
      (push (condition-case err (list (looking-at "[ ]+") (match-end 0)) (error err)) out))
    (set-match-data '(7 9))
    (push (condition-case err (list (string-match-p "[ ]+" sp) (match-data t))
            (error err))
          out)
    (nreverse out)))"##,
    );
    assert_eq!(
        observed,
        concat!(
            "OK ((0 400000) (0 400000) (0 400000) (0 400000) (0 400000) (0 400000) (0 400000) ",
            "(error \"Stack overflow in regexp matcher\") (2 5) (1 3) nil nil (0 0) (2 8) (5 8) ",
            "(1 4) (1 5) (3 6) (1 4) (2 3) (0 400000) (4 4 2 4) (t 400001) (0 (7 9)))"
        )
    );
}
