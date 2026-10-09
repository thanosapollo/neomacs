//! An accepted raw special-lowercase string is a GNU change even when its
//! bytes equal the original character; after-change covers only the changed
//! span. Ordinary simple identity casing still signals no after-change.

#[test]
fn raw_identity_region_after_change() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r##"(let* ((tbl (make-char-table 'char-code-property-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl))) (calls nil)) (set-char-table-range tbl ?a "a") (with-temp-buffer (insert "ab") (goto-char 1) (let ((after-change-functions (list (lambda (beg end old) (push (list beg end old) calls))))) (downcase-region 1 3)) (list (buffer-string) calls (point))))"##,
    );
    assert_eq!(result, r##"OK ("ab" ((1 2 1)) 1)"##);
}

#[test]
fn raw_identity_word_after_change() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r##"(let* ((tbl (make-char-table 'char-code-property-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl))) (calls nil)) (set-char-table-range tbl ?a "a") (with-temp-buffer (insert "ab") (goto-char 1) (let ((after-change-functions (list (lambda (beg end old) (push (list beg end old) calls))))) (downcase-word 1)) (list (buffer-string) calls (point))))"##,
    );
    assert_eq!(result, r##"OK ("ab" ((1 2 1)) 3)"##);
}

#[test]
fn simple_identity_region_no_after_change() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r##"(let* ((tbl (make-char-table 'char-code-property-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl))) (calls nil)) (with-temp-buffer (insert "ab") (goto-char 1) (let ((after-change-functions (list (lambda (beg end old) (push (list beg end old) calls))))) (downcase-region 1 3)) (list (buffer-string) calls (point))))"##,
    );
    assert_eq!(result, r##"OK ("ab" nil 1)"##);
}

#[test]
fn simple_identity_word_no_after_change() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r##"(let* ((tbl (make-char-table 'char-code-property-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl))) (calls nil)) (with-temp-buffer (insert "ab") (goto-char 1) (let ((after-change-functions (list (lambda (beg end old) (push (list beg end old) calls))))) (downcase-word 1)) (list (buffer-string) calls (point))))"##,
    );
    assert_eq!(result, r##"OK ("ab" nil 3)"##);
}
