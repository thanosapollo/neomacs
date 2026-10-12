//! Elisp primitives for native 3D model views (campaign design §2, §3).
//!
//! A model is a shader surface whose texture the renderer fills from OBJ
//! meshes. It shares the surface id space, the GC-managed surface handle,
//! the `(surface :id HANDLE :width W :height H)` display spec and
//! `neomacs-surface-destroy`. These primitives are internal (`--`): the
//! public API, mesh names, callbacks and readiness live in
//! `lisp/neomacs-model.el`, which maps mesh ID strings to the indices used
//! here. Loading is asynchronous; its outcome and every later report arrive
//! as `keyboard::InputEvent::Model` and reach Lisp through
//! `neomacs-model-event-functions` (see [`deliver_model_event`]).

mod subrs;
#[cfg(test)]
pub(crate) use subrs::SUBRS;
pub(crate) use subrs::register_subrs;

use neomacs_display_protocol::model::{
    MODEL_EDGE_RANGE, MODEL_MAX_HIGHLIGHT_LAYERS, MODEL_MAX_MESHES, ModelChange, ModelEvent,
    ModelHighlightLayer, ModelMeshSpec, ModelTarget, ModelView,
};

use super::error::{EvalResult, Flow, FlowKind, FlowResultExt, signal};
use super::eval::{Context, ShaderSurfaceContent, ShaderSurfaceCreateRequest};
use super::hook_runtime::{self, HookRuntime};
use super::value::{Value, list_to_vec};

/// Largest OBJ file a model accepts (design §2), checked before loading.
const MODEL_MAX_FILE_BYTES: u64 = 100_000_000;

fn model_error(message: impl Into<String>) -> Flow {
    signal("error", vec![Value::string(message.into())])
}

/// The elements of a proper list or a vector.
fn sequence_items(value: Value) -> Option<Vec<Value>> {
    list_to_vec(&value).or_else(|| value.as_vector_data().map(|v| v.as_slice().to_vec()))
}

/// Exactly `N` finite numbers from a list or vector.
fn numbers<const N: usize>(value: Value) -> Option<[f64; N]> {
    let items = sequence_items(value)?;
    if items.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = item.as_number_f64().filter(|n| n.is_finite())?;
    }
    Some(out)
}

/// A model edge in logical pixels: an integer in `MODEL_EDGE_RANGE`.
fn model_edge(value: Value, what: &str, caller: &str) -> Result<u32, Flow> {
    value
        .as_fixnum()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| MODEL_EDGE_RANGE.contains(n))
        .ok_or_else(|| {
            model_error(format!(
                "{caller}: {what} must be an integer in {}..{}",
                MODEL_EDGE_RANGE.start(),
                MODEL_EDGE_RANGE.end()
            ))
        })
}

/// `(YAW PITCH ZOOM)` or `[YAW PITCH ZOOM]`, validated against the model
/// camera contract. (`neomacs-model-create` passes a list: it keeps the
/// camera Lisp-side as one, so a vector `:view` is converted there.)
fn model_view(value: Value, caller: &str) -> Result<ModelView, Flow> {
    let [yaw, pitch, zoom] = numbers::<3>(value)
        .ok_or_else(|| model_error(format!("{caller}: VIEW must be (YAW PITCH ZOOM)")))?;
    ModelView { yaw, pitch, zoom }
        .validated()
        .map_err(|err| model_error(format!("{caller}: {err}")))
}

/// A lowercase hex SHA-256 digest.
fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 || !bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
        let pair = std::str::from_utf8(pair).ok()?;
        *slot = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

