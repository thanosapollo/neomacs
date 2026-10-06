;;; neo-term-eshell-test.el --- Detached native visual regressions -*- lexical-binding: t; -*-

(require 'ert)
(require 'cl-lib)
(require 'eshell)
(require 'neo-term-eshell)

(defvar native-comp-enable-subr-trampolines)

(defmacro neo-term-eshell-test--with-shell (&rest body)
  "Run BODY in disposable real Eshell mode, intercepting only native FFI.
Production Eshell parsing/interpreters/handles, adapter and terminal Lisp are
not replaced.  This fixture is not a PTY/parser/renderer substitute."
  (declare (indent 0) (debug t))
  `(save-window-excursion
     (let ((parent (generate-new-buffer " *native-eshell-source-test*"))
           (neo-term--terminals (make-hash-table :test 'eql))
           (process-environment (cons "SHELL=/bin/sh" process-environment))
           (file-name-handler-alist nil)
           (system-type 'gnu/linux)
           (admissions nil)
           (next-id 810)
           (destroyed nil)
           (eshell-visual-commands '("printf" "sh"))
           (cwd-pinning-supported t)
           (native-comp-enable-subr-trampolines nil))
       (unwind-protect
           (cl-letf (((symbol-function 'neomacs-terminal-cwd-pinning-p)
                      (lambda (_directory) cwd-pinning-supported))
                     ((symbol-function 'neomacs-terminal-spawn)
                      (lambda (cols rows executable argv directory environment)
                        (push (list cols rows executable argv directory environment
                                    (current-buffer)) admissions)
                        (cl-incf next-id)))
                     ((symbol-function 'neomacs-terminal-get-text)
                      (lambda (_id) "FINAL_NATIVE_SNAPSHOT"))
                     ((symbol-function 'neomacs-terminal-destroy)
                      (lambda (id) (push id destroyed) t)))
             (switch-to-buffer parent)
             (eshell-mode)
             (setq-local default-directory "/")
             (neo-term-eshell-mode 1)
             (eshell-with-handles (t 'insert)
               ,@body))
         (maphash (lambda (_id info)
                    (let ((buffer (plist-get info :buffer)))
                      (when (buffer-live-p buffer) (kill-buffer buffer))))
                  (copy-hash-table neo-term--terminals))
         (when (buffer-live-p parent) (kill-buffer parent))))))

(ert-deftest neo-term-eshell-buffer-local-opt-in-preserves-canonical-interpreter ()
  (neo-term-eshell-test--with-shell
    (let ((entry neo-term-eshell--entry)
          (original neo-term-eshell--original-entry)
          (other (generate-new-buffer " *native-eshell-other*")))
      (unwind-protect
          (progn
            (should (eq (car eshell-interpreter-alist) entry))
            (should-not (memq original eshell-interpreter-alist))
            (with-current-buffer other
              (eshell-mode)
              (should-not neo-term-eshell-mode)
              (should-not (memq entry eshell-interpreter-alist)))
            (neo-term-eshell-mode -1)
            (should-not neo-term-eshell--epoch)
            (should-not (memq entry eshell-interpreter-alist))
            (should (memq original eshell-interpreter-alist)))
        (kill-buffer other)))))

(ert-deftest neo-term-eshell-real-parser-dispatch-preserves-literal-argv-and-detaches ()
  (neo-term-eshell-test--with-shell
    (let ((before-directory default-directory))
      (should-not (eshell-eval-command
                   (eshell-parse-command "/bin/printf '%s' 'space λ' '; $()'")))
      (should (= (length admissions) 1))
      (let* ((request (car admissions))
             (terminal (nth 6 request)))
        (should (equal (nth 2 request) "/bin/printf"))
        (should (equal (nth 3 request) '("%s" "space λ" "; $()")))
        (should (equal (nth 4 request) before-directory))
        (should (equal (car (nth 5 request)) "TERM=xterm-256color"))
        (should (eq (window-buffer (selected-window)) terminal))
        (should (eq (current-buffer) parent))
        (should-not eshell-foreground-command)
        (should (equal default-directory before-directory))))))

(ert-deftest neo-term-eshell-context-refuses-pipe-background-subcommand-and-input ()
  (neo-term-eshell-test--with-shell
    (let ((eshell-current-handles (eshell-create-handles t 'insert)))
      (should (neo-term-eshell--eligible-p "/bin/printf" nil))
      (dolist (pipeline '(first t last))
        (let ((eshell-in-pipeline-p pipeline))
          (should-not (neo-term-eshell--eligible-p "/bin/printf" nil))))
      (let ((eshell-current-subjob-p t))
        (should-not (neo-term-eshell--eligible-p "/bin/printf" nil)))
      (let ((eshell-in-subcommand-p t))
        (should-not (neo-term-eshell--eligible-p "/bin/printf" nil)))
      (aset eshell-current-handles 0 'supplied-input)
      (should-not (neo-term-eshell--eligible-p "/bin/printf" nil)))
    (let ((eshell-current-handles nil))
      (should-not (neo-term-eshell--eligible-p "/bin/printf" nil)))
    ;; Canonical parser itself rejects stdin redirection; do not bypass it.
    (should-error (eshell-parse-command "/bin/printf x < file"))
    (should-not admissions)))

(ert-deftest neo-term-eshell-real-handles-refuse-redirection-and-magic-paths ()
  (neo-term-eshell-test--with-shell
    (let ((eshell-current-handles (eshell-create-handles t 'insert)))
      (eshell-set-output-handle eshell-output-handle 'insert 'redirected)
      (should-not (neo-term-eshell--eligible-p "/bin/printf" nil)))
    (let ((eshell-current-handles (eshell-create-handles t 'insert)))
      (dolist (path '("/ssh:host:/" "/safe/../ssh:host:/" "/:"))
        (let ((default-directory path))
          (should-not (neo-term-eshell--eligible-p "/bin/printf" nil))))
      (let ((file-name-handler-alist
             '(("\\`/bin/printf\\'" . must-not-call))))
        (should-not (neo-term-eshell--eligible-p "/bin/printf" nil))))
    (should-not admissions)))

(ert-deftest neo-term-eshell-unset-shell-is-canonical-not-invented-environment ()
  (neo-term-eshell-test--with-shell
    (should (neo-term-eshell--exact-environment-p '("SHELL=/bin/sh" "SHELL")))
    (should-not (neo-term-eshell--exact-environment-p '("A=a")))
    (should-not (neo-term-eshell--exact-environment-p '("SHELL" "SHELL=/bin/sh")))
    (let ((eshell-current-handles (eshell-create-handles t 'insert))
          (process-environment '("SHELL")))
      (should-not (neo-term-eshell--eligible-p "/bin/printf" nil)))
    (should-not admissions)))

(ert-deftest neo-term-eshell-settlement-retains-status-and-snapshot-without-parent-effects ()
  (neo-term-eshell-test--with-shell
    (let ((terminal (neo-term-exec "/bin/printf" '("literal") "/" '("SHELL=/bin/sh"))))
      (unwind-protect
          (let ((id (buffer-local-value 'neo-term--id terminal)))
            (with-current-buffer parent
              (setq-local default-directory "/parent/"))
            (neo-term--handle-settled id 17 nil t nil nil)
            (should (= (length destroyed) 1))
            (with-current-buffer terminal
              (should-not neo-term--id)
              (should (= (plist-get neo-term--completion :exit-code) 17))
              (should (string-match-p "FINAL_NATIVE_SNAPSHOT" (buffer-string))))
            (neo-term--handle-settled id 0 nil t nil nil)
            (should (= (length destroyed) 1))
            (should (equal (buffer-local-value 'default-directory parent) "/parent/"))
            (should (eq (window-buffer (selected-window)) terminal)))
        (when (buffer-live-p terminal) (kill-buffer terminal))))))

(ert-deftest neo-term-eshell-native-success-handback-does-not-touch-prompt-or-cwd ()
  (neo-term-eshell-test--with-shell
    (let* ((before (buffer-string))
           (origin (list :buffer parent :window (selected-window)
                         :epoch neo-term-eshell--epoch :destroy t))
           (terminal (save-current-buffer
                       (neo-term-exec "/bin/printf" nil "/"
                                      '("SHELL=/bin/sh") origin)))
           (id (buffer-local-value 'neo-term--id terminal)))
      (neo-term--handle-directory-changed id "/child/")
      (should (equal (buffer-local-value 'default-directory parent) "/"))
      (neo-term--handle-settled id 0 nil t nil nil)
      (should-not (buffer-live-p terminal))
      (should (eq (window-buffer (selected-window)) parent))
      (should (equal (buffer-string) before))
      (should (equal default-directory "/")))))

(ert-deftest neo-term-eshell-user-navigation-or-revoked-lease-refuses-handback ()
  (neo-term-eshell-test--with-shell
    (dolist (revoke '(navigate disable mode-replace))
      (switch-to-buffer parent)
      (unless (eq major-mode 'eshell-mode) (eshell-mode))
      (neo-term-eshell-mode 1)
      (let* ((origin (list :buffer parent :window (selected-window)
                           :epoch neo-term-eshell--epoch :destroy t))
             (terminal (neo-term-exec "/bin/printf" nil "/" '("SHELL=/bin/sh") origin))
             (id (buffer-local-value 'neo-term--id terminal)))
        (unwind-protect
            (progn
              (pcase revoke
                ('navigate (switch-to-buffer parent))
                ('disable (with-current-buffer parent (neo-term-eshell-mode -1)))
                ('mode-replace (with-current-buffer parent (fundamental-mode))))
              (let ((selected (window-buffer (selected-window))))
                (neo-term--handle-settled id 0 nil t nil nil)
                (should (buffer-live-p terminal))
                (should (eq (window-buffer (selected-window)) selected))))
          (when (buffer-live-p terminal) (kill-buffer terminal)))))))

(ert-deftest neo-term-eshell-signal-or-read-failure-never-success-handback ()
  (neo-term-eshell-test--with-shell
    (dolist (completion '((nil "Terminated" t nil nil)
                          (0 nil nil nil "read failed")
                          (nil nil t "wait failed" nil)))
      (switch-to-buffer parent)
      (let* ((origin (list :buffer parent :window (selected-window)
                           :epoch neo-term-eshell--epoch :destroy t))
             (terminal (neo-term-exec "/bin/printf" nil "/" '("SHELL=/bin/sh") origin))
             (id (buffer-local-value 'neo-term--id terminal)))
        (unwind-protect
            (progn
              (apply #'neo-term--handle-settled id completion)
              (should (buffer-live-p terminal))
              (should (eq (window-buffer (selected-window)) terminal)))
          (kill-buffer terminal))))))

(ert-deftest neo-term-eshell-stale-settlement-cannot-edit-same-buffer-successor ()
  (neo-term-eshell-test--with-shell
    (let* ((terminal (neo-term-exec "/bin/printf" nil "/" '("SHELL=/bin/sh")))
           (id (buffer-local-value 'neo-term--id terminal)))
      (unwind-protect
          (cl-letf (((symbol-function 'neomacs-terminal-get-text)
                     (lambda (_id)
                       (with-current-buffer terminal
                         (neo-term-mode)
                         (setq-local neo-term--id 999)
                         (let ((inhibit-read-only t)) (insert "SUCCESSOR")))
                       "STALE")))
            (neo-term--handle-settled id 0 nil t nil nil)
            (should (= (buffer-local-value 'neo-term--id terminal) 999))
            (should (equal (with-current-buffer terminal (buffer-string)) "SUCCESSOR")))
        (when (buffer-live-p terminal) (kill-buffer terminal))))))

(ert-deftest neo-term-eshell-bare-visual-name-uses-canonical-local-path-resolution ()
  (neo-term-eshell-test--with-shell
    (setq-local eshell-path-env-list '("/bin" "/usr/bin"))
    (should-not (eshell-eval-command (eshell-parse-command "printf '%s' 'space λ'")))
    (should (= (length admissions) 1))
    (should (equal (nth 2 (car admissions)) "/bin/printf"))
    (should (equal (nth 3 (car admissions)) '("%s" "space λ")))
    (with-current-buffer (nth 6 (car admissions))
      (should (eq (lookup-key (current-local-map) (kbd "C-g")) #'keyboard-quit))
      (should (eq (lookup-key (current-local-map) (kbd "C-c C-c")) #'neo-term-send-ctrl-c)))))

(ert-deftest neo-term-eshell-relative-or-handler-path-does-not-grant-native-admission ()
  (neo-term-eshell-test--with-shell
    (let ((eshell-current-handles (eshell-create-handles t 'insert)))
      (dolist (paths '((".") ("/ssh:host:/bin") ("/safe/../bin")))
        (let ((eshell-path-env-list paths))
          (should-not (neo-term-eshell--eligible-p "printf" nil))))
      (let ((eshell-path-env-list '("/bin"))
            (file-name-handler-alist '(("printf" . must-not-call))))
        (should-not (neo-term-eshell--eligible-p "printf" nil))))
    (should-not admissions)))

(ert-deftest neo-term-eshell-reservation-reentry-retires-only-new-id-not-buffer-successor ()
  (neo-term-eshell-test--with-shell
    (let (terminal)
      (unwind-protect
          (cl-letf (((symbol-function 'neomacs-terminal-spawn)
                     (lambda (&rest _args)
                       (setq terminal (current-buffer))
                       (neo-term-mode)
                       (setq-local neo-term--id 999)
                       (let ((inhibit-read-only t)) (insert "SUCCESSOR"))
                       888)))
            (should-error (neo-term-exec "/bin/printf" nil "/" '("SHELL=/bin/sh")))
            (should (buffer-live-p terminal))
            (should (equal destroyed '(888)))
            (should (= (buffer-local-value 'neo-term--id terminal) 999))
            (should (equal (with-current-buffer terminal (buffer-string)) "SUCCESSOR")))
        (when (buffer-live-p terminal) (kill-buffer terminal))))))

(ert-deftest neo-term-eshell-procfs-unavailable-selects-canonical-before-reservation ()
  (neo-term-eshell-test--with-shell
    (setq cwd-pinning-supported nil)
    ;; Native-compiled GNU Eshell must observe the process primitive double.
    (let ((native-comp-enable-subr-trampolines t)
          canonical-request)
      ;; Stop only at the canonical native process primitive. Real parser,
      ;; handles, interpreter selection and process-owner setup run unchanged.
      (cl-letf (((symbol-function 'make-process)
                 (lambda (&rest request)
                   (setq canonical-request request)
                   (throw 'canonical-process-entry 'canonical))))
        (should (eq (catch 'canonical-process-entry
                      (eshell-eval-command
                       (eshell-parse-command "/bin/printf '%s' 'canonical λ'")))
                    'canonical)))
      (should (equal (plist-get canonical-request :command)
                     '("/bin/printf" "%s" "canonical λ")))
      (should (eq (plist-get canonical-request :buffer) parent))
      (should-not admissions)
      (should (eq (window-buffer (selected-window)) parent)))))

(ert-deftest neo-term-eshell-procfs-supported-control-reserves-only-native ()
  (neo-term-eshell-test--with-shell
    ;; Native-compiled GNU Eshell must observe the process primitive double.
    (let ((native-comp-enable-subr-trampolines t)
          canonical-request)
      (cl-letf (((symbol-function 'make-process)
                 (lambda (&rest request)
                   (setq canonical-request request)
                   (throw 'canonical-process-entry 'canonical))))
        (should-not (catch 'canonical-process-entry
                      (eshell-eval-command
                       (eshell-parse-command "/bin/printf '%s' 'native λ'")))))
      (should-not canonical-request)
      (should (= (length admissions) 1))
      (should (equal (nth 3 (car admissions)) '("%s" "native λ"))))))

(defun neo-term-eshell-test--leave (kind marker)
  "Leave a production startup hook with KIND and exact MARKER."
  (pcase kind
    ('error (signal 'error (list marker)))
    ('quit (signal 'quit (list marker)))
    ('throw (throw 'startup-unwind marker))))

(defun neo-term-eshell-test--exec-outcome ()
  "Capture the original nonlocal outcome of the real public constructor."
  (catch 'startup-unwind
    (condition-case condition
        (neo-term-exec "/bin/printf" nil "/" '("SHELL=/bin/sh"))
      ((error quit) condition))))

(ert-deftest neo-term-exec-mode-hook-error-quit-and-throw-dispose-owned-startup ()
  (neo-term-eshell-test--with-shell
    (dolist (kind '(error quit throw))
      (switch-to-buffer parent)
      (let* ((window (selected-window))
             (marker (make-symbol "original-startup-condition"))
             (terminal nil)
             (neo-term-mode-hook
              (list (lambda ()
                      (setq terminal (current-buffer))
                      (neo-term-eshell-test--leave kind marker))))
             (outcome (neo-term-eshell-test--exec-outcome)))
        (should (equal outcome (if (eq kind 'throw) marker (list kind marker))))
        (should terminal)
        (should-not (buffer-live-p terminal))
        (should (eq (selected-window) window))
        (should (eq (window-buffer window) parent))
        (should-not admissions)
        (should-not destroyed)))))

(ert-deftest neo-term-exec-mode-hook-roundtrip-preserves-successor-on-all-exits ()
  (neo-term-eshell-test--with-shell
    (dolist (kind '(error quit throw))
      (switch-to-buffer parent)
      (let* ((window (selected-window))
             (marker (make-symbol "original-successor-condition"))
             (terminal nil)
             (neo-term-mode-hook
              (list (lambda ()
                      (setq terminal (current-buffer))
                      ;; Same object and restored major mode are insufficient:
                      ;; this is a different occurrence, even with no child ID.
                      (let ((neo-term-mode-hook nil))
                        (fundamental-mode)
                        (neo-term-mode))
                      (let ((inhibit-read-only t)) (insert "SUCCESSOR"))
                      (neo-term-eshell-test--leave kind marker)))))
        (unwind-protect
            (let ((outcome (neo-term-eshell-test--exec-outcome)))
              (should (equal outcome (if (eq kind 'throw) marker (list kind marker))))
              (should (buffer-live-p terminal))
              (should (eq (window-buffer window) terminal))
              (should (eq (buffer-local-value 'major-mode terminal) 'neo-term-mode))
              (should-not (buffer-local-value 'neo-term--invocation-lease terminal))
              (should (equal (with-current-buffer terminal (buffer-string)) "SUCCESSOR"))
              (should-not admissions)
              (should-not destroyed))
          (when (buffer-live-p terminal) (kill-buffer terminal)))))))

(ert-deftest neo-term-exec-mode-hook-replaces-lease-without-id-preserves-successor ()
  (neo-term-eshell-test--with-shell
    (let* ((successor (make-symbol "successor-occurrence"))
           (terminal nil)
           (neo-term-mode-hook
            (list (lambda ()
                    (setq terminal (current-buffer))
                    (setq-local neo-term--invocation-lease successor)
                    (let ((inhibit-read-only t)) (insert "SUCCESSOR"))))))
      (unwind-protect
          (progn
            (should (eq (car (neo-term-eshell-test--exec-outcome)) 'error))
            (should (buffer-live-p terminal))
            (should (eq (buffer-local-value 'neo-term--invocation-lease terminal) successor))
            (should (eq (window-buffer (selected-window)) terminal))
            (should (equal (with-current-buffer terminal (buffer-string)) "SUCCESSOR"))
            (should-not admissions))
        (when (buffer-live-p terminal) (kill-buffer terminal))))))

(ert-deftest neo-term-exec-mode-hook-navigation-preserves-selected-user-destination ()
  (neo-term-eshell-test--with-shell
    (dolist (kind '(error quit throw return))
      (dolist (navigation '(same-window other-window))
        (switch-to-buffer parent)
        (let* ((invoking (selected-window))
               (destination (generate-new-buffer " *startup-user-destination*"))
               (other (when (eq navigation 'other-window) (split-window)))
               (marker (make-symbol "navigation-condition"))
               (terminal nil)
               (neo-term-mode-hook
                (list (lambda ()
                        (setq terminal (current-buffer))
                        (when other (select-window other))
                        (switch-to-buffer destination)
                        (unless (eq kind 'return)
                          (neo-term-eshell-test--leave kind marker))))))
          (unwind-protect
              (let ((outcome (neo-term-eshell-test--exec-outcome)))
                (should (equal outcome
                               (pcase kind
                                 ('throw marker)
                                 ('return '(error "Native terminal buffer changed during mode hooks"))
                                 (_ (list kind marker)))))
                (should-not (buffer-live-p terminal))
                (should (eq (selected-window) (or other invoking)))
                (should (eq (window-buffer (selected-window)) destination))
                (should (buffer-live-p destination))
                (should-not admissions))
            (when (and other (window-live-p other)) (delete-window other))
            (kill-buffer destination)))))))

(ert-deftest neo-term-exec-startup-unwind-does-not-run-fallible-disposal-hooks ()
  (neo-term-eshell-test--with-shell
    (dolist (kind '(error quit throw))
      (switch-to-buffer parent)
      (let* ((marker (make-symbol "original-cleanup-condition"))
             (cleanup-calls 0)
             (terminal nil)
             (bad-cleanup (lambda (&rest _args)
                            (cl-incf cleanup-calls)
                            (throw 'startup-unwind 'wrong-cleanup-outcome)))
             (neo-term-mode-hook
              (list (lambda ()
                      (setq terminal (current-buffer))
                      (setq-local kill-buffer-hook (list bad-cleanup))
                      (setq-local kill-buffer-query-functions (list bad-cleanup))
                      (setq-local buffer-list-update-hook (list bad-cleanup))
                      (neo-term-eshell-test--leave kind marker)))))
        (setq-local window-scroll-functions (list bad-cleanup))
        (unwind-protect
            (let ((outcome (neo-term-eshell-test--exec-outcome)))
              (should (equal outcome (if (eq kind 'throw) marker (list kind marker))))
              (should (= cleanup-calls 0))
              (should-not (buffer-live-p terminal))
              (should (eq (window-buffer (selected-window)) parent))
              (should-not admissions))
          (with-current-buffer parent (setq-local window-scroll-functions nil)))))))

(ert-deftest neo-term-exec-other-window-unwind-with-escaping-parent-scroll ()
  (neo-term-eshell-test--with-shell
    (dolist (kind '(error quit throw))
      (dolist (scroll-kind '(error quit throw))
        (dolist (ownership '(owned successor))
          (switch-to-buffer parent)
          (let* ((invoking (selected-window))
                 (other (split-window))
                 (destination (generate-new-buffer " *startup-scroll-destination*"))
                 (marker (make-symbol "original-startup-outcome"))
                 (scroll-marker (make-symbol "escaping-parent-scroll"))
                 (successor (make-symbol "successor-startup-occurrence"))
                 (terminal nil)
                 (scroll-calls 0)
                 (scroll-hooks
                  (list (lambda (&rest _args)
                          (cl-incf scroll-calls)
                          (neo-term-eshell-test--leave scroll-kind scroll-marker))))
                 (neo-term-mode-hook
                  (list (lambda ()
                          (setq terminal (current-buffer))
                          (when (eq ownership 'successor)
                            ;; The same buffer/mode with no native ID belongs
                            ;; to a new occurrence, not this failed startup.
                            (setq-local neo-term--invocation-lease successor)
                            (let ((inhibit-read-only t)) (insert "SUCCESSOR")))
                          (select-window other)
                          (switch-to-buffer destination)
                          ;; Install after navigation: only cleanup's return
                          ;; to P in W1 can encounter this target-local hook.
                          (with-current-buffer parent
                            (setq-local window-scroll-functions scroll-hooks))
                          (neo-term-eshell-test--leave kind marker)))))
            (unwind-protect
                (let ((outcome (neo-term-eshell-test--exec-outcome)))
                  (should (equal outcome
                                 (if (eq kind 'throw) marker (list kind marker))))
                  (should (eq (if (eq kind 'throw) outcome (cadr outcome)) marker))
                  (should terminal)
                  (should (eq (selected-window) other))
                  (should (eq (window-buffer other) destination))
                  (should (eq (current-buffer) destination))
                  (should (buffer-live-p destination))
                  (should-not admissions)
                  (should-not destroyed)
                  (should (= (hash-table-count neo-term--terminals) 0))
                  (should (= scroll-calls 0))
                  (should (eq (buffer-local-value 'window-scroll-functions parent)
                              scroll-hooks))
                  (if (eq ownership 'owned)
                      (progn
                        (should-not (buffer-live-p terminal))
                        (should (eq (window-buffer invoking) parent)))
                    (should (buffer-live-p terminal))
                    (should (eq (window-buffer invoking) terminal))
                    (should (eq (buffer-local-value 'neo-term--invocation-lease terminal)
                                successor))
                    (should-not (buffer-local-value 'neo-term--id terminal))
                    (should (eq (buffer-local-value 'major-mode terminal) 'neo-term-mode))
                    (should (equal (with-current-buffer terminal (buffer-string))
                                   "SUCCESSOR")))
                  ;; Positive control: the actual native display primitive
                  ;; still invokes P's hook from a foreign current buffer.
                  ;; Cleanup must not install a general suppression policy.
                  (let ((scroll-outcome
                         (catch 'startup-unwind
                           (condition-case condition
                               (set-window-buffer invoking parent)
                             ((error quit) condition)))))
                    (should (equal scroll-outcome
                                   (if (eq scroll-kind 'throw)
                                       scroll-marker
                                     (list scroll-kind scroll-marker))))
                    (should (eq (if (eq scroll-kind 'throw)
                                    scroll-outcome
                                  (cadr scroll-outcome))
                                scroll-marker)))
                  (should (= scroll-calls 1))
                  (should (eq (selected-window) other))
                  (should (eq (window-buffer other) destination)))
              (with-current-buffer parent (setq-local window-scroll-functions nil))
              (when (buffer-live-p terminal) (kill-buffer terminal))
              (when (window-live-p other) (delete-window other))
              (kill-buffer destination))))))))

(ert-deftest neo-term-exec-mode-hook-unchanged-owner-control-reserves-once ()
  (neo-term-eshell-test--with-shell
    (let* ((seen nil)
           (neo-term-mode-hook (list (lambda () (setq seen (current-buffer)))))
           (terminal (neo-term-exec "/bin/printf" nil "/" '("SHELL=/bin/sh"))))
      (should (eq seen terminal))
      (should (buffer-live-p terminal))
      (should (= (length admissions) 1))
      (should (eq (nth 6 (car admissions)) terminal))
      (should (buffer-local-value 'neo-term--invocation-lease terminal))
      (should (eq (window-buffer (selected-window)) terminal)))))

(provide 'neo-term-eshell-test)
;;; neo-term-eshell-test.el ends here
