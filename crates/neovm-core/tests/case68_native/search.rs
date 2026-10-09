//! Selected original search fixtures; bodies and assertions are verbatim.
//! See writer/inventory.json for original identities and exclusions.

fn bootstrap_eval_one(src: &str) -> String {
    crate::test_utils::runtime_startup_eval_all(src)
        .into_iter()
        .next()
        .expect("at least one form")
}

#[test]
fn replace_match_preserves_raw_nil_boundaries_for_later_properties() {
    crate::test_utils::init_test_tracing();
    let mut ev = crate::emacs_core::eval::Context::new();
    let result = ev
        .eval_str(
            r#"(progn
  (insert (concat (make-string 46 ?x) "abc" (make-string 31 ?x)))
  (put-text-property 1 25 'face 'org-table)
  (goto-char 47)
  (looking-at "abc")
  (replace-match "abcde" t t)
  (put-text-property 43 54 'face 'org-table)
  (prin1-to-string (buffer-string)))"#,
        )
        .expect("replace-match interval shape form should evaluate");
    let printed = result.as_utf8_str().unwrap().to_owned();
    assert!(
        printed.contains("42 46 (face org-table) 46 51 (face org-table) 51 53 (face org-table)"),
        "raw nil boundaries inside later property range should be preserved: {printed}"
    );
}

#[test]
fn replace_match_uses_the_current_buffer_whatever_the_match_data_records() {
    crate::test_utils::init_test_tracing();
    let result = bootstrap_eval_one(
        r#"(list
             (with-temp-buffer
               (insert "workers = 0099\n")
               (goto-char 11)
               (set-match-data (list 11 15 11 11 11 15))
               (replace-match "0102" t t nil 2)
               (buffer-string))
             (with-temp-buffer
               (insert "workers = 0099\n")
               (set-match-data (list 11 15))
               (replace-match "X" t t nil 0)
               (buffer-string))
             (with-temp-buffer
               (insert "workers = 0099\n")
               (set-match-data (list (copy-marker 11) (copy-marker 15)))
               (replace-match "X" t t nil 0)
               (buffer-string)))"#,
    );
    assert_eq!(
        result,
        "OK (\"workers = 0102\n\" \"workers = X\n\" \"workers = X\n\")"
    );
}

#[test]
fn replace_match_reports_a_subexpression_outside_the_accessible_portion() {
    crate::test_utils::init_test_tracing();
    let result = bootstrap_eval_one(
        r#"(mapcar
             (lambda (thunk) (condition-case e (funcall thunk) (error e)))
             (list
              (lambda () (with-temp-buffer
                           (insert "abc")
                           (set-match-data (list 2 99))
                           (replace-match "X" t t nil 0)))
              (lambda () (with-temp-buffer
                           (insert "abcdef")
                           (narrow-to-region 3 6)
                           (set-match-data (list 1 2))
                           (replace-match "X" t t nil 0)))
              (lambda () (with-temp-buffer
                           (insert "abc")
                           (set-match-data (list 1 3 nil nil))
                           (replace-match "X" t t nil 1)))))"#,
    );
    assert_eq!(
        result,
        r#"OK ((args-out-of-range 2 99) (args-out-of-range 1 2) (error "replace-match subexpression does not exist" 1))"#
    );
}

#[test]
fn replace_match_adjusts_the_registers_for_the_edit_it_made() {
    crate::test_utils::init_test_tracing();
    let result = bootstrap_eval_one(
        r#"(list
             (with-temp-buffer
               (insert "aa 01 bb 02 cc")
               (set-match-data (list 4 6))
               (replace-match "999" t t nil 0)
               (list (point) (match-beginning 0) (match-end 0)))
             (with-temp-buffer
               (insert "aa 01 bb 02 cc")
               (goto-char (point-min))
               (re-search-forward "[0-9]+")
               (replace-match "999" t t nil 0)
               (list (point) (match-beginning 0) (match-end 0))))"#,
    );
    assert_eq!(result, "OK ((7 4 7) (7 4 7))");
}

