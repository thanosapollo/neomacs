;;; neomacs-companion.el --- Trusted-local companion -*- lexical-binding: t; -*-

;;; Commentary:
;; Support cooperative owner-controlled companion editing, not a sandbox.
;; All edits are headless.  No buffer/window selection, display, waits or I/O.
;; Modification callbacks are suppressed for this dedicated scratch edit;
;; native read-only checks and ordinary undo remain enabled.

;;; Code:

(defvar neomacs-companion--incarnation nil)
(defvar neomacs-companion--claims (make-hash-table :test #'equal))
(defvar neomacs-companion--serial 0)
(defvar-local neomacs-companion--claim nil)

(defun neomacs-companion-identity ()
  "Return this editor's PID and process-local incarnation.
The incarnation is an accidental-target fence, not an authentication secret."
  (unless neomacs-companion--incarnation
    (setq neomacs-companion--incarnation
          (format "%s-%s-%s" (emacs-pid) (float-time) (random))))
  (list (emacs-pid) (copy-sequence neomacs-companion--incarnation)))

(defun neomacs-companion--check-instance (instance)
  "Refuse unless INSTANCE matches this editor's current incarnation."
  (unless (equal instance (neomacs-companion-identity))
    (user-error "Companion editor instance mismatch")))

(defun neomacs-companion--retire ()
  "Irreversibly retire the current buffer's constructor claim."
  (when neomacs-companion--claim
    (remhash neomacs-companion--claim neomacs-companion--claims)
    (setq neomacs-companion--claim nil)))

(defun neomacs-companion--buffer (instance handle)
  "Return the owned buffer for INSTANCE and HANDLE, or refuse."
  (neomacs-companion--check-instance instance)
  (let ((buffer (gethash handle neomacs-companion--claims)))
    (unless (and (buffer-live-p buffer)
                 (with-current-buffer buffer
                   (and (equal handle neomacs-companion--claim)
                        (not buffer-file-name) (not (buffer-base-buffer)))))
      (user-error "Companion claim is absent or retired"))
    buffer))

(defun neomacs-companion-claim (instance)
  "Create a unique non-file companion for INSTANCE without displaying it.
Return metadata with :handle, :buffer and :tick.  Never adopt a namesake."
  (neomacs-companion--check-instance instance)
  (setq neomacs-companion--serial (1+ neomacs-companion--serial))
  (let* ((buffer (generate-new-buffer "*Neomacs companion*"))
         (handle (format "%s:%s" neomacs-companion--incarnation
                         neomacs-companion--serial)))
    (with-current-buffer buffer
      (buffer-enable-undo)
      (setq neomacs-companion--claim handle)
      (add-hook 'change-major-mode-hook #'neomacs-companion--retire nil t)
      (add-hook 'after-set-visited-file-name-hook
                #'neomacs-companion--retire nil t)
      (add-hook 'kill-buffer-hook #'neomacs-companion--retire nil t))
    (puthash handle buffer neomacs-companion--claims)
    (neomacs-companion-state instance handle)))

(defun neomacs-companion-state (instance handle)
  "Return allowlisted metadata for INSTANCE and HANDLE, without buffer text."
  (with-current-buffer (neomacs-companion--buffer instance handle)
    (list :handle (copy-sequence handle) :buffer (buffer-name)
          :tick (buffer-modified-tick))))

(defun neomacs-companion-edit (instance handle tick start end text)
  "Replace a region in the exact companion and return updated metadata.
INSTANCE and HANDLE must still own the non-file buffer.  TICK must equal
`buffer-modified-tick'.  START and END are widened character positions.
Insert TEXT literally, without properties.  Refusal never retries; an error
or quit rolls back the text transaction.  Preserve point and narrowing and
ordinary undo.  Suppress modification callbacks only inside this edit."
  (with-current-buffer (neomacs-companion--buffer instance handle)
    (unless (and (integerp tick) (= tick (buffer-modified-tick)))
      (user-error "Companion modification tick is stale"))
    (when (or buffer-read-only (eq buffer-undo-list t))
      (user-error "Companion must be writable with undo enabled"))
    (save-excursion
      (save-restriction
        (widen)
        (unless (and (integerp start) (integerp end) (stringp text)
                     (<= (point-min) start end (point-max)))
          (user-error "Invalid companion edit region"))
        (let ((literal (substring-no-properties text))
              (inhibit-modification-hooks t))
          (undo-boundary)
          (atomic-change-group
            (delete-region start end)
            (goto-char start)
            (insert literal))
          (undo-boundary))))
    (neomacs-companion-state instance handle)))

(defun neomacs-companion-retire (instance handle)
  "Retire INSTANCE's HANDLE without killing or erasing its buffer."
  (with-current-buffer (neomacs-companion--buffer instance handle)
    (neomacs-companion--retire))
  t)

(provide 'neomacs-companion)
;;; neomacs-companion.el ends here
