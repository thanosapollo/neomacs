//! #68 r020: real loader after-load Flow propagation, SOURCE_TRACED_UNRUN.
//! Authored only: no compiler, Lisp reader, GNU or native execution.

fn eval_fixture(form: &str) -> String {
    crate::test_utils::init_test_tracing();
    let support = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/emacs_core/text/casefiddle/tests/r019_unicode_support"
    );
    let quoted_support = support.replace('\\', "\\\\").replace('"', "\\\"");
    crate::test_utils::runtime_startup_eval_one(&format!(
        "(eval '(let ((load-path (cons \"{quoted_support}\" load-path))) {form}) nil)"
    ))
}

fn title_callback_error(op: &str) -> String {
    eval_fixture(&format!(r#"
(with-temp-buffer
  (set-case-table (copy-case-table (standard-case-table)))
  (set-char-table-range (char-table-extra-slot (current-case-table) 0) ?a ?Q)
  (let ((char-code-property-alist
         (list (cons 'titlecase "neo-r019-titlecase.el")))
        (after-load-functions
         (list (lambda (file)
                 (when (string-match "neo-r019-titlecase\\.el\\'" file)
                   (error "neo-r020-after-load-error"))))))
    (condition-case err ({op} ?a) (error err))))
"#))
}

#[test]
fn capitalize_char_propagates_exact_after_load_error() {
    assert_eq!(title_callback_error("capitalize"), "OK (error \"neo-r020-after-load-error\")");
}

#[test]
fn upcase_initials_char_propagates_exact_after_load_error() {
    assert_eq!(title_callback_error("upcase-initials"), "OK (error \"neo-r020-after-load-error\")");
}

#[test]
fn later_special_titlecase_callback_error_aborts_both_char_operations() {
    let result = eval_fixture(r#"
(mapcar
 (lambda (op)
   (with-temp-buffer
     (set-case-table (copy-case-table (standard-case-table)))
     (let ((neo-r019-load-log nil)
           (char-code-property-alist
            (list (cons 'titlecase "neo-r019-titlecase.el")
                  (cons 'special-uppercase "neo-r019-special-uppercase.el")
                  (cons 'special-lowercase "neo-r019-special-lowercase.el")
                  (cons 'special-titlecase "neo-r019-special-titlecase.el")))
           (after-load-functions
            (list (lambda (file)
                    (when (string-match "neo-r019-special-titlecase\\.el\\'" file)
                      (error "neo-r020-later-callback-error"))))))
       (list (condition-case err (funcall op ?é) (error err))
             neo-r019-load-log))))
 '(capitalize upcase-initials))
"#);
    assert_eq!(result, "OK (((error \"neo-r020-later-callback-error\") (titlecase special-uppercase special-lowercase special-titlecase)) ((error \"neo-r020-later-callback-error\") (titlecase special-uppercase special-lowercase special-titlecase)))");
}

#[test]
fn after_load_throw_reaches_enclosing_catch_for_both_char_operations() {
    let result = eval_fixture(r#"
(mapcar
 (lambda (op)
   (let ((char-code-property-alist
          (list (cons 'titlecase "neo-r019-titlecase.el")))
         (after-load-functions
          (list (lambda (file)
                  (when (string-match "neo-r019-titlecase\\.el\\'" file)
                    (throw 'neo-r020-tag '(neo-r020-thrown 17)))))))
     (catch 'neo-r020-tag (funcall op ?a))))
 '(capitalize upcase-initials))
"#);
    assert_eq!(result, "OK ((neo-r020-thrown 17) (neo-r020-thrown 17))");
}

#[test]
fn after_load_quit_is_not_downgraded_to_success_or_error() {
    let result = eval_fixture(r#"
(mapcar
 (lambda (op)
   (let ((char-code-property-alist
          (list (cons 'titlecase "neo-r019-titlecase.el")))
         (after-load-functions
          (list (lambda (file)
                  (when (string-match "neo-r019-titlecase\\.el\\'" file)
                    (signal 'quit nil))))))
     (condition-case err (funcall op ?a) (quit err) (error 'wrong-error))))
 '(capitalize upcase-initials))
"#);
    assert_eq!(result, "OK ((quit) (quit))");
}

#[test]
fn ordinary_load_preserves_success_history_context_and_callback_order() {
    let result = eval_fixture(r#"
(let ((neo-r020-events nil)
      (load-file-name "neo-r020-caller")
      (current-load-list '(neo-r020-caller-list))
      (char-code-property-alist
       (list (cons 'titlecase "neo-r019-titlecase.el")))
      (after-load-alist
       (list (list "neo-r019-titlecase\\.el\\'"
                   (lambda ()
                     (setq neo-r020-events (append neo-r020-events '(alist-one))))
                   (lambda ()
                     (setq neo-r020-events (append neo-r020-events '(alist-two)))))))
      (after-load-functions
       (list (lambda (file)
               (when (string-match "neo-r019-titlecase\\.el\\'" file)
                 (setq neo-r020-events
                       (append neo-r020-events
                               (list (list 'hook-one load-file-name current-load-list
                                           (not (null (assoc file load-history)))))))))
             (lambda (file)
               (when (string-match "neo-r019-titlecase\\.el\\'" file)
                 (setq neo-r020-events (append neo-r020-events '(hook-two))))))))
  (list (load "international/neo-r019-titlecase.el" nil t t t)
        neo-r020-events load-file-name current-load-list))
"#);
    assert_eq!(result, "OK (t (alist-one alist-two (hook-one \"neo-r020-caller\" (neo-r020-caller-list) t) hook-two) \"neo-r020-caller\" (neo-r020-caller-list))");
}

#[test]
fn noerror_suppresses_only_missing_support_not_body_or_callback_errors() {
    let result = eval_fixture(r#"
(mapcar
 (lambda (file)
   (let ((char-code-property-alist (list (cons 'titlecase file)))
         (after-load-functions
          (list (lambda (loaded)
                  (when (string-match "neo-r019-titlecase\\.el\\'" loaded)
                    (error "neo-r020-after-load-error"))))))
     (condition-case err
         (load (concat "international/" file) t t t t)
       (error err))))
 '("neo-r020-missing.el" "neo-r019-evaluation-error.el" "neo-r019-titlecase.el"))
"#);
    assert_eq!(result, "OK (nil (error \"neo-r019-genuine-evaluation-error\") (error \"neo-r020-after-load-error\"))");
}

#[test]
fn callback_flows_restore_coding_match_load_bindings_and_detached_roots() {
    let result = eval_fixture(r#"
(mapcar
 (lambda (kind)
   (with-temp-buffer
     (set-case-table (copy-case-table (standard-case-table)))
     (let ((coding-system-for-read 'raw-text-unix)
           (load-file-name "neo-r020-caller")
           (neo-r019-observed-coding nil)
           (char-code-property-alist
            (list (cons 'titlecase "neo-r019-titlecase.el")))
           (after-load-functions
            (list (lambda (file)
                    (when (string-match "neo-r019-titlecase\\.el\\'" file)
                      (setq char-code-property-alist nil)
                      (garbage-collect)
                      (cond ((eq kind 'error) (error "neo-r020-unwind"))
                            ((eq kind 'throw) (throw 'neo-r020-tag 'thrown))
                            ((eq kind 'quit) (signal 'quit nil))))))))
       (string-match "b" "abc")
       (let ((outcome
              (catch 'neo-r020-tag
                (condition-case err (capitalize ?é) (error err) (quit err)))))
         (garbage-collect)
         (list outcome neo-r019-observed-coding coding-system-for-read
               load-file-name (match-beginning 0) (match-end 0)
               ;; A fresh operation after unwind must not reuse detached state.
               (let ((char-code-property-alist nil)) (capitalize ?a)))))))
 '(success error throw quit))
"#);
    assert_eq!(result, "OK ((233 utf-8-emacs-unix raw-text-unix \"neo-r020-caller\" 1 2 65) ((error \"neo-r020-unwind\") utf-8-emacs-unix raw-text-unix \"neo-r020-caller\" 1 2 65) (thrown utf-8-emacs-unix raw-text-unix \"neo-r020-caller\" 1 2 65) ((quit) utf-8-emacs-unix raw-text-unix \"neo-r020-caller\" 1 2 65))");
}
