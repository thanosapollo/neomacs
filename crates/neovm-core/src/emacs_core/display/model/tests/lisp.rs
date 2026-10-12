//! `lisp/neomacs-model.el` tests: the real library loaded into a
//! bootstrapped runtime over the recording host of `model_test`, driven
//! through the same keyboard handler the frontend's `InputEvent::Model`
//! reaches.

use super::eval::{Context, ShaderSurfaceContent};
use super::model_test::{
    FIRST_MODEL_ID, RecordingModelDisplayHost, eval_printed, obj_file, scratch_dir, signal_of,
    updates,
};
use neomacs_display_protocol::model::{ModelChange, ModelEvent, ModelHit, ModelTarget, ModelView};

/// A runtime-startup context with the recording host and the real
/// `lisp/neomacs-model.el` loaded. Two mesh files' paths are in
/// `model-test-a` and `model-test-b`.
fn lisp_context(test: &str) -> (Context, RecordingModelDisplayHost) {
    let host = RecordingModelDisplayHost::default();
    let mut ctx = crate::test_utils::runtime_startup_context();
    ctx.set_display_host(Box::new(host.clone()));
    let library = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../lisp/neomacs-model.el")
        .canonicalize()
        .expect("lisp/neomacs-model.el");
    let dir = scratch_dir(test);
    let (a, b) = (obj_file(&dir, "a.obj"), obj_file(&dir, "b.obj"));
    eval_printed(
        &mut ctx,
        &format!(
            r#"(progn
             (load "{}" nil t)
             (setq model-test-a "{a}" model-test-b "{b}" model-test-log nil))"#,
            library.display()
        ),
    );
    (ctx, host)
}

/// Create `model-test-m` (meshes "a", "b") whose callback pushes
/// `(TYPE PLIST)` onto `model-test-log`.
fn create_model(ctx: &mut Context) {
    eval_printed(
        ctx,
        r#"(setq model-test-m
                 (neomacs-model-create
                  :meshes (list (list "a" model-test-a) (list "b" model-test-b nil))
                  :width 64 :height 48 :background '(0.5 0 1)
                  :callback (lambda (_model type plist)
                              (push (list type plist) model-test-log))))"#,
    );
}

fn deliver(ctx: &mut Context, event: ModelEvent) {
    ctx.handle_model_input_event(FIRST_MODEL_ID, &event)
        .expect("model event hook runs");
}

fn readiness(ctx: &mut Context, current: bool, rev: u64, req: u64) {
    deliver(ctx, ModelEvent::Readiness { current, rev, req });
}

fn current_p(ctx: &mut Context) -> String {
    eval_printed(ctx, "(neomacs-model-current-p model-test-m)")
}

#[test]
fn revision_observes_synchronous_state_before_deferred_callbacks() {
    let (mut ctx, host) = lisp_context("lisp-revision");
    create_model(&mut ctx);
    let revision = |ctx: &mut Context| eval_printed(ctx, "(neomacs-model-revision model-test-m)");
    assert_eq!(revision(&mut ctx), "nil");
    // The native state hook records these before the ordinary event hook
    // drains callbacks. Querying must not depend on a delivered callback.
    eval_printed(
        &mut ctx,
        &format!(
            "(progn (neomacs-model--note {FIRST_MODEL_ID} 'ready '(:triangles (1 1)))          (neomacs-model--note {FIRST_MODEL_ID} 'readiness '(:current t :rev 8 :req 0)))"
        ),
    );
    assert_eq!(eval_printed(&mut ctx, "model-test-log"), "nil");
    assert_eq!(revision(&mut ctx), "8");
    assert_eq!(eval_printed(&mut ctx, "(null neomacs-model--queue)"), "nil");
    readiness(&mut ctx, false, 9, 0);
    assert_eq!(revision(&mut ctx), "nil");
    readiness(&mut ctx, true, 12, 0);
    assert_eq!(revision(&mut ctx), "12");
    eval_printed(&mut ctx, "(neomacs-model-set-view model-test-m 10 20 1)");
    assert_eq!(revision(&mut ctx), "nil");
    readiness(&mut ctx, true, 13, 1);
    assert_eq!(revision(&mut ctx), "13");
    host.live.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(revision(&mut ctx), "nil");
    host.live.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(revision(&mut ctx), "nil", "replacement needs a new fence");
    readiness(&mut ctx, true, 14, 1);
    assert_eq!(revision(&mut ctx), "14");
    eval_printed(&mut ctx, "(neomacs-model-freeze model-test-m)");
    assert_eq!(revision(&mut ctx), "nil");
    eval_printed(&mut ctx, "(neomacs-model-destroy model-test-m)");
    assert_eq!(revision(&mut ctx), "nil");
}

