;;; neomacs-companion-tests.el --- Disposable proof -*- lexical-binding: t; -*-
(require 'ert)
(require 'cl-lib)
(require 'neomacs-companion)

(defmacro companion-test-with-claim (&rest body)
  (declare (indent 0) (debug t))
  `(let* ((neomacs-companion--incarnation nil)
          (neomacs-companion--claims (make-hash-table :test #'equal))
          (instance (neomacs-companion-identity))
          (receipt (neomacs-companion-claim instance))
          (handle (plist-get receipt :handle))
          (companion (get-buffer (plist-get receipt :buffer))))
     (unwind-protect (progn ,@body)
       (when (buffer-live-p companion) (kill-buffer companion)))))

(defun companion-test-snapshot (buffer)
  "Return a private snapshot of synthetic BUFFER only."
  (with-current-buffer buffer
    (list (point) (point-min) (point-max) (buffer-modified-tick)
          (buffer-modified-p) (copy-tree buffer-undo-list)
          (save-restriction (widen) (buffer-substring (point-min) (point-max))))))

(ert-deftest companion-unique-nonfile-no-adoption ()
  (let ((namesake (generate-new-buffer "*Neomacs companion*")))
    (unwind-protect
        (with-current-buffer namesake
          (insert "Human namesake")
          (let ((before (companion-test-snapshot namesake)))
            (companion-test-with-claim
              (should-not (eq companion namesake))
              (should-not (buffer-file-name companion))
              (should-not (buffer-base-buffer companion))
              (should (equal before (companion-test-snapshot namesake))))))
      (kill-buffer namesake))))

(ert-deftest companion-instance-and-incarnation-refuse ()
  (companion-test-with-claim
    (let ((before (companion-test-snapshot companion)))
      (dolist (wrong (list (list (1+ (car instance)) (cadr instance))
                          (list (car instance) "retired-incarnation")))
        (should-error (neomacs-companion-claim wrong) :type 'user-error)
        (should-error (neomacs-companion-edit wrong handle
                                             (plist-get receipt :tick) 1 1 "x")
                      :type 'user-error))
      (should (= 1 (hash-table-count neomacs-companion--claims)))
      (should (equal before (companion-test-snapshot companion))))))

(ert-deftest companion-atomic-edit-normal-undo ()
  (companion-test-with-claim
    (with-current-buffer companion
      (insert "prefix αβ suffix") (undo-boundary)
      (narrow-to-region 8 10) (goto-char 9)
      (let* ((old (buffer-modified-tick))
             (result (neomacs-companion-edit instance handle old 8 10 "γδ")))
        (should (> (plist-get result :tick) old))
        (should (= (point) 8)) ; deletion rebases the native saved marker
        (should (= (point-min) 8))
        (should (= (point-max) 10))
        (should (equal (buffer-string) "γδ"))
        (widen)
        (undo-only 1)
        (should (equal (buffer-string) "prefix αβ suffix"))))))

(ert-deftest companion-stale-tick-and-property-edit-refuse ()
  (companion-test-with-claim
    (with-current-buffer companion
      (insert "abc")
      (let ((tick (buffer-modified-tick)))
        (put-text-property 1 2 'face 'bold)
        (let ((before (companion-test-snapshot companion)))
          (should-error (neomacs-companion-edit instance handle tick 1 4 "no")
                        :type 'user-error)
          (should (equal before (companion-test-snapshot companion))))))))

(ert-deftest companion-retirement-is-irreversible ()
  (companion-test-with-claim
    (with-current-buffer companion (insert "Retain this draft"))
    (let ((before (companion-test-snapshot companion)))
      (should (neomacs-companion-retire instance handle))
      (should-error (neomacs-companion-state instance handle) :type 'user-error)
      (should-error (neomacs-companion-edit instance handle
                                           (plist-get receipt :tick) 1 1 "no")
                    :type 'user-error)
      (should (equal before (companion-test-snapshot companion))))))

(ert-deftest companion-native-file-association-and-detachment-retire ()
  (companion-test-with-claim
    (with-current-buffer companion
      (set-visited-file-name (expand-file-name "never-written" default-directory)
                             t)
      (set-visited-file-name nil t)
      (insert "Human detached notes"))
    (let ((before (companion-test-snapshot companion)))
      (should-error (neomacs-companion-state instance handle) :type 'user-error)
      (should (equal before (companion-test-snapshot companion))))))

(ert-deftest companion-mode-roundtrip-and-kill-retire ()
  (companion-test-with-claim
    (with-current-buffer companion (text-mode) (fundamental-mode))
    (should-error (neomacs-companion-state instance handle) :type 'user-error))
  (companion-test-with-claim
    (kill-buffer companion)
    (should-error (neomacs-companion-state instance handle) :type 'user-error)))

(ert-deftest companion-rename-does-not-redirect-to-namesake ()
  (companion-test-with-claim
    (let ((old-name (buffer-name companion)) (replacement nil))
      (unwind-protect
          (progn
            (with-current-buffer companion (rename-buffer "*renamed companion*" t))
            (setq replacement (generate-new-buffer old-name))
            (with-current-buffer replacement (insert "Human replacement"))
            (neomacs-companion-edit instance handle (plist-get receipt :tick)
                                    1 1 "Owned text")
            (should (equal (with-current-buffer companion (buffer-string))
                           "Owned text"))
            (should (equal (with-current-buffer replacement (buffer-string))
                           "Human replacement")))
        (when replacement (kill-buffer replacement))))))

(ert-deftest companion-error-and-quit-rollback-text-and-undo ()
  ;; Native read-only checking faults after deletion, including when bytecode
  ;; uses the insert opcode.  Observe before unwind; do not mock insert or
  ;; transaction cleanup.  The error control lets the original signal through.
  (dolist (condition '(error quit))
    (companion-test-with-claim
      (with-current-buffer companion
        (insert "abc") (undo-boundary)
        (insert "def")
        (put-text-property 1 2 'read-only t)
        (undo-boundary)
        (let ((before (list (buffer-string) (copy-tree buffer-undo-list)))
              (caught nil)
              (at-fault nil)
              (native-fault nil)
              (fault-count 0)
              (quit-injected nil)
              (debug-on-error nil)
              (debug-on-quit nil))
          (let ((signal-hook-function
                 (lambda (symbol data)
                   (when (and (eq (current-buffer) companion)
                              (eq symbol 'text-read-only))
                     (setq fault-count (1+ fault-count)
                           native-fault (cons symbol data)
                           at-fault (list (buffer-string)
                                          (copy-tree buffer-undo-list)))
                     (when (and (eq condition 'quit) (not quit-injected))
                       (setq quit-injected t)
                       ;; One shot: rollback and the injected quit must not
                       ;; reenter the observer or fault native repair.
                       (let ((signal-hook-function nil))
                         (signal 'quit '(companion-test-fault))))))))
            (condition-case failure
                (neomacs-companion-edit instance handle
                                       (buffer-modified-tick) 2 5 "FAIL")
              ((error quit) (setq caught failure)))
            ;; Assertions stay outside the signal observer/production handler.
            (should (= fault-count 1))
            (should (equal native-fault '(text-read-only)))
            (should (equal (substring-no-properties (car at-fault)) "aef"))
            (should-not (equal (cadr before) (cadr at-fault)))
            (should (eq quit-injected (eq condition 'quit)))
            (should (equal caught (if (eq condition 'quit)
                                     '(quit companion-test-fault)
                                   '(text-read-only))))
            (should (equal before (list (buffer-string)
                                       (copy-tree buffer-undo-list))))
            (should (equal-including-properties (car before) (buffer-string)))
            ;; Paired no-fault positive: the same public edit and still-armed
            ;; observer succeed when native read-only checking is inhibited.
            ;; This also witnesses recovery of the claim and ordinary undo.
            (let* ((inhibit-read-only t)
                   (tick (buffer-modified-tick))
                   (result (neomacs-companion-edit instance handle tick
                                                  2 5 "RECOVER")))
              (should (> (plist-get result :tick) tick))
              (should (equal (buffer-substring-no-properties
                              (point-min) (point-max)) "aRECOVERef"))
              (should (= fault-count 1))
              (undo-only 1)
              (should (equal-including-properties (car before) (buffer-string))))))))))

(ert-deftest companion-native-readonly-fault-rolls-back-partial-edit ()
  (companion-test-with-claim
    (with-current-buffer companion
      (insert "abcdef")
      ;; Deletion is allowed; insertion inherits the preceding read-only field.
      (put-text-property 1 2 'read-only t)
      (undo-boundary)
      (let ((before (list (buffer-string) (copy-tree buffer-undo-list))))
        (should-error (neomacs-companion-edit instance handle
                                             (buffer-modified-tick) 2 5 "X")
                      :type 'text-read-only)
        (should (equal before (list (buffer-string)
                                    (copy-tree buffer-undo-list))))
        (should (equal-including-properties (car before) (buffer-string)))))))

(ert-deftest companion-bounds-readonly-undo-disabled-refuse ()
  (companion-test-with-claim
    (dolist (range '((0 1) (2 1) (1 2) (1 "1")))
      (should-error (neomacs-companion-edit instance handle
                                           (plist-get receipt :tick)
                                           (car range) (cadr range) "x")
                    :type 'user-error))
    (with-current-buffer companion
      (let ((buffer-read-only t))
        (should-error (neomacs-companion-edit instance handle
                                             (plist-get receipt :tick) 1 1 "x")
                      :type 'user-error))
      (let ((buffer-undo-list t))
        (should-error (neomacs-companion-edit instance handle
                                             (plist-get receipt :tick) 1 1 "x")
                      :type 'user-error)))))

(ert-deftest companion-headless-human-preservation ()
  (save-window-excursion
    (with-temp-buffer
      (switch-to-buffer (current-buffer)) (delete-other-windows)
      (buffer-enable-undo)
      (insert (make-string 2000 ?λ)) (undo-boundary)
      (narrow-to-region 50 1500)
      (let* ((human (current-buffer))
             (first (selected-window)) (second (split-window-right)))
        (set-window-point first 100) (set-window-point second 500)
        (let ((before (companion-test-snapshot human))
              (start (window-start first))
              (undo-root buffer-undo-list)
              (calls 0))
          (companion-test-with-claim
            (with-current-buffer companion
              (add-hook 'before-change-functions
                        (lambda (&rest _) (cl-incf calls)) nil t)
              (add-hook 'after-change-functions
                        (lambda (&rest _) (cl-incf calls)) nil t))
            (neomacs-companion-edit instance handle
                                    (plist-get receipt :tick) 1 1 "Agent αβ work")
            (should-error (neomacs-companion-edit instance handle
                                                 (plist-get receipt :tick)
                                                 1 1 "stale") :type 'user-error)
            (neomacs-companion-retire instance handle)
            (should (= calls 0)))
          (should (eq (selected-window) first))
          (should (eq (current-buffer) human))
          (should (eq (window-buffer first) human))
          (should (eq (window-buffer second) human))
          (should (= (window-point second) 500))
          (should (= start (window-start first)))
          (should (eq undo-root buffer-undo-list))
          (should (equal before (companion-test-snapshot human))))))))

(ert-deftest companion-window-hook-precondition ()
  ;; This is a native runtime qualification, NOT a workaround in the helper.
  ;; Installed ab332517 is expected RED; corrected 1b0daaf9 must be GREEN.
  (save-window-excursion
    (with-temp-buffer
      (switch-to-buffer (current-buffer)) (delete-other-windows)
      (insert (make-string 2000 ?λ))
      (let ((first (selected-window)) (second (split-window-right)) (seen nil))
        (set-window-point first 100) (set-window-point second 500)
        (add-hook 'window-configuration-change-hook
                  (lambda ()
                    (push (list (eq (selected-window) first) (point)) seen))
                  nil t)
        (run-window-configuration-change-hook)
        (should (equal (reverse seen) '((t 100) (nil 500))))
        (should (= (window-point first) 100))
        (should (= (window-point second) 500))))))

(ert-deftest companion-owner-eval-is-not-restricted ()
  (with-temp-buffer
    (let ((buffer (current-buffer)))
      (eval `(with-current-buffer ,buffer (insert "Owner eval remains arbitrary")) t)
      (should (equal (buffer-string) "Owner eval remains arbitrary")))))

(provide 'neomacs-companion-tests)
;;; neomacs-companion-tests.el ends here
