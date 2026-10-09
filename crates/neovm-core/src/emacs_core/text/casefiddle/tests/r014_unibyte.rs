//! Tracker #68 item 4: GNU Emacs 32.0.50 oracle fixtures.
//! Capture both character codes and internal bytes, with multibyteness.
//! Original casefiddle tests remain untouched; no compiled RED claimed.

#[test]
fn unibyte_strings_fall_back_only_above_byte_range() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar (lambda (mapping)
 (with-temp-buffer (let* ((tbl (copy-case-table (standard-case-table)))
                   (up (char-table-extra-slot tbl 0)))
              (aset up ?a mapping) (aset tbl ?A mapping)
              (set-case-table tbl))
   (mapcar (lambda (op) (funcall (lambda (s) (list (multibyte-string-p s) (string-to-list s) (string-to-list (string-as-unibyte s)))) (funcall op (string-as-unibyte "aA aA"))))
           '(upcase downcase capitalize upcase-initials)))) '(255 256 321))
        "#,
    );
    assert_eq!(
        result,
        r#"OK (((nil (255 65 32 255 65) (255 65 32 255 65)) (nil (97 255 32 97 255) (97 255 32 97 255)) (nil (65 255 32 65 255) (65 255 32 65 255)) (nil (65 65 32 65 65) (65 65 32 65 65))) ((nil (65 65 32 65 65) (65 65 32 65 65)) (nil (97 97 32 97 97) (97 97 32 97 97)) (nil (65 97 32 65 97) (65 97 32 65 97)) (nil (65 65 32 65 65) (65 65 32 65 65))) ((nil (65 65 32 65 65) (65 65 32 65 65)) (nil (97 97 32 97 97) (97 97 32 97 97)) (nil (65 97 32 65 97) (65 97 32 65 97)) (nil (65 65 32 65 65) (65 65 32 65 65))))"#
    );
}

