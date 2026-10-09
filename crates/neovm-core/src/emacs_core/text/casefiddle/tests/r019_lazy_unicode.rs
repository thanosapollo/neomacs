//! #68 r019: casing-safe lazy Unicode properties and post-load buffer context.
//! New expectations are GNU SOURCE_TRACED_UNRUN, not oracle or native results.
//! Uses the ordinary production evaluator/dispatch and real fixture loading.

fn eval_fixture(form: &str) -> String {
    crate::test_utils::init_test_tracing();
    let support = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/emacs_core/text/casefiddle/tests/r019_unicode_support"
    );
    let quoted_support = support.replace('\\', "\\\\").replace('"', "\\\"");
    let form = format!(
        "(eval '(let ((load-path (cons \"{quoted_support}\" load-path))) {form}) nil)"
    );
    crate::test_utils::runtime_startup_eval_one(&form)
}

#[test]
fn unavailable_title_support_falls_back_to_up_for_both_char_operations() {
    let result = eval_fixture(r#"
(with-temp-buffer
  (set-case-table (copy-case-table (standard-case-table)))
  (set-char-table-range (char-table-extra-slot (current-case-table) 0) ?a ?Q)
  (let ((load-path nil)
        (char-code-property-alist
         '((titlecase . "neo-r019-unavailable.el"))))
    (list (capitalize ?a) (upcase-initials ?a))))
"#);
    assert_eq!(result, "OK (81 81)");
}

#[test]
fn unusable_title_tables_and_unsupported_decoders_fall_back_to_up() {
    let result = eval_fixture(r#"
(with-temp-buffer
  (set-case-table (copy-case-table (standard-case-table)))
  (set-char-table-range (char-table-extra-slot (current-case-table) 0) ?a ?Q)
  (let* ((wrong-purpose (make-char-table 'case-table ?R))
         (old-slots (get 'char-code-property-table 'char-table-extra-slots))
         (wrong-size
          (unwind-protect
              (progn
                (put 'char-code-property-table 'char-table-extra-slots 4)
                (make-char-table 'char-code-property-table ?R))
            (put 'char-code-property-table 'char-table-extra-slots old-slots)))
         (tables (list nil t 17 wrong-purpose wrong-size)))
    (dolist (decoder '(-1 1 neo-r019-lisp-decoder (lambda (x) x)))
      (let ((table (make-char-table 'char-code-property-table ?R)))
        (set-char-table-extra-slot table 1 decoder)
        (setq tables (append tables (list table)))))
    (mapcar
     (lambda (table)
       (let ((char-code-property-alist (list (cons 'titlecase table))))
         (list (capitalize ?a) (upcase-initials ?a))))
     tables)))
"#);
    assert_eq!(result, "OK ((81 81) (81 81) (81 81) (81 81) (81 81) (81 81) (81 81) (81 81) (81 81))");
}

#[test]
fn title_properties_are_character_only_and_explicit_identity_wins() {
    let result = eval_fixture(r#"
(with-temp-buffer
  (set-case-table (copy-case-table (standard-case-table)))
  (set-char-table-range (char-table-extra-slot (current-case-table) 0) ?é ?Q)
  (let* ((table (make-char-table 'char-code-property-table nil))
         (char-code-property-alist (list (cons 'titlecase table))))
    (mapcar
     (lambda (property)
       (set-char-table-range table ?é property)
       (list (capitalize ?é) (upcase-initials ?é)))
     '(233 nil "SS" neo-r019-not-a-character -1 4194304))))
"#);
    assert_eq!(result, "OK ((233 233) (81 81) (81 81) (81 81) (81 81) (81 81))");
}

#[test]
fn supported_decoder_table_uses_gnu_raw_title_character_lookup() {
    let result = eval_fixture(r#"
(with-temp-buffer
  (set-case-table (copy-case-table (standard-case-table)))
  (set-char-table-range (char-table-extra-slot (current-case-table) 0) ?a ?Q)
  (let* ((table (make-char-table 'char-code-property-table nil))
         (char-code-property-alist (list (cons 'titlecase table))))
    (set-char-table-extra-slot table 1 0)
    (set-char-table-extra-slot table 4 [82])
    (set-char-table-range table ?a 0)
    (list (capitalize ?a) (upcase-initials ?a))))
"#);
    assert_eq!(result, "OK (0 0)");
}

#[test]
fn successfully_loaded_wrong_purpose_title_table_is_unavailable_to_casing() {
    let result = eval_fixture(r#"
(with-temp-buffer
  (set-case-table (copy-case-table (standard-case-table)))
  (set-char-table-range (char-table-extra-slot (current-case-table) 0) ?a ?Q)
  (mapcar
   (lambda (op)
     (let ((char-code-property-alist
            (list (cons 'titlecase "neo-r019-wrong-purpose.el"))))
       (funcall op ?a)))
   '(capitalize upcase-initials)))
"#);
    assert_eq!(result, "OK (81 81)");
}

#[test]
fn genuine_lazy_file_evaluation_errors_propagate_for_both_char_operations() {
    let result = eval_fixture(r#"
(with-temp-buffer
  (set-case-table (copy-case-table (standard-case-table)))
  (set-char-table-range (char-table-extra-slot (current-case-table) 0) ?a ?Q)
  (mapcar
   (lambda (op)
     (let ((char-code-property-alist
            (list (cons 'titlecase "neo-r019-evaluation-error.el"))))
       (condition-case err (funcall op ?a) (error err))))
   '(capitalize upcase-initials)))
"#);
    assert_eq!(result, "OK ((error \"neo-r019-genuine-evaluation-error\") (error \"neo-r019-genuine-evaluation-error\"))");
}

fn replacement_after_load(op: &str, mode: &str) -> String {
    eval_fixture(&format!(r#"
(with-temp-buffer
  (set-case-table (copy-case-table (standard-case-table)))
  (set-char-table-range (char-table-extra-slot (current-case-table) 0) ?ß ?Q)
  (let ((char-code-property-alist
         (list (cons 'titlecase "neo-r019-titlecase.el")))
        (after-load-functions
         (list
          (lambda (file)
            (when (string-match "neo-r019-titlecase\\.el\\'" file)
              (if (eq '{mode} 'whole)
                  (set-case-table (copy-case-table (standard-case-table)))
                (set-char-table-extra-slot
                 (current-case-table) 0 (make-char-table 'case-table nil)))
              (set-char-table-range
               (char-table-extra-slot (current-case-table) 0) ?ß ?R)
              ;; GNU publishes the replaced extra slot to its separate Up BVAR.
              (set-case-table (current-case-table))
              (garbage-collect))))))
    (list ({op} ?ß)
          (char-table-range
           (char-table-extra-slot (current-case-table) 0) ?ß))))
"#))
}

#[test]
fn capitalize_char_observes_lazy_after_load_whole_case_table_replacement() {
    assert_eq!(replacement_after_load("capitalize", "whole"), "OK (82 82)");
}

#[test]
fn upcase_initials_char_observes_lazy_after_load_whole_case_table_replacement() {
    assert_eq!(replacement_after_load("upcase-initials", "whole"), "OK (82 82)");
}

#[test]
fn capitalize_char_observes_lazy_after_load_up_slot_replacement() {
    assert_eq!(replacement_after_load("capitalize", "up"), "OK (82 82)");
}

#[test]
fn upcase_initials_char_observes_lazy_after_load_up_slot_replacement() {
    assert_eq!(replacement_after_load("upcase-initials", "up"), "OK (82 82)");
}

#[test]
fn all_lazy_casing_properties_prepare_before_current_up_selection() {
    let result = eval_fixture(r#"
(mapcar
 (lambda (op)
   (with-temp-buffer
     (set-case-table (copy-case-table (standard-case-table)))
     (set-char-table-range (char-table-extra-slot (current-case-table) 0) ?ß ?Q)
     (let ((neo-r019-load-log nil)
           (char-code-property-alist
            (list (cons 'titlecase "neo-r019-titlecase.el")
                  (cons 'special-uppercase "neo-r019-special-uppercase.el")
                  (cons 'special-lowercase "neo-r019-special-lowercase.el")
                  (cons 'special-titlecase "neo-r019-special-titlecase.el")))
           (after-load-functions
            (list
             (lambda (file)
               (when (string-match "neo-r019-special-titlecase\\.el\\'" file)
                 (set-char-table-extra-slot
                  (current-case-table) 0 (make-char-table 'case-table nil))
                 (set-char-table-range
                  (char-table-extra-slot (current-case-table) 0) ?ß ?R)
                 (set-case-table (current-case-table))
                 (garbage-collect))))))
       (list (funcall op ?ß) neo-r019-load-log))))
 '(capitalize upcase-initials))
"#);
    assert_eq!(result, "OK ((82 (titlecase special-uppercase special-lowercase special-titlecase)) (82 (titlecase special-uppercase special-lowercase special-titlecase)))");
}

#[test]
fn lazy_casing_load_preserves_match_data_and_scopes_read_coding_on_all_exits() {
    let result = eval_fixture(r#"
(mapcar
 (lambda (file)
   (with-temp-buffer
     (set-case-table (copy-case-table (standard-case-table)))
     (let ((neo-r019-observed-coding nil)
           (coding-system-for-read 'raw-text-unix)
           (char-code-property-alist (list (cons 'titlecase file))))
       (string-match "b" "abc")
       (let ((outcome (condition-case err (capitalize ?é) (error err))))
         (list outcome neo-r019-observed-coding coding-system-for-read
               (match-beginning 0) (match-end 0))))))
 '("neo-r019-titlecase.el" "neo-r019-evaluation-error.el"))
"#);
    assert_eq!(result, "OK ((233 utf-8-emacs-unix raw-text-unix 1 2) ((error \"neo-r019-genuine-evaluation-error\") utf-8-emacs-unix raw-text-unix 1 2))");
}

#[test]
fn retained_title_table_and_lazy_alist_cell_survive_detaching_callbacks() {
    let result = eval_fixture(r#"
(mapcar
 (lambda (op)
   (with-temp-buffer
     (set-case-table (copy-case-table (standard-case-table)))
     (set-char-table-range (char-table-extra-slot (current-case-table) 0) ?é ?Q)
     (let ((neo-r019-load-log nil)
           (char-code-property-alist
            (list (cons 'titlecase "neo-r019-titlecase.el")
                  (cons 'special-uppercase "neo-r019-detach-title.el")))
           (after-load-functions
            (list
             (lambda (file)
               (when (string-match "neo-r019-titlecase\\.el\\'" file)
                 (setq char-code-property-alist
                       (list (cons 'special-uppercase "neo-r019-detach-title.el")))
                 (garbage-collect))))))
       (funcall op ?é))))
 '(capitalize upcase-initials))
"#);
    assert_eq!(result, "OK (233 233)");
}
