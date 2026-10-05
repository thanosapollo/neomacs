;;; neomacs-companion-receipts-tests.el --- Receipt journeys -*- lexical-binding: t; -*-

(require 'neomacs-companion-tests)
(require 'neomacs-companion-receipts)

(defmacro companion-receipt-with-claim (&rest body)
  (declare (indent 0) (debug t))
  `(let ((neomacs-companion-receipts--operations (make-hash-table :test #'equal)))
     (companion-test-with-claim ,@body)))

(ert-deftest companion-receipt-lost-reply-readback-no-duplicate ()
  (companion-receipt-with-claim
    (let ((tick (plist-get receipt :tick)))
      ;; Deliberately discard the reply after the editor completed its edit.
      (neomacs-companion-edit-once instance "lost-1" handle tick 1 1 "αβ code")
      (let ((readback (neomacs-companion-operation-state instance "lost-1"))
            (before (companion-test-snapshot companion)))
        (should (eq (plist-get readback :status) 'succeeded))
        (should (equal readback (neomacs-companion-edit-once
                                instance "lost-1" handle tick 1 1 "αβ code")))
        (should (equal before (companion-test-snapshot companion)))
        (should (equal "αβ code" (with-current-buffer companion (buffer-string))))))))

(ert-deftest companion-receipt-tick-conflict-is-stable ()
  (companion-receipt-with-claim
    (let ((tick (plist-get receipt :tick)))
      (with-current-buffer companion (insert "Human adjustment"))
      (let ((before (companion-test-snapshot companion)))
        (should-error (neomacs-companion-edit-once instance "stale" handle tick
                                                  1 1 "no") :type 'user-error)
        (let ((failed (neomacs-companion-operation-state instance "stale")))
          (should (eq (plist-get failed :status) 'failed))
          (should (eq (plist-get failed :condition) 'user-error))
          (should (equal failed (neomacs-companion-edit-once
                                instance "stale" handle tick 1 1 "no"))))
        (should (equal before (companion-test-snapshot companion)))))))

(ert-deftest companion-receipt-payload-conflict-and-wrong-instance ()
  (companion-receipt-with-claim
    (let ((tick (plist-get receipt :tick)))
      (neomacs-companion-edit-once instance "same-id" handle tick 1 1 "yes")
      (let ((before (companion-test-snapshot companion)))
        (dolist (payload (list (list "other-handle" tick 1 1 "yes")
                              (list handle (1+ tick) 1 1 "yes")
                              (list handle tick 2 2 "yes")
                              (list handle tick 1 2 "yes")
                              (list handle tick 1 1 "different")))
          (should-error (apply #'neomacs-companion-edit-once
                               instance "same-id" payload) :type 'user-error))
        (dolist (wrong (list (list (1+ (car instance)) (cadr instance))
                            (list (car instance) "old-incarnation")))
          (should-error (neomacs-companion-operation-state wrong "same-id")
                        :type 'user-error)
          (should-error (neomacs-companion-edit-once wrong "same-id" handle
                                                    tick 1 1 "yes")
                        :type 'user-error))
        (should (equal before (companion-test-snapshot companion)))))))

(ert-deftest companion-receipt-success-survives-undo-and-retirement ()
  (companion-receipt-with-claim
    (let* ((tick (plist-get receipt :tick))
           (success (neomacs-companion-edit-once instance "undo-1" handle tick
                                                 1 1 "Agent draft")))
      (with-current-buffer companion
        (undo-only 1)
        (should (equal "" (buffer-string))))
      (neomacs-companion-retire instance handle)
      (let ((before (companion-test-snapshot companion)))
        (should (equal success (neomacs-companion-operation-state instance "undo-1")))
        (should (equal success (neomacs-companion-edit-once instance "undo-1"
                                                          handle tick 1 1 "Agent draft")))
        (should (equal before (companion-test-snapshot companion)))))))

(ert-deftest companion-receipt-native-partial-error-atomic-rollback ()
  (companion-receipt-with-claim
    (with-current-buffer companion
      (insert "abcdef")
      (put-text-property 1 2 'read-only t)
      (undo-boundary)
      (let ((before (list (buffer-string) (copy-tree buffer-undo-list)))
            (tick (buffer-modified-tick)))
        (should-error (neomacs-companion-edit-once instance "rollback" handle
                                                  tick 2 5 "X") :type 'text-read-only)
        (should (equal before (list (buffer-string) (copy-tree buffer-undo-list))))
        (let ((failed (neomacs-companion-operation-state instance "rollback")))
          (should (eq (plist-get failed :status) 'failed))
          (should (eq (plist-get failed :condition) 'text-read-only))
          (should (equal failed (neomacs-companion-edit-once instance "rollback"
                                                           handle tick 2 5 "X"))))
        (should (equal before (list (buffer-string) (copy-tree buffer-undo-list))))))))

(ert-deftest companion-receipt-error-quit-receipts-propagate ()
  ;; Inject into the boundary function, so compiled insert opcodes do not
  ;; invalidate this receipt test.  Atomic text rollback is tested separately.
  (dolist (condition '(error quit))
    (companion-receipt-with-claim
      (let ((caught nil) (before (companion-test-snapshot companion)))
        (cl-letf (((symbol-function 'neomacs-companion-edit)
                   (lambda (&rest _) (signal condition nil))))
          (condition-case failure
              (neomacs-companion-edit-once instance "failure" handle
                                           (plist-get receipt :tick) 1 1 "no")
            ((error quit) (setq caught (car failure)))))
        (should (eq caught condition))
        (should (eq (plist-get (neomacs-companion-operation-state
                               instance "failure") :condition) condition))
        (should (equal before (companion-test-snapshot companion)))))))

(ert-deftest companion-receipt-running-and-nonlocal-exit-no-replay ()
  (companion-receipt-with-claim
    (let ((tick (plist-get receipt :tick)) (seen nil) (calls 0))
      (cl-letf (((symbol-function 'neomacs-companion-edit)
                 (lambda (&rest _)
                   (cl-incf calls)
                   (setq seen (neomacs-companion-edit-once
                               instance "throw" handle tick 1 1 "x"))
                   (throw 'receipt-test 'escaped))))
        (should (eq (catch 'receipt-test
                      (neomacs-companion-edit-once instance "throw" handle
                                                   tick 1 1 "x")) 'escaped))
        (should (eq (plist-get seen :status) 'running))
        (should (eq (plist-get (neomacs-companion-edit-once
                               instance "throw" handle tick 1 1 "x") :status)
                    'indeterminate))
        (should (= calls 1))))))

(ert-deftest companion-receipt-defensive-copy-and-literal-equivalence ()
  (companion-receipt-with-claim
    (let* ((tick (plist-get receipt :tick))
           (id (propertize "literal" 'fixture 'id-properties))
           (text (propertize "aβ" 'face 'bold))
           (success (neomacs-companion-edit-once instance id handle tick 1 1 text)))
      (aset id 0 ?X)
      (aset text 0 ?X)
      (aset (plist-get success :operation-id) 0 ?X)
      (aset (plist-get (plist-get success :result) :handle) 0 ?X)
      (setf (plist-get success :status) 'corrupted)
      (let ((readback (neomacs-companion-edit-once
                       instance "literal" handle tick 1 1 "aβ")))
        (should (eq (plist-get readback :status) 'succeeded))
        (should-not (text-properties-at 0 (plist-get readback :operation-id)))
        (should (equal (plist-get (plist-get readback :result) :handle) handle)))
      (with-current-buffer companion
        (should (equal (buffer-string) "aβ"))
        (should-not (text-properties-at 1))))))

(ert-deftest companion-receipt-bounded-capacity-no-eviction ()
  (companion-receipt-with-claim
    (let ((neomacs-companion-receipts-limit 2)
          (tick (plist-get receipt :tick)))
      (neomacs-companion-edit-once instance "one" handle tick 1 1 "x")
      (should-error (neomacs-companion-edit-once instance "two" handle tick
                                                1 1 "x") :type 'user-error)
      (let ((before (companion-test-snapshot companion)))
        (should-error (neomacs-companion-edit-once instance "three" handle tick
                                                  1 1 "x") :type 'user-error)
        (should (eq (plist-get (neomacs-companion-operation-state instance "one")
                               :status) 'succeeded))
        (should (eq (plist-get (neomacs-companion-operation-state instance "three")
                               :status) 'absent))
        (should (equal before (companion-test-snapshot companion)))))))

(ert-deftest companion-receipt-validation-before-admission ()
  (companion-receipt-with-claim
    (dolist (id (list "" "bad id" "λ" (make-string 129 ?a) 2))
      (should-error (neomacs-companion-edit-once instance id handle
                                                (plist-get receipt :tick) 1 1 "x")
                    :type 'user-error))
    (should-error (neomacs-companion-edit-once instance "long-handle"
                                              (make-string 257 ?h)
                                              (plist-get receipt :tick) 1 1 "x")
                  :type 'user-error)
    (should-error (neomacs-companion-edit-once instance "too-big" handle
                                              (plist-get receipt :tick) 1 1
                                              (make-string 65537 ?x)) :type 'user-error)
    (should (= 0 (hash-table-count neomacs-companion-receipts--operations)))
    (should (equal (neomacs-companion-operation-state instance "unsubmitted")
                   '(:operation-id "unsubmitted" :status absent)))))

(ert-deftest companion-receipt-study-code-coworking-preserves-human ()
  (save-window-excursion
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (delete-other-windows)
      (buffer-enable-undo)
      (insert (make-string 2000 ?λ) "\nUnsent human draft")
      (undo-boundary)
      (narrow-to-region 50 1500)
      (let* ((human (current-buffer))
             (first (selected-window)) (second (split-window-right)))
        (set-window-point first 100)
        (set-window-point second 500)
        (set-window-start first 50 t)
        (set-window-start second 80 t)
        (let ((before (companion-test-snapshot human))
              (undo-root buffer-undo-list)
              (starts (list (window-start first) (window-start second))))
          (companion-receipt-with-claim
            (let* ((tick (plist-get receipt :tick))
                   (success (neomacs-companion-edit-once
                             instance "study-code" handle tick 1 1
                             "Question → explanation → code experiment")))
              (should (equal success (neomacs-companion-operation-state
                                      instance "study-code")))
              (should (equal success (neomacs-companion-edit-once
                                      instance "study-code" handle tick 1 1
                                      "Question → explanation → code experiment")))
              (should-error (neomacs-companion-edit-once instance "stale-next"
                                                        handle tick 1 1 "no")
                            :type 'user-error)
              (with-current-buffer companion (undo-only 1))
              (neomacs-companion-retire instance handle)))
          (should (eq (selected-window) first))
          (should (eq (current-buffer) human))
          (should (eq (window-buffer first) human))
          (should (eq (window-buffer second) human))
          (should (= (window-point first) 100))
          (should (= (window-point second) 500))
          (should (equal starts (list (window-start first) (window-start second))))
          (should (eq undo-root buffer-undo-list))
          (should (equal before (companion-test-snapshot human))))))))

(provide 'neomacs-companion-receipts-tests)
;;; neomacs-companion-receipts-tests.el ends here
