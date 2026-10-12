//! The render-thread model store (design §4.1, §4.6, §5, §6.2).
//!
//! `ModelStore` lives outside the renderer and survives renderer loss: per
//! model it keeps the validated CPU scene, camera, highlight, background,
//! logical size, status, the readiness fence and the interaction state. GPU
//! work goes through [`ModelGpuHost`], implemented by the renderer (its
//! `ModelCache`) and by a recording fake in the tests.
//!
//! Everything the store learns for Lisp becomes a `(id, ModelEvent)` in
//! [`ModelStore::take_events`], in the order the state changes happened; the
//! render thread forwards them on the one ordered input channel.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use neomacs_display_protocol::model::{
    MODEL_MAX_HIGHLIGHT_LAYERS, ModelChange, ModelEvent, ModelHighlightLayer, ModelHit,
    ModelMeshSpec, ModelTarget, ModelView,
};

use super::gpu::{
    GpuHighlight, GpuLayer, GpuPaint, GpuPoint, ModelDraw, PickPoll, PickRequest, RawPick,
};
use super::loader::{LoadRequest, LoadResult, ModelLoader};
use super::readiness::{Readiness, WindowKey};
use super::scene::ModelScene;

/// Pointer travel (logical px) before a press becomes an orbit.
pub const ORBIT_THRESHOLD: f64 = 4.0;
/// Quiet time after the last wheel input of a burst before its `View` is
/// reported.
pub const WHEEL_QUIET: Duration = Duration::from_millis(150);
/// A pick not answered by then is dropped with `Error{req: None}`.
pub const PICK_TIMEOUT: Duration = Duration::from_millis(500);
/// Poll interval while a load or a pick is outstanding.
pub const POLL_INTERVAL: Duration = Duration::from_millis(16);
/// Most vertices over every live model's retained scene (design §4.3).
pub const MAX_LIVE_VERTICES: u64 = 8_000_000;
/// Most triangles over every live model's retained scene (design §4.3).
pub const MAX_LIVE_TRIANGLES: u64 = 8_000_000;

/// The GPU operations the store needs. Each `Err` leaves the model's GPU
/// state as it was before the call.
pub trait ModelGpuHost {
    /// Reserve budget, upload `scene` and install its texture as surface
    /// `id`; answers the physical size.
    fn model_upload(
        &mut self,
        id: u32,
        scene: &ModelScene,
        logical: (u32, u32),
        scale: f32,
    ) -> Result<(u32, u32), String>;
    /// New render targets for a new logical size or scale (old and new
    /// overlap in the budget until the new ones exist).
    fn model_resize(
        &mut self,
        id: u32,
        logical: (u32, u32),
        scale: f32,
    ) -> Result<(u32, u32), String>;
    fn model_render(&mut self, id: u32, draw: &ModelDraw) -> Result<(), String>;
    /// Release every GPU resource and reservation of `id`. Idempotent.
    fn model_free(&mut self, id: u32);
    fn model_pick_start(&mut self, id: u32, request: &PickRequest) -> Result<(), String>;
    fn model_pick_poll(&mut self, id: u32) -> PickPoll;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelStatus {
    Loading,
    Ready,
    Failed,
}

/// What a texture shows: the state it was drawn from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snapshot {
    pub rev: u64,
    pub req: u64,
    pub view: ModelView,
    pub logical: (u32, u32),
    pub size_px: (u32, u32),
}

struct PendingPick {
    started: Instant,
    snapshot: Snapshot,
    /// Timed out: its answer, when the mapping finally returns, is dropped.
    abandoned: bool,
}

struct Entry {
    token: u64,
    status: ModelStatus,
    scene: Option<Arc<ModelScene>>,
    /// The GPU resources of the current renderer exist.
    uploaded: bool,
    view: ModelView,
    target: Option<ModelTarget>,
    selected: Option<u16>,
    /// Coloured layers; a layers request clears `target` and `selected`
    /// and a target/selection request clears these.
    layers: Vec<ModelHighlightLayer>,
    /// A highlight request made while loading, validated at upload.
    unvalidated_highlight: Option<u64>,
    background: [f32; 4],
    logical: (u32, u32),
    size_px: (u32, u32),
    frozen: bool,
    dirty: bool,
    readiness: Readiness,
    /// What the texture shows now.
    rendered: Option<Snapshot>,
    /// What each showing window presented, for picks.
    presented: BTreeMap<WindowKey, Snapshot>,
    /// Windows whose older pictures (composition ring, pinned pane
    /// pictures, frame/theme transition and child resize crossfade
    /// sources) may still hold this model's pixels, whether or not the
    /// window shows it now: every window that presented it, until that
    /// window's older-picture owners are all discarded (see
    /// [`ModelStore::presented_blended`]).
    pictured_in: BTreeSet<WindowKey>,
    pick: Option<PendingPick>,
    queued_pick: Option<(Snapshot, (u32, u32))>,
    wheel_deadline: Option<Instant>,
}

/// A left-button press on a model glyph, until release.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Capture {
    id: u32,
    window: WindowKey,
    start: (f64, f64),
    last: (f64, f64),
    orbiting: bool,
}

/// Where a pointer event landed on a model glyph: the model, the window
/// and the normalised position inside the composited quad (top-left 0,0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphHit {
    pub id: u32,
    pub u: f32,
    pub v: f32,
}

