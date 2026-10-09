//! Tracker #68 residual: a resolved nil custom Up entry is identity.
//! Production-linked fixtures; source snapshots are not compiled RED/GREEN.
//! Install derived tables before mutating Up, avoiding GNU canon regeneration.
//! GNU 32.0.50 verified these five revised expressions after table installation.
//! The first pre-install nil setup failure is retained; native proof is pending.

#[test]
fn nil_custom_up_character_and_multibyte_string_are_identity() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(with-temp-buffer
  (let* ((tbl (copy-case-table (standard-case-table)))
         (up (char-table-extra-slot tbl 0)))
    (set-case-table tbl)
    (set-char-table-range up ?a nil)
    (set-char-table-range up ?é nil)
    (set-char-table-range up ?ß nil)
    (set-char-table-range up ?b ?Q)
    (set-char-table-range up ?ü ?Y))
  (let ((text (upcase "ébéßü")))
    (list (upcase ?a) (upcase ?é) (upcase ?ß)
          (upcase ?b) (upcase ?ü)
          (string-to-list text) (multibyte-string-p text)
          (string-to-list (upcase (string-to-multibyte "ab")))
          (capitalize ?é) (upcase-initials ?é)
          (string-to-list (capitalize "ébé"))
          (string-to-list (upcase-initials "ébé")))))
        "#,
    );
    assert_eq!(result, "OK (97 233 223 81 89 (233 81 233 83 83 89) t (97 81) 201 201 (201 98 233) (201 98 233))");
}

#[test]
fn nil_custom_up_regions_and_words_preserve_expansion_precedence() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar
 (lambda (op)
   (with-temp-buffer
     (let* ((tbl (copy-case-table (standard-case-table)))
            (up (char-table-extra-slot tbl 0)))
       (set-case-table tbl)
       (set-char-table-range up ?é nil)
       (set-char-table-range up ?ß nil)
       (set-char-table-range up ?b ?Q)
       (set-char-table-range up ?ü ?Y))
     (insert "ébéßü ébéßü") (goto-char 1)
     (if (eq op 'upcase-word) (upcase-word 2)
       (upcase-region (point-min) (point-max)))
     (list (string-to-list (buffer-string)) (point) (point-max))))
 '(upcase-region upcase-word))
        "#,
    );
    assert_eq!(result, "OK (((233 81 233 83 83 89 32 233 81 233 83 83 89) 1 14) ((233 81 233 83 83 89 32 233 81 233 83 83 89) 14 14))");
}

#[test]
fn nil_custom_up_replace_match_conversion_in_both_destinations() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar
 (lambda (fixed)
   (mapcar
    (lambda (target)
      (with-temp-buffer
        (let* ((tbl (copy-case-table (standard-case-table)))
               (up (char-table-extra-slot tbl 0)))
          (set-case-table tbl)
          (set-char-table-range up ?é nil)
          (set-char-table-range up ?ß nil)
          (set-char-table-range up ?b ?Q)
          (set-char-table-range up ?ü ?Y))
        (string-to-list
         (if (eq target 'string)
             (progn (string-match "ZZ" "ZZ")
                    (replace-match "ébéßü" fixed t "ZZ"))
           (insert "ZZ") (goto-char 1) (search-forward "ZZ")
           (replace-match "ébéßü" fixed t) (buffer-string)))))
    '(string buffer)))
 '(nil t))
        "#,
    );
    assert_eq!(result, "OK (((233 81 233 83 83 89) (233 81 233 83 83 89)) ((233 98 233 223 252) (233 98 233 223 252)))");
}

#[test]
fn nil_custom_up_replace_match_classification_is_not_unicode_lowercase() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar
 (lambda (mapping)
   (mapcar
    (lambda (target)
      (with-temp-buffer
        (let* ((tbl (copy-case-table (standard-case-table)))
               (up (char-table-extra-slot tbl 0)))
          (set-case-table tbl)
          (set-char-table-range up ?é mapping))
        (if (eq target 'string)
            (progn (string-match "AéZ" "AéZ")
                   (replace-match "cd" nil t "AéZ"))
          (insert "AéZ") (goto-char 1) (search-forward "AéZ")
          (replace-match "cd" nil t) (buffer-string))))
    '(string buffer)))
 '(nil 201))
        "#,
    );
    assert_eq!(result, "OK ((\"CD\" \"CD\") (\"Cd\" \"Cd\"))");
}

#[test]
fn nil_custom_up_resolves_default_and_parent_before_identity() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar
 (lambda (mode)
   (with-temp-buffer
     (let* ((tbl (copy-case-table (standard-case-table)))
            (up (char-table-extra-slot tbl 0)))
       (set-case-table tbl)
       (set-char-table-range up ?é nil)
       (cond
        ((eq mode 'explicit) (set-char-table-range up ?é ?Q))
        ((eq mode 'default) (set-char-table-range up nil ?Q))
        ((eq mode 'parent)
         (let ((parent (make-char-table 'case-table nil)))
           (set-char-table-range parent ?é ?Q)
           (set-char-table-parent up parent)))))
     (list (upcase ?é) (string-to-list (upcase "é")))))
 '(nil explicit default parent))
        "#,
    );
    assert_eq!(result, "OK ((233 (233)) (81 (81)) (81 (81)) (81 (81)))");
}

#[test]
fn custom_up_does_not_replace_simple_identity_titlecase() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar
 (lambda (code)
   (with-temp-buffer
     (let* ((tbl (copy-case-table (standard-case-table)))
            (up (char-table-extra-slot tbl 0)))
       (set-case-table tbl)
       (set-char-table-range up code ?Q))
     (list (upcase code) (capitalize code) (upcase-initials code))))
 '(97 233 453 223))
        "#,
    );
    assert_eq!(result, "OK ((81 65 65) (81 201 201) (81 453 453) (81 81 81))");
}
