//! Lazy after-load current-buffer text regressions; native/revision3/gnu-* controls.

#[test]
fn lazy_region_current_text() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (progn (delete-region 1 2) (goto-char 1) (insert "Z"))))))) (with-temp-buffer (insert "AB") (downcase-region 1 (point-max)) (buffer-string))))"##,
    );
    assert_eq!(result, r##"OK "zb""##);
}

#[test]
fn lazy_word_current_text() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (progn (delete-region 1 2) (goto-char 1) (insert "Z"))))))) (with-temp-buffer (insert "AB") (goto-char 1) (downcase-word 1) (buffer-string))))"##,
    );
    assert_eq!(result, r##"OK "zb""##);
}

#[test]
fn lazy_region_byte_width() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (progn (delete-region 1 2) (goto-char 1) (insert "É"))))))) (with-temp-buffer (insert "AB") (downcase-region 1 (point-max)) (list (buffer-string) (point) (point-max) (position-bytes (point-max))))))"##,
    );
    assert_eq!(result, r##"OK ("éb" 2 3 4)"##);
}

#[test]
fn lazy_word_byte_width() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (progn (delete-region 1 2) (goto-char 1) (insert "É"))))))) (with-temp-buffer (insert "AB") (goto-char 1) (downcase-word 1) (list (buffer-string) (point) (point-max) (position-bytes (point-max))))))"##,
    );
    assert_eq!(result, r##"OK ("éb" 3 3 4)"##);
}

#[test]
fn lazy_negative_word_byte_width() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (progn (delete-region 1 2) (goto-char 1) (insert "É"))))))) (with-temp-buffer (insert "AB") (goto-char (point-max)) (downcase-word -1) (list (buffer-string) (point) (point-max) (position-bytes (point-max))))))"##,
    );
    assert_eq!(result, r##"OK ("éb" 3 3 4)"##);
}

#[test]
fn lazy_region_character_length() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (progn (delete-region 1 2) (goto-char 1) (insert "ZQ"))))))) (with-temp-buffer (insert "AB") (downcase-region 1 (point-max)) (list (buffer-string) (point) (point-max)))))"##,
    );
    assert_eq!(result, r##"OK ("zqB" 3 4)"##);
}

#[test]
fn lazy_word_character_length() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (progn (delete-region 1 2) (goto-char 1) (insert "ZQ"))))))) (with-temp-buffer (insert "AB") (goto-char 1) (downcase-word 1) (list (buffer-string) (point) (point-max)))))"##,
    );
    assert_eq!(result, r##"OK ("zqB" 3 4)"##);
}

#[test]
fn lazy_region_narrowed_byte_width() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (progn (delete-region 2 3) (goto-char 2) (insert "É"))))))) (with-temp-buffer (insert "XABY") (narrow-to-region 2 4) (downcase-region (point-min) (point-max)) (list (buffer-string) (point) (point-min) (point-max) (progn (widen) (buffer-string))))))"##,
    );
    assert_eq!(result, r##"OK ("éb" 3 2 4 "XébY")"##);
}

#[test]
fn lazy_region_callback_narrowing() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (progn (delete-region 1 2) (goto-char 1) (insert "É") (narrow-to-region 2 3))))))) (with-temp-buffer (insert "AB") (condition-case e (downcase-region 1 (point-max)) (args-out-of-range nil)) (buffer-string))))"##,
    );
    assert_eq!(result, r##"OK "B""##);
}
