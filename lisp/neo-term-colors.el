;;; neo-term-colors.el --- GNU face palette bridge for neo-term -*- lexical-binding: t -*-

;; Copyright (C) 2026 Neomacs Contributors
;; License: GPL-3.0-or-later

;;; Commentary:

;; Preserve GNU term's customization surface.  The displaying frame's default
;; face and `ansi-term-color-vector' own the default and ANSI colours; RGB and
;; extended application colours do not pass through Lisp.  This bridge only
;; publishes small palette snapshots when faces change, never terminal cells.

;;; Code:

(require 'cl-lib)
(require 'term)

(declare-function neomacs-terminal-set-palette "neovm-core" (frame palette))

(defvar neo-term--palette-cache (make-hash-table :test 'eq :weakness 'key)
  "Last successfully published palette for each display frame.")
(defvar neo-term--palette-timer nil
  "One pending face-change refresh, not a polling timer.")
(defvar neo-term--palette-active nil
  "Non-nil after the first native terminal constructor activates the bridge.")

(defun neo-term--face-rgb (face attribute frame &optional fallback)
  "Return resolved RGB bytes for FACE's ATTRIBUTE on FRAME.
FALLBACK is a color used only when normal face resolution supplies no color."
  (let* ((color (face-attribute face attribute frame 'default))
         (rgb (and (stringp color) (color-values color frame))))
    (unless rgb
      (setq rgb (and fallback (color-values fallback frame))))
    (unless rgb
      (error "Cannot resolve terminal face %s %s on %s" face attribute frame))
    (mapcar (lambda (value) (round value 257)) rgb)))

(defun neo-term--frame-palette (frame)
  "Return the native palette vector resolved on explicit display FRAME.
Use GNU term faces, which inherit ANSI faces but can have higher-priority user
or theme overrides.  Resolve foreground and background independently."
  (let* ((foreground (face-attribute 'default :foreground frame 'default))
         (cursor (face-attribute 'cursor :background frame t))
         (slots (cl-loop for index from 1 to 16
                         collect (aref ansi-term-color-vector index)))
         (colors (append
                  (list (neo-term--face-rgb 'default :foreground frame)
                        (neo-term--face-rgb 'default :background frame)
                        (mapcar (lambda (value) (round value 257))
                                (or (and (stringp cursor) (color-values cursor frame))
                                    (color-values foreground frame))))
                  (mapcar (lambda (face) (neo-term--face-rgb face :foreground frame)) slots)
                  (mapcar (lambda (face) (neo-term--face-rgb face :background frame)) slots))))
    (vconcat (apply #'append colors) (list (and ansi-color-bold-is-bright t)))))

(defun neo-term--palette-frames ()
  "Return the graphical display frames which own native terminal palettes."
  (cl-remove-if-not #'display-graphic-p (frame-list)))

(defun neo-term--refresh-palettes (&optional _ignored)
  "Publish changed palettes for current graphical frames.
Existing terminal grids retain semantic colors and are reprojected by the
renderer.  Future terminals use the same frame cache without PTY polling."
  (when neo-term--palette-timer
    (cancel-timer neo-term--palette-timer))
  (setq neo-term--palette-timer nil)
  (when (and neo-term--palette-active
             (fboundp 'neomacs-terminal-set-palette))
    (dolist (frame (neo-term--palette-frames))
      (let ((palette (neo-term--frame-palette frame)))
        (unless (equal palette (gethash frame neo-term--palette-cache))
          ;; Cache only after a successful native queue operation.
          (neomacs-terminal-set-palette frame palette)
          (puthash frame palette neo-term--palette-cache))))))

(defun neo-term--schedule-palette-refresh (&rest _ignored)
  "Coalesce face changes into one ordinary timer callback.
GNU has theme hooks but no public general face-change hook, so narrow named
advice observes its face setters.  No advice repaints or edits any face."
  (when (and neo-term--palette-active (not neo-term--palette-timer))
    (setq neo-term--palette-timer
          (run-at-time 0 nil #'neo-term--refresh-palettes))))

(defun neo-term--palette-variable-changed (_symbol _value operation _where)
  "Observe a GNU palette option change described by OPERATION."
  (unless (eq operation 'let)
    (neo-term--schedule-palette-refresh)))

(defun neo-term--retire-frame-palette (frame)
  "Retire the cached native palette before FRAME is deleted."
  (when (gethash frame neo-term--palette-cache)
    (neomacs-terminal-set-palette frame nil)
    (remhash frame neo-term--palette-cache)))

(defun neo-term--ensure-palettes ()
  "Activate the native palette bridge and publish frame colors before spawn."
  (when (fboundp 'neomacs-terminal-set-palette)
    (unless neo-term--palette-active
      (setq neo-term--palette-active t)
      (add-hook 'enable-theme-functions #'neo-term--schedule-palette-refresh)
      (add-hook 'disable-theme-functions #'neo-term--schedule-palette-refresh)
      (add-hook 'after-make-frame-functions #'neo-term--refresh-palettes)
      (add-hook 'delete-frame-functions #'neo-term--retire-frame-palette)
      ;; Theme changes, Custom, direct setters and inherited face changes are
      ;; distinct authorities.  Coalescing avoids intermediate palette churn.
      (dolist (setter '(set-face-attribute face-spec-set face-spec-recalc
                                           custom-theme-set-faces modify-frame-parameters copy-face))
        (advice-add setter :after #'neo-term--schedule-palette-refresh))
      (dolist (variable '(ansi-color-bold-is-bright ansi-term-color-vector))
        (add-variable-watcher variable #'neo-term--palette-variable-changed)))
    (when neo-term--palette-timer
      (cancel-timer neo-term--palette-timer)
      (setq neo-term--palette-timer nil))
    (neo-term--refresh-palettes)))

(provide 'neo-term-colors)
;;; neo-term-colors.el ends here