#[test]
fn create_registers_the_model_and_tracks_status_and_readiness() {
    let (mut ctx, host) = lisp_context("lisp-lifecycle");
    create_model(&mut ctx);
    {
        let creates = host.creates.lock().expect("creates");
        let request = creates.last().expect("create reached the host");
        assert_eq!((request.width, request.height), (64, 48));
        match &request.content {
            ShaderSurfaceContent::Model {
                meshes,
                view,
                background,
            } => {
                assert_eq!(meshes.len(), 2);
                assert_eq!(*view, ModelView::default());
                assert_eq!(*background, [0.5, 0.0, 1.0, 1.0]);
            }
            other => panic!("expected model content, got {other:?}"),
        }
    }
    let el = eval_printed;
    assert_eq!(
        el(&mut ctx, "(neomacs-model-status model-test-m)"),
        "(loading)"
    );
    assert_eq!(el(&mut ctx, "(neomacs-model-view model-test-m)"), "(0 0 1)");
    assert_eq!(current_p(&mut ctx), "nil");
    assert_eq!(
        el(&mut ctx, "(hash-table-count neomacs-model--registry)"),
        "1"
    );

    deliver(
        &mut ctx,
        ModelEvent::Ready {
            triangles: vec![1, 1],
        },
    );
    assert_eq!(
        el(&mut ctx, "(neomacs-model-status model-test-m)"),
        r#"(ready :triangles (1 1) :meshes ("a" "b"))"#
    );
    assert_eq!(current_p(&mut ctx), "nil", "no readiness yet");
    readiness(&mut ctx, true, 1, 0);
    assert_eq!(current_p(&mut ctx), "t");

    // A new request turns current-p nil synchronously.
    assert_eq!(
        el(&mut ctx, "(neomacs-model-set-view model-test-m 10 20 1.5)"),
        "1"
    );
    assert_eq!(current_p(&mut ctx), "nil");
    assert_eq!(
        el(&mut ctx, "(neomacs-model-view model-test-m)"),
        "(10 20 1.5)"
    );
    assert_eq!(
        updates(&host),
        vec![(
            FIRST_MODEL_ID,
            1,
            ModelChange::View(ModelView {
                yaw: 10.0,
                pitch: 20.0,
                zoom: 1.5,
            }),
        )]
    );
    // An acknowledgement of an older request does not restore it ...
    readiness(&mut ctx, true, 2, 0);
    assert_eq!(current_p(&mut ctx), "nil");
    // ... the one settling the latest request does.
    readiness(&mut ctx, true, 3, 1);
    assert_eq!(current_p(&mut ctx), "t");
    // A renderer-side change (an orbit) drops it until presented.
    readiness(&mut ctx, false, 4, 1);
    assert_eq!(current_p(&mut ctx), "nil");
    deliver(
        &mut ctx,
        ModelEvent::View {
            view: ModelView {
                yaw: 30.0,
                pitch: -90.0,
                zoom: 1.25,
            },
            rev: 5,
        },
    );
    readiness(&mut ctx, true, 5, 1);
    assert_eq!(current_p(&mut ctx), "t");
    assert_eq!(
        el(&mut ctx, "(neomacs-model-view model-test-m)"),
        "(30.0 -90.0 1.25)"
    );
    assert_eq!(
        el(&mut ctx, "(mapcar #'car (reverse model-test-log))"),
        "(ready readiness readiness readiness readiness view readiness)"
    );

    // Freezing is a request too, and current-p stays nil afterwards.
    assert_eq!(el(&mut ctx, "(neomacs-model-freeze model-test-m)"), "2");
    readiness(&mut ctx, true, 6, 2);
    assert_eq!(current_p(&mut ctx), "nil");
    assert_eq!(
        signal_of(&mut ctx, "(neomacs-model-set-view model-test-m 0 0 1)"),
        r#"(user-error "Model is frozen")"#
    );

    // Failed stays queryable; destroy is idempotent.
    deliver(
        &mut ctx,
        ModelEvent::Failed {
            error: "device lost".to_owned(),
        },
    );
    assert_eq!(
        el(&mut ctx, "(neomacs-model-status model-test-m)"),
        r#"(failed :error "device lost")"#
    );
    eval_printed(
        &mut ctx,
        "(progn (neomacs-model-destroy model-test-m) (neomacs-model-destroy model-test-m))",
    );
    assert_eq!(
        el(&mut ctx, "(neomacs-model-status model-test-m)"),
        "(destroyed)"
    );
    assert_eq!(
        *host.destroys.lock().expect("destroys"),
        vec![FIRST_MODEL_ID]
    );
    // Events for a destroyed model are dropped.
    let before = el(&mut ctx, "(length model-test-log)");
    readiness(&mut ctx, true, 7, 2);
    assert_eq!(el(&mut ctx, "(length model-test-log)"), before);
}

#[test]
fn picks_are_delivered_only_for_the_current_picture() {
    let (mut ctx, _host) = lisp_context("lisp-picks");
    create_model(&mut ctx);
    deliver(
        &mut ctx,
        ModelEvent::Ready {
            triangles: vec![1, 1],
        },
    );
    readiness(&mut ctx, true, 7, 0);
    let pick = |hit: Option<ModelHit>, rev: u64| ModelEvent::Pick {
        hit,
        view: ModelView::default(),
        rev,
        req: 0,
    };
    let hit = ModelHit {
        mesh: 1,
        face: 2,
        barycentric: [0.25, 0.25, 0.5],
        point: [1.0, 2.0, 3.0],
    };
    // A pick of an older picture is stale.
    deliver(&mut ctx, pick(Some(hit), 6));
    // The current picture: delivered, the mesh index mapped to its ID.
    deliver(&mut ctx, pick(Some(hit), 7));
    deliver(&mut ctx, pick(None, 7));
    // After a new request the picture is no longer current.
    eval_printed(&mut ctx, "(neomacs-model-set-view model-test-m 0 0 2)");
    deliver(&mut ctx, pick(Some(hit), 7));
    assert_eq!(
        eval_printed(
            &mut ctx,
            "(delq nil (mapcar (lambda (e) (and (eq (car e) 'pick) (cadr e))) \
             (reverse model-test-log)))"
        ),
        "((:hit (:mesh \"b\" :face 2 :barycentric [0.25 0.25 0.5] :point [1.0 2.0 3.0]) \
         :view (0.0 0.0 1.0) :rev 7 :req 0) \
         (:hit nil :view (0.0 0.0 1.0) :rev 7 :req 0))"
    );
}

