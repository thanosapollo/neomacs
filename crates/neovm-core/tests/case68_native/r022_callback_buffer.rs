//! Lazy casing preparation follows the buffer left by after-load-functions.
//! Region modification hooks still target the initiating buffer; word scanning
//! precedes preparation, but word point settles in the prepared current buffer.

#[test]
fn lazy_region_populated_buffer_switch() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((other (get-buffer-create "review-other"))) (with-current-buffer other (erase-buffer) (insert "XY")) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (set-buffer other)))))) (with-temp-buffer (insert "AB") (let ((target (current-buffer))) (downcase-region 1 3) (list (buffer-string) (with-current-buffer target (buffer-string)) (with-current-buffer other (buffer-string))))))))"##,
    );
    assert_eq!(result, r##"OK ("xy" "AB" "xy")"##);
}

#[test]
fn lazy_word_populated_buffer_switch() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((other (get-buffer-create "review-other"))) (with-current-buffer other (erase-buffer) (insert "XY") (goto-char 1)) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (set-buffer other)))))) (with-temp-buffer (insert "AB") (goto-char 1) (let ((target (current-buffer))) (downcase-word 1) (list (buffer-string) (with-current-buffer target (buffer-string)) (with-current-buffer other (buffer-string)) (point) (with-current-buffer target (point)) (with-current-buffer other (point))))))))"##,
    );
    assert_eq!(result, r##"OK ("xy" "AB" "xy" 3 1 3)"##);
}
