;;; neomacs-companion-receipts.el --- Companion edit receipts -*- lexical-binding: t; -*-

;;; Commentary:
;; Add operation readback to the accepted companion, not another ownership
;; framework.  Process-local, bounded, no eviction and no automatic replay.
;; Arbitrary trusted eval is outside this cooperative receipt contract.

;;; Code:

(require 'neomacs-companion)

(defconst neomacs-companion-receipts-limit 256
  "Maximum admitted operation IDs per editor process; never evict them.")
(defconst neomacs-companion-receipts-text-limit 65536
  "Maximum characters retained in an operation's literal payload.")
(defvar neomacs-companion-receipts--operations (make-hash-table :test #'equal))

(defun neomacs-companion-receipts--copy (value)
  "Copy plain receipt VALUE, including its mutable strings."
  (cond ((stringp value) (substring-no-properties value))
        ((consp value)
         (cons (neomacs-companion-receipts--copy (car value))
               (neomacs-companion-receipts--copy (cdr value))))
        (t value)))

(defun neomacs-companion-receipts--check-id (operation-id)
  "Refuse unless OPERATION-ID is a bounded ASCII identifier."
  (unless (and (stringp operation-id) (<= 1 (length operation-id) 128)
               (string-match-p "\\`[A-Za-z0-9_.:-]+\\'" operation-id))
    (user-error "Invalid companion operation ID")))

(defun neomacs-companion-operation-state (instance operation-id)
  "Return INSTANCE's receipt for OPERATION-ID, or an absent status.
This is historical execution evidence, not a current buffer snapshot.
Readback does not need a live claim and never returns payload text."
  (neomacs-companion--check-instance instance)
  (neomacs-companion-receipts--check-id operation-id)
  (let ((record (gethash operation-id neomacs-companion-receipts--operations)))
    (neomacs-companion-receipts--copy
     (if record (plist-get record :receipt)
       (list :operation-id operation-id :status 'absent)))))

(defun neomacs-companion-edit-once
    (instance operation-id handle tick start end text)
  "Apply one receipt-qualified edit in INSTANCE's companion.
OPERATION-ID identifies the exact HANDLE, TICK, START, END and literal TEXT
payload.  Repeated identical requests return the original receipt without
editing, even after ordinary undo or retirement.  Conflicting reuse refuses.

New IDs consume one of 256 non-evicting slots; TEXT is at most 65536 characters.
HANDLE is at most 256 characters.  Retain plain payloads only in process memory.
Return a plain receipt with :operation-id and :status.  Success includes the
accepted helper's metadata as :result.  Failure includes a condition symbol
as :condition, not an error message or payload.  Error and quit propagate on
first execution, but their receipt is readable afterwards.  Nonlocal exits
leave an indeterminate receipt and must not be replayed.  No waits or I/O.
This covers only the companion edit, not arbitrary owner eval or callbacks."
  (neomacs-companion--check-instance instance)
  (neomacs-companion-receipts--check-id operation-id)
  (unless (and (stringp handle) (<= 1 (length handle) 256)
               (integerp tick) (integerp start)
               (integerp end) (stringp text)
               (<= (length text) neomacs-companion-receipts-text-limit))
    (user-error "Invalid companion operation payload"))
  (let* ((payload (list handle tick start end (substring-no-properties text)))
         (existing (gethash operation-id neomacs-companion-receipts--operations)))
    (if existing
        (progn
          (unless (equal payload (plist-get existing :payload))
            (user-error "Companion operation payload conflict"))
          (neomacs-companion-operation-state instance operation-id))
      (when (>= (hash-table-count neomacs-companion-receipts--operations)
                neomacs-companion-receipts-limit)
        (user-error "Companion receipt capacity reached; no IDs were evicted"))
      (let* ((id (substring-no-properties operation-id))
             (record (list :payload (neomacs-companion-receipts--copy payload)
                           :receipt (list :operation-id id :status 'running)))
             (settled nil))
        (puthash id record neomacs-companion-receipts--operations)
        (unwind-protect
            (condition-case failure
                (let ((result (neomacs-companion-edit
                               instance handle tick start end (nth 4 payload))))
                  (setf (plist-get record :receipt)
                        (list :operation-id id :status 'succeeded :result result))
                  (setq settled t)
                  (neomacs-companion-operation-state instance id))
              ((error quit)
               (setf (plist-get record :receipt)
                     (list :operation-id id :status 'failed
                           :condition (car failure)))
               (setq settled t)
               (signal (car failure) (cdr failure))))
          (unless settled
            (setf (plist-get record :receipt)
                  (list :operation-id id :status 'indeterminate))))))))

(provide 'neomacs-companion-receipts)
;;; neomacs-companion-receipts.el ends here