#[test]
fn callback_errors_are_logged_and_nested_events_are_queued() {
    let (mut ctx, _host) = lisp_context("lisp-reentrancy");
    eval_printed(
        &mut ctx,
        &format!(
            r#"(setq model-test-m
                 (neomacs-model-create
                  :meshes (list (list "a" model-test-a))
                  :callback
                  (lambda (_model type _plist)
                    (push type model-test-log)
                    (when (eq type 'ready)
                      ;; An event arriving while this callback runs (as
                      ;; inside its `sit-for') must wait for it to return,
                      ;; even when both hooks are run for it directly.
                      (run-hook-with-args 'neomacs-model--state-functions
                       {FIRST_MODEL_ID} 'readiness '(:current t :rev 1 :req 0))
                      (run-hook-with-args 'neomacs-model-event-functions
                       {FIRST_MODEL_ID} 'readiness '(:current t :rev 1 :req 0))
                      (push 'ready-end model-test-log)
                      (error "Model test boom")))))"#
        ),
    );
    deliver(&mut ctx, ModelEvent::Ready { triangles: vec![1] });
    assert_eq!(
        eval_printed(&mut ctx, "(reverse model-test-log)"),
        "(ready ready-end readiness)"
    );
    assert_eq!(current_p(&mut ctx), "t");
    assert_eq!(
        eval_printed(
            &mut ctx,
            r#"(with-current-buffer (messages-buffer)
                 (and (string-match-p "Model test boom" (buffer-string)) t))"#
        ),
        "t",
        "the callback error is logged to *Messages*"
    );
    assert_eq!(eval_printed(&mut ctx, "neomacs-model--queue"), "nil");
}

#[test]
fn insert_composes_keymaps_resizes_moves_and_dies_with_its_buffer() {
    let (mut ctx, host) = lisp_context("lisp-insert");
    create_model(&mut ctx);
    // A consumer map binding all three mouse events still yields the
    // mandatory bindings (design §5.3).
    let printed = eval_printed(
        &mut ctx,
        r#"(let ((consumer (make-sparse-keymap)))
             (define-key consumer [down-mouse-1] 'model-test-down)
             (define-key consumer [mouse-1] 'model-test-click)
             (define-key consumer [drag-mouse-1] 'model-test-drag)
             (define-key consumer "x" 'model-test-x)
             (with-current-buffer (get-buffer-create "model-test-1")
               (neomacs-model-insert model-test-m :keymap consumer)
               (let ((map (get-text-property (point-min) 'keymap))
                     (display (get-text-property (point-min) 'display)))
                 (list (lookup-key map [down-mouse-1])
                       (lookup-key map [mouse-1])
                       (lookup-key map [drag-mouse-1])
                       (lookup-key map "x")
                       (car display)
                       (eq (plist-get (cdr display) :id) model-test-m)
                       (plist-get (cdr display) :width)
                       (plist-get (cdr display) :height)
                       (buffer-substring-no-properties (point-min) (point-max))))))"#,
    );
    assert_eq!(
        printed,
        r#"(neomacs-model-mouse-select ignore ignore model-test-x surface t 64 48 " ")"#
    );
    // Default keymap: camera keys; explicit nil: the mouse map only. (A
    // second model, so `model-test-m' stays attached to model-test-1.)
    assert_eq!(
        eval_printed(
            &mut ctx,
            r#"(let ((other (setq model-test-other
                                  (neomacs-model-create
                                   :meshes (list (list "a" model-test-a))))))
               (with-current-buffer (get-buffer-create "model-test-keys")
                 (neomacs-model-insert other :string "M")
                 (neomacs-model-insert other :keymap nil)
                 (list (buffer-substring-no-properties (point-min) (point-max))
                       (lookup-key (get-text-property 1 'keymap) "f")
                       (lookup-key (get-text-property 2 'keymap) "f")
                       (lookup-key (get-text-property 2 'keymap) [mouse-1]))))"#
        ),
        r#"("M " neomacs-model-yaw-forward nil ignore)"#
    );

    // Resize follows into the attached buffer's display spec.
    eval_printed(
        &mut ctx,
        r#"(with-current-buffer "model-test-1"
             (neomacs-model-insert model-test-m))"#,
    );
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model-resize model-test-m 128 96)"),
        "1"
    );
    assert_eq!(
        eval_printed(
            &mut ctx,
            r#"(with-current-buffer "model-test-1"
                 (mapcar (lambda (pos)
                           (let ((display (get-text-property pos 'display)))
                             (list (plist-get (cdr display) :width)
                                   (plist-get (cdr display) :height))))
                         (list 1 2)))"#
        ),
        "((128 96) (128 96))"
    );
    assert_eq!(
        updates(&host).last(),
        Some(&(
            FIRST_MODEL_ID,
            1,
            ModelChange::Resize {
                width: 128,
                height: 96,
            },
        ))
    );

    // Inserting elsewhere moves the model; the old glyph shows nothing.
    assert_eq!(
        eval_printed(
            &mut ctx,
            r#"(progn
                 (with-current-buffer (get-buffer-create "model-test-2")
                   (neomacs-model-insert model-test-m))
                 (with-current-buffer "model-test-1"
                   (get-text-property (point-min) 'display)))"#
        ),
        "nil"
    );
    // Killing the old buffer leaves the model alive; killing the attached
    // one destroys it.
    eval_printed(&mut ctx, r#"(kill-buffer "model-test-1")"#);
    assert!(host.destroys.lock().expect("destroys").is_empty());
    eval_printed(&mut ctx, r#"(kill-buffer "model-test-2")"#);
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model-status model-test-m)"),
        "(destroyed)"
    );
    assert_eq!(
        *host.destroys.lock().expect("destroys"),
        vec![FIRST_MODEL_ID]
    );
    let printed = signal_of(&mut ctx, "(neomacs-model-set-view model-test-m 0 0 1)");
    assert!(printed.contains("Model is destroyed"), "{printed}");
}

#[test]
fn set_highlight_maps_mesh_ids_to_indices() {
    let (mut ctx, host) = lisp_context("lisp-highlight");
    create_model(&mut ctx);
    eval_printed(
        &mut ctx,
        r#"(progn
             (neomacs-model-set-highlight
              model-test-m
              :target '(:mesh "b" :kind point :face 2 :barycentric [0.2 0.3 0.5]
                        :tolerance 0.1)
              :selected "a")
             (neomacs-model-set-highlight
              model-test-m :target '(:mesh "a" :kind region :faces (3 1)))
             (neomacs-model-set-highlight model-test-m :selected "b"))"#,
    );
    let id = FIRST_MODEL_ID;
    let highlight = |target, selected| ModelChange::Highlight { target, selected };
    assert_eq!(
        updates(&host),
        vec![
            (
                id,
                1,
                highlight(
                    Some(ModelTarget::Point {
                        mesh: 1,
                        face: 2,
                        barycentric: [0.2, 0.3, 0.5],
                        tolerance: 0.1,
                    }),
                    Some(0),
                ),
            ),
            (
                id,
                2,
                highlight(
                    Some(ModelTarget::Region {
                        mesh: 0,
                        faces: vec![3, 1],
                    }),
                    None,
                ),
            ),
            (id, 3, highlight(None, Some(1))),
        ]
    );
    let printed = signal_of(
        &mut ctx,
        r#"(neomacs-model-set-highlight model-test-m :target '(:mesh "zz" :kind object))"#,
    );
    assert!(printed.contains("Unknown model mesh"), "{printed}");
    // A rejected request consumes no request number.
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model-set-highlight model-test-m)"),
        "4"
    );
    assert_eq!(updates(&host).len(), 4);
}

#[test]
fn open_shows_a_standalone_buffer_with_camera_keys() {
    let (mut ctx, host) = lisp_context("lisp-open");
    let printed = eval_printed(
        &mut ctx,
        r#"(let ((buffer (neomacs-model-open (list (list "a" model-test-a))
                                             :name "Arm")))
             (with-current-buffer buffer
               (neomacs-model-yaw-forward)
               (neomacs-model-pitch-previous)
               (neomacs-model-zoom-in)
               (list (buffer-name) major-mode buffer-read-only
                     (lookup-key neomacs-model-view-mode-map "q")
                     (lookup-key neomacs-model-view-mode-map "f")
                     (neomacs-model-view neomacs-model--buffer-model)
                     (progn (neomacs-model-reset-view)
                            (neomacs-model-view neomacs-model--buffer-model)))))"#,
    );
    assert_eq!(
        printed,
        r#"("*Model: Arm*" neomacs-model-view-mode t quit-window neomacs-model-yaw-forward (15 345 1.1) (0 0 1))"#
    );
    assert_eq!(updates(&host).len(), 4);
    // Zoom keys stay inside 0.25..4.
    eval_printed(
        &mut ctx,
        r#"(with-current-buffer "*Model: Arm*"
             (dotimes (_ 30) (neomacs-model-zoom-in)))"#,
    );
    assert_eq!(
        eval_printed(
            &mut ctx,
            r#"(with-current-buffer "*Model: Arm*"
                 (nth 2 (neomacs-model-view neomacs-model--buffer-model)))"#
        ),
        "4.0"
    );
}

#[test]
fn pending_camera_is_not_overwritten_by_older_queued_interaction() {
    let (mut ctx, _host) = lisp_context("lisp-pending-camera");
    create_model(&mut ctx);
    deliver(
        &mut ctx,
        ModelEvent::Ready {
            triangles: vec![1, 1],
        },
    );
    readiness(&mut ctx, true, 1, 0);
    eval_printed(&mut ctx, "(neomacs-model-set-view model-test-m 10 20 2)");
    // An interaction report already queued when Lisp issued request 1.
    deliver(
        &mut ctx,
        ModelEvent::View {
            view: ModelView {
                yaw: -45.0,
                pitch: 0.0,
                zoom: 1.0,
            },
            rev: 2,
        },
    );
    readiness(&mut ctx, true, 2, 0);
    assert_eq!(current_p(&mut ctx), "nil");
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model-view model-test-m)"),
        "(10 20 2)"
    );
    readiness(&mut ctx, true, 3, 1);
    assert_eq!(current_p(&mut ctx), "t");
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model-view model-test-m)"),
        "(10 20 2)"
    );
    // An interaction begun after settlement is authoritative again.
    readiness(&mut ctx, false, 4, 1);
    deliver(
        &mut ctx,
        ModelEvent::View {
            view: ModelView {
                yaw: 25.0,
                pitch: 20.0,
                zoom: 2.0,
            },
            rev: 4,
        },
    );
    readiness(&mut ctx, true, 4, 1);
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model-view model-test-m)"),
        "(25.0 20.0 2.0)"
    );
}

