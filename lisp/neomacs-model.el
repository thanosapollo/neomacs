;;; neomacs-model.el --- Native 3D model views -*- lexical-binding: t -*-

;; Copyright (C) 2026 Free Software Foundation, Inc.

;; Author: Neomacs Contributors
;; Keywords: multimedia, graphics

;; This file is part of GNU Emacs.

;; GNU Emacs is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or
;; (at your option) any later version.

;; GNU Emacs is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with GNU Emacs.  If not, see <https://www.gnu.org/licenses/>.

;;; Commentary:

;; NeoMacs extension (experimental): OBJ scenes drawn natively by the GPU
;; renderer inside an ordinary buffer.  A MODEL is a shader-surface handle
;; (opaque, GC-managed); it is shown by a `(surface :id MODEL ...)' display
;; spec and loaded asynchronously.
;;
;;   (setq m (neomacs-model-create
;;            :meshes '(("humerus" "/data/humerus.obj" SHA256-OR-NIL) ...)
;;            :view '(0 -90 1.1) :callback #'my-model-callback))
;;   (neomacs-model-insert m)                 ; glyph at point
;;   (neomacs-model-open MESHES :name "Arm")  ; standalone buffer
;;
;; Requests (`neomacs-model-set-view', `-set-highlight', `-resize',
;; `-freeze') return a request number.  `neomacs-model-current-p' is non-nil
;; once the renderer has settled the latest request and presents the latest
;; picture in every window showing MODEL.  The callback is called with
;; (MODEL EVENT PLIST) for the events `ready', `failed', `error', `view',
;; `readiness' and `pick'; a `pick' is delivered only when it names the
;; picture Lisp holds as current, with its hit mesh given by ID.
;;
;; Left-dragging a model orbits it and a click picks; the wheel zooms.
;; Mesh IDs never reach the renderer: they are mapped to indices here.

;;; Code:

(require 'cl-lib)
(require 'color)

(declare-function neomacs-model--available-p "model/mod.rs" ())
(declare-function neomacs-model--create "model/mod.rs"
                  (meshes view background width height))
(declare-function neomacs-model--id "model/mod.rs" (handle))
(declare-function neomacs-model--set-view "model/mod.rs"
                  (handle req yaw pitch zoom))
(declare-function neomacs-model--set-highlight "model/mod.rs"
                  (handle req target selected))
(declare-function neomacs-model--resize "model/mod.rs" (handle req width height))
(declare-function neomacs-model--freeze "model/mod.rs" (handle req))
(declare-function neomacs-surface-destroy "shader_surface/mod.rs" (id))

(defgroup neomacs-model nil
  "Native 3D model views."
  :group 'multimedia
  :prefix "neomacs-model-")

(defvar neomacs-model-event-functions nil
  "Abnormal hook run for each native model event.
Each function is called with (ID TYPE PLIST): the integer surface
id, the event symbol and its plist.  Events are delivered in order
and never nested: one arriving while a member runs (for example
inside its `sit-for') waits until every member has handled the
current one.  A member's error is logged and the others still run.
`neomacs-model--dispatch' is the member that serves
`neomacs-model-create' callbacks.")

(defvar neomacs-model--state-functions nil
  "Internal hook run with (ID TYPE PLIST) as soon as an event arrives.
Unlike `neomacs-model-event-functions' it also runs while a member
of that hook is active, so input read afterwards always sees the
event's effect on the model state.  Members must only record
state: never call user code, wait or read input.")

;;;; Registry

(cl-defstruct (neomacs-model--entry
               (:constructor neomacs-model--entry-create)
               (:copier nil))
  "Lisp-side state of one model (design §6.3)."
  id callback buffer
  (status '(loading))
  ;; Vector of mesh ID strings, in renderer index order.
  mesh-ids
  view initial-view
  ;; Size the glyph shows: the latest pending resize, else the accepted one.
  width height
  ;; Size of the last resize the renderer applied, (WIDTH HEIGHT).
  accepted-size
  ;; Resizes not yet settled, ((REQ WIDTH HEIGHT) ...), oldest first.
  resizes
  ;; Latest request number sent to the renderer.
  (issued 0)
  ;; Last `readiness' as (CURRENT REV REQ), or nil.
  readiness
  ;; A requested camera stays authoritative until its request is settled;
  ;; older interaction events may still be queued when Lisp issues it.
  pending-view
  frozen)