#[test]
fn unibyte_regions_and_words_keep_byte_truncation() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar (lambda (mapping)
 (mapcar (lambda (op)
  (with-temp-buffer (set-buffer-multibyte nil) (let* ((tbl (copy-case-table (standard-case-table)))
                   (up (char-table-extra-slot tbl 0)))
              (aset up ?a mapping) (aset tbl ?A mapping)
              (set-case-table tbl))
   (insert "aA aA") (goto-char 1)
   (if (memq op '(upcase-word downcase-word capitalize-word)) (funcall op 2) (funcall op 1 6))
   (list (funcall (lambda (s) (list (multibyte-string-p s) (string-to-list s) (string-to-list (string-as-unibyte s)))) (buffer-string)) (point) (point-max))))
 '(upcase-region downcase-region capitalize-region upcase-initials-region upcase-word downcase-word capitalize-word))) '(255 256 321))
        "#,
    );
    assert_eq!(
        result,
        r#"OK ((((nil (255 65 32 255 65) (255 65 32 255 65)) 1 6) ((nil (97 255 32 97 255) (97 255 32 97 255)) 1 6) ((nil (65 255 32 65 255) (65 255 32 65 255)) 1 6) ((nil (65 65 32 65 65) (65 65 32 65 65)) 1 6) ((nil (255 65 32 255 65) (255 65 32 255 65)) 6 6) ((nil (97 255 32 97 255) (97 255 32 97 255)) 6 6) ((nil (65 255 32 65 255) (65 255 32 65 255)) 6 6)) (((nil (0 65 32 0 65) (0 65 32 0 65)) 1 6) ((nil (97 0 32 97 0) (97 0 32 97 0)) 1 6) ((nil (65 0 32 65 0) (65 0 32 65 0)) 1 6) ((nil (65 65 32 65 65) (65 65 32 65 65)) 1 6) ((nil (0 65 32 0 65) (0 65 32 0 65)) 6 6) ((nil (97 0 32 97 0) (97 0 32 97 0)) 6 6) ((nil (65 0 32 65 0) (65 0 32 65 0)) 6 6)) (((nil (65 65 32 65 65) (65 65 32 65 65)) 1 6) ((nil (97 65 32 97 65) (97 65 32 97 65)) 1 6) ((nil (65 65 32 65 65) (65 65 32 65 65)) 1 6) ((nil (65 65 32 65 65) (65 65 32 65 65)) 1 6) ((nil (65 65 32 65 65) (65 65 32 65 65)) 6 6) ((nil (97 65 32 97 65) (97 65 32 97 65)) 6 6) ((nil (65 65 32 65 65) (65 65 32 65 65)) 6 6)))"#
    );
}

#[test]
fn unibyte_replace_match_uses_destination_case_rules() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar (lambda (mapping)
 (mapcar (lambda (matched)
  (mapcar (lambda (fixed)
   (mapcar (lambda (target)
    (with-temp-buffer (set-buffer-multibyte nil) (let* ((tbl (copy-case-table (standard-case-table)))
                   (up (char-table-extra-slot tbl 0)))
              (aset up ?a mapping) (aset tbl ?A mapping)
              (set-case-table tbl))
     (let ((replacement (string-as-unibyte "aA aA")))
      (if (eq target 'string)
       (progn (string-match ".*" matched)
        (funcall (lambda (s) (list (multibyte-string-p s) (string-to-list s) (string-to-list (string-as-unibyte s)))) (replace-match replacement fixed t matched)))
       (insert matched) (goto-char 1) (re-search-forward ".*")
       (replace-match replacement fixed t)
       (funcall (lambda (s) (list (multibyte-string-p s) (string-to-list s) (string-to-list (string-as-unibyte s)))) (buffer-string)))))) '(string buffer))) '(nil t))) '("ZZ" "Zz"))) '(255 256))
        "#,
    );
    assert_eq!(
        result,
        r#"OK (((((nil (255 65 32 255 65) (255 65 32 255 65)) (nil (255 65 32 255 65) (255 65 32 255 65))) ((nil (97 65 32 97 65) (97 65 32 97 65)) (nil (97 65 32 97 65) (97 65 32 97 65)))) (((nil (65 65 32 65 65) (65 65 32 65 65)) (nil (65 65 32 65 65) (65 65 32 65 65))) ((nil (97 65 32 97 65) (97 65 32 97 65)) (nil (97 65 32 97 65) (97 65 32 97 65))))) ((((nil (65 65 32 65 65) (65 65 32 65 65)) (nil (0 65 32 0 65) (0 65 32 0 65))) ((nil (97 65 32 97 65) (97 65 32 97 65)) (nil (97 65 32 97 65) (97 65 32 97 65)))) (((nil (65 65 32 65 65) (65 65 32 65 65)) (nil (65 65 32 65 65) (65 65 32 65 65))) ((nil (97 65 32 97 65) (97 65 32 97 65)) (nil (97 65 32 97 65) (97 65 32 97 65))))))"#
    );
}

#[test]
fn unibyte_ordinary_and_nil_downcase_remain_distinct() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(mapcar (lambda (missing)
 (with-temp-buffer (set-buffer-multibyte nil)
  (let* ((tbl (copy-case-table (standard-case-table))) (up (char-table-extra-slot tbl 0)))
   (aset up ?a ?X) (aset tbl ?A (if missing nil ?x)) (set-case-table tbl))
  (list
   (mapcar (lambda (op) (funcall (lambda (s) (list (multibyte-string-p s) (string-to-list s) (string-to-list (string-as-unibyte s)))) (funcall op (string-as-unibyte "aA aA")))) '(upcase downcase capitalize upcase-initials))
   (progn (insert "aA aA") (downcase-region 1 3) (goto-char 4) (downcase-word 1)
    (list (funcall (lambda (s) (list (multibyte-string-p s) (string-to-list s) (string-to-list (string-as-unibyte s)))) (buffer-string)) (point)))))) '(nil t))
        "#,
    );
    assert_eq!(
        result,
        r#"OK ((((nil (88 65 32 88 65) (88 65 32 88 65)) (nil (97 120 32 97 120) (97 120 32 97 120)) (nil (65 120 32 65 120) (65 120 32 65 120)) (nil (65 65 32 65 65) (65 65 32 65 65))) ((nil (97 120 32 97 120) (97 120 32 97 120)) 6)) (((nil (88 65 32 88 65) (88 65 32 88 65)) (nil (97 65 32 97 65) (97 65 32 97 65)) (nil (65 65 32 65 65) (65 65 32 65 65)) (nil (65 65 32 65 65) (65 65 32 65 65))) ((nil (97 65 32 97 65) (97 65 32 97 65)) 6)))"#
    );
}

#[test]
fn multibyte_wide_cases_are_not_restricted_to_ascii() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(with-temp-buffer (let ((mapping 256)) (let* ((tbl (copy-case-table (standard-case-table)))
                   (up (char-table-extra-slot tbl 0)))
              (aset up ?a mapping) (aset tbl ?A mapping)
              (set-case-table tbl)))
 (list (upcase ?a) (downcase ?A)
  (mapcar (lambda (op) (funcall (lambda (s) (list (multibyte-string-p s) (string-to-list s) (string-to-list (string-as-unibyte s)))) (funcall op (string-to-multibyte "aA aA")))) '(upcase downcase capitalize upcase-initials))
  (progn (insert "aA aA") (upcase-region 1 3) (goto-char 4) (downcase-word 1)
   (list (funcall (lambda (s) (list (multibyte-string-p s) (string-to-list s) (string-to-list (string-as-unibyte s)))) (buffer-string)) (point) (point-max)))))
        "#,
    );
    assert_eq!(
        result,
        r#"OK (256 256 ((t (256 65 32 256 65) (196 128 65 32 196 128 65)) (t (97 256 32 97 256) (97 196 128 32 97 196 128)) (t (65 256 32 65 256) (65 196 128 32 65 196 128)) (t (65 65 32 65 65) (65 65 32 65 65))) ((t (256 65 32 97 256) (196 128 65 32 97 196 128)) 6 6))"#
    );
}

#[test]
fn unibyte_fallback_preserves_raw_bytes_and_string_properties() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
(with-temp-buffer (let ((mapping 256)) (let* ((tbl (copy-case-table (standard-case-table)))
                   (up (char-table-extra-slot tbl 0)))
              (aset up ?a mapping) (aset tbl ?A mapping)
              (set-case-table tbl)))
 (let ((s (unibyte-string 97 255 65 0)))
  (put-text-property 0 4 'face 'bold s)
  (list
   (mapcar (lambda (op)
     (let ((out (funcall op s)))
      (list (funcall (lambda (s) (list (multibyte-string-p s) (string-to-list s) (string-to-list (string-as-unibyte s)))) out) (get-text-property 0 'face out) (get-text-property 3 'face out)))) '(upcase downcase))
   (funcall (lambda (s) (list (multibyte-string-p s) (string-to-list s) (string-to-list (string-as-unibyte s)))) s) (get-text-property 0 'face s))))
        "#,
    );
    assert_eq!(
        result,
        r#"OK ((((nil (65 255 65 0) (65 255 65 0)) bold bold) ((nil (97 255 97 0) (97 255 97 0)) bold bold)) (nil (97 255 65 0) (97 255 65 0)) bold)"#
    );
}