#[test]
fn replace_match_expands_an_ampersand_as_the_subexp_like_gnu() {
    crate::test_utils::init_test_tracing();
    let result = bootstrap_eval_one(
        r#"(list
             (progn (string-match "\\(foo\\)\\(BAR\\)" "xxfooBARyy")
                    (replace-match "<\\&>" t nil "xxfooBARyy" 2))
             (progn (string-match "\\(foo\\)\\(BAR\\)" "xxfooBARyy")
                    (replace-match "<\\1|\\&>" t nil "xxfooBARyy" 2))
             (progn (string-match "\\(foo\\)\\(BAR\\)" "xxfooBARyy")
                    (replace-match "<\\&>" t nil "xxfooBARyy"))
             (progn (string-match "\\(FOO\\)" "FOO")
                    (replace-match "baz" nil nil "FOO" 1))
             (with-temp-buffer
               (insert "xxfooBARyy")
               (goto-char (point-min))
               (re-search-forward "\\(foo\\)\\(BAR\\)")
               (replace-match "<\\&>" t nil nil 2)
               (buffer-string)))"#,
    );
    assert_eq!(
        result,
        r#"OK ("xxfoo<BAR>yy" "xxfoo<foo|BAR>yy" "xx<fooBAR>yy" "BAZ" "xxfoo<BAR>yy")"#
    );
}

#[test]
fn replace_match_cases_with_the_buffers_tables_like_gnu() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
        (let ((run (lambda (setup matched newtext)
                    (with-temp-buffer
                      (set-syntax-table (make-syntax-table))
                      (funcall setup)
                      (let ((case-fold-search t)
                            (s (concat "<" matched ">")))
                        (string-match (regexp-quote matched) s)
                        (list (replace-match newtext nil nil s)
                              (progn (erase-buffer) (insert s) (goto-char (point-min))
                                     (search-forward matched)
                                     (replace-match newtext nil nil)
                                     (buffer-string))))))))
          (list
           ;; H made caseless: no capitalized initial, so no change.
           (funcall run (lambda ()
                  (let ((tbl (copy-case-table (standard-case-table))))
                    (set-case-syntax ?H "w" tbl)
                    (set-case-table tbl)))
                "Hello" "world")
           ;; Word boundaries come from the syntax table.
           (funcall run (lambda () (modify-syntax-entry ?- "w")) "Hello" "foo-bar")
           (funcall run #'ignore "Hello" "foo-bar")
           (funcall run (lambda () (setq-local case-symbols-as-words t)) "Hello" "foo_bar")
           ;; The prefix flag matters only in the buffer.
           (funcall run (lambda () (modify-syntax-entry ?' "w p")) "Hello" "'foo")
           ;; Initials are titlecased.
           (funcall run #'ignore "École" "ǆemal")))
        "#,
    );
    assert_eq!(
        result,
        r#"OK (("<world>" "<world>") ("<Foo-bar>" "<Foo-bar>") ("<Foo-Bar>" "<Foo-Bar>") ("<Foo_bar>" "<Foo_bar>") ("<'foo>" "<'Foo>") ("<ǅemal>" "<ǅemal>"))"#
    );
}

#[test]
fn replace_match_case_starts_after_a_newline_like_gnu() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
        (with-temp-buffer
          (set-syntax-table (make-syntax-table))
          (modify-syntax-entry ?\n "w")
          (let ((s "<hello>"))
            (string-match "hello" s)
            (list (replace-match "world" nil nil s)
                  (progn (insert s) (goto-char 1) (search-forward "hello")
                         (replace-match "world") (buffer-string)))))
        "#,
    );
    assert_eq!(result, r#"OK ("<World>" "<World>")"#);
}

#[test]
fn replace_match_cases_in_the_targets_representation_like_gnu() {
    crate::test_utils::init_test_tracing();
    let result = crate::test_utils::runtime_startup_eval_one(
        r#"
        (list
         (with-temp-buffer
           (let ((tbl (copy-case-table (standard-case-table))))
             (set-case-syntax-pair ?Ā ?X tbl)
             (set-case-table tbl))
           (insert "<Hello>") (goto-char 1) (search-forward "Hello")
           (replace-match (string-as-unibyte "Xoo"))
           (append (buffer-string) nil))
         (with-temp-buffer
           (set-buffer-multibyte nil)
           (insert "<Hello>") (goto-char 1) (search-forward "Hello")
           (replace-match "éoo")
           (append (buffer-string) nil))
         (with-temp-buffer
           (let ((tbl (copy-case-table (standard-case-table))))
             (set-case-syntax-pair ?Ā ?X tbl)
             (set-case-table tbl))
           (let ((s "<Hello>"))
             (string-match "Hello" s)
             (append (replace-match (string-as-unibyte "Xoo") nil nil s) nil))))
        "#,
    );
    assert_eq!(
        result,
        "OK ((60 256 111 111 62) (60 233 111 111 62) (60 88 111 111 62))"
    );
}
