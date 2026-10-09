//! Tracker #68 item 3: special uppercase before a custom one-to-one mapping.
//! All four exact expressions have retained GNU 32.0.50 oracle output.
//! Native execution remains pending; source snapshots are not compiled RED/GREEN.

#[test]
fn sharp_s_special_uppercase_precedes_custom_pair() {
    crate::test_utils::init_test_tracing();
    let mut ev = crate::test_utils::runtime_startup_context();
    let result = ev
        .eval_str(
            r#"
(with-temp-buffer
  (let ((tbl (copy-case-table (standard-case-table))))
    (set-case-syntax-pair ?X ?ß tbl)
    (set-case-syntax-pair ?Q ?a tbl)
    (set-case-table tbl))
  (list (upcase ?ß) (upcase "ß") (upcase "aßa")
        (capitalize ?ß) (capitalize "ßa")
        (upcase-initials ?ß) (upcase-initials "ßa")
        (downcase ?X) (downcase "XQ")))
            "#,
        )
        .expect("custom sharp-s conversion");
    let values = crate::emacs_core::value::list_to_vec(&result).expect("result list");
    assert_eq!(values.len(), 9);
    // Character conversion remains one-to-one and uses the installed Up table.
    for i in [0, 3, 5] {
        assert_eq!(values[i].as_int(), Some(88));
    }
    for (i, expected) in [(1, "SS"), (2, "QSSQ"), (4, "Ssa"), (6, "Ssa")] {
        assert_eq!(values[i].as_utf8_str(), Some(expected));
    }
    assert_eq!(values[7].as_int(), Some(223));
    // ASCII input is unibyte: the ordinary Down mapping still fits one byte.
    let down = values[8].as_lisp_string().expect("downcased string");
    assert!(!down.is_multibyte());
    assert_eq!(down.as_bytes(), &[223, b'a']);
}

#[test]
fn special_uppercase_regions_and_words_keep_ordinary_mappings() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar
 (lambda (op)
   (with-temp-buffer
     (let ((tbl (copy-case-table (standard-case-table))))
       (set-case-syntax-pair ?X ?ß tbl)
       (set-case-syntax-pair ?Q ?a tbl)
       (set-case-table tbl))
     (insert "aßa aßa") (goto-char 1)
     (if (eq op 'upcase-word) (upcase-word 2)
       (upcase-region (point-min) (point-max)))
     (list (buffer-string) (point) (point-max))))
 '(upcase-region upcase-word))
        "#,
    );
    assert_eq!(result, r#"OK (("QSSQ QSSQ" 1 10) ("QSSQ QSSQ" 10 10))"#);
}

#[test]
fn allcaps_replace_match_special_uppercase_in_both_destinations() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar
 (lambda (fixed)
   (mapcar
    (lambda (target)
      (with-temp-buffer
        (let ((tbl (copy-case-table (standard-case-table))))
          (set-case-syntax-pair ?X ?ß tbl)
          (set-case-syntax-pair ?Q ?a tbl)
          (set-case-table tbl))
        (let ((source "pre ZZ post"))
          (if (eq target 'string)
              (progn (string-match "ZZ" source)
                     (replace-match "aßa" fixed t source))
            (insert source) (goto-char 1) (search-forward "ZZ")
            (replace-match "aßa" fixed t) (buffer-string)))))
    '(string buffer)))
 '(nil t))
        "#,
    );
    assert_eq!(
        result,
        r#"OK (("pre QSSQ post" "pre QSSQ post") ("pre aßa post" "pre aßa post"))"#
    );
}

#[test]
fn ordinary_custom_upcase_and_titlecase_remain_distinct() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(with-temp-buffer
  (let ((tbl (copy-case-table (standard-case-table))))
    (set-case-syntax-pair ?Q ?a tbl)
    (set-case-table tbl))
  (list (upcase ?a) (upcase "abc")
        (capitalize "abc") (upcase-initials "abc")
        (upcase (string-to-multibyte "abc"))
        (progn (insert "abc abc") (upcase-region 1 4) (goto-char 5)
               (upcase-word 1) (buffer-string))
        (progn (string-match "ZZ" "ZZ") (replace-match "abc" nil t "ZZ"))))
        "#,
    );
    assert_eq!(result, r#"OK (81 "QBC" "Abc" "Abc" "QBC" "QBC QBC" "QBC")"#);
}
