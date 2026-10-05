;;; neo-term-eshell.el --- Opt-in detached native visual commands -*- lexical-binding: t; -*-

;;; Commentary:
;; Enable `neo-term-eshell-mode' in one Eshell buffer after loading this library
;; with M-x load-library RET neo-term-eshell RET. No init-file change is needed.
;; This narrow adapter requires Linux procfs and an explicit SHELL environment
;; binding (portable-pty otherwise invents one). Basename searches require local
;; absolute PATH entries. Unsupported contexts keep ordinary Eshell processing.
;; Only canonical visual
;; commands with proven local, interactive, non-pipeline/non-background context
;; use neo-term. Everything else keeps ordinary canonical Eshell processing.
;; This is the GNU detached visual-command contract: return nil immediately,
;; not a foreground process.  && and $? do NOT observe this child's status.
;; No stdin redirection, inline display, Eat, shell text or second process owner.

;;; Code:

(require 'neo-term)
(require 'em-term)
(require 'esh-cmd)
(require 'esh-io)
(require 'esh-var)

(defvar-local neo-term-eshell--epoch nil
  "Lease for the current buffer's opt-in Eshell visual session.")
(defvar-local neo-term-eshell--entry nil
  "Exact interpreter entry installed by this buffer's opt-in mode.")
(defvar-local neo-term-eshell--original-entry nil
  "Canonical visual pair replaced at its existing position, restored on disable.")

(defun neo-term-eshell--exact-environment-p (environment)
  "Whether ENVIRONMENT can override portable-pty's implicit SHELL binding.
Refuse missing/unset SHELL rather than invent a shell value.  First name wins."
  (let ((entry (cl-find-if (lambda (entry)
                             (or (equal entry "SHELL")
                                 (string-prefix-p "SHELL=" entry)))
                           environment)))
    (and entry (string-prefix-p "SHELL=" entry))))

(defun neo-term-eshell--local-command-p (command)
  "Whether COMMAND and all canonical search candidates are handler-free.
Accept an absolute local executable or a plain basename with entirely local
absolute PATH entries. Relative/dot PATH contexts keep canonical processing."
  (or (neo-term--local-directory command)
      (and (stringp command) (> (length command) 0)
           (not (cl-some (lambda (entry) (string-match-p (car entry) command))
                         file-name-handler-alist))
           (not (string-match-p "[/:\0-\37\177]" command))
           (let ((paths (eshell-get-path t)))
             (and paths
                  (cl-every
                   (lambda (path)
                     (and (neo-term--local-directory path)
                          (cl-every
                           (lambda (suffix)
                             (neo-term--local-directory
                              (concat (file-name-as-directory path) command suffix)))
                           eshell-binary-suffixes)))
                   paths))))))

(declare-function neomacs-terminal-cwd-pinning-p "neovm-core" (directory))

(defun neo-term-eshell--eligible-p (command args)
  "Prove safe visual COMMAND/ARGS eligibility from canonical Eshell context.
Fail closed for unknown handles, stdin, subcommands, pipelines, background,
remote/magic paths or redirected output.  No input string parsing.  Canonical
Eshell here rejects `<'; slot zero must also be empty (no supplied input).
Do not invoke remote handlers just to ask whether a path is native."
  (and neo-term-eshell-mode
       (eq major-mode 'eshell-mode)
       (fboundp 'neomacs-terminal-spawn)
       (eq system-type 'gnu/linux)
       (neo-term-eshell--exact-environment-p (eshell-environment-variables))
       (neo-term--local-directory default-directory)
       ;; Observe a real local directory fd/procfs inode before interpreter
       ;; selection. The renderer retains authoritative spawn-time refusal.
       (fboundp 'neomacs-terminal-cwd-pinning-p)
       (neomacs-terminal-cwd-pinning-p default-directory)
       (not eshell-in-pipeline-p)
       (not eshell-current-subjob-p)
       (not eshell-in-subcommand-p)
       (vectorp eshell-current-handles)
       (= (length eshell-current-handles) eshell-number-of-handles)
       (null (aref eshell-current-handles 0))
       (eshell-interactive-output-p 'all)
       ;; Require default handles too: an explicit > /dev/tty is not a grant.
       (cadr (aref eshell-current-handles eshell-output-handle))
       (cadr (aref eshell-current-handles eshell-error-handle))
       (neo-term-eshell--local-command-p command)
       (eshell-visual-command-p command args)))

(defun neo-term-eshell--exec (&rest args)
  "Run Eshell-resolved visual ARGS with canonical detached semantics.
Recheck context at dispatch.  Do not start a second process on failure."
  (unless (neo-term-eshell--eligible-p (car args) (cdr args))
    (error "Native visual command no longer has an eligible Eshell context"))
  (let* ((parent (current-buffer))
         (window (selected-window))
         (epoch neo-term-eshell--epoch)
         (directory default-directory)
         ;; Identical interpreter/argument resolution to eshell-exec-visual,
         ;; minus term.el's PTY.  Keep interpreter argv, never join/quote a string.
         (eshell-interpreter-alist nil)
         (interp (let ((file-name-handler-alist nil))
                   ;; A shebang discovered during canonical resolution must
                   ;; not acquire remote handler authority before validation.
                   (eshell-find-interpreter (car args) (cdr args))))
         (program (car interp))
         (argv (flatten-tree (eshell-stringify-list
                              (append (cdr interp) (cdr args)))))
         ;; xterm-256color is this native emulator's declared TERM, not
         ;; term.el's eterm-color. Every other Eshell env binding is captured.
         (environment (cons "TERM=xterm-256color"
                            (copy-sequence (eshell-environment-variables)))))
    (unless (and (eq (current-buffer) parent)
                 (buffer-live-p parent)
                 (eq major-mode 'eshell-mode)
                 neo-term-eshell-mode (eq neo-term-eshell--epoch epoch)
                 (equal default-directory directory)
                 (neo-term-eshell--eligible-p (car args) (cdr args))
                 (neo-term--local-directory program)
                 (neo-term-eshell--exact-environment-p environment))
      (error "Native visual command lost its local Eshell invocation lease"))
    (save-current-buffer
      (neo-term-exec program argv directory environment
                     (list :buffer parent :window window :epoch epoch
                           :destroy eshell-destroy-buffer-when-process-dies))))
  nil)

;;;###autoload
(define-minor-mode neo-term-eshell-mode
  "Buffer-local opt-in for detached Eshell visual commands using neo-term.
Do not enable globally.  Non-eligible invocations keep the canonical route.
Native child status is shown only in its terminal buffer, never used for
Eshell foreground continuation, &&, $? or the parent working directory."
  :lighter " NeoVisual"
  (unless (eq major-mode 'eshell-mode)
    (setq neo-term-eshell-mode nil)
    (user-error "Enable native visual commands only in an Eshell buffer"))
  (when neo-term-eshell--entry
    (setq-local eshell-interpreter-alist
                (mapcar (lambda (entry)
                          (if (eq entry neo-term-eshell--entry)
                              neo-term-eshell--original-entry entry))
                        eshell-interpreter-alist)))
  (setq neo-term-eshell--entry nil neo-term-eshell--original-entry nil
        neo-term-eshell--epoch nil)
  (when neo-term-eshell-mode
    (unless (fboundp 'neomacs-terminal-spawn)
      (setq neo-term-eshell-mode nil)
      (user-error "This runtime has no exact native terminal spawn API"))
    (let ((entries (cl-remove-if-not
                    (lambda (entry)
                      (and (eq (car entry) #'eshell-visual-command-p)
                           (eq (cdr entry) #'eshell-exec-visual)))
                    eshell-interpreter-alist)))
      (unless (= (length entries) 1)
        (setq neo-term-eshell-mode nil)
        (user-error "Native visual mode needs exactly one canonical visual interpreter"))
      (setq neo-term-eshell--original-entry (car entries)))
    (setq neo-term-eshell--epoch (make-symbol "neo-term-eshell"))
    (setq neo-term-eshell--entry
          (cons #'neo-term-eshell--eligible-p #'neo-term-eshell--exec))
    ;; Replace only the canonical visual pair IN PLACE. Retaining that pair
    ;; after a rejection would send a pipeline/background command to term.el
    ;; instead of ordinary Eshell process/handle authority.
    (setq-local eshell-interpreter-alist
                (mapcar (lambda (entry)
                          (if (eq entry neo-term-eshell--original-entry)
                              neo-term-eshell--entry entry))
                        eshell-interpreter-alist))))

(provide 'neo-term-eshell)
;;; neo-term-eshell.el ends here