#[test]
fn rejected_insert_preserves_attachment_and_callbacks_use_that_buffer() {
    let (mut ctx, host) = lisp_context("lisp-insert-failure");
    create_model(&mut ctx);
    eval_printed(
        &mut ctx,
        r#"(progn
      (with-current-buffer (get-buffer-create "model-owner")
        (neomacs-model-insert model-test-m))
      (setf (neomacs-model--entry-callback
             (neomacs-model--entry model-test-m))
            (lambda (_model _event _plist)
              (setq model-test-callback-buffer (buffer-name)))))"#,
    );
    let signal = signal_of(
        &mut ctx,
        r#"(with-current-buffer
         (get-buffer-create "model-read-only")
       (setq buffer-read-only t)
       (neomacs-model-insert model-test-m))"#,
    );
    assert!(signal.starts_with("(buffer-read-only"), "{signal}");
    assert_eq!(
        eval_printed(
            &mut ctx,
            r#"(with-current-buffer "model-owner"
      (eq (plist-get (cdr (get-text-property 1 'display)) :id) model-test-m))"#
        ),
        "t"
    );
    deliver(
        &mut ctx,
        ModelEvent::Ready {
            triangles: vec![1, 1],
        },
    );
    assert_eq!(
        eval_printed(&mut ctx, "model-test-callback-buffer"),
        r#""model-owner""#
    );
    eval_printed(&mut ctx, r#"(kill-buffer "model-read-only")"#);
    assert!(host.destroys.lock().expect("destroys").is_empty());
    eval_printed(&mut ctx, r#"(kill-buffer "model-owner")"#);
    assert_eq!(
        *host.destroys.lock().expect("destroys"),
        vec![FIRST_MODEL_ID]
    );
}

/// Queue `events` as frontend input, as the render thread's ordered input
/// channel delivers them: model events and keys interleaved.
fn queue_input(ctx: &mut Context, events: Vec<crate::keyboard::InputEvent>) {
    for event in events {
        ctx.command_loop
            .keyboard
            .pending_input_events
            .push_back(event);
    }
}

fn model_input(event: ModelEvent) -> crate::keyboard::InputEvent {
    crate::keyboard::InputEvent::Model {
        id: FIRST_MODEL_ID,
        event,
    }
}

fn key_input(c: char) -> crate::keyboard::InputEvent {
    crate::keyboard::InputEvent::key_press(crate::keyboard::KeyEvent::char(c))
}

/// A ready, current `model-test-m` whose callback logs `(TYPE NESTED)`
/// and, for a `view' event, runs WAIT (a nested wait or input reader)
/// and logs `(key KEY CURRENT-P)` for the key it then reads. Two more
/// members follow `neomacs-model--dispatch' on the event hook: one that
/// always signals and one that logs `(hook TYPE NESTED)`.
fn nested_delivery_context(test: &str, wait: &str) -> Context {
    let (mut ctx, _host) = lisp_context(test);
    create_model(&mut ctx);
    deliver(
        &mut ctx,
        ModelEvent::Ready {
            triangles: vec![1, 1],
        },
    );
    readiness(&mut ctx, true, 1, 0);
    assert_eq!(current_p(&mut ctx), "t");
    eval_printed(
        &mut ctx,
        &format!(
            r#"(progn
             (defvar model-test-active nil)
             (setq model-test-log nil model-test-hooks nil)
             (setf (neomacs-model--entry-callback
                    (neomacs-model--entry model-test-m))
                   (lambda (model type _plist)
                     (push (list type model-test-active) model-test-log)
                     (when (eq type 'view)
                       (let ((model-test-active t))
                         {wait}
                         (let ((key (read-event)))
                           (push (list 'key key (neomacs-model-current-p model)
                                       (neomacs-model-revision model))
                                 model-test-log))))))
             (add-hook 'neomacs-model-event-functions
                       (lambda (_id _type _plist) (error "Model test hook boom"))
                       t)
             (add-hook 'neomacs-model-event-functions
                       (lambda (_id type _plist)
                         (push (list 'hook type model-test-active)
                               model-test-hooks))
                       t))"#
        ),
    );
    ctx
}

/// The view event's callback waits; behind it on the channel come
/// R{nil} and then the key it reads.
fn nested_delivery_input() -> Vec<crate::keyboard::InputEvent> {
    vec![
        model_input(ModelEvent::View {
            view: ModelView {
                yaw: 30.0,
                pitch: 0.0,
                zoom: 1.0,
            },
            rev: 2,
        }),
        model_input(ModelEvent::Readiness {
            current: false,
            rev: 3,
            req: 0,
        }),
        key_input('x'),
        key_input('y'),
    ]
}

fn assert_nested_delivery(ctx: &mut Context, log: &str) {
    // The callback for R{nil} ran after the view callback returned, but
    // the key read inside it already saw R{nil}: current-p is nil.
    assert_eq!(eval_printed(ctx, "(reverse model-test-log)"), log);
    // Every member saw both events, in order, none nested; the signalling
    // member stopped neither the others nor the delivery.
    assert_eq!(
        eval_printed(ctx, "(reverse model-test-hooks)"),
        "((hook view nil) (hook readiness nil))"
    );
    assert_eq!(
        eval_printed(
            ctx,
            r#"(with-current-buffer (messages-buffer)
                 (let ((n 0) (start 0))
                   (while (string-match "Model test hook boom" (buffer-string) start)
                     (setq n (1+ n) start (match-end 0)))
                   n))"#
        ),
        "2",
        "each member error is logged to *Messages*"
    );
    assert_eq!(current_p(ctx), "nil");
}

#[test]
fn nested_input_reader_sees_readiness_queued_behind_the_callback() {
    // Route: the read-event path (`handle_read_char_input_event`).
    let mut ctx = nested_delivery_context("lisp-nested-read", "nil");
    queue_input(&mut ctx, nested_delivery_input());
    assert_eq!(eval_printed(&mut ctx, "(read-event)"), "121");
    assert_nested_delivery(&mut ctx, "((view nil) (key 120 nil nil) (readiness nil))");
}

#[test]
fn nested_wait_sees_readiness_queued_behind_the_callback() {
    // Route: special input serviced inside a wait
    // (`service_wait_request_special_input_events`), both for the outer
    // delivery and inside the callback.
    let mut ctx = nested_delivery_context(
        "lisp-nested-wait",
        "(accept-process-output nil 0.05)
         (push (list 'waited (neomacs-model-current-p model)) model-test-log)",
    );
    queue_input(&mut ctx, nested_delivery_input());
    assert_eq!(
        eval_printed(
            &mut ctx,
            "(progn (accept-process-output nil 0.05) (read-event))"
        ),
        "121"
    );
    assert_nested_delivery(
        &mut ctx,
        "((view nil) (waited nil) (key 120 nil nil) (readiness nil))",
    );
}

#[test]
fn nested_reader_and_wait_see_current_revision_before_deferred_callback() {
    for (name, wait) in [
        ("lisp-nested-current-read", "nil"),
        (
            "lisp-nested-current-wait",
            "(accept-process-output nil 0.05)",
        ),
    ] {
        let mut ctx = nested_delivery_context(name, wait);
        let mut input = nested_delivery_input();
        input[1] = model_input(ModelEvent::Readiness {
            current: true,
            rev: 7,
            req: 0,
        });
        queue_input(&mut ctx, input);
        assert_eq!(eval_printed(&mut ctx, "(read-event)"), "121");
        assert_eq!(
            eval_printed(&mut ctx, "(reverse model-test-log)"),
            "((view nil) (key 120 t 7) (readiness nil))"
        );
        assert_eq!(
            eval_printed(&mut ctx, "(reverse model-test-hooks)"),
            "((hook view nil) (hook readiness nil))"
        );
    }
}

#[test]
fn renderer_loss_makes_current_p_nil_before_a_later_key() {
    let (mut ctx, host) = lisp_context("lisp-renderer-loss");
    create_model(&mut ctx);
    deliver(
        &mut ctx,
        ModelEvent::Ready {
            triangles: vec![1, 1],
        },
    );
    readiness(&mut ctx, true, 1, 0);
    assert_eq!(current_p(&mut ctx), "t");

    // Loss reported only through availability (no readiness event yet):
    // the key read afterwards must not see the old acknowledgement.
    host.live.store(false, std::sync::atomic::Ordering::SeqCst);
    queue_input(&mut ctx, vec![key_input('x')]);
    assert_eq!(
        eval_printed(
            &mut ctx,
            "(list (read-event) (neomacs-model-current-p model-test-m) \
             (neomacs-model-available-p))"
        ),
        "(120 nil nil)"
    );
    // The store's R{nil} at loss, then a later key, still no renderer.
    queue_input(
        &mut ctx,
        vec![
            model_input(ModelEvent::Readiness {
                current: false,
                rev: 2,
                req: 0,
            }),
            key_input('y'),
        ],
    );
    assert_eq!(
        eval_printed(
            &mut ctx,
            "(list (read-event) (neomacs-model-current-p model-test-m))"
        ),
        "(121 nil)"
    );
    // A replacement renderer alone does not revive the pre-loss level ...
    host.live.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(current_p(&mut ctx), "nil");
    // ... its re-upload and presentation do.
    readiness(&mut ctx, false, 3, 0);
    assert_eq!(current_p(&mut ctx), "nil");
    readiness(&mut ctx, true, 3, 0);
    assert_eq!(current_p(&mut ctx), "t");
}

/// MODEL's glyph sizes in BUFFER as `((W H) ...)`, nil for a glyph
/// without a display spec.
fn glyph_sizes(ctx: &mut Context, buffer: &str) -> String {
    eval_printed(
        ctx,
        &format!(
            r#"(with-current-buffer {buffer:?}
                 (let (sizes)
                   (dotimes (i (buffer-size))
                     (let ((display (get-text-property (1+ i) 'display)))
                       (push (and display
                                  (list (plist-get (cdr display) :width)
                                        (plist-get (cdr display) :height)))
                             sizes)))
                   (nreverse sizes)))"#
        ),
    )
}

fn resize_error(ctx: &mut Context, req: u64) {
    deliver(
        ctx,
        ModelEvent::Error {
            req: Some(req),
            error: "model resize exceeds the GPU budget".to_owned(),
        },
    );
}

#[test]
fn rejected_resize_restores_the_accepted_glyph_size() {
    let (mut ctx, host) = lisp_context("lisp-resize-refusal");
    create_model(&mut ctx);
    eval_printed(
        &mut ctx,
        r#"(with-current-buffer (get-buffer-create "model-resize-a")
             (neomacs-model-insert model-test-m))"#,
    );
    deliver(
        &mut ctx,
        ModelEvent::Ready {
            triangles: vec![1, 1],
        },
    );
    readiness(&mut ctx, true, 1, 0);
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-a"), "((64 48))");

    // Two outstanding resizes; the glyph follows the latest at once.
    eval_printed(
        &mut ctx,
        "(list (neomacs-model-resize model-test-m 128 96) \
               (neomacs-model-resize model-test-m 256 192))",
    );
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-a"), "((256 192))");
    assert_eq!(updates(&host).len(), 2);
    // The renderer applies 1 and refuses 2 over budget: the glyph falls
    // back to 1's size, which is now the accepted one.
    resize_error(&mut ctx, 2);
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-a"), "((128 96))");
    readiness(&mut ctx, true, 2, 2);
    assert_eq!(current_p(&mut ctx), "t");
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-a"), "((128 96))");

    // Refusing an older request never overwrites a newer one, even after
    // the model moved to another buffer.
    eval_printed(&mut ctx, "(neomacs-model-resize model-test-m 300 200)");
    eval_printed(
        &mut ctx,
        r#"(with-current-buffer (get-buffer-create "model-resize-b")
             (insert "x")
             (neomacs-model-insert model-test-m))"#,
    );
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-a"), "(nil)");
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-b"), "(nil (300 200))");
    eval_printed(&mut ctx, "(neomacs-model-resize model-test-m 400 300)");
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-b"), "(nil (400 300))");
    resize_error(&mut ctx, 3);
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-b"), "(nil (400 300))");
    readiness(&mut ctx, false, 3, 4);
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-b"), "(nil (400 300))");

    // A refusal with nothing newer pending returns to the accepted 4.
    eval_printed(&mut ctx, "(neomacs-model-resize model-test-m 512 512)");
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-b"), "(nil (512 512))");
    resize_error(&mut ctx, 5);
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-b"), "(nil (400 300))");
    assert_eq!(glyph_sizes(&mut ctx, "model-resize-a"), "(nil)");
    readiness(&mut ctx, true, 4, 5);
    assert_eq!(current_p(&mut ctx), "t");
}

#[test]
fn weak_registry_collects_a_callback_to_handle_cycle() {
    let (mut ctx, host) = lisp_context("lisp-registry-gc");
    // The callback closes over its own handle: the registry value refers
    // to its key. Only `model-test-keep' holds the handle from outside.
    eval_printed(
        &mut ctx,
        r#"(progn
             (setq model-test-keep
                   (let ((model (neomacs-model-create
                                 :meshes (list (list "a" model-test-a)))))
                     (setf (neomacs-model--entry-callback
                            (neomacs-model--entry model))
                           (lambda (m type _plist)
                             (push (list type (eq m model)) model-test-log)))
                     model))
             nil)"#,
    );
    eval_printed(&mut ctx, "(garbage-collect)");
    assert!(
        host.destroys.lock().expect("destroys").is_empty(),
        "GC freed a model still reachable from a global"
    );
    assert_eq!(
        eval_printed(&mut ctx, "(hash-table-count neomacs-model--registry)"),
        "1"
    );
    // Still reachable: the callback still runs.
    deliver(&mut ctx, ModelEvent::Ready { triangles: vec![1] });
    assert_eq!(eval_printed(&mut ctx, "model-test-log"), "((ready t))");
    assert!(ctx.live_models.contains(&FIRST_MODEL_ID));

    // Unreachable apart from the cycle: collected, its surface freed.
    eval_printed(&mut ctx, "(setq model-test-keep nil)");
    eval_printed(&mut ctx, "(garbage-collect)");
    assert_eq!(
        *host.destroys.lock().expect("destroys"),
        vec![FIRST_MODEL_ID]
    );
    assert!(!ctx.live_models.contains(&FIRST_MODEL_ID));
    assert_eq!(
        eval_printed(&mut ctx, "(hash-table-count neomacs-model--registry)"),
        "0"
    );
    // Later events for its id are dropped.
    deliver(
        &mut ctx,
        ModelEvent::Readiness {
            current: true,
            rev: 1,
            req: 0,
        },
    );
    assert_eq!(eval_printed(&mut ctx, "model-test-log"), "((ready t))");
}

// ----------------------------------------------------------------------
// Round 2: non-local exits out of a callback (pre-review MUST 2).

/// Models 1 and 2. Model 1's `ready' callback waits, which services
/// model 2's `ready' and `readiness{t}' queued behind it, then runs EXIT.
/// Both callbacks log `(N TYPE)'; a second event-hook member logs
/// `(hook ID TYPE)'.
fn non_local_exit_context(test: &str, exit: &str) -> Context {
    let (mut ctx, _host) = lisp_context(test);
    eval_printed(
        &mut ctx,
        &format!(
            r#"(progn
             (setq model-test-log nil model-test-hooks nil)
             (setq model-test-m
                   (neomacs-model-create
                    :meshes (list (list "a" model-test-a))
                    :callback (lambda (_model type _plist)
                                (push (list 1 type) model-test-log)
                                (when (eq type 'ready)
                                  (accept-process-output nil 0.05)
                                  {exit}))))
             (setq model-test-m2
                   (neomacs-model-create
                    :meshes (list (list "a" model-test-a))
                    :callback (lambda (_model type _plist)
                                (push (list 2 type) model-test-log))))
             (add-hook 'neomacs-model-event-functions
                       (lambda (id type _plist)
                         (push (list 'hook id type) model-test-hooks))
                       t))"#
        ),
    );
    let model = |id, event| crate::keyboard::InputEvent::Model { id, event };
    queue_input(
        &mut ctx,
        vec![
            model(FIRST_MODEL_ID, ModelEvent::Ready { triangles: vec![1] }),
            model(FIRST_MODEL_ID + 1, ModelEvent::Ready { triangles: vec![1] }),
            model(
                FIRST_MODEL_ID + 1,
                ModelEvent::Readiness {
                    current: true,
                    rev: 1,
                    req: 0,
                },
            ),
        ],
    );
    ctx
}

/// Model 2's state is current the moment its events arrive, whatever
/// model 1's callback then does.
fn assert_model_2_state(ctx: &mut Context) {
    assert_eq!(
        eval_printed(ctx, "(neomacs-model-status model-test-m2)"),
        r#"(ready :triangles (1) :meshes ("a"))"#
    );
    assert_eq!(
        eval_printed(ctx, "(neomacs-model-current-p model-test-m2)"),
        "t"
    );
}

fn assert_all_delivered(ctx: &mut Context, hooks: &str) {
    assert_eq!(
        eval_printed(ctx, "(reverse model-test-log)"),
        "((1 ready) (2 ready) (2 readiness))"
    );
    assert_eq!(eval_printed(ctx, "(reverse model-test-hooks)"), hooks);
    assert_eq!(eval_printed(ctx, "neomacs-model--queue"), "nil");
}

/// A `throw' out of a callback (the "wait for the model" idiom) leaves
/// model 2's state right at once, and its queued callbacks and hook
/// members run at the next wait, with no further model event.
#[test]
fn a_throw_out_of_a_callback_strands_no_event_until_the_next_wait() {
    let mut ctx = non_local_exit_context("lisp-throw-wait", "(throw 'model-test-done 'thrown)");
    assert_eq!(
        eval_printed(
            &mut ctx,
            "(catch 'model-test-done (accept-process-output nil 0.05) 'no-throw)"
        ),
        "thrown"
    );
    assert_model_2_state(&mut ctx);
    eval_printed(&mut ctx, "(accept-process-output nil 0.01)");
    assert_all_delivered(
        &mut ctx,
        &format!(
            "((hook {} ready) (hook {} readiness))",
            FIRST_MODEL_ID + 1,
            FIRST_MODEL_ID + 1
        ),
    );
    assert_model_2_state(&mut ctx);
}

/// The same throw; the next command-loop read delivers what it left.
#[test]
fn a_throw_out_of_a_callback_strands_no_event_until_the_next_read() {
    let mut ctx = non_local_exit_context("lisp-throw-read", "(throw 'model-test-done 'thrown)");
    assert_eq!(
        eval_printed(
            &mut ctx,
            "(catch 'model-test-done (accept-process-output nil 0.05) 'no-throw)"
        ),
        "thrown"
    );
    assert_model_2_state(&mut ctx);
    assert_eq!(eval_printed(&mut ctx, "(read-event nil nil 0.01)"), "nil");
    assert_all_delivered(
        &mut ctx,
        &format!(
            "((hook {} ready) (hook {} readiness))",
            FIRST_MODEL_ID + 1,
            FIRST_MODEL_ID + 1
        ),
    );
}

/// A quit out of a callback (C-g inside its `sit-for') is a signal: like
/// GNU's `safe_run_hooks' the delivery logs it and goes on, so model 2's
/// callbacks run in the same delivery and its state is right.
#[test]
fn a_quit_out_of_a_callback_is_logged_and_strands_nothing() {
    let mut ctx = non_local_exit_context("lisp-quit", "(signal 'quit nil)");
    assert_eq!(
        eval_printed(
            &mut ctx,
            "(condition-case nil
                 (progn (accept-process-output nil 0.05) 'no-quit)
               (quit 'quit))"
        ),
        "no-quit"
    );
    assert_model_2_state(&mut ctx);
    assert_all_delivered(
        &mut ctx,
        &format!(
            "((hook {} ready) (hook {} ready) (hook {} readiness))",
            FIRST_MODEL_ID,
            FIRST_MODEL_ID + 1,
            FIRST_MODEL_ID + 1
        ),
    );
}

// ----------------------------------------------------------------------
// Round 2: pre-review SHOULD 2-3, NIT 1-2.

/// A vector `:view' is kept as a list: `neomacs-model-view' and the
/// camera keys work before any renderer `view' event.
#[test]
fn a_vector_view_works_with_the_camera_keys() {
    let (mut ctx, host) = lisp_context("lisp-vector-view");
    eval_printed(
        &mut ctx,
        r#"(setq model-test-m
                 (neomacs-model-create :meshes (list (list "a" model-test-a))
                                       :view [0 -90 1.5]))"#,
    );
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model-view model-test-m)"),
        "(0 -90 1.5)"
    );
    assert_eq!(
        eval_printed(
            &mut ctx,
            r#"(with-current-buffer (get-buffer-create "model-test-vector")
                 (neomacs-model-insert model-test-m)
                 (goto-char (point-min))
                 (neomacs-model-yaw-forward)
                 (neomacs-model-pitch-next)
                 (prog1 (neomacs-model-view model-test-m)
                   (neomacs-model-reset-view)))"#
        ),
        // Requested angles are wrapped into 0..360 (OPUS-REVIEW NIT 8).
        "(15 285 1.5)"
    );
    assert_eq!(
        updates(&host).last(),
        Some(&(
            FIRST_MODEL_ID,
            3,
            ModelChange::View(ModelView {
                yaw: 0.0,
                pitch: 270.0,
                zoom: 1.5,
            }),
        ))
    );
}

/// The kill hook survives a major-mode change in the attached buffer.
#[test]
fn killing_the_buffer_destroys_the_model_after_a_major_mode_change() {
    let (mut ctx, host) = lisp_context("lisp-mode-change");
    create_model(&mut ctx);
    eval_printed(
        &mut ctx,
        r#"(with-current-buffer (get-buffer-create "model-test-mode")
             (neomacs-model-insert model-test-m)
             (fundamental-mode))"#,
    );
    eval_printed(&mut ctx, r#"(kill-buffer "model-test-mode")"#);
    assert_eq!(
        *host.destroys.lock().expect("destroys"),
        vec![FIRST_MODEL_ID]
    );
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model-status model-test-m)"),
        "(destroyed)"
    );
}

/// Moving a model leaves nothing of it in the old buffer: no glyph
/// property a later resize could revive, and no buffer model the camera
/// keys would still drive.
#[test]
fn moving_a_model_leaves_nothing_of_it_in_the_old_buffer() {
    let (mut ctx, _host) = lisp_context("lisp-detach");
    create_model(&mut ctx);
    // A -> B -> A, then resize: only A's newest glyph shows the model.
    assert_eq!(
        eval_printed(
            &mut ctx,
            r#"(progn
                 (with-current-buffer (get-buffer-create "model-test-a")
                   (neomacs-model-insert model-test-m))
                 (with-current-buffer (get-buffer-create "model-test-b")
                   (neomacs-model-insert model-test-m))
                 (with-current-buffer "model-test-a"
                   (goto-char (point-max))
                   (neomacs-model-insert model-test-m))
                 (neomacs-model-resize model-test-m 96 96)
                 (with-current-buffer "model-test-a"
                   (list (get-text-property 1 'neomacs-model)
                         (get-text-property 1 'display)
                         (eq (get-text-property 2 'neomacs-model) model-test-m)
                         (plist-get (cdr (get-text-property 2 'display)) :width))))"#
        ),
        "(nil nil t 96)"
    );
    // A standalone buffer's model moved elsewhere is no longer its own.
    assert_eq!(
        eval_printed(
            &mut ctx,
            r#"(let* ((buffer (neomacs-model-open (list (list "a" model-test-a))
                                                  :name "detach"))
                      (model (buffer-local-value 'neomacs-model--buffer-model
                                                 buffer)))
                 (with-current-buffer (get-buffer-create "model-test-c")
                   (neomacs-model-insert model))
                 (with-current-buffer buffer
                   (goto-char (point-min))
                   (list neomacs-model--buffer-model
                         (get-text-property (point-min) 'neomacs-model)
                         (condition-case nil (neomacs-model--at-point)
                           (user-error 'no-model)))))"#
        ),
        "(nil nil no-model)"
    );
}

/// An empty `:string' is refused and leaves the attachment alone.
#[test]
fn an_empty_insert_string_is_refused() {
    let (mut ctx, _host) = lisp_context("lisp-empty-string");
    create_model(&mut ctx);
    assert_eq!(
        eval_printed(
            &mut ctx,
            r#"(progn
                 (with-current-buffer (get-buffer-create "model-test-a")
                   (neomacs-model-insert model-test-m))
                 (with-current-buffer (get-buffer-create "model-test-b")
                   (list (condition-case nil
                             (neomacs-model-insert model-test-m :string "")
                           (error 'refused))
                         (buffer-size)
                         (with-current-buffer "model-test-a"
                           (and (get-text-property 1 'display) t)))))"#
        ),
        "(refused 0 t)"
    );
}

// ----------------------------------------------------------------------
// Round 3: SOL-REVIEW-2 MUST 4-5.

/// The nested-delivery context, plus one chronological log
/// `model-test-order' of the callback and of event-hook members both
/// before and after `neomacs-model--dispatch'.
fn whole_event_order_context(test: &str, wait: &str) -> Context {
    let mut ctx = nested_delivery_context(test, wait);
    eval_printed(
        &mut ctx,
        r#"(progn
             (defvar model-test-inner nil)
             (setq model-test-order nil
                   model-test-inner (neomacs-model--entry-callback
                                     (neomacs-model--entry model-test-m)))
             (add-hook 'neomacs-model-event-functions
                       (lambda (_id type _plist)
                         (push (list 'first type) model-test-order)))
             (add-hook 'neomacs-model-event-functions
                       (lambda (_id type _plist)
                         (push (list 'last type) model-test-order))
                       t)
             (setf (neomacs-model--entry-callback
                    (neomacs-model--entry model-test-m))
                   (lambda (model type plist)
                     (push (list 'callback type) model-test-order)
                     (funcall model-test-inner model type plist))))"#,
    );
    ctx
}

/// Every member, the callback included, handles the view event before
/// any of them handles the readiness event that arrived meanwhile.
fn assert_whole_event_order(ctx: &mut Context) {
    assert_eq!(
        eval_printed(ctx, "(reverse model-test-order)"),
        "((first view) (callback view) (last view) \
         (first readiness) (callback readiness) (last readiness))"
    );
}

#[test]
fn every_member_handles_an_event_before_the_next_at_the_read_route() {
    let mut ctx = whole_event_order_context("lisp-order-read", "nil");
    queue_input(&mut ctx, nested_delivery_input());
    assert_eq!(eval_printed(&mut ctx, "(read-event)"), "121");
    assert_whole_event_order(&mut ctx);
    assert_nested_delivery(&mut ctx, "((view nil) (key 120 nil nil) (readiness nil))");
}

#[test]
fn every_member_handles_an_event_before_the_next_at_the_wait_route() {
    let mut ctx = whole_event_order_context(
        "lisp-order-wait",
        "(accept-process-output nil 0.05)
         (push (list 'waited (neomacs-model-current-p model)) model-test-log)",
    );
    queue_input(&mut ctx, nested_delivery_input());
    assert_eq!(
        eval_printed(
            &mut ctx,
            "(progn (accept-process-output nil 0.05) (read-event))"
        ),
        "121"
    );
    assert_whole_event_order(&mut ctx);
    assert_nested_delivery(
        &mut ctx,
        "((view nil) (waited nil) (key 120 nil nil) (readiness nil))",
    );
}

/// Models 1 and 2, model 2 ready and current at rev 1. Model 1's
/// callback, for a `view' event, runs WAIT then ACTION; both callbacks
/// log `(N TYPE)' onto `model-test-log'.
fn stale_pick_context(test: &str, wait: &str, action: &str) -> Context {
    let (mut ctx, _host) = lisp_context(test);
    eval_printed(
        &mut ctx,
        &format!(
            r#"(progn
             (setq model-test-log nil)
             (setq model-test-m
                   (neomacs-model-create
                    :meshes (list (list "a" model-test-a))
                    :callback (lambda (_model type _plist)
                                (push (list 1 type) model-test-log)
                                (when (eq type 'view)
                                  {wait}
                                  {action}))))
             (setq model-test-m2
                   (neomacs-model-create
                    :meshes (list (list "a" model-test-a))
                    :callback (lambda (_model type _plist)
                                (push (list 2 type) model-test-log)))))"#
        ),
    );
    let second = FIRST_MODEL_ID + 1;
    for event in [
        ModelEvent::Ready { triangles: vec![1] },
        ModelEvent::Readiness {
            current: true,
            rev: 1,
            req: 0,
        },
    ] {
        ctx.handle_model_input_event(second, &event)
            .expect("model event hook runs");
    }
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model-current-p model-test-m2)"),
        "t"
    );
    eval_printed(&mut ctx, "(setq model-test-log nil)");
    ctx
}

fn model_2_pick() -> crate::keyboard::InputEvent {
    crate::keyboard::InputEvent::Model {
        id: FIRST_MODEL_ID + 1,
        event: ModelEvent::Pick {
            hit: Some(ModelHit {
                mesh: 0,
                face: 0,
                barycentric: [0.25, 0.25, 0.5],
                point: [0.0, 0.0, 0.0],
            }),
            view: ModelView::default(),
            rev: 1,
            req: 0,
        },
    }
}

/// Model 1's view event, then a pick of model 2's current picture, then
/// model 2's R{nil}, then the keys model 1's callback and the test read.
fn pick_then_invalidation_input() -> Vec<crate::keyboard::InputEvent> {
    vec![
        model_input(ModelEvent::View {
            view: ModelView::default(),
            rev: 1,
        }),
        model_2_pick(),
        crate::keyboard::InputEvent::Model {
            id: FIRST_MODEL_ID + 1,
            event: ModelEvent::Readiness {
                current: false,
                rev: 2,
                req: 0,
            },
        },
        key_input('x'),
        key_input('y'),
    ]
}

const READ_KEY: &str = "(push (list 'key (read-event) (neomacs-model-current-p model-test-m2)) \
                        model-test-log)";

/// A pick accepted while current, then invalidated (R{nil}) before its
/// callback can run: the callback must not run (read route).
#[test]
fn a_queued_pick_invalidated_before_its_callback_is_dropped_at_the_read_route() {
    let mut ctx = stale_pick_context("lisp-stale-pick-read", "nil", READ_KEY);
    queue_input(&mut ctx, pick_then_invalidation_input());
    assert_eq!(eval_printed(&mut ctx, "(read-event)"), "121");
    assert_eq!(
        eval_printed(&mut ctx, "(reverse model-test-log)"),
        "((1 view) (key 120 nil) (2 readiness))"
    );
}

/// The same through a nested wait.
#[test]
fn a_queued_pick_invalidated_before_its_callback_is_dropped_at_the_wait_route() {
    let mut ctx = stale_pick_context(
        "lisp-stale-pick-wait",
        "(accept-process-output nil 0.05)",
        READ_KEY,
    );
    queue_input(&mut ctx, pick_then_invalidation_input());
    assert_eq!(
        eval_printed(
            &mut ctx,
            "(progn (accept-process-output nil 0.05) (read-event))"
        ),
        "121"
    );
    assert_eq!(
        eval_printed(&mut ctx, "(reverse model-test-log)"),
        "((1 view) (key 120 nil) (2 readiness))"
    );
}

/// A pick accepted while current, then a new Lisp request for that model
/// before its callback runs: the pick names an obsolete picture.
#[test]
fn a_queued_pick_is_dropped_after_a_newer_lisp_request() {
    let mut ctx = stale_pick_context(
        "lisp-stale-pick-request",
        "(accept-process-output nil 0.05)",
        "(push (list 'requested (neomacs-model-set-view model-test-m2 10 0 1)) model-test-log)",
    );
    queue_input(
        &mut ctx,
        vec![
            model_input(ModelEvent::View {
                view: ModelView::default(),
                rev: 1,
            }),
            model_2_pick(),
        ],
    );
    eval_printed(
        &mut ctx,
        "(progn (accept-process-output nil 0.05) (accept-process-output nil 0.01))",
    );
    assert_eq!(
        eval_printed(&mut ctx, "(reverse model-test-log)"),
        "((1 view) (requested 1))"
    );
    // A pick of the picture current again is delivered.
    for event in [
        ModelEvent::Readiness {
            current: true,
            rev: 2,
            req: 1,
        },
        ModelEvent::Pick {
            hit: None,
            view: ModelView::default(),
            rev: 2,
            req: 1,
        },
    ] {
        ctx.handle_model_input_event(FIRST_MODEL_ID + 1, &event)
            .expect("model event hook runs");
    }
    assert_eq!(
        eval_printed(&mut ctx, "(mapcar #'cadr (reverse model-test-log))"),
        "(view 1 readiness pick)"
    );
}

/// OPUS-REVIEW NIT 8: a requested camera is wrapped into 0..360 like the
/// renderer's own views; an invalid angle still reaches the renderer
/// unchanged, to be refused there.
#[test]
fn requested_angles_are_wrapped_like_renderer_views() {
    let (mut ctx, host) = lisp_context("lisp-wrapped-view");
    create_model(&mut ctx);
    assert_eq!(
        eval_printed(
            &mut ctx,
            "(progn (neomacs-model-set-view model-test-m 450 -15 1.5) \
                    (neomacs-model-view model-test-m))"
        ),
        "(90 345 1.5)"
    );
    assert_eq!(
        updates(&host).last(),
        Some(&(
            FIRST_MODEL_ID,
            1,
            ModelChange::View(ModelView {
                yaw: 90.0,
                pitch: 345.0,
                zoom: 1.5,
            }),
        ))
    );
    // Out of range is not wrapped into range: it is still refused.
    assert!(
        signal_of(&mut ctx, "(neomacs-model-set-view model-test-m 2e6 0 1)")
            .contains("finite angles"),
    );
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model-view model-test-m)"),
        "(90 345 1.5)"
    );
    assert_eq!(updates(&host).len(), 1);
}