pub struct ModelStore {
    entries: BTreeMap<u32, Entry>,
    next_token: u64,
    loader: Option<ModelLoader>,
    events: Vec<(u32, ModelEvent)>,
    capture: Option<Capture>,
    /// Device scale models are rendered at (the largest of the windows).
    scale: f32,
    /// Windows hidden, iconified or occluded: their presentations are not
    /// showings until the window system shows them again.
    hidden: HashSet<WindowKey>,
}

impl Default for ModelStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelStore {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_token: 1,
            loader: None,
            events: Vec::new(),
            capture: None,
            scale: 1.0,
            hidden: HashSet::new(),
        }
    }

    pub fn contains(&self, id: u32) -> bool {
        self.entries.contains_key(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn status(&self, id: u32) -> Option<ModelStatus> {
        self.entries.get(&id).map(|e| e.status)
    }

    pub fn view(&self, id: u32) -> Option<ModelView> {
        self.entries.get(&id).map(|e| e.view)
    }

    pub fn take_events(&mut self) -> Vec<(u32, ModelEvent)> {
        std::mem::take(&mut self.events)
    }

    fn emit(&mut self, id: u32, event: ModelEvent) {
        self.events.push((id, event));
    }

    /// Evaluate `id`'s fence and emit `Readiness` if its value changed.
    fn evaluate(&mut self, id: u32) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        if let Some((current, rev, req)) = entry.readiness.evaluate() {
            self.events
                .push((id, ModelEvent::Readiness { current, rev, req }));
        }
    }

    fn fail(&mut self, id: u32, error: String, host: Option<&mut dyn ModelGpuHost>) {
        if let Some(host) = host {
            host.model_free(id);
        }
        // An interaction it ends settles first with its `View` (design
        // §6.2: interacting lasts until its `View`).
        let settled = self
            .entries
            .get(&id)
            .filter(|e| e.readiness.interacting())
            .map(|e| (e.view, e.readiness.rev()));
        if let Some((view, rev)) = settled {
            self.emit(id, ModelEvent::View { view, rev });
        }
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.status = ModelStatus::Failed;
            entry.scene = None;
            entry.uploaded = false;
            entry.dirty = false;
            entry.pick = None;
            entry.queued_pick = None;
            entry.wheel_deadline = None;
            entry.readiness.set_interacting(false);
        }
        if self.capture.is_some_and(|c| c.id == id) {
            self.capture = None;
        }
        self.emit(id, ModelEvent::Failed { error });
        self.evaluate(id);
    }

    // ------------------------------------------------------------------
    // Lifecycle

    /// A new model: queue its load. A full loader queue fails it at once.
    pub fn create(
        &mut self,
        id: u32,
        meshes: Vec<ModelMeshSpec>,
        view: ModelView,
        background: [f32; 4],
        logical: (u32, u32),
    ) {
        let token = self.next_token;
        self.next_token += 1;
        self.entries.insert(
            id,
            Entry {
                token,
                status: ModelStatus::Loading,
                scene: None,
                uploaded: false,
                view,
                target: None,
                selected: None,
                layers: Vec::new(),
                unvalidated_highlight: None,
                background,
                logical,
                size_px: (0, 0),
                frozen: false,
                dirty: false,
                readiness: Readiness::default(),
                rendered: None,
                presented: BTreeMap::new(),
                pictured_in: BTreeSet::new(),
                pick: None,
                queued_pick: None,
                wheel_deadline: None,
            },
        );
        if self.loader.is_none() {
            match ModelLoader::spawn() {
                Ok(loader) => self.loader = Some(loader),
                Err(error) => {
                    self.fail(id, format!("model loader: {error}"), None);
                    return;
                }
            }
        }
        let request = LoadRequest {
            id,
            token,
            meshes,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        if let Err(error) = self.loader.as_mut().expect("spawned").submit(request) {
            self.fail(id, error, None);
        }
    }

    /// Free `id` now: cancel its load, drop its picks, capture and wheel
    /// deadline, and release its GPU resources and budget. Idempotent.
    pub fn free(&mut self, id: u32, host: Option<&mut dyn ModelGpuHost>) {
        if self.entries.remove(&id).is_none() {
            return;
        }
        if let Some(loader) = self.loader.as_mut() {
            loader.cancel(id);
        }
        if self.capture.is_some_and(|c| c.id == id) {
            self.capture = None;
        }
        if let Some(host) = host {
            host.model_free(id);
        }
    }

    /// Whether a load is outstanding (the loop then polls every 16 ms).
    pub fn has_pending_loads(&self) -> bool {
        self.loader.as_ref().is_some_and(ModelLoader::has_pending)
    }

    /// Collect finished loads and upload them (when a renderer is live).
    pub fn poll_loads(&mut self, mut host: Option<&mut dyn ModelGpuHost>) {
        let results = match self.loader.as_mut() {
            Some(loader) => loader.drain(),
            None => return,
        };
        for result in results {
            let host: Option<&mut dyn ModelGpuHost> = match host {
                Some(ref mut h) => Some(&mut **h),
                None => None,
            };
            self.loaded(result, host);
        }
    }

    /// Vertices and triangles of every retained scene: the live totals the
    /// aggregate caps bound. Derived from the scenes themselves, so a
    /// scene dropped by free or failure releases its share with it.
    fn live_geometry(&self) -> (u64, u64) {
        self.entries
            .values()
            .filter_map(|e| e.scene.as_ref())
            .fold((0, 0), |(v, t), scene| {
                let (sv, st) = scene_geometry(scene);
                (v + sv, t + st)
            })
    }

    /// Why `scene` may not join the live models, if it may not.
    fn admission_error(&self, scene: &ModelScene) -> Option<String> {
        let (vertices, triangles) = self.live_geometry();
        let (sv, st) = scene_geometry(scene);
        if vertices + sv > MAX_LIVE_VERTICES {
            Some(format!(
                "live models would exceed {MAX_LIVE_VERTICES} vertices"
            ))
        } else if triangles + st > MAX_LIVE_TRIANGLES {
            Some(format!(
                "live models would exceed {MAX_LIVE_TRIANGLES} triangles"
            ))
        } else {
            None
        }
    }

    fn loaded(&mut self, result: LoadResult, host: Option<&mut dyn ModelGpuHost>) {
        let LoadResult { id, token, outcome } = result;
        // A result whose (id, token) is no longer live is dropped.
        if self.entries.get(&id).is_none_or(|e| e.token != token) {
            return;
        }
        // Admission against the live aggregate caps comes before the scene
        // is retained or uploaded, renderer or not.
        let outcome = outcome.and_then(|scene| match self.admission_error(&scene) {
            Some(error) => Err(error),
            None => Ok(scene),
        });
        match outcome {
            Err(error) => self.fail(id, error, host),
            Ok(scene) => {
                self.entries.get_mut(&id).expect("live").scene = Some(scene);
                if let Some(host) = host {
                    self.upload(id, host);
                }
            }
        }
    }

    /// Upload every loaded scene the current renderer lacks (a load that
    /// finished with no renderer, or a renderer replaced since).
    pub fn upload_pending(&mut self, host: &mut dyn ModelGpuHost) {
        let ids: Vec<u32> = self
            .entries
            .iter()
            .filter(|(_, e)| e.scene.is_some() && !e.uploaded && e.status != ModelStatus::Failed)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.upload(id, host);
        }
    }

    /// The renderer is gone: its GPU resources went with it.
    ///
    /// Invalidated at once (design §4.6, §6.2), not at the next upload: an
    /// active orbit or wheel burst settles with its `View` (the applied view
    /// stays); every model with pixels gets a new revision; what each window
    /// presented, and what the lost texture showed, is forgotten; the fence
    /// is evaluated, so a model that was current reports `Readiness{current:
    /// nil}` now. The caller sends these events on the ordered input channel
    /// before any later input.
    pub fn renderer_lost(&mut self) {
        let orbiting = self.capture.take().filter(|c| c.orbiting).map(|c| c.id);
        let ids: Vec<u32> = self.entries.keys().copied().collect();
        for id in ids {
            let entry = self.entries.get_mut(&id).expect("live");
            let had_pixels = entry.uploaded;
            entry.uploaded = false;
            entry.pick = None;
            entry.queued_pick = None;
            entry.rendered = None;
            entry.presented.clear();
            // Every window's composition ring, transition and crossfade
            // sources died with the device.
            entry.pictured_in.clear();
            entry.readiness.left_all();
            // Provenance is gone first, so settling cannot report a stale
            // true level on the way.
            let wheel = entry.wheel_deadline.take().is_some();
            if wheel || orbiting == Some(id) {
                self.settle_interaction(id);
            }
            let entry = self.entries.get_mut(&id).expect("live");
            if had_pixels {
                entry.readiness.bump();
            }
            self.evaluate(id);
        }
    }

    fn upload(&mut self, id: u32, host: &mut dyn ModelGpuHost) {
        let scale = self.scale;
        let entry = self.entries.get_mut(&id).expect("live");
        let scene = entry.scene.clone().expect("loaded");
        match host.model_upload(id, &scene, entry.logical, scale) {
            Err(error) => self.fail(id, error, Some(host)),
            Ok(size_px) => {
                let first = entry.status == ModelStatus::Loading;
                entry.status = ModelStatus::Ready;
                entry.uploaded = true;
                entry.size_px = size_px;
                entry.dirty = true;
                entry.readiness.bump();
                if first {
                    let triangles = scene.triangle_counts();
                    self.emit(id, ModelEvent::Ready { triangles });
                }
                if let Some(req) = self
                    .entries
                    .get_mut(&id)
                    .unwrap()
                    .unvalidated_highlight
                    .take()
                {
                    self.validate_highlight(id, Some(req));
                }
                self.evaluate(id);
            }
        }
    }

    /// Check the highlight against the loaded topology. An invalid one is
    /// an applied fallback (design §2): cleared, redrawn, `Error{req}`.
    fn validate_highlight(&mut self, id: u32, req: Option<u64>) {
        let entry = self.entries.get_mut(&id).expect("live");
        let scene = entry.scene.as_ref().expect("loaded");
        let error = entry
            .target
            .as_ref()
            .and_then(|t| target_error(scene, t))
            .or_else(|| {
                entry
                    .selected
                    .filter(|m| *m as usize >= scene.meshes().len())
                    .map(|m| format!("selected mesh {m} out of range"))
            })
            .or_else(|| {
                entry
                    .layers
                    .iter()
                    .find_map(|layer| target_error(scene, &layer.target))
            });
        if let Some(error) = error {
            entry.target = None;
            entry.selected = None;
            entry.layers.clear();
            entry.dirty = true;
            entry.readiness.bump();
            self.emit(id, ModelEvent::Error { req, error });
        }
    }

    /// The device scale models render at changed (DPI change or a move to
    /// another monitor): new targets for every uploaded model.
    pub fn set_scale(&mut self, scale: f32, host: &mut dyn ModelGpuHost) {
        if !(scale.is_finite() && scale > 0.0) || (scale - self.scale).abs() <= 0.001 {
            return;
        }
        self.scale = scale;
        let ids: Vec<u32> = self
            .entries
            .iter()
            .filter(|(_, e)| e.uploaded)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let entry = self.entries.get_mut(&id).expect("live");
            match host.model_resize(id, entry.logical, scale) {
                Ok(size_px) => {
                    entry.size_px = size_px;
                    entry.dirty = true;
                    entry.readiness.bump();
                }
                Err(error) => self.emit(id, ModelEvent::Error { req: None, error }),
            }
            self.evaluate(id);
        }
    }

    // ------------------------------------------------------------------
    // Lisp requests

    /// Apply Lisp request `req` (design §3). Every request is settled:
    /// applied, or rejected with `Error{req}`.
    pub fn update(
        &mut self,
        id: u32,
        req: u64,
        change: ModelChange,
        host: Option<&mut dyn ModelGpuHost>,
    ) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        let rejection = if entry.status == ModelStatus::Failed {
            Some("model failed".to_owned())
        } else if entry.frozen {
            Some("model is frozen".to_owned())
        } else {
            None
        };
        if let Some(error) = rejection {
            entry.readiness.settle(req);
            self.emit(
                id,
                ModelEvent::Error {
                    req: Some(req),
                    error,
                },
            );
            self.evaluate(id);
            return;
        }
        let ready = entry.status == ModelStatus::Ready;
        match change {
            ModelChange::View(view) => match view.validated() {
                Ok(view) => {
                    entry.view = view;
                    if ready {
                        entry.dirty = true;
                        entry.readiness.bump();
                    }
                }
                Err(error) => self.emit(
                    id,
                    ModelEvent::Error {
                        req: Some(req),
                        error,
                    },
                ),
            },
            ModelChange::Highlight { target, selected } => {
                let (target, selected) = match target.map(ModelTarget::validated).transpose() {
                    Ok(target) => (target, selected),
                    Err(error) => {
                        // Malformed beyond topology: the same applied
                        // fallback as an out-of-range face (no highlight),
                        // reported by one `Error` for the request.
                        self.emit(
                            id,
                            ModelEvent::Error {
                                req: Some(req),
                                error,
                            },
                        );
                        (None, None)
                    }
                };
                let entry = self.entries.get_mut(&id).expect("live");
                let changed = entry.target != target
                    || entry.selected != selected
                    || !entry.layers.is_empty();
                entry.target = target;
                entry.selected = selected;
                entry.layers.clear();
                if ready {
                    if changed {
                        entry.dirty = true;
                        entry.readiness.bump();
                    }
                    self.validate_highlight(id, Some(req));
                } else {
                    entry.unvalidated_highlight = Some(req);
                }
            }
            ModelChange::HighlightLayers(layers) => {
                let layers = if layers.len() > MODEL_MAX_HIGHLIGHT_LAYERS {
                    Err(format!(
                        "at most {MODEL_MAX_HIGHLIGHT_LAYERS} highlight layers, got {}",
                        layers.len()
                    ))
                } else {
                    layers
                        .into_iter()
                        .map(ModelHighlightLayer::validated)
                        .collect::<Result<Vec<_>, _>>()
                };
                let layers = layers.unwrap_or_else(|error| {
                    // The same applied fallback as a malformed target:
                    // no highlight, one `Error` for the request.
                    self.emit(
                        id,
                        ModelEvent::Error {
                            req: Some(req),
                            error,
                        },
                    );
                    Vec::new()
                });
                let entry = self.entries.get_mut(&id).expect("live");
                let changed =
                    entry.target.is_some() || entry.selected.is_some() || entry.layers != layers;
                entry.target = None;
                entry.selected = None;
                entry.layers = layers;
                if ready {
                    if changed {
                        entry.dirty = true;
                        entry.readiness.bump();
                    }
                    self.validate_highlight(id, Some(req));
                } else {
                    entry.unvalidated_highlight = Some(req);
                }
            }
            ModelChange::Resize { width, height } => {
                let logical = (width, height);
                let range = neomacs_display_protocol::model::MODEL_EDGE_RANGE;
                if !range.contains(&width) || !range.contains(&height) {
                    self.emit(
                        id,
                        ModelEvent::Error {
                            req: Some(req),
                            error: format!("model size {width}x{height} outside 32..2048"),
                        },
                    );
                } else if ready && entry.uploaded {
                    match host.map(|h| h.model_resize(id, logical, self.scale)) {
                        Some(Ok(size_px)) => {
                            let entry = self.entries.get_mut(&id).expect("live");
                            entry.logical = logical;
                            entry.size_px = size_px;
                            entry.dirty = true;
                            entry.readiness.bump();
                        }
                        // A refused resize keeps the old texture.
                        Some(Err(error)) => self.emit(
                            id,
                            ModelEvent::Error {
                                req: Some(req),
                                error,
                            },
                        ),
                        None => self.entries.get_mut(&id).expect("live").logical = logical,
                    }
                } else {
                    entry.logical = logical;
                }
            }
            ModelChange::Freeze => {
                entry.frozen = true;
                entry.queued_pick = None;
                entry.wheel_deadline = None;
                if self.capture.is_some_and(|c| c.id == id) {
                    self.end_capture(id);
                }
                let entry = self.entries.get_mut(&id).expect("live");
                if entry.readiness.interacting() {
                    let (view, rev) = (entry.view, entry.readiness.rev());
                    entry.readiness.set_interacting(false);
                    self.emit(id, ModelEvent::View { view, rev });
                }
            }
        }
        self.entries
            .get_mut(&id)
            .expect("live")
            .readiness
            .settle(req);
        self.evaluate(id);
    }

    // ------------------------------------------------------------------
    // Drawing and presentation

    /// Whether any model needs drawing (the loop then requests a redraw).
    pub fn has_dirty(&self) -> bool {
        self.entries
            .values()
            .any(|e| e.dirty && e.uploaded && e.status == ModelStatus::Ready)
    }

    /// Draw every dirty model into its texture.
    pub fn render_dirty(&mut self, host: &mut dyn ModelGpuHost) {
        let ids: Vec<u32> = self
            .entries
            .iter()
            .filter(|(_, e)| e.dirty && e.uploaded && e.status == ModelStatus::Ready)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let entry = self.entries.get_mut(&id).expect("live");
            let scene = entry.scene.clone().expect("ready");
            let draw = ModelDraw {
                view: entry.view,
                logical: entry.logical,
                background: entry.background,
                highlight: if entry.layers.is_empty() {
                    gpu_highlight(&scene, entry.target.as_ref(), entry.selected)
                } else {
                    gpu_layers(&scene, &entry.layers)
                },
            };
            match host.model_render(id, &draw) {
                Ok(()) => {
                    entry.dirty = false;
                    entry.rendered = Some(Snapshot {
                        rev: entry.readiness.rev(),
                        req: entry.readiness.settled(),
                        view: entry.view,
                        logical: entry.logical,
                        size_px: entry.size_px,
                    });
                }
                Err(error) => self.fail(id, error, Some(&mut *host)),
            }
        }
    }

    /// What model `id`'s texture shows now, if it can be composited.
    pub fn rendered(&self, id: u32) -> Option<Snapshot> {
        self.entries
            .get(&id)
            .filter(|e| e.status == ModelStatus::Ready && e.uploaded)
            .and_then(|e| e.rendered)
    }

    /// [`Self::rendered`] for every model that has one: what a composite
    /// drawing now would picture of each.
    pub fn rendered_snapshots(&self) -> BTreeMap<u32, Snapshot> {
        self.entries
            .keys()
            .filter_map(|id| Some((*id, self.rendered(*id)?)))
            .collect()
    }

    /// `window` presented a frame whose whole picture (root and child
    /// frames, freshly composited or retained) holds exactly the models in
    /// `shown`, each at the snapshot its pixels came from. Those join (or
    /// stay) at that snapshot's rev, which is also what a click there
    /// picks; every other model leaves, including one whose glyph the
    /// frame carries fully clipped.
    pub fn presented(&mut self, window: WindowKey, shown: &BTreeMap<u32, Snapshot>) {
        self.note_pictured(window, shown);
        if self.hidden.contains(&window) {
            return;
        }
        let ids: Vec<u32> = self.entries.keys().copied().collect();
        for id in ids {
            let entry = self.entries.get_mut(&id).expect("live");
            if entry.status != ModelStatus::Ready {
                continue;
            }
            match shown.get(&id) {
                Some(snapshot) => {
                    entry.readiness.presented(window, snapshot.rev);
                    entry.presented.insert(window, *snapshot);
                }
                None => {
                    entry.readiness.left(window);
                    entry.presented.remove(&window);
                }
            }
            self.evaluate(id);
        }
    }

    /// `window` presented a frame whose composition also shows an older
    /// picture than its fresh glyph passes (a pane morph, a frame or theme
    /// transition, a child resize crossfade), with `fresh` what those
    /// passes composited. The older pixels may hold any model the window
    /// presented before at an older snapshot, which one snapshot cannot
    /// describe (SOL-REVIEW-2 MUST 2). So every model the window now
    /// composites, shows, or may still picture in its older pictures stays
    /// (or joins) as a member at no one rev: the fence stays down and no
    /// pick resolves in this window until it presents a settled frame. A
    /// model in none of these is untouched.
    ///
    /// "May still picture" is not current membership (SOL-REVIEW-3 MUST 1):
    /// a window that was hidden or occluded, or presented frames without
    /// the model, keeps its composition ring and transition sources, which
    /// a crossfade after exposure blends in. It is every model the window
    /// presented, fresh or blended, hidden or not, until its older-picture
    /// owners are all discarded: when the window is destroyed
    /// ([`Self::window_destroyed`]), on device loss ([`Self::renderer_lost`]),
    /// or when the model is freed. A settled frame proves nothing here: the
    /// ring's previous slot can be older than the last presented frame
    /// (retained-static frames do not rotate it), and a surface resize
    /// flushes the ring and frame transitions but not child crossfade
    /// sources. The cost is a lowered fence during a later blend in a window
    /// that once showed the model.
    pub fn presented_blended(&mut self, window: WindowKey, fresh: &BTreeMap<u32, Snapshot>) {
        self.note_pictured(window, fresh);
        if self.hidden.contains(&window) {
            return;
        }
        let ids: Vec<u32> = self.entries.keys().copied().collect();
        for id in ids {
            let entry = self.entries.get_mut(&id).expect("live");
            if entry.status != ModelStatus::Ready {
                continue;
            }
            if fresh.contains_key(&id)
                || entry.readiness.is_showing_in(window)
                || entry.pictured_in.contains(&window)
            {
                entry.readiness.presented_unsettled(window);
                entry.presented.remove(&window);
                self.evaluate(id);
            }
        }
    }

    /// `window` composited (and presented) the models in `picture`: their
    /// pixels may now live in its older pictures.
    fn note_pictured(&mut self, window: WindowKey, picture: &BTreeMap<u32, Snapshot>) {
        for id in picture.keys() {
            if let Some(entry) = self.entries.get_mut(id) {
                entry.pictured_in.insert(window);
            }
        }
    }

    /// `window` was destroyed: it leaves (as [`Self::window_gone`]), and its
    /// older pictures are gone with it.
    pub fn window_destroyed(&mut self, window: WindowKey) {
        self.window_gone(window);
        for entry in self.entries.values_mut() {
            entry.pictured_in.remove(&window);
        }
    }

    /// `window` showed nothing in its last attempt, or is hidden: it leaves
    /// every showing set and its capture ends. Its older pictures, which a
    /// later blend may show, are kept ([`Self::presented_blended`]).
    pub fn window_gone(&mut self, window: WindowKey) {
        self.hidden.remove(&window);
        if self.capture.is_some_and(|c| c.window == window) {
            let id = self.capture.expect("captured").id;
            self.end_capture(id);
        }
        let ids: Vec<u32> = self.entries.keys().copied().collect();
        for id in ids {
            let entry = self.entries.get_mut(&id).expect("live");
            entry.readiness.left(window);
            entry.presented.remove(&window);
            self.evaluate(id);
        }
    }

    /// `window` is hidden, iconified or occluded: it leaves at once (as
    /// [`Self::window_gone`]) and its presentations are ignored until
    /// [`Self::window_shown`], so it rejoins only by presenting after that.
    pub fn window_hidden(&mut self, window: WindowKey) {
        self.window_gone(window);
        self.hidden.insert(window);
    }

    /// `window` is shown (exposed) again; its next presentation rejoins.
    pub fn window_shown(&mut self, window: WindowKey) {
        self.hidden.remove(&window);
    }

    // ------------------------------------------------------------------
    // Interaction (design §5.1)

    /// Ready, uploaded and not frozen: pointer input may drive it.
    pub fn is_interactive(&self, id: u32) -> bool {
        self.entries
            .get(&id)
            .is_some_and(|e| e.status == ModelStatus::Ready && !e.frozen && e.uploaded)
    }

    /// An unmodified left press on `hit` in `window` at frame position
    /// `pos`: starts a capture. Answers whether it did (the press still
    /// reaches Lisp either way).
    pub fn press(&mut self, window: WindowKey, hit: GlyphHit, pos: (f64, f64)) -> bool {
        if !self.is_interactive(hit.id) {
            return false;
        }
        if let Some(previous) = self.capture {
            self.end_capture(previous.id);
        }
        self.capture = Some(Capture {
            id: hit.id,
            window,
            start: pos,
            last: pos,
            orbiting: false,
        });
        true
    }

    /// Pointer motion at `pos` in `window`. Answers whether the motion was
    /// consumed by a capture (and so must not reach Lisp).
    pub fn motion(&mut self, window: WindowKey, pos: (f64, f64)) -> bool {
        let Some(mut capture) = self.capture else {
            return false;
        };
        if capture.window != window {
            return true;
        }
        if !capture.orbiting {
            let (dx, dy) = (pos.0 - capture.start.0, pos.1 - capture.start.1);
            if dx.hypot(dy) <= ORBIT_THRESHOLD {
                return true;
            }
            capture.orbiting = true;
            capture.last = capture.start;
        }
        let (dx, dy) = (pos.0 - capture.last.0, pos.1 - capture.last.1);
        capture.last = pos;
        self.capture = Some(capture);
        let id = capture.id;
        let Some(entry) = self.entries.get_mut(&id) else {
            self.capture = None;
            return true;
        };
        entry.view = entry.view.orbited(dx, dy);
        entry.dirty = true;
        entry.readiness.set_interacting(true);
        entry.readiness.bump();
        self.evaluate(id);
        true
    }

    /// Button release, landing on `hit` (if a model glyph) in `window`.
    /// An orbit settles with one `View`; a click without an orbit, on the
    /// model it pressed, picks.
    pub fn release(
        &mut self,
        window: WindowKey,
        hit: Option<GlyphHit>,
        now: Instant,
        host: &mut dyn ModelGpuHost,
    ) {
        let Some(capture) = self.capture.take() else {
            return;
        };
        if capture.orbiting {
            self.settle_interaction(capture.id);
            return;
        }
        let Some(hit) = hit.filter(|h| h.id == capture.id && window == capture.window) else {
            return;
        };
        let Some(entry) = self.entries.get_mut(&hit.id) else {
            return;
        };
        let Some(snapshot) = entry.presented.get(&window).copied() else {
            return;
        };
        let (w, h) = snapshot.size_px;
        let texel = (
            ((hit.u.clamp(0.0, 1.0) * w as f32) as u32).min(w.saturating_sub(1)),
            ((hit.v.clamp(0.0, 1.0) * h as f32) as u32).min(h.saturating_sub(1)),
        );
        entry.queued_pick = Some((snapshot, texel));
        self.start_queued_pick(hit.id, now, host);
    }

    /// Focus loss, or the capturing window or model went away: an orbit
    /// already applied stays and is reported by one `View`.
    pub fn cancel_capture(&mut self) {
        if let Some(capture) = self.capture {
            self.end_capture(capture.id);
        }
    }

    /// Window `window` lost focus: the capture it holds, if any, ends (as
    /// [`Self::cancel_capture`]); a capture held in another window stays.
    pub fn cancel_capture_in(&mut self, window: WindowKey) {
        if self.capture.is_some_and(|c| c.window == window) {
            self.cancel_capture();
        }
    }

    fn end_capture(&mut self, id: u32) {
        let Some(capture) = self.capture.take() else {
            return;
        };
        if capture.orbiting && capture.id == id {
            self.settle_interaction(id);
        }
    }

    /// Whether `id` has an active interaction reason: a held orbit or a
    /// wheel burst before its quiet deadline. The two are independent.
    fn interaction_active(&self, id: u32) -> bool {
        self.capture.is_some_and(|c| c.id == id && c.orbiting)
            || self
                .entries
                .get(&id)
                .is_some_and(|e| e.wheel_deadline.is_some())
    }

    /// One interaction reason of `id` ended (the caller already cleared
    /// it). Once none remains, the interaction settles with one `View`,
    /// then the fence is evaluated; while another is still active nothing
    /// is reported.
    fn settle_interaction(&mut self, id: u32) {
        if self.interaction_active(id) {
            return;
        }
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        if !entry.readiness.interacting() {
            return;
        }
        let (view, rev) = (entry.view, entry.readiness.rev());
        entry.readiness.set_interacting(false);
        self.emit(id, ModelEvent::View { view, rev });
        self.evaluate(id);
    }

    /// `notches` wheel notches over model `id` (positive zooms in; zero
    /// for input that completed no notch). Answers whether the wheel was
    /// consumed.
    ///
    /// Input that changes no camera (no whole notch, or a notch at the
    /// zoom clamp) starts no interaction, but keeps a burst already
    /// active alive: the burst settles a quiet period after its last
    /// input, not after its last zoom.
    pub fn wheel(&mut self, id: u32, notches: i32, now: Instant) -> bool {
        if !self.is_interactive(id) {
            return false;
        }
        let entry = self.entries.get_mut(&id).expect("live");
        let zoomed = entry.view.zoomed(notches);
        if zoomed == entry.view {
            if entry.wheel_deadline.is_some() {
                entry.wheel_deadline = Some(now + WHEEL_QUIET);
            }
            return true;
        }
        entry.wheel_deadline = Some(now + WHEEL_QUIET);
        entry.readiness.set_interacting(true);
        entry.view = zoomed;
        entry.dirty = true;
        entry.readiness.bump();
        self.evaluate(id);
        true
    }

    // ------------------------------------------------------------------
    // Time

    /// The next instant the loop must wake for model work, if any.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let poll = (self.has_pending_loads() || self.entries.values().any(|e| e.pick.is_some()))
            .then(|| now + POLL_INTERVAL);
        self.entries
            .values()
            .filter_map(|e| e.wheel_deadline)
            .chain(poll)
            .min()
    }

    /// Service wheel deadlines and outstanding picks.
    pub fn tick(&mut self, now: Instant, host: Option<&mut dyn ModelGpuHost>) {
        let quiet: Vec<u32> = self
            .entries
            .iter()
            .filter(|(_, e)| e.wheel_deadline.is_some_and(|d| d <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in quiet {
            self.entries.get_mut(&id).expect("live").wheel_deadline = None;
            self.settle_interaction(id);
        }
        let Some(host) = host else {
            return;
        };
        let picking: Vec<u32> = self
            .entries
            .iter()
            .filter(|(_, e)| e.pick.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in picking {
            self.poll_pick(id, now, host);
        }
    }

    fn start_queued_pick(&mut self, id: u32, now: Instant, host: &mut dyn ModelGpuHost) {
        let entry = self.entries.get_mut(&id).expect("live");
        if entry.pick.is_some() {
            // The buffer is reused only after its mapping returns; this
            // request replaces any earlier queued one meanwhile.
            return;
        }
        let Some((snapshot, texel)) = entry.queued_pick.take() else {
            return;
        };
        let request = PickRequest {
            view: snapshot.view,
            logical: snapshot.logical,
            size_px: snapshot.size_px,
            texel,
        };
        match host.model_pick_start(id, &request) {
            Ok(()) => {
                entry.pick = Some(PendingPick {
                    started: now,
                    snapshot,
                    abandoned: false,
                });
            }
            Err(error) => self.emit(id, ModelEvent::Error { req: None, error }),
        }
    }

    fn poll_pick(&mut self, id: u32, now: Instant, host: &mut dyn ModelGpuHost) {
        let poll = host.model_pick_poll(id);
        let entry = self.entries.get_mut(&id).expect("live");
        let Some(pending) = entry.pick.as_mut() else {
            return;
        };
        let event = match poll {
            PickPoll::Pending => {
                if !pending.abandoned
                    && now.saturating_duration_since(pending.started) >= PICK_TIMEOUT
                {
                    // Dropped; its staging buffer is reused only once the
                    // mapping returns, and that late answer is discarded.
                    pending.abandoned = true;
                    self.emit(
                        id,
                        ModelEvent::Error {
                            req: None,
                            error: "model pick timed out".to_owned(),
                        },
                    );
                }
                return;
            }
            PickPoll::Done(raw) => {
                let pending = entry.pick.take().expect("pending");
                (!pending.abandoned).then(|| {
                    let scene = entry.scene.clone().expect("ready");
                    ModelEvent::Pick {
                        hit: resolve_pick(&scene, raw),
                        view: pending.snapshot.view,
                        rev: pending.snapshot.rev,
                        req: pending.snapshot.req,
                    }
                })
            }
            PickPoll::Failed(error) => {
                let pending = entry.pick.take().expect("pending");
                (!pending.abandoned).then_some(ModelEvent::Error { req: None, error })
            }
            // The GPU forgot the pick (its renderer was replaced).
            PickPoll::Idle => {
                entry.pick = None;
                None
            }
        };
        if let Some(event) = event {
            self.emit(id, event);
        }
        self.start_queued_pick(id, now, host);
    }
}

/// (vertices, triangles) of `scene`.
fn scene_geometry(scene: &ModelScene) -> (u64, u64) {
    let vertices: usize = scene.meshes().iter().map(|m| m.vertices.len()).sum();
    (vertices as u64, scene.triangle_count() as u64)
}

/// Map a GPU pick to the Lisp-facing hit: per-mesh face and the point in
/// original coordinates.
pub fn resolve_pick(scene: &ModelScene, raw: RawPick) -> Option<ModelHit> {
    let (global, barycentric) = raw.hit?;
    let (mesh, face) = scene.locate(global)?;
    let point = scene.point_on(mesh, face, barycentric)?;
    Some(ModelHit {
        mesh,
        face,
        barycentric,
        point,
    })
}

/// Why `target` does not fit `scene`'s topology, if it does not.
fn target_error(scene: &ModelScene, target: &ModelTarget) -> Option<String> {
    let mesh = target.mesh();
    let Some(m) = scene.meshes().get(mesh as usize) else {
        return Some(format!("highlight mesh {mesh} out of range"));
    };
    let faces = m.triangles.len() as u64;
    match target {
        ModelTarget::Object { .. } => None,
        ModelTarget::Point { face, .. } => {
            (u64::from(*face) >= faces).then(|| format!("highlight face {face} out of range"))
        }
        ModelTarget::Region { faces: list, .. } => list
            .iter()
            .find(|f| u64::from(**f) >= faces)
            .map(|f| format!("highlight face {f} out of range")),
    }
}

/// Resolve a (validated) highlight into what the GPU paints. A target
/// suppresses the selection entirely (canvas-3d `render.py:155`).
pub fn gpu_highlight(
    scene: &ModelScene,
    target: Option<&ModelTarget>,
    selected: Option<u16>,
) -> GpuHighlight {
    let mut out = GpuHighlight::default();
    match target {
        None => out.mesh = selected,
        Some(ModelTarget::Object { mesh }) => out.mesh = Some(*mesh),
        Some(ModelTarget::Point {
            mesh,
            face,
            barycentric,
            tolerance,
        }) => {
            if let Some(centre) = scene.point_on(*mesh, *face, *barycentric) {
                out.point = Some(GpuPoint {
                    mesh: *mesh,
                    centre: scene.normalise(centre),
                    radius: (tolerance / scene.radius()) as f32,
                });
            }
        }
        Some(ModelTarget::Region { mesh, faces }) => {
            out.faces = Some(
                faces
                    .iter()
                    .filter_map(|f| scene.global_face(*mesh, *f))
                    .collect(),
            );
        }
    }
    out
}

/// Resolve (validated) coloured layers into what the GPU paints, in
/// order: a later layer wins where layers overlap. A point whose face
/// has no position paints nothing, as with a single target.
pub fn gpu_layers(scene: &ModelScene, layers: &[ModelHighlightLayer]) -> GpuHighlight {
    let layers = layers
        .iter()
        .filter_map(|layer| {
            let paint = match &layer.target {
                ModelTarget::Object { mesh } => GpuPaint::Mesh(*mesh),
                ModelTarget::Point {
                    mesh,
                    face,
                    barycentric,
                    tolerance,
                } => GpuPaint::Point(GpuPoint {
                    mesh: *mesh,
                    centre: scene.normalise(scene.point_on(*mesh, *face, *barycentric)?),
                    radius: (tolerance / scene.radius()) as f32,
                }),
                ModelTarget::Region { mesh, faces } => GpuPaint::Faces(
                    faces
                        .iter()
                        .filter_map(|f| scene.global_face(*mesh, *f))
                        .collect(),
                ),
            };
            Some(GpuLayer {
                color: layer.color,
                paint,
            })
        })
        .collect();
    GpuHighlight {
        layers,
        ..GpuHighlight::default()
    }
}

#[cfg(test)]
#[path = "store/tests.rs"]
mod tests;
