//! Production evaluator regressions for signed casing extents and later edits.

const CASES: &[(&str, &str)] = &[("AB", ""), ("AİB", "xy"), ("İAB", "xy"), ("AİB", "éxy")];

fn expression(text: &str, expansion: &str, word: bool) -> String {
    let operation = if word {
        "(goto-char 1) (downcase-word 1)"
    } else {
        "(downcase-region 1 (point-max))"
    };
    format!(
        r##"(let* ((tbl (make-char-table 'char-code-property-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl)))) (set-char-table-range tbl ?A "") (set-char-table-range tbl ?İ "{expansion}") (with-temp-buffer (insert "{text}") {operation} (let ((before (list (buffer-string) (point) (point-max) (position-bytes (point)) (position-bytes (point-max))))) (insert "!") (list before (buffer-string) (point) (point-max) (position-bytes (point)) (position-bytes (point-max))))))"##,
    )
}

#[test]
fn empty_special_lowercase_region_contracts_anchors_before_insertion() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(&expression("AB", "", false));
    assert_eq!(result, r##"OK (("b" 2 2 2 2) "b!" 3 3 3 3)"##);
}

#[test]
fn empty_special_lowercase_word_contracts_anchors_before_insertion() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(&expression("AB", "", true));
    assert_eq!(result, r##"OK (("b" 2 2 2 2) "b!" 3 3 3 3)"##);
}

#[test]
fn mixed_special_lowercase_region_and_word_keep_signed_coordinate_pairs() {
    crate::test_utils::init_test_tracing();
    for word in [false, true] {
        for (text, expansion) in &CASES[1..] {
            let expected = match (*text, *expansion) {
                ("AİB", "xy") | ("İAB", "xy") => r##"OK (("xyb" 4 4 4 4) "xyb!" 5 5 5 5)"##,
                ("AİB", "éxy") => r##"OK (("éxyb" 5 5 6 6) "éxyb!" 6 6 7 7)"##,
                _ => unreachable!(),
            };
            let src = expression(text, expansion, word);
            assert_eq!(
                crate::test_utils::runtime_startup_eval_one(&src),
                expected,
                "{src}"
            );
        }
    }
}
