;;; neomacs-mcp-companion.el --- Cooperative MCP tools -*- lexical-binding: t; -*-

;;; Commentary:
;; Optional tool registration atop the existing companion claim and receipt
;; APIs.  No second ownership system, file editing, display or learner access.

;;; Code:
(require 'neomacs-mcp)
(require 'neomacs-companion-receipts)

(defun neomacs-mcp--companion-instance (arguments)
  "Fence ARGUMENTS, then return the companion's existing native identity."
  (neomacs-mcp--instance arguments)
  (neomacs-companion-identity))

(defun neomacs-mcp--plain-object (plist)
  "Convert the helper's plain PLIST to a JSON object without private payloads."
  (apply #'neomacs-mcp--object
         (cl-loop for (key value) on plist by #'cddr
                  append (list (substring (symbol-name key) 1)
                               (cond ((keywordp (car-safe value)) (neomacs-mcp--plain-object value))
                                     ((symbolp value) (symbol-name value))
                                     (t value))))))

(defun neomacs-mcp--companion-claim (arguments)
  "Claim an undisplayed scratch companion for fenced ARGUMENTS."
  (neomacs-mcp--plain-object
   (neomacs-companion-claim (neomacs-mcp--companion-instance arguments))))

(defun neomacs-mcp--companion-read (arguments)
  "Read the live claim named by ARGUMENTS without moving the study window."
  (let* ((instance (neomacs-mcp--companion-instance arguments))
         (handle (gethash "handle" arguments))
         (state (neomacs-mcp--plain-object (neomacs-companion-state instance handle))))
    (with-current-buffer (neomacs-companion--buffer instance handle)
      (save-restriction
        (widen)
        (when (> (buffer-size) neomacs-companion-receipts-text-limit)
          (user-error "Companion text exceeds read limit"))
        (puthash "text" (buffer-substring-no-properties (point-min) (point-max)) state)))
    state))

(defun neomacs-mcp--companion-edit (arguments)
  "Apply the helper's receipt-qualified edit described by ARGUMENTS."
  (neomacs-mcp--plain-object
   (neomacs-companion-edit-once
    (neomacs-mcp--companion-instance arguments) (gethash "operationId" arguments)
    (gethash "handle" arguments) (gethash "tick" arguments)
    (gethash "start" arguments) (gethash "end" arguments) (gethash "text" arguments))))

(defun neomacs-mcp--companion-receipt (arguments)
  "Read the historical operation receipt named by fenced ARGUMENTS."
  (neomacs-mcp--plain-object
   (neomacs-companion-operation-state
    (neomacs-mcp--companion-instance arguments) (gethash "operationId" arguments))))

(defun neomacs-mcp--companion-undo (arguments)
  "Undo one ordinary group in the exact live claim named by ARGUMENTS.
Require the expected tick; do not auto-retry or rewrite an edit receipt."
  (let* ((instance (neomacs-mcp--companion-instance arguments))
         (handle (gethash "handle" arguments)))
    (with-current-buffer (neomacs-companion--buffer instance handle)
      (unless (= (gethash "tick" arguments) (buffer-modified-tick))
        (user-error "Companion modification tick is stale"))
      (when (or buffer-read-only (eq buffer-undo-list t))
        (user-error "Companion must be writable with undo enabled"))
      (save-excursion
        (save-restriction
          (widen)
          (let ((inhibit-modification-hooks t)) (undo-only 1))))
      (neomacs-mcp--plain-object (neomacs-companion-state instance handle)))))

(defun neomacs-mcp--companion-retire (arguments)
  "Retire the exact claim named by ARGUMENTS without killing its buffer."
  (neomacs-companion-retire (neomacs-mcp--companion-instance arguments)
                            (gethash "handle" arguments))
  (neomacs-mcp--object "retired" t))

;;;###autoload
(defun neomacs-mcp-enable-companion-tools ()
  "Register optional cooperative companion tools without creating any buffers.
Receipt capacity is finite and process-local.  Never imply restart durability."
  (interactive)
  (let ((instance '("instance" . "string"))
        (handle '("handle" . "string"))
        (tick '("tick" . "integer"))
        (operation '("operationId" . "string")))
    (dolist (entry
             (list
              (list "neomacs_companion_claim" "Claim an undisplayed non-file buffer." (list instance) #'neomacs-mcp--companion-claim)
              (list "neomacs_companion_read" "Read a live claimed buffer and modification tick." (list instance handle) #'neomacs-mcp--companion-read)
              (list "neomacs_companion_edit" "Edit once by operation ID and expected tick; 256 non-evicting process-local receipts." (list instance handle operation tick '("start" . "integer") '("end" . "integer") '("text" . "string")) #'neomacs-mcp--companion-edit)
              (list "neomacs_companion_receipt" "Read historical execution evidence before retrying an uncertain edit." (list instance operation) #'neomacs-mcp--companion-receipt)
              (list "neomacs_companion_undo" "Undo one ordinary group at expected tick. No automatic replay or receipt." (list instance handle tick) #'neomacs-mcp--companion-undo)
              (list "neomacs_companion_retire" "Retire a claim without killing or erasing its buffer." (list instance handle) #'neomacs-mcp--companion-retire)))
      (neomacs-mcp-register-tool
       (nth 0 entry) (nth 1 entry)
       (neomacs-mcp--schema (nth 2 entry) (mapcar #'car (nth 2 entry)))
       (nth 3 entry)))))

(provide 'neomacs-mcp-companion)
;;; neomacs-mcp-companion.el ends here
