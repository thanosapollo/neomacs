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
                      ;; inside its `sit-for') must wait for it to return.
                      (neomacs-model--dispatch
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
        r#"("*Model: Arm*" neomacs-model-view-mode t quit-window neomacs-model-yaw-forward (15 -15 1.1) (0 0 1))"#
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