/// One `(PATH SHA256-or-nil)` entry. The file must exist now as a regular
/// file (symlinks followed) of at most 100 MB; the loader re-checks all of
/// it, and the digest, against the bytes it actually parses.
fn mesh_spec(entry: Value) -> Result<ModelMeshSpec, Flow> {
    let bad = |what: String| model_error(format!("neomacs-model--create: {what}"));
    let items = list_to_vec(&entry)
        .filter(|items| (1..=2).contains(&items.len()))
        .ok_or_else(|| bad("each mesh must be (PATH SHA256-OR-NIL)".to_owned()))?;
    let path = items[0]
        .as_lisp_string()
        .and_then(|s| s.as_utf8_str().map(str::to_owned))
        .ok_or_else(|| bad("mesh PATH must be a string".to_owned()))?;
    let path = std::path::PathBuf::from(path);
    if !path.is_absolute() || path.extension().is_none_or(|ext| ext != "obj") {
        return Err(bad(format!(
            "mesh {} must be an absolute .obj path",
            path.display()
        )));
    }
    let metadata =
        std::fs::metadata(&path).map_err(|err| bad(format!("mesh {}: {err}", path.display())))?;
    if !metadata.is_file() {
        return Err(bad(format!(
            "mesh {} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() > MODEL_MAX_FILE_BYTES {
        return Err(bad(format!(
            "mesh {} is larger than {MODEL_MAX_FILE_BYTES} bytes",
            path.display()
        )));
    }
    let sha256 = match items.get(1).copied().filter(|v| !v.is_nil()) {
        None => None,
        Some(digest) => Some(
            digest
                .as_lisp_string()
                .and_then(|s| s.as_utf8_str())
                .and_then(parse_sha256)
                .ok_or_else(|| {
                    bad("mesh SHA256 must be nil or 64 lowercase hex digits".to_owned())
                })?,
        ),
    };
    Ok(ModelMeshSpec { path, sha256 })
}

/// The host when a model renderer is live, else the user error every
/// session without a graphical renderer gets (batch, TTY, headless).
fn live_host(eval: &Context) -> Result<&dyn super::eval::DisplayHost, Flow> {
    eval.display_host
        .as_deref()
        .filter(|host| host.model_renderer_available())
        .ok_or_else(|| {
            signal(
                "user-error",
                vec![Value::string("3D view needs a graphical Neomacs frame")],
            )
        })
}

/// (neomacs-model--available-p) — non-nil iff a model renderer is live now.
fn available(eval: &mut Context, _args: Vec<Value>) -> EvalResult {
    Ok(Value::bool_val(
        eval.display_host
            .as_ref()
            .is_some_and(|host| host.model_renderer_available()),
    ))
}

/// (neomacs-model--create MESHES VIEW BACKGROUND WIDTH HEIGHT)
///
/// MESHES is a list of 1..255 `(PATH SHA256-OR-NIL)`, VIEW `(YAW PITCH
/// ZOOM)`, BACKGROUND three sRGB components in 0..1 (list or vector),
/// WIDTH and HEIGHT logical pixels in 32..2048. Argument errors signal
/// here; load failures arrive later as a `failed` event. Returns the
/// GC-managed surface handle.
fn create(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const CALLER: &str = "neomacs-model--create";
    let entries = list_to_vec(&args[0])
        .filter(|entries| (1..=MODEL_MAX_MESHES).contains(&entries.len()))
        .ok_or_else(|| {
            model_error(format!(
                "{CALLER}: MESHES must be a list of 1..{MODEL_MAX_MESHES} meshes"
            ))
        })?;
    let meshes = entries
        .into_iter()
        .map(mesh_spec)
        .collect::<Result<Vec<_>, _>>()?;
    let view = model_view(args[1], CALLER)?;
    let [r, g, b] = numbers::<3>(args[2])
        .filter(|rgb| rgb.iter().all(|c| (0.0..=1.0).contains(c)))
        .ok_or_else(|| {
            model_error(format!(
                "{CALLER}: BACKGROUND must be three numbers in 0..1"
            ))
        })?;
    let width = model_edge(args[3], "WIDTH", CALLER)?;
    let height = model_edge(args[4], "HEIGHT", CALLER)?;
    let request = ShaderSurfaceCreateRequest {
        content: ShaderSurfaceContent::Model {
            meshes,
            view,
            background: [r as f32, g as f32, b as f32, 1.0],
        },
        width,
        height,
        animate: false,
        fps: None,
    };
    let id = live_host(eval)?
        .create_model(request)
        .map_err(|err| model_error(format!("{CALLER}: {err}")))?;
    eval.live_models.insert(id);
    Ok(Value::make_surface_handle(id))
}

/// (neomacs-model--id HANDLE) — the integer surface id of a model handle,
/// the ID that `neomacs-model-event-functions` receives.
fn id(_eval: &mut Context, args: Vec<Value>) -> EvalResult {
    let id = args[0].as_surface_handle().ok_or_else(|| {
        signal(
            "wrong-type-argument",
            vec![Value::symbol("neomacs-model"), args[0]],
        )
    })?;
    Ok(Value::fixnum(i64::from(id)))
}

/// The surface id of HANDLE, a model handle not yet destroyed.
fn live_model_id(eval: &Context, handle: Value, caller: &str) -> Result<u32, Flow> {
    let id = handle.as_surface_handle().ok_or_else(|| {
        signal(
            "wrong-type-argument",
            vec![Value::symbol("neomacs-model"), handle],
        )
    })?;
    if !eval.live_models.contains(&id) {
        return Err(model_error(format!("{caller}: model {id} is destroyed")));
    }
    Ok(id)
}

/// A mesh index: an integer in 0..255.
fn mesh_index(value: Value, caller: &str) -> Result<u16, Flow> {
    value
        .as_fixnum()
        .and_then(|n| u16::try_from(n).ok())
        .filter(|n| usize::from(*n) < MODEL_MAX_MESHES)
        .ok_or_else(|| {
            model_error(format!(
                "{caller}: mesh index must be an integer in 0..{}",
                MODEL_MAX_MESHES - 1
            ))
        })
}

fn face_index(value: Value, caller: &str) -> Result<u32, Flow> {
    value
        .as_fixnum()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| model_error(format!("{caller}: face must be a non-negative integer")))
}

/// Send request REQ (a positive integer the Lisp wrapper numbers per
/// model) changing live model HANDLE.
fn send_update(
    eval: &mut Context,
    handle: Value,
    req: Value,
    caller: &str,
    change: ModelChange,
) -> EvalResult {
    let id = live_model_id(eval, handle, caller)?;
    let req = req
        .as_fixnum()
        .filter(|n| *n > 0)
        .ok_or_else(|| model_error(format!("{caller}: REQ must be a positive integer")))?;
    let host = eval
        .display_host
        .as_deref()
        .ok_or_else(|| model_error(format!("{caller}: no display host in this session")))?;
    host.update_model(id, req as u64, change)
        .map_err(|err| model_error(format!("{caller}: {err}")))?;
    Ok(Value::NIL)
}

/// (neomacs-model--set-view HANDLE REQ YAW PITCH ZOOM)
fn set_view(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const CALLER: &str = "neomacs-model--set-view";
    // A non-number becomes NaN, which `validated` rejects.
    let [yaw, pitch, zoom] =
        [args[2], args[3], args[4]].map(|v| v.as_number_f64().unwrap_or(f64::NAN));
    let view = ModelView { yaw, pitch, zoom }
        .validated()
        .map_err(|err| model_error(format!("{CALLER}: {err}")))?;
    send_update(eval, args[0], args[1], CALLER, ModelChange::View(view))
}

/// Parse a highlight TARGET plist whose `:mesh` is already an index:
/// `(:mesh I :kind object)`, `(:mesh I :kind point :face N :barycentric
/// [A B C] :tolerance R)` or `(:mesh I :kind region :faces (N ...))`.
fn model_target(spec: Value, caller: &str) -> Result<ModelTarget, Flow> {
    let items = list_to_vec(&spec)
        .filter(|items| items.len().is_multiple_of(2))
        .ok_or_else(|| model_error(format!("{caller}: TARGET must be a plist")))?;
    let get = |key: &str| {
        items
            .chunks_exact(2)
            .find(|pair| pair[0].as_symbol_name() == Some(key))
            .map(|pair| pair[1])
            .unwrap_or(Value::NIL)
    };
    let mesh = mesh_index(get(":mesh"), caller)?;
    let target = match get(":kind").as_symbol_name() {
        Some("object") => ModelTarget::Object { mesh },
        Some("point") => ModelTarget::Point {
            mesh,
            face: face_index(get(":face"), caller)?,
            barycentric: numbers::<3>(get(":barycentric"))
                .ok_or_else(|| model_error(format!("{caller}: :barycentric must be [A B C]")))?,
            tolerance: get(":tolerance")
                .as_number_f64()
                .ok_or_else(|| model_error(format!("{caller}: :tolerance must be a number")))?,
        },
        Some("region") => ModelTarget::Region {
            mesh,
            faces: list_to_vec(&get(":faces"))
                .ok_or_else(|| model_error(format!("{caller}: :faces must be a list")))?
                .into_iter()
                .map(|face| face_index(face, caller))
                .collect::<Result<_, _>>()?,
        },
        _ => {
            return Err(model_error(format!(
                "{caller}: :kind must be object, point or region"
            )));
        }
    };
    target
        .validated()
        .map_err(|err| model_error(format!("{caller}: {err}")))
}

/// (neomacs-model--set-highlight HANDLE REQ TARGET SELECTED)
///
/// TARGET is nil or a target plist (see [`model_target`]); SELECTED is nil
/// or a mesh index. Both replace the current highlight.
fn set_highlight(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const CALLER: &str = "neomacs-model--set-highlight";
    let target = match args[2] {
        target if target.is_nil() => None,
        target => Some(model_target(target, CALLER)?),
    };
    let selected = match args[3] {
        selected if selected.is_nil() => None,
        selected => Some(mesh_index(selected, CALLER)?),
    };
    send_update(
        eval,
        args[0],
        args[1],
        CALLER,
        ModelChange::Highlight { target, selected },
    )
}

/// (neomacs-model--set-highlight-layers HANDLE REQ LAYERS)
///
/// LAYERS is a list of at most [`MODEL_MAX_HIGHLIGHT_LAYERS`] entries
/// `(TARGET R G B)`: a target plist (see [`model_target`]) and its sRGB
/// colour, three numbers in 0..1. They replace the whole highlight,
/// target and selection included; nil clears it.
fn set_highlight_layers(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const CALLER: &str = "neomacs-model--set-highlight-layers";
    let entries = list_to_vec(&args[2])
        .ok_or_else(|| model_error(format!("{CALLER}: LAYERS must be a list")))?;
    if entries.len() > MODEL_MAX_HIGHLIGHT_LAYERS {
        return Err(model_error(format!(
            "{CALLER}: at most {MODEL_MAX_HIGHLIGHT_LAYERS} highlight layers, got {}",
            entries.len()
        )));
    }
    let layers = entries
        .into_iter()
        .map(|entry| {
            let items = list_to_vec(&entry)
                .filter(|items| items.len() == 4)
                .ok_or_else(|| {
                    model_error(format!("{CALLER}: each layer must be (TARGET R G B)"))
                })?;
            let mut rgb = [0.0; 3];
            for (slot, item) in rgb.iter_mut().zip(&items[1..]) {
                *slot = item
                    .as_number_f64()
                    .filter(|c| (0.0..=1.0).contains(c))
                    .ok_or_else(|| {
                        model_error(format!(
                            "{CALLER}: layer colour must be three numbers in 0..1"
                        ))
                    })?;
            }
            let [r, g, b] = rgb;
            ModelHighlightLayer {
                target: model_target(items[0], CALLER)?,
                color: [r as f32, g as f32, b as f32],
            }
            .validated()
            .map_err(|err| model_error(format!("{CALLER}: {err}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    send_update(
        eval,
        args[0],
        args[1],
        CALLER,
        ModelChange::HighlightLayers(layers),
    )
}

/// (neomacs-model--resize HANDLE REQ WIDTH HEIGHT)
fn resize(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    const CALLER: &str = "neomacs-model--resize";
    let width = model_edge(args[2], "WIDTH", CALLER)?;
    let height = model_edge(args[3], "HEIGHT", CALLER)?;
    send_update(
        eval,
        args[0],
        args[1],
        CALLER,
        ModelChange::Resize { width, height },
    )
}

/// (neomacs-model--freeze HANDLE REQ)
fn freeze(eval: &mut Context, args: Vec<Value>) -> EvalResult {
    send_update(
        eval,
        args[0],
        args[1],
        "neomacs-model--freeze",
        ModelChange::Freeze,
    )
}

/// A renderer counter as a Lisp integer.
fn integer(n: u64) -> Value {
    Value::make_int(i64::try_from(n).unwrap_or(i64::MAX))
}

fn view_list(view: &ModelView) -> Value {
    Value::list(vec![
        Value::make_float(view.yaw),
        Value::make_float(view.pitch),
        Value::make_float(view.zoom),
    ])
}

fn float_vector(values: &[f64; 3]) -> Value {
    Value::vector(values.iter().map(|v| Value::make_float(*v)).collect())
}

/// The plist of one model event (design §6), as the hook receives it:
///
/// - ready: `(:triangles (N ...))`
/// - failed: `(:error STR)`
/// - error: `(:req REQ-OR-NIL :error STR)`
/// - view: `(:view (Y P Z) :rev REV)`
/// - readiness: `(:current BOOL :rev REV :req REQ)`
/// - pick: `(:hit HIT :view (Y P Z) :rev REV :req REQ)`, HIT nil or
///   `(:mesh INDEX :face F :barycentric [A B C] :point [X Y Z])`.
pub(crate) fn model_event_plist(event: &ModelEvent) -> Value {
    let kw = Value::keyword;
    match event {
        ModelEvent::Ready { triangles } => Value::list(vec![
            kw(":triangles"),
            Value::list(triangles.iter().map(|n| integer(u64::from(*n))).collect()),
        ]),
        ModelEvent::Failed { error } => Value::list(vec![kw(":error"), Value::string(error)]),
        ModelEvent::Error { req, error } => Value::list(vec![
            kw(":req"),
            req.map_or(Value::NIL, integer),
            kw(":error"),
            Value::string(error),
        ]),
        ModelEvent::View { view, rev } => Value::list(vec![
            kw(":view"),
            view_list(view),
            kw(":rev"),
            integer(*rev),
        ]),
        ModelEvent::Readiness { current, rev, req } => Value::list(vec![
            kw(":current"),
            Value::bool_val(*current),
            kw(":rev"),
            integer(*rev),
            kw(":req"),
            integer(*req),
        ]),
        ModelEvent::Pick {
            hit,
            view,
            rev,
            req,
        } => {
            let hit = hit.map_or(Value::NIL, |hit| {
                Value::list(vec![
                    kw(":mesh"),
                    integer(u64::from(hit.mesh)),
                    kw(":face"),
                    integer(u64::from(hit.face)),
                    kw(":barycentric"),
                    float_vector(&hit.barycentric),
                    kw(":point"),
                    float_vector(&hit.point),
                ])
            });
            Value::list(vec![
                kw(":hit"),
                hit,
                kw(":view"),
                view_list(view),
                kw(":rev"),
                integer(*rev),
                kw(":req"),
                integer(*req),
            ])
        }
    }
}

/// The hook delivering model events to Lisp, each member called with
/// `(ID TYPE PLIST)`. An unbound or nil hook runs nothing, so events
/// before `neomacs-model.el` loads are dropped.
const MODEL_EVENT_HOOK: &str = "neomacs-model-event-functions";

/// `neomacs-model.el`'s internal hook, run with the same arguments the
/// moment an event arrives, even while an event hook member runs. Its
/// members only record state, so input read afterwards sees it.
const MODEL_STATE_HOOK: &str = "neomacs-model--state-functions";

/// `neomacs-model.el`'s variable that both hooks see bound to the serial
/// number of the event they run for. The library tags each callback it
/// queues with it, so its event-hook member runs only the callbacks of
/// the event being delivered: a later event's wait for their own
/// delivery, after every member has handled this one.
const MODEL_EVENT_SERIAL: &str = "neomacs-model--event";

/// Model events waiting for [`MODEL_EVENT_HOOK`] (design §6.1).
#[derive(Default)]
pub(crate) struct ModelEventQueue {
    /// (serial, id, event), oldest first.
    pending: std::collections::VecDeque<(u64, u32, ModelEvent)>,
    /// True while a [`deliver_model_event`] further up the stack runs the
    /// event hook; it delivers what is queued behind its current event.
    draining: bool,
    /// Serial number of the latest event received.
    serial: u64,
}

/// Deliver one model event from either keyboard special-input site: run
/// the state hook now, then the event hook for this event and every
/// event queued behind it, oldest first ([`drain_model_events`]). While a
/// member runs (inside its `sit-for`, `accept-process-output` or
/// `read-event`), a newly arrived event only updates state and waits in
/// the queue, so no member is ever re-entered. A member's error, or quit,
/// is logged and the next member still runs, as with GNU's
/// `safe_run_hooks`; a throw propagates, and the events behind it wait
/// for the next wait or command-loop read.
pub(crate) fn deliver_model_event(eval: &mut Context, id: u32, event: &ModelEvent) -> EvalResult {
    eval.model_events.serial += 1;
    let serial = eval.model_events.serial;
    run_model_hook(eval, MODEL_STATE_HOOK, serial, id, event)?;
    eval.model_events
        .pending
        .push_back((serial, id, event.clone()));
    drain_model_events(eval)?;
    Ok(Value::NIL)
}

/// Run the event hook for every queued model event, oldest first, unless
/// a delivery further up the stack is already doing so. Every wait and
/// every command-loop read also calls this, so events a throw left
/// queued (their state already recorded) are delivered at the next safe
/// point without waiting for another model event. Idempotent; answers
/// whether it delivered anything.
pub(crate) fn drain_model_events(eval: &mut Context) -> Result<bool, Flow> {
    if eval.model_events.draining || eval.model_events.pending.is_empty() {
        return Ok(false);
    }
    eval.model_events.draining = true;
    let mut result = Ok(true);
    while let Some((serial, id, event)) = eval.model_events.pending.pop_front() {
        if let Err(flow) = run_model_hook(eval, MODEL_EVENT_HOOK, serial, id, &event) {
            result = Err(flow);
            break;
        }
    }
    eval.model_events.draining = false;
    result
}

/// Call each member of HOOK with `(ID TYPE PLIST)`, isolating errors:
/// like GNU's `safe_run_hooks` an error is reported as `Error in HOOK
/// (FUNCTION): ERR`, but the member stays on the hook.
///
/// [`MODEL_EVENT_SERIAL`] is bound to SERIAL while the members run.
fn run_model_hook(
    eval: &mut Context,
    hook: &str,
    serial: u64,
    id: u32,
    event: &ModelEvent,
) -> Result<(), Flow> {
    let hook_sym = hook_runtime::hook_symbol_by_name(eval, hook);
    let value = hook_runtime::hook_value_by_id(eval, hook_sym).unwrap_or(Value::NIL);
    let funcs = hook_runtime::collect_hook_functions_in_state(eval, hook_sym, value, true);
    if funcs.is_empty() {
        return Ok(());
    }
    let count = eval.specpdl.len();
    eval.try_specbind_or_unwind_to(
        count,
        super::intern::intern(MODEL_EVENT_SERIAL),
        Value::fixnum(serial as i64),
    )?;
    let args = [
        Value::fixnum(i64::from(id)),
        Value::symbol(event.kind()),
        model_event_plist(event),
    ];
    let result = eval.with_hook_root_scope(|eval| {
        for value in funcs.iter().chain(&args).copied() {
            eval.push_hook_root(value);
        }
        for func in funcs {
            // Delivery runs outside any Lisp body, so retire each member's
            // temps here: a handle it `let'-bound must not stay rooted
            // (the registry's weak keys depend on it, design §6.3).
            let result =
                eval.with_eval_temp_root_boundary(|eval| eval.call_hook_callable(func, &args));
            match result.kinded() {
                Ok(_) => {}
                Err(FlowKind::Signal(ref sig)) => {
                    eval.report_safe_hook_error(hook_sym, func, sig)?;
                }
                Err(flow) => return Err(Flow::from_kind(flow)),
            }
        }
        Ok(())
    });
    eval.unbind_to(count);
    result
}
