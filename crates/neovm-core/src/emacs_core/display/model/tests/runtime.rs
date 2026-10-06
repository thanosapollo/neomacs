//! Native model primitive tests (`display/model/mod.rs`).
//!
//! A recording host stands in for the GUI: it reports a live renderer on
//! demand and records every create, update and destroy, so the tests can
//! check what reaches the host and that rejected arguments never do.

use super::eval::{
    Context, DisplayHost, GuiFrameHostRequest, ShaderSurfaceContent, ShaderSurfaceCreateRequest,
};
use neomacs_display_protocol::model::{
    ModelChange, ModelEvent, ModelHit, ModelMeshSpec, ModelTarget, ModelView,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// First id the stub host hands out; each create takes the next one.
pub(super) const FIRST_MODEL_ID: u32 = 100;

#[derive(Clone)]
pub(super) struct RecordingModelDisplayHost {
    pub(super) live: Arc<AtomicBool>,
    pub(super) next_id: Arc<AtomicU32>,
    pub(super) creates: Arc<Mutex<Vec<ShaderSurfaceCreateRequest>>>,
    pub(super) updates: Arc<Mutex<Vec<(u32, u64, ModelChange)>>>,
    pub(super) destroys: Arc<Mutex<Vec<u32>>>,
}

impl Default for RecordingModelDisplayHost {
    fn default() -> Self {
        Self {
            live: Arc::new(AtomicBool::new(true)),
            next_id: Arc::new(AtomicU32::new(FIRST_MODEL_ID)),
            creates: Arc::default(),
            updates: Arc::default(),
            destroys: Arc::default(),
        }
    }
}

impl DisplayHost for RecordingModelDisplayHost {
    fn realize_gui_frame(&mut self, _request: GuiFrameHostRequest) -> Result<(), String> {
        Ok(())
    }

    fn resize_gui_frame(&mut self, _request: GuiFrameHostRequest) -> Result<(), String> {
        Ok(())
    }

    fn model_renderer_available(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    fn create_model(&self, request: ShaderSurfaceCreateRequest) -> Result<u32, String> {
        self.creates.lock().expect("model creates").push(request);
        Ok(self.next_id.fetch_add(1, Ordering::SeqCst))
    }

    fn update_model(&self, id: u32, req: u64, change: ModelChange) -> Result<(), String> {
        self.updates
            .lock()
            .expect("model updates")
            .push((id, req, change));
        Ok(())
    }

    fn destroy_shader_surface(&self, id: u32) -> Result<(), String> {
        self.destroys.lock().expect("model destroys").push(id);
        Ok(())
    }
}

fn model_context() -> (Context, RecordingModelDisplayHost) {
    let host = RecordingModelDisplayHost::default();
    let mut ctx = Context::new();
    ctx.set_display_host(Box::new(host.clone()));
    (ctx, host)
}

/// A per-test scratch directory holding small OBJ files.
pub(super) fn scratch_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("neovm-model-test-{}-{test}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

pub(super) fn obj_file(dir: &std::path::Path, name: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").expect("write obj");
    path.to_str().expect("utf-8 scratch path").to_owned()
}

/// Evaluate SOURCE and return `(format "%S" VALUE)`; a signal panics
/// with its printed form.
pub(super) fn eval_printed(ctx: &mut Context, source: &str) -> String {
    let value = ctx
        .eval_str(&format!(
            "(condition-case err (concat \"OK \" (format \"%S\" {source})) \
             (error (concat \"SIGNAL \" (format \"%S\" err))))"
        ))
        .unwrap_or_else(|err| panic!("{source} failed: {err:?}"));
    let printed = value
        .as_lisp_string()
        .and_then(|s| s.as_utf8_str().map(str::to_owned))
        .expect("format returns a string");
    match printed.strip_prefix("OK ") {
        Some(printed) => printed.to_owned(),
        None => panic!("{source} signalled {printed}"),
    }
}

/// SOURCE's signal as `(SYMBOL . DATA)`, printed; panics when it returns.
pub(super) fn signal_of(ctx: &mut Context, source: &str) -> String {
    eval_printed(
        ctx,
        &format!("(condition-case err (progn {source} 'no-signal) (error err))"),
    )
}

#[test]
fn create_carries_meshes_digests_view_background_and_size_to_the_host() {
    let (mut ctx, host) = model_context();
    let dir = scratch_dir("create");
    let a = obj_file(&dir, "a.obj");
    let b = obj_file(&dir, "b.obj");
    let digest = "00ff".repeat(16);

    let handle = ctx
        .eval_str(&format!(
            r#"(neomacs-model--create '(("{a}" nil) ("{b}" "{digest}"))
                                      '(0 -90 1.1) [0.5 0 1] 64 48)"#
        ))
        .expect("create");
    assert_eq!(handle.as_surface_handle(), Some(FIRST_MODEL_ID));
    assert!(ctx.live_models.contains(&FIRST_MODEL_ID));

    let creates = host.creates.lock().expect("creates");
    let request = creates.last().expect("create reached the host");
    assert_eq!((request.width, request.height), (64, 48));
    assert!(!request.animate);
    assert_eq!(request.fps, None);
    let mut sha = [0u8; 32];
    for pair in sha.chunks_exact_mut(2) {
        pair.copy_from_slice(&[0x00, 0xff]);
    }
    assert_eq!(
        request.content,
        ShaderSurfaceContent::Model {
            meshes: vec![
                ModelMeshSpec {
                    path: PathBuf::from(&a),
                    sha256: None,
                },
                ModelMeshSpec {
                    path: PathBuf::from(&b),
                    sha256: Some(sha),
                },
            ],
            view: ModelView {
                yaw: 0.0,
                pitch: -90.0,
                zoom: 1.1,
            },
            background: [0.5, 0.0, 1.0, 1.0],
        }
    );
    // A one-element entry means no digest; background may be a list.
    drop(creates);
    ctx.eval_str(&format!(
        r#"(neomacs-model--create '(("{a}")) '(0 0 1) '(0 0 0) 32 2048)"#
    ))
    .expect("create without digest");
}

#[test]
fn create_rejects_each_bad_argument_before_the_host() {
    let (mut ctx, host) = model_context();
    let dir = scratch_dir("reject");
    let a = obj_file(&dir, "a.obj");
    let txt = dir.join("a.txt");
    std::fs::write(&txt, "v 0 0 0\n").expect("write txt");
    let subdir = dir.join("d.obj");
    std::fs::create_dir_all(&subdir).expect("dir named .obj");
    let missing = dir.join("missing.obj");
    let ok_mesh = format!(r#"'(("{a}"))"#);
    let too_many = format!(
        "'({})",
        (0..256).map(|_| format!(r#"("{a}")"#)).collect::<String>()
    );
    let cases: Vec<(String, &str)> = vec![
        // MESHES
        ("nil '(0 0 1) [0 0 0] 64 64".to_owned(), "MESHES"),
        (format!("{too_many} '(0 0 1) [0 0 0] 64 64"), "MESHES"),
        (
            "'((\"rel.obj\")) '(0 0 1) [0 0 0] 64 64".to_owned(),
            "absolute .obj",
        ),
        (
            format!("'((\"{}\")) '(0 0 1) [0 0 0] 64 64", txt.display()),
            "absolute .obj",
        ),
        (
            format!("'((\"{}\")) '(0 0 1) [0 0 0] 64 64", missing.display()),
            "missing.obj",
        ),
        (
            format!("'((\"{}\")) '(0 0 1) [0 0 0] 64 64", subdir.display()),
            "not a regular file",
        ),
        (
            format!("'((\"{a}\" \"ABCD\")) '(0 0 1) [0 0 0] 64 64"),
            "SHA256",
        ),
        (
            format!(
                "'((\"{a}\" \"{}\")) '(0 0 1) [0 0 0] 64 64",
                "AB".repeat(32)
            ),
            "SHA256",
        ),
        (
            format!("'((\"{a}\" nil extra)) '(0 0 1) [0 0 0] 64 64"),
            "SHA256-OR-NIL",
        ),
        ("'((1)) '(0 0 1) [0 0 0] 64 64".to_owned(), "PATH"),
        // VIEW
        (format!("{ok_mesh} '(0 0) [0 0 0] 64 64"), "VIEW"),
        (format!("{ok_mesh} '(0 0 9) [0 0 0] 64 64"), "zoom"),
        (format!("{ok_mesh} '(2e6 0 1) [0 0 0] 64 64"), "zoom"),
        (format!("{ok_mesh} '(0 0 x) [0 0 0] 64 64"), "VIEW"),
        // BACKGROUND
        (format!("{ok_mesh} '(0 0 1) [2 0 0] 64 64"), "BACKGROUND"),
        (format!("{ok_mesh} '(0 0 1) [0 0] 64 64"), "BACKGROUND"),
        // WIDTH / HEIGHT
        (format!("{ok_mesh} '(0 0 1) [0 0 0] 31 64"), "WIDTH"),
        (format!("{ok_mesh} '(0 0 1) [0 0 0] 64 2049"), "HEIGHT"),
        (format!("{ok_mesh} '(0 0 1) [0 0 0] 64.0 64"), "WIDTH"),
    ];
    for (args, expect) in cases {
        let printed = signal_of(&mut ctx, &format!("(neomacs-model--create {args})"));
        assert!(
            printed.starts_with("(error ") && printed.contains(expect),
            "(neomacs-model--create {args}) signalled {printed}, expected {expect}"
        );
    }
    assert!(
        host.creates.lock().expect("creates").is_empty(),
        "rejected forms never reach the host"
    );
    assert!(ctx.live_models.is_empty());
}

#[test]
fn create_without_a_live_renderer_is_a_user_error() {
    let (mut ctx, host) = model_context();
    let dir = scratch_dir("user-error");
    let a = obj_file(&dir, "a.obj");
    let form = format!(r#"(neomacs-model--create '(("{a}")) '(0 0 1) [0 0 0] 64 64)"#);
    let expected = r#"(user-error "3D view needs a graphical Neomacs frame")"#;

    assert_eq!(eval_printed(&mut ctx, "(neomacs-model--available-p)"), "t");
    host.live.store(false, Ordering::SeqCst);
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model--available-p)"),
        "nil"
    );
    assert_eq!(signal_of(&mut ctx, &form), expected);

    // Batch and TTY sessions have no display host at all.
    let mut bare = Context::new();
    assert_eq!(
        eval_printed(&mut bare, "(neomacs-model--available-p)"),
        "nil"
    );
    assert_eq!(signal_of(&mut bare, &form), expected);
    assert!(host.creates.lock().expect("creates").is_empty());
}

#[test]
fn surface_destroy_and_gc_free_a_model() {
    let (mut ctx, host) = model_context();
    let dir = scratch_dir("destroy");
    let a = obj_file(&dir, "a.obj");
    ctx.eval_str(&format!(
        r#"(progn
             (setq model-test-a (neomacs-model--create '(("{a}")) '(0 0 1) [0 0 0] 64 64))
             (setq model-test-b (neomacs-model--create '(("{a}")) '(0 0 1) [0 0 0] 64 64))
             (neomacs-surface-destroy model-test-a))"#
    ))
    .expect("create and destroy");
    assert_eq!(
        *host.destroys.lock().expect("destroys"),
        vec![FIRST_MODEL_ID]
    );
    assert!(!ctx.live_models.contains(&FIRST_MODEL_ID));
    assert!(ctx.live_models.contains(&(FIRST_MODEL_ID + 1)));

    ctx.eval_str("(setq model-test-b nil)").expect("drop b");
    ctx.eval_str("(garbage-collect)").expect("gc");
    assert!(
        host.destroys
            .lock()
            .expect("destroys")
            .contains(&(FIRST_MODEL_ID + 1)),
        "GC frees an unreferenced model"
    );
    assert!(ctx.live_models.is_empty());
}

/// Run each event through the keyboard handler with a recording hook and
/// return the printed `(ID TYPE PLIST)` lists in order.
fn hook_records(ctx: &mut Context, events: &[(u32, ModelEvent)]) -> Vec<String> {
    ctx.eval_str(
        r#"(progn
             (setq model-test-events nil)
             (setq neomacs-model-event-functions
                   (list (lambda (id type plist)
                           (setq model-test-events
                                 (cons (format "%S" (list id type plist))
                                       model-test-events))))))"#,
    )
    .expect("install recording hook");
    for (id, event) in events {
        ctx.handle_model_input_event(*id, event)
            .expect("model event hook runs");
    }
    let list = ctx
        .eval_str("(reverse model-test-events)")
        .expect("records");
    let out: Vec<String> = super::value::list_to_vec(&list)
        .expect("a list")
        .into_iter()
        .map(|v| {
            v.as_lisp_string()
                .and_then(|s| s.as_utf8_str().map(str::to_owned))
                .expect("string record")
        })
        .collect();
    assert_eq!(out.len(), events.len());
    out
}

#[test]
fn every_model_event_reaches_the_hook_as_id_type_plist() {
    let (mut ctx, _host) = model_context();
    let view = ModelView {
        yaw: 30.0,
        pitch: -90.0,
        zoom: 1.5,
    };
    let records = hook_records(
        &mut ctx,
        &[
            (
                7,
                ModelEvent::Ready {
                    triangles: vec![12, 4],
                },
            ),
            (
                7,
                ModelEvent::Failed {
                    error: "bad digest".to_owned(),
                },
            ),
            (
                7,
                ModelEvent::Error {
                    req: Some(3),
                    error: "over budget".to_owned(),
                },
            ),
            (
                7,
                ModelEvent::Error {
                    req: None,
                    error: "pick timeout".to_owned(),
                },
            ),
            (7, ModelEvent::View { view, rev: 9 }),
            (
                7,
                ModelEvent::Readiness {
                    current: true,
                    rev: 9,
                    req: 3,
                },
            ),
            (
                7,
                ModelEvent::Readiness {
                    current: false,
                    rev: 10,
                    req: 3,
                },
            ),
            (
                7,
                ModelEvent::Pick {
                    hit: Some(ModelHit {
                        mesh: 1,
                        face: 42,
                        barycentric: [0.25, 0.25, 0.5],
                        point: [1.5, -2.0, 0.125],
                    }),
                    view,
                    rev: 9,
                    req: 3,
                },
            ),
            (
                7,
                ModelEvent::Pick {
                    hit: None,
                    view,
                    rev: 9,
                    req: 3,
                },
            ),
        ],
    );
    assert_eq!(
        records,
        vec![
            "(7 ready (:triangles (12 4)))",
            r#"(7 failed (:error "bad digest"))"#,
            r#"(7 error (:req 3 :error "over budget"))"#,
            r#"(7 error (:req nil :error "pick timeout"))"#,
            "(7 view (:view (30.0 -90.0 1.5) :rev 9))",
            "(7 readiness (:current t :rev 9 :req 3))",
            "(7 readiness (:current nil :rev 10 :req 3))",
            "(7 pick (:hit (:mesh 1 :face 42 :barycentric [0.25 0.25 0.5] \
             :point [1.5 -2.0 0.125]) :view (30.0 -90.0 1.5) :rev 9 :req 3))",
            "(7 pick (:hit nil :view (30.0 -90.0 1.5) :rev 9 :req 3))",
        ]
    );
}

#[test]
fn model_events_without_the_library_loaded_are_dropped() {
    let (mut ctx, _host) = model_context();
    assert!(
        ctx.obarray
            .symbol_value("neomacs-model-event-functions")
            .is_none(),
        "the primitive path starts without neomacs-model.el"
    );
    let result = ctx
        .handle_model_input_event(1, &ModelEvent::Ready { triangles: vec![1] })
        .expect("an unbound hook runs nothing");
    assert!(result.is_nil());
}

// ---------------------------------------------------------------------------
// Update primitives (`neomacs-model--set-view` and friends).

pub(super) fn updates(host: &RecordingModelDisplayHost) -> Vec<(u32, u64, ModelChange)> {
    host.updates.lock().expect("model updates").clone()
}

/// A bare context holding one live two-mesh model in `model-test-m`.
fn context_with_model(test: &str) -> (Context, RecordingModelDisplayHost) {
    let (mut ctx, host) = model_context();
    let a = obj_file(&scratch_dir(test), "a.obj");
    ctx.eval_str(&format!(
        r#"(setq model-test-m (neomacs-model--create '(("{a}") ("{a}")) '(0 0 1) [0 0 0] 64 64))"#
    ))
    .expect("create");
    (ctx, host)
}

#[test]
fn update_primitives_send_each_change_with_its_request_number() {
    let (mut ctx, host) = context_with_model("updates");
    assert_eq!(
        eval_printed(&mut ctx, "(neomacs-model--id model-test-m)"),
        FIRST_MODEL_ID.to_string()
    );
    ctx.eval_str(
        r#"(progn
             (neomacs-model--set-view model-test-m 1 30 -90.5 2)
             (neomacs-model--set-highlight model-test-m 2 '(:mesh 1 :kind object) 0)
             (neomacs-model--set-highlight
              model-test-m 3
              '(:mesh 0 :kind point :face 4 :barycentric [0.2 0.3 0.5] :tolerance 0.1)
              nil)
             (neomacs-model--set-highlight
              model-test-m 4 '(:mesh 1 :kind region :faces (3 1)) nil)
             (neomacs-model--set-highlight model-test-m 5 nil nil)
             (neomacs-model--resize model-test-m 6 128 96)
             (neomacs-model--freeze model-test-m 7))"#,
    )
    .expect("updates");
    let id = FIRST_MODEL_ID;
    let highlight = |target, selected| ModelChange::Highlight { target, selected };
    assert_eq!(
        updates(&host),
        vec![
            (
                id,
                1,
                ModelChange::View(ModelView {
                    yaw: 30.0,
                    pitch: -90.5,
                    zoom: 2.0,
                }),
            ),
            (
                id,
                2,
                highlight(Some(ModelTarget::Object { mesh: 1 }), Some(0))
            ),
            (
                id,
                3,
                highlight(
                    Some(ModelTarget::Point {
                        mesh: 0,
                        face: 4,
                        barycentric: [0.2, 0.3, 0.5],
                        tolerance: 0.1,
                    }),
                    None,
                ),
            ),
            (
                id,
                4,
                highlight(
                    Some(ModelTarget::Region {
                        mesh: 1,
                        faces: vec![3, 1],
                    }),
                    None,
                ),
            ),
            (id, 5, highlight(None, None)),
            (
                id,
                6,
                ModelChange::Resize {
                    width: 128,
                    height: 96,
                },
            ),
            (id, 7, ModelChange::Freeze),
        ]
    );
}

#[test]
fn update_primitives_reject_bad_arguments_and_destroyed_models() {
    let (mut ctx, host) = context_with_model("update-reject");
    let cases = [
        ("(neomacs-model--set-view model-test-m 0 0 0 1)", "REQ"),
        ("(neomacs-model--set-view model-test-m -1 0 0 1)", "REQ"),
        ("(neomacs-model--set-view model-test-m 1.0 0 0 1)", "REQ"),
        ("(neomacs-model--set-view model-test-m 1 0 0 5)", "zoom"),
        ("(neomacs-model--set-view model-test-m 1 'x 0 1)", "zoom"),
        (
            "(neomacs-model--set-highlight model-test-m 1 '(:mesh 0 :kind bogus) nil)",
            ":kind",
        ),
        (
            "(neomacs-model--set-highlight model-test-m 1 '(:mesh 255 :kind object) nil)",
            "mesh index",
        ),
        (
            "(neomacs-model--set-highlight model-test-m 1 '(:mesh 0 :kind object) 255)",
            "mesh index",
        ),
        (
            "(neomacs-model--set-highlight model-test-m 1 \
             '(:mesh 0 :kind point :face 0 :barycentric [0.2 0.3 0.6] :tolerance 1) nil)",
            "weights",
        ),
        (
            "(neomacs-model--set-highlight model-test-m 1 \
             '(:mesh 0 :kind point :face 0 :barycentric [0.2 0.3 0.5] :tolerance 0) nil)",
            "tolerance",
        ),
        (
            "(neomacs-model--set-highlight model-test-m 1 '(:mesh 0 :kind region :faces nil) nil)",
            "at least one face",
        ),
        (
            "(neomacs-model--set-highlight model-test-m 1 '(:mesh 0 :kind region :faces (1 1)) nil)",
            "distinct",
        ),
        (
            "(neomacs-model--set-highlight model-test-m 1 '(:mesh 0 :kind region :faces (-1)) nil)",
            "face",
        ),
        (
            "(neomacs-model--set-highlight model-test-m 1 '(:mesh 0 :kind) nil)",
            "plist",
        ),
        ("(neomacs-model--resize model-test-m 1 31 64)", "WIDTH"),
        ("(neomacs-model--resize model-test-m 1 64 4096)", "HEIGHT"),
    ];
    for (form, expect) in cases {
        let printed = signal_of(&mut ctx, form);
        assert!(
            printed.starts_with("(error ") && printed.contains(expect),
            "{form} signalled {printed}, expected {expect}"
        );
    }
    assert_eq!(
        signal_of(&mut ctx, "(neomacs-model--freeze 7 1)"),
        "(wrong-type-argument neomacs-model 7)"
    );
    assert_eq!(
        signal_of(&mut ctx, "(neomacs-model--id 7)"),
        "(wrong-type-argument neomacs-model 7)"
    );
    // A destroyed handle refuses every request.
    ctx.eval_str("(neomacs-surface-destroy model-test-m)")
        .expect("destroy");
    let printed = signal_of(&mut ctx, "(neomacs-model--freeze model-test-m 1)");
    assert!(printed.contains("destroyed"), "{printed}");
    assert!(updates(&host).is_empty(), "rejections never reach the host");
}
