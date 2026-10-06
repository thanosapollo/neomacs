;;; neo-term.el --- GPU-accelerated terminal emulator for Neomacs -*- lexical-binding: t -*-

;; Copyright (C) 2026 Neomacs Contributors
;; License: GPL-3.0-or-later

;;; Commentary:

;; neo-term provides a GPU-accelerated terminal emulator backed by
;; rio-vt + portable-pty + wgpu, integrated into the neomacs display engine.
;;
;; Three display modes:
;;   - Window mode: terminal fills a regular Emacs window/buffer
;;   - Inline mode: terminal embedded inline in a buffer (like images)
;;   - Floating mode: terminal rendered as a floating overlay
;;
;; Usage:
;;   M-x neo-term          -- open a terminal in the current window
;;   M-x neo-term-floating -- open a floating terminal overlay

;;; Code:

(require 'cl-lib)
(declare-function neo-term--ensure-palettes "neo-term-colors" ())

(defgroup neo-term nil
  "GPU-accelerated terminal emulator."
  :group 'terminals
  :prefix "neo-term-")

(defcustom neo-term-shell nil
  "Shell program to run.  nil means use `explicit-shell-file-name' or $SHELL."
  :type '(choice (const :tag "Default" nil) string)
  :group 'neo-term)

(defcustom neo-term-default-cols 80
  "Default terminal width in columns."
  :type 'integer
  :group 'neo-term)

(defcustom neo-term-default-rows 24
  "Default terminal height in rows."
  :type 'integer
  :group 'neo-term)

(defvar neo-term--terminals (make-hash-table :test 'eql)
  "Hash table mapping terminal-id to terminal info plists.")

(defvar neo-term--next-buffer-num 1
  "Next buffer number for naming.")

;; These are Rust builtins routed through the active GUI display host.
(declare-function neomacs-terminal-create "neovm-core"
                  (cols rows mode &optional shell))
(declare-function neomacs-terminal-write "neovm-core"
                  (terminal-id string))
(declare-function neomacs-terminal-resize "neovm-core"
                  (terminal-id cols rows))
(declare-function neomacs-terminal-destroy "neovm-core"
                  (terminal-id))
(declare-function neomacs-terminal-set-float "neovm-core"
                  (terminal-id x y opacity))
(declare-function neomacs-terminal-get-text "neovm-core"
                  (terminal-id))

(defun neo-term--shell-path ()
  "Return shell program to use."
  (or neo-term-shell
      (bound-and-true-p explicit-shell-file-name)
      (getenv "SHELL")
      "/bin/sh"))

(defun neo-term--create (cols rows mode &optional shell)
  "Create a terminal.  MODE is 0=Window, 1=Inline, 2=Floating.
Returns terminal ID or nil on failure."
  (let ((shell-path (or shell (neo-term--shell-path)))
        (invoking-buffer (current-buffer))
        (owner (and (eql mode 0) (eq major-mode 'neo-term-mode)
                    (current-buffer))))
    (condition-case err
        (progn
          (require 'neo-term-colors)
          (neo-term--ensure-palettes)
          (when (and (eql mode 0)
                     (not (and (buffer-live-p invoking-buffer)
                               (eq (current-buffer) invoking-buffer)
                               (or (null owner) (eq major-mode 'neo-term-mode)))))
            (error "Native terminal owner changed during palette setup"))
          (let ((id (neomacs-terminal-create cols rows mode shell-path)))
            (when (and id (> id 0))
              (puthash id (append (list :id id :cols cols :rows rows :mode mode
                                        :shell shell-path)
                                  (when (and owner (buffer-live-p owner)
                                             (eq (buffer-local-value 'major-mode owner)
                                                 'neo-term-mode))
                                    (list :buffer owner)))
                       neo-term--terminals)
              id)))
      (error
       (message "neo-term: failed to create terminal: %s" (error-message-string err))
       nil))))

(defun neo-term--destroy (terminal-id)
  "Destroy a terminal."
  (when terminal-id
    ;; Retire Lisp authority before calling the host, which can report events.
    (remhash terminal-id neo-term--terminals)
    (ignore-errors (neomacs-terminal-destroy terminal-id))))

(defun neo-term--write (terminal-id string)
  "Send STRING to the terminal."
  (when (and terminal-id string)
    (neomacs-terminal-write terminal-id string)))

(defun neo-term--resize (terminal-id cols rows)
  "Resize a terminal."
  (when terminal-id
    (neomacs-terminal-resize terminal-id cols rows)))

;;; Major mode

(defvar neo-term-mode-map
  (let ((map (make-sparse-keymap)))
    ;; Suppress default self-insert so stray keys don't modify the buffer
    (suppress-keymap map t)

    ;; All printable ASCII characters (space=32 through ~=126)
    (dotimes (i 95)
      (define-key map (string (+ i 32)) #'neo-term-send-key))

    ;; Standard editing keys
    (define-key map (kbd "RET") #'neo-term-send-return)
    (define-key map (kbd "DEL") #'neo-term-send-backspace)
    (define-key map (kbd "<backspace>") #'neo-term-send-backspace)
    (define-key map (kbd "TAB") #'neo-term-send-tab)
    (define-key map [escape] #'neo-term-send-escape)

    ;; Arrow keys, navigation keys, and function keys (send ANSI sequences)
    (dolist (key '("<up>" "<down>" "<left>" "<right>"
                   "<home>" "<end>" "<prior>" "<next>"
                   "<insert>" "<delete>"
                   "<f1>" "<f2>" "<f3>" "<f4>" "<f5>" "<f6>"
                   "<f7>" "<f8>" "<f9>" "<f10>" "<f11>" "<f12>"))
      (define-key map (kbd key) #'neo-term-send-special))

    ;; Control keys sent directly to terminal (C-a=1 .. C-z=26)
    ;; Skip C-c(3)=our prefix, C-i(9)=TAB, C-m(13)=RET
    (dotimes (i 26)
      (let ((ctrl-char (1+ i)))
        (unless (memq ctrl-char '(3 9 13))
          (define-key map (vector ctrl-char) #'neo-term-send-ctrl))))

    ;; C-c prefix for Emacs-level commands
    (define-key map (kbd "C-c C-c") #'neo-term-send-ctrl-c)
    (define-key map (kbd "C-c C-d") #'neo-term-send-ctrl-d)
    (define-key map (kbd "C-c C-z") #'neo-term-send-ctrl-z)
    (define-key map (kbd "C-c C-\\") #'neo-term-send-ctrl-backslash)
    (define-key map (kbd "C-c C-q") #'neo-term-quit)
    map)
  "Keymap for `neo-term-mode'.")

(define-derived-mode neo-term-mode fundamental-mode "NeoTerm"
  "Major mode for neo-term GPU terminal buffers.

\\{neo-term-mode-map}"
  :group 'neo-term
  (setq-local buffer-read-only t)
  (setq-local truncate-lines t)
  (setq-local neo-term--id nil)
  (add-hook 'kill-buffer-hook #'neo-term--kill-buffer-terminal nil t)
  (add-hook 'change-major-mode-hook #'neo-term--kill-buffer-terminal nil t))

(defvar-local neo-term--id nil
  "Terminal ID for this buffer.")

(defun neo-term--kill-buffer-terminal ()
  "Destroy the current buffer's terminal on kill or major-mode replacement."
  (when neo-term--id
    (let ((id neo-term--id))
      ;; Clear first so errors or recursive buffer cleanup cannot queue a
      ;; second destroy for the same host-owned terminal.
      (setq neo-term--id nil)
      (neo-term--destroy id))))

(defun neo-term-send-key ()
  "Send the current key to the terminal."
  (interactive)
  (when neo-term--id
    (let* ((keys (this-command-keys))
           (str (if (stringp keys) keys (string (event-basic-type last-input-event)))))
      (neo-term--write neo-term--id str))))

(defun neo-term-send-return ()
  "Send Return to the terminal."
  (interactive)
  (when neo-term--id (neo-term--write neo-term--id "\r")))

(defun neo-term-send-backspace ()
  "Send Backspace to the terminal."
  (interactive)
  (when neo-term--id (neo-term--write neo-term--id "\177")))

(defun neo-term-send-tab ()
  "Send Tab to the terminal."
  (interactive)
  (when neo-term--id (neo-term--write neo-term--id "\t")))

(defun neo-term-send-escape ()
  "Send Escape to the terminal."
  (interactive)
  (when neo-term--id (neo-term--write neo-term--id "\e")))

(defun neo-term-send-ctrl ()
  "Send control character to the terminal."
  (interactive)
  (when neo-term--id
    (neo-term--write neo-term--id (string last-input-event))))

(defun neo-term-send-special ()
  "Send special key (arrows, function keys, etc.) as ANSI escape sequence."
  (interactive)
  (when neo-term--id
    (let ((seq (neo-term--key-to-ansi last-input-event)))
      (when seq (neo-term--write neo-term--id seq)))))

(defun neo-term--key-to-ansi (key)
  "Convert Emacs KEY event symbol to ANSI escape sequence string."
  (pcase key
    ('up     "\e[A")
    ('down   "\e[B")
    ('right  "\e[C")
    ('left   "\e[D")
    ('home   "\e[H")
    ('end    "\e[F")
    ('prior  "\e[5~")
    ('next   "\e[6~")
    ('insert "\e[2~")
    ('delete "\e[3~")
    ('f1  "\eOP")
    ('f2  "\eOQ")
    ('f3  "\eOR")
    ('f4  "\eOS")
    ('f5  "\e[15~")
    ('f6  "\e[17~")
    ('f7  "\e[18~")
    ('f8  "\e[19~")
    ('f9  "\e[20~")
    ('f10 "\e[21~")
    ('f11 "\e[23~")
    ('f12 "\e[24~")))

(defun neo-term-send-ctrl-c ()
  "Send C-c to the terminal."
  (interactive)
  (when neo-term--id (neo-term--write neo-term--id "\003")))

(defun neo-term-send-ctrl-d ()
  "Send C-d to the terminal."
  (interactive)
  (when neo-term--id (neo-term--write neo-term--id "\004")))

(defun neo-term-send-ctrl-z ()
  "Send C-z to the terminal."
  (interactive)
  (when neo-term--id (neo-term--write neo-term--id "\032")))

(defun neo-term-send-ctrl-backslash ()
  "Send C-\\ to the terminal."
  (interactive)
  (when neo-term--id (neo-term--write neo-term--id "\034")))

(defun neo-term-quit ()
  "Kill the terminal and close the buffer."
  (interactive)
  (kill-buffer))

(defvar-local neo-term--completion nil
  "Native child status/drain plist, never Eshell foreground exit status.")

(defvar-local neo-term--invocation-lease nil
  "Occurrence lease for constructing an exact-command terminal buffer.")
;; Keep the creation occurrence through the ONE intended major-mode reset.
;; Every later transition retires it before locals can be reset or restored.
(put 'neo-term--invocation-lease 'permanent-local t)

(defvar neo-term--initializing-lease nil
  "Dynamically bound one-transition permission for `neo-term-exec'.")

(defun neo-term--retire-invocation ()
  "Retire this buffer's occurrence except for its first intended mode reset."
  (if (and neo-term--invocation-lease
           (eq neo-term--initializing-lease neo-term--invocation-lease))
      ;; Consume before any subsequent change-mode hooks can reenter.
      (setq neo-term--initializing-lease nil)
    (setq neo-term--invocation-lease nil)
    (remove-hook 'change-major-mode-hook #'neo-term--retire-invocation t)))
(put 'neo-term--retire-invocation 'permanent-local-hook t)

(defvar-local neo-term--origin nil
  "Captured Eshell origin lease; no parent-directory publication authority.")

(defun neo-term--owns-buffer-p (terminal-id info buffer)
  "Whether INFO still owns TERMINAL-ID and the exact BUFFER object."
  (and (eq info (gethash terminal-id neo-term--terminals))
       (bufferp buffer) (buffer-live-p buffer)
       (eq (buffer-local-value 'major-mode buffer) 'neo-term-mode)
       (eql (buffer-local-value 'neo-term--id buffer) terminal-id)))

(defun neo-term--handle-settled (terminal-id code signal drained wait-error read-error)
  "Settle native wait plus output completion once, retaining final diagnostics.
CODE and SIGNAL are exclusive; DRAINED means parser consumed EOF.  Errors are
not successful exits.  This hook never resumes Eshell or sets its exit status."
  (let* ((info (gethash terminal-id neo-term--terminals))
         (buffer (plist-get info :buffer)))
    (when (neo-term--owns-buffer-p terminal-id info buffer)
      (let ((text (condition-case nil (neomacs-terminal-get-text terminal-id)
                    (error nil))))
        ;; Snapshot acquisition can run arbitrary host/advice code.
        (when (neo-term--owns-buffer-p terminal-id info buffer)
          (with-current-buffer buffer
            (let ((origin neo-term--origin)
                  (completion (list :exit-code code :signal signal :output-drained drained
                                    :wait-error wait-error :read-error read-error)))
              (setq-local neo-term--completion completion)
              (setq neo-term--id nil neo-term--origin nil)
              (neo-term--destroy terminal-id)
              ;; Host destruction may run hooks: do not mutate a reused mode.
              (when (and (buffer-live-p buffer)
                         (eq major-mode 'neo-term-mode) (null neo-term--id)
                         (eq neo-term--completion completion))
                (let ((inhibit-read-only t) (inhibit-modification-hooks t))
                  (erase-buffer)
                  (when (stringp text) (insert text))
                  (insert (format "\n[Child: %s; output %s%s%s]\n"
                                  (or signal code "unknown")
                                  (if drained "drained" "not drained")
                                  (if wait-error (concat "; wait: " wait-error) "")
                                  (if read-error (concat "; read: " read-error) ""))))
                (when (and (eql code 0) (null signal) drained
                           (null wait-error) (null read-error)
                           (plist-get origin :destroy))
                  (let ((window (plist-get origin :window))
                        (parent (plist-get origin :buffer))
                        (epoch (plist-get origin :epoch)))
                    ;; Return only an unchanged, selected invoking window.
                    ;; User navigation, mode reuse, disable or buffer replacement
                    ;; revoke handback. No prompt insertion or parent cwd change.
                    (when (and (window-live-p window)
                               (eq window (selected-window))
                               (eq (window-buffer window) buffer)
                               (buffer-live-p parent)
                               (eq (buffer-local-value 'major-mode parent) 'eshell-mode)
                               epoch
                               (eq (buffer-local-value 'neo-term-eshell--epoch parent) epoch))
                      (set-window-buffer window parent)
                      (when (and (buffer-live-p buffer)
                                 (eq (buffer-local-value 'major-mode buffer) 'neo-term-mode)
                                 (null (buffer-local-value 'neo-term--id buffer))
                                 (eq (buffer-local-value 'neo-term--completion buffer) completion))
                        (kill-buffer buffer)))))))))))))

(defvar neo-term-settled-functions nil
  "Hook with (ID CODE SIGNAL DRAINED WAIT-ERROR READ-ERROR).
Only exact-command terminals provide authoritative status; SIGNAL is the
portable-pty native description, not a numeric POSIX signal.  The legacy
ID-only exit hook runs afterward.  No foreground Eshell status is implied.")

(add-hook 'neo-term-settled-functions #'neo-term--handle-settled)

(defun neo-term--handle-exit (terminal-id)
  "Handle terminal TERMINAL-ID process exit."
  (dolist (buf (buffer-list))
    (with-current-buffer buf
      (when (and (eq major-mode 'neo-term-mode)
                 (eql neo-term--id terminal-id))
        (neo-term--destroy terminal-id)
        (setq neo-term--id nil)
        (let ((inhibit-read-only t))
          (goto-char (point-max))
          (insert "\n[Process exited]\n"))
        (message "neo-term: terminal %d exited" terminal-id)))))

(defun neo-term--handle-create-failed (terminal-id error)
  "Handle creation failure ERROR for TERMINAL-ID."
  ;; Retire the failed reservation through the typed Rust lifecycle.  There is
  ;; no PTY to tear down, but the renderer must acknowledge removal of the ID.
  (ignore-errors (neomacs-terminal-destroy terminal-id))
  (remhash terminal-id neo-term--terminals)
  (dolist (buf (buffer-list))
    (with-current-buffer buf
      (when (and (eq major-mode 'neo-term-mode)
                 (eql neo-term--id terminal-id))
        ;; The renderer owns the failed lifecycle record; clearing the local
        ;; ID prevents kill-buffer cleanup from attempting to destroy it.
        (setq neo-term--id nil)
        (let ((inhibit-read-only t))
          (goto-char (point-max))
          (insert (format "\n[Terminal creation failed: %s]\n" error)))
        (message "neo-term: terminal %d creation failed: %s"
                 terminal-id error)))))

(defun neo-term--handle-title-changed (terminal-id title)
  "Handle terminal TERMINAL-ID title change to TITLE."
  (dolist (buf (buffer-list))
    (with-current-buffer buf
      (when (and (eq major-mode 'neo-term-mode)
                 (eql neo-term--id terminal-id))
        (rename-buffer (format "*neo-term: %s*" title) t)))))

(defun neo-term--local-directory (directory)
  "Return DIRECTORY with a trailing slash if safe for local projection.
DIRECTORY is an absolute directory already decoded and validated by Rust,
not an OSC payload or URI.  Do not decode, canonicalize or access it here.
Refuse Emacs magic file names, controls and any file-name-handler match,
including handlers restricted to operations other than directory handling."
  (when (and (stringp directory)
             (> (length directory) 0)
             (eq (aref directory 0) ?/)
             (not (string-match-p "\\`/[^/]*:" directory))
             ;; Reject decoded dot components before lexical file expansion
             ;; can expose a leading magic name.  Do not expand or decode here.
             (not (cl-some (lambda (part) (member part '("." "..")))
                           (split-string directory "/" t)))
             (not (string-match-p "[\0-\37\177]" directory)))
    (let ((local-directory
           (if (eq (aref directory (1- (length directory))) ?/)
               directory
             (concat directory "/"))))
      ;; Test both spellings without invoking a handler.  Operation-specific
      ;; lookup or inhibition must not grant authority to a magic file name.
      (unless (cl-some (lambda (entry)
                         (or (string-match-p (car entry) directory)
                             (string-match-p (car entry) local-directory)))
                       file-name-handler-alist)
        local-directory))))

(defun neo-term--handle-directory-changed (terminal-id directory)
  "Project Rust-validated DIRECTORY into TERMINAL-ID's live owning buffer.
Only a window terminal attached to that exact `neo-term-mode' buffer may
change its buffer-local `default-directory'.  Ignore unavailable, retired
or replaced owners.  Never resolve buffer names or change window selection."
  (let* ((info (gethash terminal-id neo-term--terminals))
         (buffer (plist-get info :buffer)))
    (when (and info (eql (plist-get info :mode) 0)
               (bufferp buffer) (buffer-live-p buffer))
      (with-current-buffer buffer
        (when (and (eq major-mode 'neo-term-mode)
                   (eql neo-term--id terminal-id))
          (when-let* ((local-directory (neo-term--local-directory directory)))
            (unless (equal default-directory local-directory)
              (setq-local default-directory local-directory))))))))

(defvar neo-term-exit-functions nil
  "Functions called with a terminal ID after its child process exits.")

(defvar neo-term-create-failed-functions nil
  "Functions called with a terminal ID and renderer creation error.")

(defvar neo-term-title-changed-functions nil
  "Functions called with a terminal ID and its new title.")

(defvar neo-term-directory-changed-functions nil
  "Functions called with (TERMINAL-ID DIRECTORY) after a cwd change.
Rust emits only strictly validated local absolute UTF-8 directories,
decoded once, never URIs.  Unavailable or rejected reports emit no change.
The standard handler updates only the live owning window-terminal buffer.")

(add-hook 'neo-term-exit-functions #'neo-term--handle-exit)
(add-hook 'neo-term-create-failed-functions #'neo-term--handle-create-failed)
(add-hook 'neo-term-title-changed-functions #'neo-term--handle-title-changed)
(add-hook 'neo-term-directory-changed-functions
          #'neo-term--handle-directory-changed)

;;; Public API

;;;###autoload
(defun neo-term ()
  "Open a new GPU-accelerated terminal in the current window."
  (interactive)
  (let* ((initial-directory default-directory)
         (buf-name (format "*neo-term-%d*" neo-term--next-buffer-num))
         (buf (get-buffer-create buf-name)))
    (switch-to-buffer buf)
    (neo-term-mode)
    ;; Capture the invoking buffer's context lexically, before switching or
    ;; running mode hooks; this does not change the native spawn API.
    (setq-local default-directory initial-directory)
    ;; The Rust boundary captures the current buffer as the typed owner of a
    ;; Window terminal, so creation must occur after switching to BUF.
    (let ((id (neo-term--create neo-term-default-cols neo-term-default-rows
                                0))) ; mode=0 (Window)
      (unless id
        (kill-buffer buf)
        (error "Failed to create terminal"))
      (cl-incf neo-term--next-buffer-num)
      (setq-local neo-term--id id)
      (message "neo-term: terminal %d created (%dx%d)"
               id neo-term-default-cols neo-term-default-rows))))

;;;###autoload
(defun neo-term-floating (&optional x y cols rows)
  "Open a floating GPU terminal overlay.
Optional X, Y set the floating position.
Optional COLS, ROWS set the terminal size."
  (interactive)
  (let* ((cols (or cols neo-term-default-cols))
         (rows (or rows neo-term-default-rows))
         (id (neo-term--create cols rows 2))) ; mode=2 (Floating)
    (unless id
      (error "Failed to create floating terminal"))
    (when (or x y)
      (neomacs-terminal-set-float
       id (or x 100.0) (or y 100.0) 0.95))
    (message "neo-term: floating terminal %d created (%dx%d)" id cols rows)
    id))

(declare-function neomacs-terminal-spawn "neovm-core"
                  (cols rows executable argv directory environment))

(defun neo-term-exec (executable argv directory environment &optional origin)
  "Run exact EXECUTABLE/ARGV in local DIRECTORY with ENVIRONMENT.
Use the existing native PTY/parser/renderer in a separate buffer.  ENVIRONMENT
is a complete Emacs process-environment list (first name wins, bare names
unset); the native boundary refuses non-UTF-8 and NUL.  ORIGIN is an optional
Eshell handback lease.  Return the new buffer, not a foreground process."
  (unless (and (fboundp 'neomacs-terminal-spawn)
               (neo-term--local-directory executable)
               (neo-term--local-directory directory)
               (listp argv) (cl-every #'stringp argv)
               (listp environment) (cl-every #'stringp environment))
    (error "Exact native terminal requires local executable/cwd and string argv/environment"))
  (let ((buffer (generate-new-buffer
                 (format "*neo-term: %s*" (file-name-nondirectory executable))))
        (window (selected-window))
        (parent (current-buffer))
        (lease (make-symbol "neo-term-invocation"))
        (complete nil))
    (with-current-buffer buffer
      (setq-local neo-term--invocation-lease lease)
      (add-hook 'change-major-mode-hook #'neo-term--retire-invocation nil t))
    (unwind-protect
        (progn
          (switch-to-buffer buffer)
          (unless (and (eq (current-buffer) buffer) (buffer-live-p buffer)
                       (eq neo-term--invocation-lease lease))
            (error "Native terminal buffer changed during selection"))
          (let ((neo-term--initializing-lease lease))
            (neo-term-mode))
          (unless (and (eq (current-buffer) buffer) (buffer-live-p buffer)
                       (eq (selected-window) window)
                       (eq (window-buffer window) buffer)
                       (eq neo-term--invocation-lease lease)
                       (eq major-mode 'neo-term-mode) (null neo-term--id))
            (error "Native terminal buffer changed during mode hooks"))
          (setq-local default-directory directory)
          (setq-local neo-term--origin origin)
          ;; Editor navigation/quit are not child control bytes. C-c C-c is
          ;; still an explicit terminal interrupt, C-c C-q destroys this child.
          (let ((map (copy-keymap neo-term-mode-map)))
            (define-key map (kbd "C-x") (lookup-key (current-global-map) (kbd "C-x")))
            (define-key map (kbd "C-g") #'keyboard-quit)
            (use-local-map map))
          (require 'neo-term-colors)
          (neo-term--ensure-palettes)
          ;; Loading face definitions or user advice can revoke the owner.
          (unless (and (buffer-live-p buffer) (eq (current-buffer) buffer)
                       (eq (selected-window) window)
                       (eq (window-buffer window) buffer)
                       (eq neo-term--invocation-lease lease)
                       (eq major-mode 'neo-term-mode) (null neo-term--id))
            (error "Native terminal buffer changed during palette setup"))
          (let ((id (neomacs-terminal-spawn
                     neo-term-default-cols neo-term-default-rows
                     executable argv directory environment)))
            (unless (and (integerp id) (> id 0))
              (error "Native terminal reservation failed"))
            (unless (and (buffer-live-p buffer)
                         (eq (current-buffer) buffer)
                         (eq major-mode 'neo-term-mode) (null neo-term--id)
                         (eq neo-term--invocation-lease lease))
              (neo-term--destroy id)
              (error "Native terminal buffer changed during reservation"))
            (setq-local neo-term--id id)
            (puthash id (list :id id :mode 0 :buffer buffer)
                     neo-term--terminals))
          (setq complete t)
          buffer)
      (unless complete
        ;; Cleanup must not replace the original error, quit or thrown value.
        ;; No child belongs to this unreserved buffer. Suppress disposal hooks
        ;; that could repurpose it while native kill-buffer is already underway.
        (let ((inhibit-quit t))
          (condition-case nil
              (when (and (window-live-p window)
                         (eq (window-buffer window) buffer)
                         (buffer-live-p parent) (buffer-live-p buffer)
                         (eq (buffer-local-value 'neo-term--invocation-lease buffer) lease)
                         (null (buffer-local-value 'neo-term--id buffer)))
                ;; Detach the owned startup display even after other-window
                ;; navigation, without selecting it or the parent.  Otherwise
                ;; kill-buffer replaces it via unsuppressed parent-local scroll
                ;; callbacks, which can escape the original startup unwind.
                (with-current-buffer parent
                  (let ((window-scroll-functions nil)
                        (buffer-list-update-hook nil))
                    (set-window-buffer window parent))))
            ((error quit) nil))
          ;; Revalidate even if restoration failed or replaced the occurrence.
          (condition-case nil
              (when (and (buffer-live-p buffer)
                         (eq (buffer-local-value 'neo-term--invocation-lease buffer) lease)
                         (null (buffer-local-value 'neo-term--id buffer)))
                (with-current-buffer buffer
                  (setq neo-term--invocation-lease nil)
                  (let ((kill-buffer-hook nil)
                        (kill-buffer-query-functions nil)
                        (buffer-list-update-hook nil))
                    (kill-buffer buffer))))
            ((error quit) nil)))))))

(provide 'neo-term)
;;; neo-term.el ends here
