;;; neo-term-cwd-test.el --- Cwd projection regressions -*- lexical-binding: t -*-

;; Copyright (C) 2026 Neomacs Contributors
;; License: GPL-3.0-or-later

;;; Commentary:

;; Projection-only ERT source: exercise the production Lisp hook and handler
;; with buffer/terminal-info fixtures.  Constructor and cleanup tests replace
;; only the native create/destroy boundary.  These tests neither start a PTY
;; nor establish native parser, UTF-8 validation, OSC delivery or GPU proof.
;; Run with lisp/ on load-path once editor execution is authorized.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'neo-term)

(defvar native-comp-enable-subr-trampolines)

(defun neo-term-cwd-test--owner (id directory)
  "Create a projection-only buffer fixture for ID in DIRECTORY."
  (let ((buffer (generate-new-buffer " *neo-term-cwd-owner*")))
    (with-current-buffer buffer
      (neo-term-mode)
      (setq-local neo-term--id id)
      (setq-local default-directory directory)
      (puthash id (list :id id :mode 0 :buffer buffer) neo-term--terminals))
    buffer))

(defmacro neo-term-cwd-test--with-owners (bindings &rest body)
  "Evaluate BODY with isolated projection-only owner BINDINGS.
Each binding is (VARIABLE ID DIRECTORY).  Stub native destruction so
fixture cleanup cannot affect a real host terminal."
  (declare (indent 1) (debug (sexp body)))
  `(let ((neo-term--terminals (make-hash-table :test 'eql))
         (file-name-handler-alist nil)
         (native-comp-enable-subr-trampolines nil))
     (cl-letf (((symbol-function 'neomacs-terminal-destroy)
                (lambda (_id) nil)))
       (let ,(mapcar (lambda (binding)
                      `(,(car binding)
                        (neo-term-cwd-test--owner ,(nth 1 binding)
                                                  ,(nth 2 binding))))
                    bindings)
         (unwind-protect
             (progn ,@body)
           (mapc (lambda (buffer)
                   (when (buffer-live-p buffer)
                     (kill-buffer buffer)))
                 (list ,@(mapcar #'car bindings))))))))

(defun neo-term-cwd-test--report (id directory)
  "Deliver a projection-only ID and DIRECTORY through the production hook."
  (let ((neo-term-directory-changed-functions
         (list #'neo-term--handle-directory-changed)))
    (run-hook-with-args 'neo-term-directory-changed-functions id directory)))

(ert-deftest neo-term-cwd-hook-registers-production-handler ()
  (should (memq #'neo-term--handle-directory-changed
                neo-term-directory-changed-functions)))

(ert-deftest neo-term-cwd-projects-decoded-unicode-and-literal-escapes ()
  (neo-term-cwd-test--with-owners ((owner 11 "/fixture/original/"))
    ;; Rust has already decoded the URI: literal percent text must stay so.
    (neo-term-cwd-test--report 11 "/fixture/Έργα space/#[]%20\\literal")
    (with-current-buffer owner
      (should (equal default-directory
                     "/fixture/Έργα space/#[]%20\\literal/"))
      (should (local-variable-p 'default-directory)))))

(ert-deftest neo-term-cwd-preserves-root-and-unchanged-directory ()
  (neo-term-cwd-test--with-owners ((owner 12 "/fixture/already/"))
    (let ((initial (buffer-local-value 'default-directory owner)))
      (neo-term-cwd-test--report 12 "/fixture/already")
      (should (eq initial (buffer-local-value 'default-directory owner))))
    (neo-term-cwd-test--report 12 "/")
    (should (equal "/" (buffer-local-value 'default-directory owner)))
    (neo-term-cwd-test--report 12 "/fixture/trailing/")
    (should (equal "/fixture/trailing/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-unavailable-report-leaves-directory-unchanged ()
  (neo-term-cwd-test--with-owners ((owner 13 "/fixture/original/"))
    (dolist (directory '(nil "" 17))
      (neo-term-cwd-test--report 13 directory))
    (neo-term-cwd-test--report 999 "/fixture/unknown")
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-preserves-current-buffer-and-unrelated-selection ()
  (neo-term-cwd-test--with-owners ((owner 14 "/fixture/original/"))
    (save-window-excursion
      (with-temp-buffer
        (let ((unrelated (current-buffer))
              (window (selected-window)))
          (setq-local default-directory "/fixture/unrelated/")
          (set-window-buffer window unrelated)
          (with-temp-buffer
            (let ((caller (current-buffer)))
              (setq-local default-directory "/fixture/callback/")
              (neo-term-cwd-test--report 14 "/fixture/new")
              (should (eq caller (current-buffer)))
              (should (equal default-directory "/fixture/callback/"))
              (should (eq window (selected-window)))
              (should (eq unrelated (window-buffer window)))
              (should (equal "/fixture/unrelated/"
                             (buffer-local-value 'default-directory unrelated))))))))
    (should (equal "/fixture/new/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-separates-multiple-terminals ()
  (neo-term-cwd-test--with-owners ((first 21 "/fixture/first/")
                                 (second 22 "/fixture/second/"))
    (neo-term-cwd-test--report 22 "/fixture/second-new")
    (should (equal "/fixture/first/"
                   (buffer-local-value 'default-directory first)))
    (neo-term-cwd-test--report 21 "/fixture/first-new")
    (should (equal "/fixture/first-new/"
                   (buffer-local-value 'default-directory first)))
    (should (equal "/fixture/second-new/"
                   (buffer-local-value 'default-directory second)))))

(ert-deftest neo-term-cwd-retains-renamed-object-not-same-name-replacement ()
  (neo-term-cwd-test--with-owners ((owner 23 "/fixture/original/"))
    (let ((old-name (buffer-name owner)))
      (with-current-buffer owner
        (rename-buffer " *neo-term-cwd-renamed*" t))
      (let ((replacement (generate-new-buffer old-name))
            (lookups nil))
        (unwind-protect
            (progn
              (with-current-buffer replacement
                (setq-local default-directory "/fixture/replacement/"))
              (cl-letf (((symbol-function 'get-buffer)
                         (lambda (&rest args) (push args lookups) nil))
                        ((symbol-function 'buffer-list)
                         (lambda (&rest args) (push args lookups) nil)))
                (neo-term-cwd-test--report 23 "/fixture/renamed-new"))
              (should-not lookups)
              (should (equal "/fixture/renamed-new/"
                             (buffer-local-value 'default-directory owner)))
              (should (equal "/fixture/replacement/"
                             (buffer-local-value 'default-directory replacement))))
          (kill-buffer replacement))))))

(ert-deftest neo-term-cwd-kill-and-name-recreation-refuse-stale-id ()
  (neo-term-cwd-test--with-owners ((owner 24 "/fixture/original/"))
    (let ((name (buffer-name owner)))
      (kill-buffer owner)
      (should-not (gethash 24 neo-term--terminals))
      ;; Native IDs are monotonic within a host; the replacement has a new ID.
      (let ((replacement (neo-term-cwd-test--owner 25 "/fixture/replacement/")))
        (unwind-protect
            (progn
              (with-current-buffer replacement (rename-buffer name))
              (neo-term-cwd-test--report 24 "/fixture/stale")
              (should (equal "/fixture/replacement/"
                             (buffer-local-value 'default-directory replacement)))
              (neo-term-cwd-test--report 25 "/fixture/live")
              (should (equal "/fixture/live/"
                             (buffer-local-value 'default-directory replacement))))
          (kill-buffer replacement))))))

(ert-deftest neo-term-cwd-mode-replacement-retires-authority ()
  (neo-term-cwd-test--with-owners ((owner 26 "/fixture/original/"))
    (with-current-buffer owner
      (fundamental-mode)
      (should-not (gethash 26 neo-term--terminals))
      (should-not neo-term--id)
      (neo-term-mode)
      ;; Restoring an old local ID must not restore its retired hash authority.
      (setq-local neo-term--id 26)
      (neo-term-cwd-test--report 26 "/fixture/stale")
      (should (equal default-directory "/fixture/original/")))))

(ert-deftest neo-term-cwd-destroy-retires-before-reentrant-host-event ()
  (neo-term-cwd-test--with-owners ((owner 27 "/fixture/original/"))
    (let ((observations nil))
      (cl-letf (((symbol-function 'neomacs-terminal-destroy)
                 (lambda (id)
                   (push (gethash id neo-term--terminals) observations)
                   (neo-term-cwd-test--report id "/fixture/reentrant"))))
        (neo-term--destroy 27))
      (should (equal observations '(nil))))
    (neo-term-cwd-test--report 27 "/fixture/stale")
    (should-not (gethash 27 neo-term--terminals))
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-destroy-error-still-retires-authority ()
  (neo-term-cwd-test--with-owners ((owner 28 "/fixture/original/"))
    (cl-letf (((symbol-function 'neomacs-terminal-destroy)
               (lambda (_id) (error "Projection-only host failure"))))
      (neo-term--destroy 28))
    (should-not (gethash 28 neo-term--terminals))
    (neo-term-cwd-test--report 28 "/fixture/stale")
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-rejects-mismatched-local-id ()
  (neo-term-cwd-test--with-owners ((owner 31 "/fixture/original/"))
    (with-current-buffer owner (setq-local neo-term--id 32))
    (puthash 32 (list :id 32 :mode 0 :buffer owner) neo-term--terminals)
    (neo-term-cwd-test--report 31 "/fixture/stale")
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))
    (neo-term-cwd-test--report 32 "/fixture/live")
    (should (equal "/fixture/live/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-requires-live-major-mode-even-with-hash ()
  (neo-term-cwd-test--with-owners ((owner 33 "/fixture/original/"))
    ;; Deliberately bypass normal mode cleanup to test the handler's own guard.
    (with-current-buffer owner (setq major-mode 'fundamental-mode))
    (neo-term-cwd-test--report 33 "/fixture/wrong-mode")
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-requires-hash-authority ()
  (neo-term-cwd-test--with-owners ((owner 34 "/fixture/original/"))
    (remhash 34 neo-term--terminals)
    (neo-term-cwd-test--report 34 "/fixture/no-authority")
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-rejects-named-or-missing-buffer-owner ()
  (neo-term-cwd-test--with-owners ((owner 35 "/fixture/original/"))
    (dolist (buffer (list (buffer-name owner) nil))
      (puthash 35 (list :id 35 :mode 0 :buffer buffer) neo-term--terminals)
      (neo-term-cwd-test--report 35 "/fixture/not-an-object"))
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-rejects-dead-object-even-with-stale-hash ()
  (neo-term-cwd-test--with-owners ((owner 36 "/fixture/original/"))
    (let ((info (gethash 36 neo-term--terminals)))
      (kill-buffer owner)
      (puthash 36 info neo-term--terminals)
      ;; Entering a dead owner would signal; the production guard must refuse.
      (neo-term-cwd-test--report 36 "/fixture/stale")
      (should-not (buffer-live-p owner)))))

(ert-deftest neo-term-cwd-rejects-inline-and-floating-authority ()
  (neo-term-cwd-test--with-owners ((owner 37 "/fixture/original/"))
    (dolist (mode '(1 2 nil))
      (puthash 37 (list :id 37 :mode mode :buffer owner) neo-term--terminals)
      (neo-term-cwd-test--report 37 "/fixture/nonwindow"))
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-refuses-tramp-quoted-relative-and-uri-inputs ()
  (neo-term-cwd-test--with-owners ((owner 41 "/fixture/original/"))
    (dolist (directory '("/ssh:host:/tmp" "/sudo::/tmp"
                         "/ssh:host|sudo:root:/tmp" "/:/tmp"
                         "relative/path" "~/path" "file:///tmp"))
      (neo-term-cwd-test--report 41 directory))
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-refuses-control-characters ()
  (neo-term-cwd-test--with-owners ((owner 42 "/fixture/original/"))
    (dolist (control '(0 9 10 13 27 31 127))
      (neo-term-cwd-test--report 42 (concat "/fixture/" (string control) "name")))
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-refuses-all-matching-handlers-without-invoking ()
  (neo-term-cwd-test--with-owners ((owner 43 "/fixture/original/"))
    (let* ((handler (make-symbol "neo-term-cwd-test-handler"))
           (file-name-handler-alist
            (list (cons "\\`/fixture/raw\\'" handler)
                  (cons "\\`/fixture/final/\\'" handler)))
           (inhibit-file-name-handlers (list handler))
           (inhibit-file-name-operation 'file-name-as-directory)
           (calls nil))
      ;; A handler limited to another operation still makes the name unsafe.
      (put handler 'operations '(write-region))
      (cl-letf (((symbol-function handler)
                 (lambda (&rest args) (push args calls))))
        (neo-term-cwd-test--report 43 "/fixture/raw")
        (neo-term-cwd-test--report 43 "/fixture/final")
        (should (equal "/fixture/original/"
                       (buffer-local-value 'default-directory owner)))
        (neo-term-cwd-test--report 43 "/fixture/local"))
      (should-not calls)
      (should (equal "/fixture/local/"
                     (buffer-local-value 'default-directory owner))))))

(ert-deftest neo-term-cwd-uses-no-filesystem-or-second-decoder ()
  (neo-term-cwd-test--with-owners ((owner 44 "/fixture/original/"))
    (let ((calls nil))
      (cl-letf (((symbol-function 'file-directory-p)
                 (lambda (&rest args) (push (cons 'file-directory-p args) calls)))
                ((symbol-function 'file-exists-p)
                 (lambda (&rest args) (push (cons 'file-exists-p args) calls)))
                ((symbol-function 'file-truename)
                 (lambda (&rest args) (push (cons 'file-truename args) calls)))
                ((symbol-function 'expand-file-name)
                 (lambda (&rest args) (push (cons 'expand-file-name args) calls)))
                ((symbol-function 'file-name-as-directory)
                 (lambda (&rest args)
                   (push (cons 'file-name-as-directory args) calls)))
                ((symbol-function 'file-remote-p)
                 (lambda (&rest args) (push (cons 'file-remote-p args) calls)))
                ((symbol-function 'url-unhex-string)
                 (lambda (&rest args) (push (cons 'url-unhex-string args) calls)))
                ((symbol-function 'decode-coding-string)
                 (lambda (&rest args)
                   (push (cons 'decode-coding-string args) calls))))
        (neo-term-cwd-test--report 44 "/not-required-to-exist/α%2F"))
      (should-not calls))
    (should (equal "/not-required-to-exist/α%2F/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-create-records-object-only-for-window-mode-owner ()
  (let ((neo-term--terminals (make-hash-table :test 'eql))
        (next-id 50))
    (cl-letf (((symbol-function 'neomacs-terminal-create)
               (lambda (_cols _rows _mode _shell) (cl-incf next-id))))
      (with-temp-buffer
        (neo-term-mode)
        (let ((id (neo-term--create 80 24 0 "/fixture/shell")))
          (should (eq (current-buffer)
                      (plist-get (gethash id neo-term--terminals) :buffer))))
        (dolist (mode '(1 2))
          (let ((id (neo-term--create 80 24 mode "/fixture/shell")))
            (should-not (plist-member (gethash id neo-term--terminals) :buffer)))))
      (with-temp-buffer
        (let ((id (neo-term--create 80 24 0 "/fixture/shell")))
          (should-not (plist-member (gethash id neo-term--terminals) :buffer)))))))

(ert-deftest neo-term-cwd-public-constructor-inherits-lexical-parent-directory ()
  (let ((neo-term--terminals (make-hash-table :test 'eql))
        (neo-term--next-buffer-num 1)
        (native-comp-enable-subr-trampolines nil)
        (neo-term-mode-hook
         (list (lambda () (setq-local default-directory "/fixture/mode-hook/"))))
        (spawn-context nil)
        (create-buffer (symbol-function 'get-buffer-create))
        (created nil))
    (cl-letf (((symbol-function 'get-buffer-create)
               ;; Never reuse an existing personal *neo-term-1* buffer.
               ;; generate-new-buffer calls the replaced function when interpreted.
               (lambda (name &optional inhibit-buffer-hooks)
                 (if (equal name "*neo-term-1*")
                     (setq created
                           (funcall create-buffer
                                    (generate-new-buffer-name
                                     " *neo-term-cwd-created*")
                                    inhibit-buffer-hooks))
                   ;; Interpreted `with-temp-buffer' also uses this primitive.
                   (funcall create-buffer name inhibit-buffer-hooks))))
              ((symbol-function 'neomacs-terminal-create)
               (lambda (_cols _rows mode _shell)
                 (setq spawn-context (list mode major-mode default-directory))
                 61))
              ((symbol-function 'neomacs-terminal-destroy) (lambda (_id) nil)))
      (save-window-excursion
        (with-temp-buffer
          (setq-local default-directory "/fixture/invoking/Έργα/")
          (let ((parent (current-buffer)))
            (unwind-protect
                (progn
                  (neo-term)
                  (should (equal spawn-context
                                 '(0 neo-term-mode "/fixture/invoking/Έργα/")))
                  (should (eq created (plist-get (gethash 61 neo-term--terminals)
                                                :buffer)))
                  (should (equal default-directory "/fixture/invoking/Έργα/"))
                  (should (equal (buffer-local-value 'default-directory parent)
                                 "/fixture/invoking/Έργα/")))
              (when (buffer-live-p created) (kill-buffer created)))))))))

(ert-deftest neo-term-cwd-does-not-project-into-foreign-eshell-parent-context ()
  (neo-term-cwd-test--with-owners ((owner 62 "/fixture/terminal/"))
    (with-temp-buffer
      ;; A foreign context fixture, not an Eshell/native integration claim.
      (setq major-mode 'eshell-mode)
      (setq-local default-directory "/ssh:fixture:/parent/")
      (set (make-local-variable 'eshell-last-arguments) '("keep" "α"))
      (set (make-local-variable 'eshell-last-command-name) "parent-command")
      (let ((parent (current-buffer)))
        (neo-term-cwd-test--report 62 "/fixture/terminal-new")
        (should (eq parent (current-buffer)))
        (should (eq major-mode 'eshell-mode))
        (should (equal default-directory "/ssh:fixture:/parent/"))
        (should (equal (symbol-value 'eshell-last-arguments) '("keep" "α")))
        (should (equal (symbol-value 'eshell-last-command-name) "parent-command"))))
    (should (equal "/fixture/terminal-new/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-exit-retires-authority ()
  (neo-term-cwd-test--with-owners ((owner 63 "/fixture/original/"))
    (neo-term--handle-exit 63)
    (should-not (gethash 63 neo-term--terminals))
    (should-not (buffer-local-value 'neo-term--id owner))
    (neo-term-cwd-test--report 63 "/fixture/stale")
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-create-failure-retires-authority ()
  (neo-term-cwd-test--with-owners ((owner 64 "/fixture/original/"))
    (neo-term--handle-create-failed 64 "Projection-only failure")
    (should-not (gethash 64 neo-term--terminals))
    (should-not (buffer-local-value 'neo-term--id owner))
    (neo-term-cwd-test--report 64 "/fixture/stale")
    (should (equal "/fixture/original/"
                   (buffer-local-value 'default-directory owner)))))

(ert-deftest neo-term-cwd-dot-segments-cannot-expose-anchored-handler ()
  (neo-term-cwd-test--with-owners ((owner 65 "/fixture/original/"))
    (let* ((handler (make-symbol "neo-term-cwd-dot-handler"))
           (file-name-handler-alist
            (list (cons "\\`/\\(?:ssh:host:\\|sudo::\\|:\\)" handler)))
           (calls nil))
      (cl-letf (((symbol-function handler)
                 (lambda (&rest args)
                   (push args calls)
                   (when (eq (car args) 'expand-file-name)
                     (cadr args)))))
        ;; Inputs here are the once-decoded Rust projection, not URI bytes.
        (dolist (directory '("/tmp/../ssh:host:/work" "/./ssh:host:/work"
                             "/tmp/../sudo::/work" "/./sudo::/work"
                             "/tmp/../:/work" "/./:/work"
                             "/tmp/." "/tmp/.." "/tmp/../"))
          (neo-term-cwd-test--report 65 directory)
          (with-current-buffer owner
            (should (equal default-directory "/fixture/original/"))
            (should-not calls)
            ;; Real ordinary relative predicate, not an expansion mock.
            (file-exists-p "neo-term-cwd-source-regression-probe")
            (should-not calls)))
        ;; Calibrate the anchored handler against native lexical expansion.
        ;; The predecessor admitted this default-directory spelling.
        (with-current-buffer owner
          (let ((default-directory "/tmp/../ssh:host:/work/"))
            (file-exists-p "neo-term-cwd-source-regression-probe")))
        (should calls)))))

(ert-deftest neo-term-cwd-dot-admission-keeps-unicode-and-literal-percent ()
  (neo-term-cwd-test--with-owners ((owner 66 "/fixture/original/"))
    (dolist (directory '("/home/α/.hidden" "/home/.../a..b"
                         "/home/%2e%2e" "/home/%2F"))
      (neo-term-cwd-test--report 66 directory)
      (should (equal (concat directory "/")
                     (buffer-local-value 'default-directory owner))))))

(provide 'neo-term-cwd-test)
;;; neo-term-cwd-test.el ends here