(defvar neomacs-model--registry (make-hash-table :test 'eq :weakness 'key)
  "MODEL handle -> `neomacs-model--entry'.
Weak in the key, so an unreferenced model is collected even when
its callback closes over it; collection frees its GPU objects.")

(defun neomacs-model--entry (model)
  "Return MODEL's registry entry, or signal."
  (or (gethash model neomacs-model--registry)
      (signal 'wrong-type-argument (list 'neomacs-model model))))

(defun neomacs-model--find (id)
  "Return the live MODEL handle whose surface id is ID, or nil."
  (catch 'found
    (maphash (lambda (model entry)
               (when (eql (neomacs-model--entry-id entry) id)
                 (throw 'found model)))
             neomacs-model--registry)
    nil))

(defun neomacs-model--mesh-index (entry mesh-id)
  "Return ENTRY's renderer index of MESH-ID, or signal."
  (or (cl-position mesh-id (neomacs-model--entry-mesh-ids entry)
                   :test #'equal)
      (error "Unknown model mesh %S" mesh-id)))

;;;; Creation

;;;###autoload
(defun neomacs-model-available-p ()
  "Return non-nil if a renderer that can draw native models is live now."
  (and (fboundp 'neomacs-model--available-p)
       (neomacs-model--available-p)))

(defun neomacs-model--rgb (color)
  "Return colour name COLOR as three numbers in 0..1, or nil."
  (let ((rgb (ignore-errors (color-name-to-rgb color))))
    (and (proper-list-p rgb)
         (= (length rgb) 3)
         (cl-every (lambda (c) (and (numberp c) (<= 0 c 1))) rgb)
         rgb)))

(defun neomacs-model--background (color)
  "Return COLOR as three numbers in 0..1.
COLOR is a colour name, a list or vector of three numbers, or nil
for the `default' face background (black when it has none)."
  (cond
   ((null color)
    (or (neomacs-model--rgb (face-background 'default nil t)) (list 0 0 0)))
   ((stringp color)
    (or (neomacs-model--rgb color) (error "Unknown model background %S" color)))
   (t color)))

;;;###autoload
(defun neomacs-model-create (&rest plist)
  "Create a native model from PLIST and return it.
Keys:
  :meshes ((ID PATH [SHA256]) ...)  required, 1..255 meshes with
            unique string IDs and absolute .obj PATHs; SHA256 is the
            lowercase hex digest the loader checks.
  :width, :height  logical pixels, 32..2048 (default 512).
  :view (YAW PITCH ZOOM)  initial camera, a list or vector
            (default (0 0 1)).
  :background  colour name or (R G B) in 0..1 (default: the
            `default' face background).
  :callback FUNCTION  called with (MODEL EVENT PLIST), see Commentary.
Loading is asynchronous: file, digest, parse and GPU failures arrive
as a `failed' event.  Signals a `user-error' without a graphical
renderer."
  (let* ((meshes (plist-get plist :meshes))
         (ids (mapcar #'car meshes))
         ;; Kept as a list: `neomacs-model-view' and the camera keys
         ;; take one apart with `nth'.
         (view (let ((view (or (plist-get plist :view) (list 0 0 1))))
                 (if (vectorp view) (append view nil) view)))
         (width (or (plist-get plist :width) 512))
         (height (or (plist-get plist :height) 512)))
    (unless (and (consp meshes) (cl-every #'consp meshes)
                 (cl-every #'stringp ids))
      (error "Model :meshes must be ((ID PATH [SHA256]) ...)"))
    (when (/= (length ids) (length (delete-dups (copy-sequence ids))))
      (error "Model mesh IDs must be unique"))
    (let* ((model (neomacs-model--create
                   (mapcar (lambda (mesh) (list (nth 1 mesh) (nth 2 mesh)))
                           meshes)
                   view
                   (neomacs-model--background (plist-get plist :background))
                   width height)))
      (puthash model
               (neomacs-model--entry-create
                :id (neomacs-model--id model)
                :callback (plist-get plist :callback)
                :mesh-ids (vconcat ids)
                :view (copy-sequence view)
                :initial-view (copy-sequence view)
                :width width :height height
                :accepted-size (list width height))
               neomacs-model--registry)
      model)))

;;;; Requests

(defun neomacs-model--request (model send)
  "Send a request to MODEL with SEND, called with the request number.
Return the request number once SEND succeeds."
  (let* ((entry (neomacs-model--entry model))
         (req (1+ (neomacs-model--entry-issued entry))))
    (when (eq (car (neomacs-model--entry-status entry)) 'destroyed)
      (error "Model is destroyed"))
    (when (neomacs-model--entry-frozen entry)
      (user-error "Model is frozen"))
    (funcall send req)
    ;; Raising `issued' makes `neomacs-model-current-p' nil at once.
    (setf (neomacs-model--entry-issued entry) req)
    req))

(defun neomacs-model-set-view (model yaw pitch zoom)
  "Request camera YAW PITCH (degrees) and ZOOM (0.25..4) for MODEL.
Return the request number."
  (let* ((entry (neomacs-model--entry model))
         (req (neomacs-model--request
               model (lambda (req)
                       (neomacs-model--set-view model req yaw pitch zoom)))))
    (setf (neomacs-model--entry-view entry) (list yaw pitch zoom)
          (neomacs-model--entry-pending-view entry) req)
    req))

(defun neomacs-model--target (entry spec)
  "Return highlight SPEC with its mesh ID replaced by ENTRY's index."
  (when spec
    (let ((copy (copy-sequence spec)))
      (plist-put copy :mesh
                 (neomacs-model--mesh-index entry (plist-get spec :mesh))))))

(defun neomacs-model-set-highlight (model &rest plist)
  "Replace MODEL's highlight with PLIST; return the request number.
:target SPEC or nil, SPEC one of
  (:mesh ID :kind object)
  (:mesh ID :kind point :face N :barycentric [A B C] :tolerance R)
  (:mesh ID :kind region :faces (N ...))
:selected MESH-ID or nil, painted only when there is no target."
  (let* ((entry (neomacs-model--entry model))
         (target (neomacs-model--target entry (plist-get plist :target)))
         (selected (let ((id (plist-get plist :selected)))
                     (and id (neomacs-model--mesh-index entry id)))))
    (neomacs-model--request
     model (lambda (req)
             (neomacs-model--set-highlight model req target selected)))))

(defun neomacs-model-resize (model width height)
  "Resize MODEL to WIDTH x HEIGHT logical pixels (32..2048).
Return the request number.  The glyph inserted by
`neomacs-model-insert' or `neomacs-model-open' follows at once, and
returns to the previous size if the renderer refuses the request."
  (let ((req (neomacs-model--request
              model (lambda (req)
                      (neomacs-model--resize model req width height))))
        (entry (neomacs-model--entry model)))
    (setf (neomacs-model--entry-resizes entry)
          (nconc (neomacs-model--entry-resizes entry)
                 (list (list req width height))))
    (neomacs-model--show-size model entry)
    req))

(defun neomacs-model--show-size (model entry)
  "Give MODEL's glyphs ENTRY's latest pending size, else its accepted one."
  (let ((size (or (cdr (car (last (neomacs-model--entry-resizes entry))))
                  (neomacs-model--entry-accepted-size entry))))
    (unless (and (eql (car size) (neomacs-model--entry-width entry))
                 (eql (cadr size) (neomacs-model--entry-height entry)))
      (setf (neomacs-model--entry-width entry) (car size)
            (neomacs-model--entry-height entry) (cadr size))
      (neomacs-model--update-glyphs model entry))))

(defun neomacs-model--settle-resizes (entry settled)
  "Accept ENTRY's pending resizes up to request SETTLED.
The renderer settles requests in order, so each of them it did not
refuse (see `neomacs-model--refuse-resize') was applied."
  (let (applied)
    (while (and (neomacs-model--entry-resizes entry)
                (<= (car (car (neomacs-model--entry-resizes entry))) settled))
      (setq applied (pop (neomacs-model--entry-resizes entry))))
    (when applied
      (setf (neomacs-model--entry-accepted-size entry) (cdr applied)))))

(defun neomacs-model--refuse-resize (model entry req)
  "Forget the pending resize REQ in MODEL's ENTRY: the renderer refused it.
The glyph shows the newest resize still pending, else the accepted size."
  (let ((resize (assq req (neomacs-model--entry-resizes entry))))
    (when resize
      (setf (neomacs-model--entry-resizes entry)
            (delq resize (neomacs-model--entry-resizes entry)))
      (neomacs-model--show-size model entry))))

(defun neomacs-model-freeze (model)
  "Keep MODEL's last picture and stop interaction and requests.
Irreversible; `neomacs-model-destroy' still works.  Return the
request number."
  (prog1 (neomacs-model--request
          model (lambda (req) (neomacs-model--freeze model req)))
    (setf (neomacs-model--entry-frozen (neomacs-model--entry model)) t)))

;;;; Queries

(defun neomacs-model-view (model)
  "Return the latest camera of MODEL known to Lisp, (YAW PITCH ZOOM)."
  (copy-sequence (neomacs-model--entry-view (neomacs-model--entry model))))

(defun neomacs-model-status (model)
  "Return MODEL's status.
One of (loading), (ready :triangles (N ...) :meshes (ID ...)),
\(failed :error STRING) or (destroyed)."
  (copy-tree (neomacs-model--entry-status (neomacs-model--entry model))))

(defun neomacs-model-current-p (model)
  "Return non-nil if MODEL shows its latest content everywhere.
That is: loaded, not frozen, a renderer live, no interaction in
progress, and the renderer's last readiness settles the latest
request and reports the latest picture presented in every window
showing MODEL."
  (let ((entry (neomacs-model--entry model)))
    ;; A lost renderer presents nothing: its last readiness is void even
    ;; after a replacement starts, until that one reports again.
    (unless (neomacs-model-available-p)
      (setf (neomacs-model--entry-readiness entry) nil))
    (let ((readiness (neomacs-model--entry-readiness entry)))
      (and (eq (car (neomacs-model--entry-status entry)) 'ready)
           (not (neomacs-model--entry-frozen entry))
           (car readiness)
           (eql (nth 2 readiness) (neomacs-model--entry-issued entry))))))

;;;; Destruction

(defun neomacs-model-destroy (model)
  "Free MODEL's renderer resources now.  Idempotent."
  (let ((entry (neomacs-model--entry model)))
    (unless (eq (car (neomacs-model--entry-status entry)) 'destroyed)
      (setf (neomacs-model--entry-status entry) (list 'destroyed)
            (neomacs-model--entry-buffer entry) nil)
      (neomacs-surface-destroy model)))
  nil)

(defun neomacs-model--kill-buffer ()
  "Destroy the models attached to the current buffer."
  (let ((buffer (current-buffer)) doomed)
    (maphash (lambda (model entry)
               (when (eq (neomacs-model--entry-buffer entry) buffer)
                 (push model doomed)))
             neomacs-model--registry)
    (mapc #'neomacs-model-destroy doomed)))

;;;; Events

(defvar neomacs-model--queue nil
  "Callbacks (ID TYPE PLIST) waiting to run, oldest first.
By id, so a queued callback does not keep its model alive.")

(defvar neomacs-model--draining nil
  "Non-nil while `neomacs-model--dispatch' runs callbacks.")

(defun neomacs-model--note (id type plist)
  "Apply the model event (ID TYPE PLIST) and queue its callback.
Runs as soon as the event arrives, even inside a callback, so it
only records state; `neomacs-model--dispatch' runs the callback."
  (let* ((model (neomacs-model--find id))
         (entry (and model (gethash model neomacs-model--registry))))
    (when (and entry
               (not (eq (car (neomacs-model--entry-status entry)) 'destroyed)))
      (pcase type
        ('ready
         (setf (neomacs-model--entry-status entry)
               (list 'ready
                     :triangles (plist-get plist :triangles)
                     :meshes (append (neomacs-model--entry-mesh-ids entry)
                                     nil))))
        ('failed
         (setf (neomacs-model--entry-status entry)
               (list 'failed :error (plist-get plist :error))))
        ('error
         ;; Error{REQ} settles REQ and everything before it; a refused
         ;; resize changes nothing in the renderer.
         (let ((req (plist-get plist :req)))
           (when req
             (neomacs-model--settle-resizes entry (1- req))
             (neomacs-model--refuse-resize model entry req))))
        ('view
         ;; The channel settles a camera request before reporting an
         ;; interaction begun after it.  Until then, these reports can
         ;; only name an older camera, queued before Lisp's request.
         (unless (neomacs-model--entry-pending-view entry)
           (setf (neomacs-model--entry-view entry) (plist-get plist :view))))
        ('readiness
         (setf (neomacs-model--entry-readiness entry)
               (list (plist-get plist :current)
                     (plist-get plist :rev)
                     (plist-get plist :req)))
         (neomacs-model--settle-resizes entry (plist-get plist :req))
         (let ((pending (neomacs-model--entry-pending-view entry)))
           (when (and pending (>= (plist-get plist :req) pending))
             (setf (neomacs-model--entry-pending-view entry) nil))))
        ('pick
         ;; Only a pick of the picture Lisp holds as current names what
         ;; the user saw (design §6.2); anything else is stale.
         (setq plist
               (and (neomacs-model-current-p model)
                    (eql (plist-get plist :rev)
                         (nth 1 (neomacs-model--entry-readiness entry)))
                    (neomacs-model--pick-plist entry plist)))
         (unless plist (setq type nil))))
      (when (and type (neomacs-model--entry-callback entry))
        (setq neomacs-model--queue
              (nconc neomacs-model--queue (list (list id type plist))))))))

(add-hook 'neomacs-model--state-functions #'neomacs-model--note)

(defun neomacs-model--dispatch (&rest _event)
  "Run the queued model callbacks, oldest first.
A member of `neomacs-model-event-functions'; the queue holds the
callbacks of the events `neomacs-model--note' recorded.  An event
that arrives while a callback runs (for example inside its
`sit-for') has its callback run after it instead of nested."
  (unless neomacs-model--draining
    (let ((neomacs-model--draining t))
      (while neomacs-model--queue
        (apply #'neomacs-model--run-callback (pop neomacs-model--queue))))))

(add-hook 'neomacs-model-event-functions #'neomacs-model--dispatch)

(defun neomacs-model--pick-plist (entry plist)
  "Return pick PLIST with its hit mesh index mapped to ENTRY's mesh ID."
  (let ((hit (plist-get plist :hit))
        (copy (copy-sequence plist)))
    (when hit
      (setq hit (copy-sequence hit))
      (plist-put hit :mesh (aref (neomacs-model--entry-mesh-ids entry)
                                 (plist-get hit :mesh)))
      (plist-put copy :hit hit))
    copy))

(defun neomacs-model--run-callback (id type plist)
  "Call the callback of model ID with (MODEL TYPE PLIST), logging any error.
Nothing runs once MODEL is destroyed.  The attached buffer is
current while it is live."
  (let* ((model (neomacs-model--find id))
         (entry (and model (gethash model neomacs-model--registry)))
         (callback (and entry (neomacs-model--entry-callback entry)))
         (buffer (and entry (neomacs-model--entry-buffer entry))))
    (when (and callback
               (not (eq (car (neomacs-model--entry-status entry)) 'destroyed)))
      (condition-case err
          (if (buffer-live-p buffer)
              (with-current-buffer buffer
                (funcall callback model type plist))
            (funcall callback model type plist))
        (error
         (message "neomacs-model: callback for %S failed: %S" type err))))))

;;;; Keymaps and commands

(defun neomacs-model-mouse-select (event)
  "Select the window of mouse EVENT and move point to its position.
Bound on `down-mouse-1' over a model so the press never starts
`mouse-drag-region': the renderer owns the drag (orbit and pick)."
  (interactive "e")
  (let* ((posn (event-start event))
         (window (posn-window posn))
         (pos (posn-point posn)))
    (when (window-live-p window)
      (select-window window)
      (when (integer-or-marker-p pos)
        (goto-char pos)))))

(defvar neomacs-model-mouse-map
  (let ((map (make-sparse-keymap)))
    (define-key map [down-mouse-1] #'neomacs-model-mouse-select)
    (define-key map [mouse-1] #'ignore)
    (define-key map [drag-mouse-1] #'ignore)
    map)
  "Mouse bindings every model glyph has, searched before its keymap.")

(defvar-local neomacs-model--buffer-model nil
  "The model of a `neomacs-model-view-mode' buffer.")

(defun neomacs-model--at-point ()
  "Return the model at point, or the current buffer's model."
  (or (get-char-property (point) 'neomacs-model)
      (and (> (point) (point-min))
           (get-char-property (1- (point)) 'neomacs-model))
      neomacs-model--buffer-model
      (user-error "No model here")))

(defun neomacs-model--nudge (dyaw dpitch zoom-factor)
  "Turn the model at point by DYAW, DPITCH degrees; scale zoom by ZOOM-FACTOR."
  (let* ((model (neomacs-model--at-point))
         (view (neomacs-model-view model)))
    (neomacs-model-set-view model
                            (+ (nth 0 view) dyaw)
                            (+ (nth 1 view) dpitch)
                            (min 4.0 (max 0.25 (* (nth 2 view) zoom-factor))))))

(defun neomacs-model-yaw-forward ()
  "Turn the model at point by 15 degrees of yaw."
  (interactive)
  (neomacs-model--nudge 15 0 1))

(defun neomacs-model-yaw-backward ()
  "Turn the model at point by -15 degrees of yaw."
  (interactive)
  (neomacs-model--nudge -15 0 1))

(defun neomacs-model-pitch-next ()
  "Tilt the model at point by 15 degrees of pitch."
  (interactive)
  (neomacs-model--nudge 0 15 1))

(defun neomacs-model-pitch-previous ()
  "Tilt the model at point by -15 degrees of pitch."
  (interactive)
  (neomacs-model--nudge 0 -15 1))

(defun neomacs-model-zoom-in ()
  "Zoom the model at point in by 10%."
  (interactive)
  (neomacs-model--nudge 0 0 1.1))

(defun neomacs-model-zoom-out ()
  "Zoom the model at point out by 10%."
  (interactive)
  (neomacs-model--nudge 0 0 (/ 1 1.1)))

(defun neomacs-model-reset-view ()
  "Return the model at point to the view it was created with."
  (interactive)
  (let ((model (neomacs-model--at-point)))
    (apply #'neomacs-model-set-view model
           (neomacs-model--entry-initial-view (neomacs-model--entry model)))))

(defvar neomacs-model-view-map
  (let ((map (make-sparse-keymap)))
    (define-key map "f" #'neomacs-model-yaw-forward)
    (define-key map "b" #'neomacs-model-yaw-backward)
    (define-key map "n" #'neomacs-model-pitch-next)
    (define-key map "p" #'neomacs-model-pitch-previous)
    (define-key map "+" #'neomacs-model-zoom-in)
    (define-key map "-" #'neomacs-model-zoom-out)
    (define-key map "0" #'neomacs-model-reset-view)
    map)
  "Default keymap of an inserted model glyph: camera keys.")

(defvar neomacs-model-view-mode-map
  (let ((map (make-sparse-keymap)))
    (set-keymap-parent map (make-composed-keymap neomacs-model-view-map
                                                 special-mode-map))
    (define-key map "q" #'quit-window)
    map)
  "Keymap of `neomacs-model-view-mode': camera keys and `q'.")

(define-derived-mode neomacs-model-view-mode special-mode "Model"
  "Major mode of a standalone native model buffer.
\\{neomacs-model-view-mode-map}")

;;;; Insertion

(defun neomacs-model--display-spec (model entry)
  "Return MODEL's display spec at the size recorded in ENTRY."
  (list 'surface :id model
        :width (neomacs-model--entry-width entry)
        :height (neomacs-model--entry-height entry)))

(defun neomacs-model--map-glyphs (buffer model function)
  "Call FUNCTION with START END of each MODEL glyph in BUFFER."
  (when (buffer-live-p buffer)
    (with-current-buffer buffer
      (save-restriction
        (widen)
        (let ((pos (point-min)) next)
          (while (< pos (point-max))
            (setq next (next-single-property-change pos 'neomacs-model
                                                    nil (point-max)))
            (when (eq (get-text-property pos 'neomacs-model) model)
              (funcall function pos next))
            (setq pos next)))))))

(defun neomacs-model--update-glyphs (model entry)
  "Give MODEL's glyphs in its attached buffer ENTRY's current size."
  (neomacs-model--map-glyphs
   (neomacs-model--entry-buffer entry) model
   (lambda (start end)
     (with-silent-modifications
       (put-text-property start end 'display
                          (neomacs-model--display-spec model entry))))))

(defun neomacs-model--detach (model entry)
  "Blank MODEL's glyphs in the buffer ENTRY attaches it to; forget it.
Nothing of MODEL stays there: a later resize cannot revive an old
glyph, and the camera keys no longer find MODEL in that buffer."
  (let ((old (neomacs-model--entry-buffer entry)))
    (neomacs-model--map-glyphs
     old model
     (lambda (start end)
       (with-silent-modifications
         (remove-text-properties start end
                                 '(display nil keymap nil neomacs-model nil)))))
    (when (buffer-live-p old)
      (with-current-buffer old
        (when (eq neomacs-model--buffer-model model)
          (setq neomacs-model--buffer-model nil))))
    (setf (neomacs-model--entry-buffer entry) nil)))

(defun neomacs-model-insert (model &rest plist)
  "Insert MODEL's glyph at point and attach MODEL to the current buffer.
A model is attached to one buffer: inserting it elsewhere moves it,
and killing that buffer destroys MODEL.  PLIST keys:
:keymap  the glyph's keymap (default `neomacs-model-view-map', nil
         for none); `neomacs-model-mouse-map' always comes first.
:string  the text carrying the glyph, not empty (default \" \")."
  (let* ((entry (neomacs-model--entry model))
         (keymap (if (plist-member plist :keymap)
                     (plist-get plist :keymap)
                   neomacs-model-view-map))
         (string (or (plist-get plist :string) " ")))
    (when (eq (car (neomacs-model--entry-status entry)) 'destroyed)
      (error "Model is destroyed"))
    (unless (and (stringp string) (> (length string) 0))
      (error "Model :string must be a non-empty string"))
    ;; Do not transfer attachment until insertion succeeds: a read-only
    ;; destination must leave the old glyph live.
    (insert (propertize string
                        'display (neomacs-model--display-spec model entry)
                        'keymap (make-composed-keymap
                                 (delq nil (list neomacs-model-mouse-map
                                                 keymap)))
                        'neomacs-model model
                        'rear-nonsticky t))
    (unless (eq (neomacs-model--entry-buffer entry) (current-buffer))
      (neomacs-model--detach model entry))
    (setf (neomacs-model--entry-buffer entry) (current-buffer))
    (add-hook 'kill-buffer-hook #'neomacs-model--kill-buffer nil t)
    nil))

;;;###autoload
(defun neomacs-model-open (meshes &rest plist)
  "Show MESHES in a buffer *Model: NAME* in `neomacs-model-view-mode'.
MESHES and PLIST are as for `neomacs-model-create', plus :name
\(default: the first mesh ID).  Return the buffer."
  (let* ((name (or (plist-get plist :name) (car (car meshes))))
         (model (apply #'neomacs-model-create :meshes meshes plist))
         (buffer (get-buffer-create (format "*Model: %s*" name))))
    (with-current-buffer buffer
      ;; A reused buffer drops the model it showed before.
      (neomacs-model--kill-buffer)
      (neomacs-model-view-mode)
      (setq neomacs-model--buffer-model model)
      (let ((inhibit-read-only t))
        (erase-buffer)
        (neomacs-model-insert model))
      (goto-char (point-min)))
    (pop-to-buffer buffer)
    buffer))

(provide 'neomacs-model)
;;; neomacs-model.el ends here
