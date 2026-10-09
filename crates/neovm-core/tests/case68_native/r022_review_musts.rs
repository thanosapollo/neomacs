//! Regression-first corrections for the three executed r22 review MUSTs.
//! GNU literal controls: native/revision2/gnu-5/results.json.

#[test]
fn property_unavailable() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(let ((char-code-property-alist (list (cons 'special-lowercase nil)))) (list (capitalize "AİB") (downcase "AİB")))"##,
    );
    assert_eq!(result, r##"OK ("Aİb" "aİb")"##);
}

#[test]
fn property_custom_raw_string() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(let* ((tbl (make-char-table 'char-code-property-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl)))) (set-char-table-range tbl ?İ "xy") (list (capitalize "AİB") (downcase "AİB") (with-temp-buffer (insert "AİB") (capitalize-region 1 (point-max)) (buffer-string))))"##,
    );
    assert_eq!(result, r##"OK ("Axyb" "axyb" "Axyb")"##);
}

#[test]
fn property_genuine_lazy_error() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-evaluation-error.el")))) (condition-case e (capitalize "AİB") (error (list 'caught (car e) (cadr e)))))"##,
    );
    assert_eq!(result, r##"OK (caught error "neo-r019-genuine-evaluation-error")"##);
}

#[test]
fn nil_category_same_script() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(let ((word-separating-categories '((nil . nil)))) (with-temp-buffer (let ((ct (make-category-table))) (set-char-table-range ct nil nil) (set-char-table-range ct t nil) (set-category-table ct)) (insert "aB") (list (progn (goto-char 1) (forward-word 1) (point)) (progn (goto-char 3) (forward-word -1) (point)) (progn (goto-char 3) (backward-word 1) (point)) (progn (goto-char 1) (capitalize-word 1) (list (buffer-string) (point))))))"##,
    );
    assert_eq!(result, r##"OK (3 1 1 ("Ab" 3))"##);
}

#[test]
fn nil_category_different_script() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(let ((word-combining-categories '((nil . nil)))) (with-temp-buffer (let ((ct (make-category-table))) (set-char-table-range ct nil nil) (set-char-table-range ct t nil) (set-category-table ct)) (insert "aΒ") (list (progn (goto-char 1) (forward-word 1) (point)) (progn (goto-char 3) (forward-word -1) (point)) (progn (goto-char 1) (capitalize-word 1) (list (buffer-string) (point))))))"##,
    );
    assert_eq!(result, r##"OK (2 2 ("AΒ" 2))"##);
}

#[test]
fn empty_category_control() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(let ((word-separating-categories '((nil . nil)))) (with-temp-buffer (set-category-table (make-category-table)) (insert "aB") (list (progn (goto-char 1) (forward-word 1) (point)) (progn (goto-char 3) (backward-word 1) (point)))))"##,
    );
    assert_eq!(result, r##"OK (2 2)"##);
}

#[test]
fn downcase_prefix_region_string() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(with-temp-buffer (modify-syntax-entry ?A "w p") (insert "AΣ") (downcase-region 1 (point-max)) (list (buffer-string) (downcase "AΣ")))"##,
    );
    assert_eq!(result, r##"OK ("aσ" "aς")"##);
}

#[test]
fn nil_category_either_side() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(mapcar (lambda (c) (let ((word-separating-categories '((nil . nil)))) (with-temp-buffer (let ((ct (make-category-table))) (set-char-table-range ct nil nil) (set-char-table-range ct t nil) (set-char-table-range ct (if (= c ?a) ?B ?a) (make-category-set "")) (set-category-table ct)) (insert "aB") (list (progn (goto-char 1) (forward-word 1) (point)) (progn (goto-char 3) (backward-word 1) (point)) (progn (goto-char 1) (capitalize-word 1) (list (buffer-string) (point))))))) '(?a ?B))"##,
    );
    assert_eq!(result, r##"OK ((3 1 ("Ab" 3)) (3 1 ("Ab" 3)))"##);
}

#[test]
fn nil_category_either_side_different_script() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(mapcar (lambda (c) (let ((word-combining-categories '((nil . nil)))) (with-temp-buffer (let ((ct (make-category-table))) (set-char-table-range ct nil nil) (set-char-table-range ct t nil) (set-char-table-range ct (if (= c ?a) ?Β ?a) (make-category-set "")) (set-category-table ct)) (insert "aΒ") (list (progn (goto-char 1) (forward-word 1) (point)) (progn (goto-char 3) (backward-word 1) (point)) (progn (goto-char 1) (capitalize-word 1) (list (buffer-string) (point))))))) '(?a ?Β))"##,
    );
    assert_eq!(result, r##"OK ((2 2 ("AΒ" 2)) (2 2 ("AΒ" 2)))"##);
}

#[test]
fn downcase_prefix_word_negative_string() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(mapcar (lambda (n) (with-temp-buffer (modify-syntax-entry ?A "w p") (insert "AΣ") (goto-char (if (> n 0) 1 3)) (let ((word-combining-categories '((nil . nil)))) (downcase-word n)) (list (buffer-string) (point) (downcase "AΣ")))) '(1 -1))"##,
    );
    assert_eq!(result, r##"OK (("aσ" 3 "aς") ("aσ" 3 "aς"))"##);
}

#[test]
fn downcase_prefix_already_inword() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(with-temp-buffer (modify-syntax-entry ?A "w p") (insert "BAΣ") (downcase-region 1 (point-max)) (list (buffer-string) (downcase "BAΣ")))"##,
    );
    assert_eq!(result, r##"OK ("baς" "baς")"##);
}

#[test]
fn special_lowercase_raw_type_decoder_controls() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(mapcar (lambda (decoder) (let* ((tbl (make-char-table 'char-code-property-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl)))) (set-char-table-extra-slot tbl 1 decoder) (set-char-table-range tbl ?İ "xy") (list (capitalize "AİB") (downcase "AİB")))) '(nil 0 -1 1))"##,
    );
    assert_eq!(result, r##"OK (("Axyb" "axyb") ("Axyb" "axyb") ("Aİb" "aİb") ("Aİb" "aİb"))"##);
}

#[test]
fn special_lowercase_nonstring_and_byte_limit() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(mapcar (lambda (v) (let* ((tbl (make-char-table 'char-code-property-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl)))) (set-char-table-range tbl ?İ v) (list (capitalize "AİB") (downcase "AİB")))) '(nil 120 "" "x" "123456" "1234567"))"##,
    );
    assert_eq!(result, r##"OK (("Aİb" "aİb") ("Aİb" "aİb") ("Ab" "ab") ("Axb" "axb") ("A123456b" "a123456b") ("Aİb" "aİb"))"##);
}

#[test]
fn special_lowercase_decoder_zero_raw_index() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(let* ((tbl (make-char-table 'char-code-property-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl)))) (set-char-table-extra-slot tbl 1 0) (set-char-table-extra-slot tbl 4 [nil "xy"]) (set-char-table-range tbl ?İ 1) (list (capitalize "AİB") (downcase "AİB")))"##,
    );
    assert_eq!(result, r##"OK ("Aİb" "aİb")"##);
}

#[test]
fn special_lowercase_wrong_purpose() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(let* ((tbl (make-char-table 'case-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl)))) (set-char-table-range tbl ?İ "xy") (list (capitalize "AİB") (downcase "AİB")))"##,
    );
    assert_eq!(result, r##"OK ("Aİb" "aİb")"##);
}

#[test]
fn special_lowercase_sigma_postprocess() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(let* ((tbl (make-char-table 'char-code-property-table nil)) (char-code-property-alist (list (cons 'special-lowercase tbl)))) (set-char-table-range tbl ?Σ "xy") (list (capitalize "AΣ AΣB") (downcase "AΣ AΣB")))"##,
    );
    assert_eq!(result, r##"OK ("Aς Axyb" "aς axyb")"##);
}

#[test]
fn special_lowercase_lazy_error_all_down_targets() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(mapcar (lambda (op) (let ((load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-evaluation-error.el")))) (condition-case e (if (eq op 'downcase) (downcase "AİB") (with-temp-buffer (insert "AİB") (goto-char 1) (if (memq op '(downcase-word capitalize-word)) (funcall op 1) (funcall op 1 (point-max))))) (error (list 'caught (car e) (cadr e)))))) '(downcase downcase-region capitalize-region downcase-word capitalize-word))"##,
    );
    assert_eq!(result, r##"OK ((caught error "neo-r019-genuine-evaluation-error") (caught error "neo-r019-genuine-evaluation-error") (caught error "neo-r019-genuine-evaluation-error") (caught error "neo-r019-genuine-evaluation-error") (caught error "neo-r019-genuine-evaluation-error"))"##);
}

#[test]
fn special_lowercase_lazy_load_identity_refresh() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let* ((neo-r019-load-log nil) (loads nil) (load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase "neo-r019-special-lowercase.el"))) (after-load-functions (list (lambda (file) (if (equal (file-name-nondirectory file) "neo-r019-special-lowercase.el") (setq loads (cons (file-name-nondirectory file) loads))) (let ((ct (copy-case-table (standard-case-table)))) (set-char-table-range ct ?B ?x) (set-case-table ct)) (garbage-collect))))) (with-temp-buffer (list (capitalize (concat "AİB")) loads (char-table-p (cdr (assq 'special-lowercase char-code-property-alist)))))))"##,
    );
    assert_eq!(result, r##"OK ("Aİx" ("neo-r019-special-lowercase.el") t)"##);
}

#[test]
fn special_lowercase_final_load_refresh_roots() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::r022_eval_with_support(
        r##"(progn (defvar neo-r019-load-log nil) (let* ((neo-r019-load-log nil) (lower (make-char-table 'char-code-property-table nil)) (load-path (cons "__CASE68_SUPPORT_DIRECTORY__" load-path)) (char-code-property-alist (list (cons 'special-lowercase lower) (cons 'special-titlecase "neo-r019-special-titlecase.el"))) (after-load-functions (list (lambda (file) (setcdr (assq 'special-lowercase char-code-property-alist) nil) (let ((ct (copy-case-table (standard-case-table)))) (set-char-table-range ct ?B ?x) (set-case-table ct)) (garbage-collect))))) (set-char-table-range lower ?İ "xy") (setq lower nil) (with-temp-buffer (capitalize (concat "AİB")))))"##,
    );
    assert_eq!(result, r##"OK "Axyx""##);
}

