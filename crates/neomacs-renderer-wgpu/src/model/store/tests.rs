//! Store behaviour over a recording fake GPU host: loads, requests while
//! loading, settlement, readiness through presentation, the applied
//! fallback, resize refusal, freeze, interaction and picks.

use super::*;
use crate::model::obj::parse_obj;
use crate::model::scene::remaining_limits;

#[derive(Default)]
struct FakeHost {
    log: Vec<String>,
    refuse_resize: bool,
    refuse_upload: bool,
    draws: Vec<ModelDraw>,
    picks: Vec<PickRequest>,
    pick_answer: Option<PickPoll>,
}

impl ModelGpuHost for FakeHost {
    fn model_upload(
        &mut self,
        id: u32,
        _scene: &ModelScene,
        logical: (u32, u32),
        scale: f32,
    ) -> Result<(u32, u32), String> {
        self.log.push(format!("upload {id}"));
        if self.refuse_upload {
            return Err("over model budget".to_owned());
        }
        Ok((
            (logical.0 as f32 * scale) as u32,
            (logical.1 as f32 * scale) as u32,
        ))
    }
    fn model_resize(
        &mut self,
        id: u32,
        logical: (u32, u32),
        scale: f32,
    ) -> Result<(u32, u32), String> {
        self.log.push(format!("resize {id} {logical:?}"));
        if self.refuse_resize {
            return Err("over model budget".to_owned());
        }
        Ok((
            (logical.0 as f32 * scale) as u32,
            (logical.1 as f32 * scale) as u32,
        ))
    }
    fn model_render(&mut self, id: u32, draw: &ModelDraw) -> Result<(), String> {
        self.log.push(format!("render {id}"));
        self.draws.push(draw.clone());
        Ok(())
    }
    fn model_free(&mut self, id: u32) {
        self.log.push(format!("free {id}"));
    }
    fn model_pick_start(&mut self, _id: u32, request: &PickRequest) -> Result<(), String> {
        self.picks.push(*request);
        Ok(())
    }
    fn model_pick_poll(&mut self, _id: u32) -> PickPoll {
        self.pick_answer.take().unwrap_or(PickPoll::Pending)
    }
}

const QUAD: &[u8] = b"v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n";
const W: WindowKey = 1;

fn scene() -> Arc<ModelScene> {
    let a = parse_obj(QUAD, remaining_limits(&[])).unwrap();
    let b = parse_obj(QUAD, remaining_limits(&[])).unwrap();
    Arc::new(ModelScene::new(vec![a, b]).unwrap())
}

/// A store with model 5 created and its load delivered (no loader thread:
/// the result is injected as the loader would answer it).
fn loaded_store(host: &mut FakeHost) -> ModelStore {
    let mut store = ModelStore::new();
    store.entries.insert(5, loading_entry(1));
    store.loaded(
        LoadResult {
            id: 5,
            token: 1,
            outcome: Ok(scene()),
        },
        h(host),
    );
    store
}

fn loading_entry(token: u64) -> Entry {
    Entry {
        token,
        status: ModelStatus::Loading,
        scene: None,
        uploaded: false,
        view: ModelView::default(),
        target: None,
        selected: None,
        unvalidated_highlight: None,
        background: [0.0, 0.0, 0.0, 1.0],
        logical: (64, 32),
        size_px: (0, 0),
        frozen: false,
        dirty: false,
        readiness: Readiness::default(),
        rendered: None,
        presented: BTreeMap::new(),
        pick: None,
        queued_pick: None,
        wheel_deadline: None,
    }
}

/// The fake as the trait object the store takes (coercion does not reach
/// inside an `Option`).
fn h(host: &mut FakeHost) -> Option<&mut dyn ModelGpuHost> {
    Some(host)
}

fn shown(store: &mut ModelStore, host: &mut FakeHost) {
    store.render_dirty(host);
    store.presented(W, &HashSet::from([5]), &HashSet::from([5]));
}

fn kinds(events: &[(u32, ModelEvent)]) -> Vec<String> {
    events
        .iter()
        .map(|(_, e)| match e {
            ModelEvent::Readiness { current, rev, req } => format!("R{{{current},{rev},{req}}}"),
            ModelEvent::Error { req, .. } => format!("Error{{{req:?}}}"),
            ModelEvent::View { rev, .. } => format!("View{{{rev}}}"),
            ModelEvent::Pick { rev, req, hit, .. } => {
                format!("Pick{{{rev},{req},{}}}", hit.is_some())
            }
            other => other.kind().to_owned(),
        })
        .collect()
}

#[test]
fn an_idle_load_becomes_ready_and_current_once_presented() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    assert_eq!(store.status(5), Some(ModelStatus::Ready));
    assert_eq!(kinds(&store.take_events()), ["ready"]);
    assert!(store.has_dirty());
    shown(&mut store, &mut host);
    assert_eq!(kinds(&store.take_events()), ["R{true,1,0}"]);
    assert_eq!(host.draws[0].logical, (64, 32));
}

#[test]
fn a_stale_load_result_is_dropped() {
    let mut host = FakeHost::default();
    let mut store = ModelStore::new();
    store.entries.insert(5, loading_entry(2));
    store.loaded(
        LoadResult {
            id: 5,
            token: 1,
            outcome: Ok(scene()),
        },
        h(&mut host),
    );
    assert_eq!(store.status(5), Some(ModelStatus::Loading));
    assert!(host.log.is_empty());
}

#[test]
fn an_upload_over_budget_fails_the_model_and_frees_it() {
    let mut host = FakeHost {
        refuse_upload: true,
        ..FakeHost::default()
    };
    let store = &mut loaded_store(&mut host);
    assert_eq!(store.status(5), Some(ModelStatus::Failed));
    assert_eq!(kinds(&store.take_events()), ["failed"]);
    assert_eq!(host.log, ["upload 5", "free 5"]);
}

#[test]
fn requests_while_loading_are_kept_and_the_latest_applies() {
    let mut host = FakeHost::default();
    let mut store = ModelStore::new();
    store.entries.insert(5, loading_entry(1));
    let view = |yaw| ModelView {
        yaw,
        pitch: 0.0,
        zoom: 1.0,
    };
    store.update(5, 1, ModelChange::View(view(10.0)), None);
    store.update(5, 2, ModelChange::View(view(20.0)), None);
    store.update(
        5,
        3,
        ModelChange::Highlight {
            target: Some(ModelTarget::Region {
                mesh: 1,
                faces: vec![9],
            }),
            selected: None,
        },
        None,
    );
    store.take_events();
    store.loaded(
        LoadResult {
            id: 5,
            token: 1,
            outcome: Ok(scene()),
        },
        h(&mut host),
    );
    // Face 9 does not exist: validated at upload, cleared, Error{3}.
    assert_eq!(kinds(&store.take_events()), ["ready", "Error{Some(3)}"]);
    shown(&mut store, &mut host);
    let draw = host.draws.last().unwrap();
    assert_eq!(draw.view.yaw, 20.0);
    assert_eq!(draw.highlight, GpuHighlight::default());
    assert_eq!(kinds(&store.take_events()), ["R{true,2,3}"]);
}

#[test]
fn a_new_request_settles_and_restores_readiness_after_presentation() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    store.update(
        5,
        1,
        ModelChange::Highlight {
            target: Some(ModelTarget::Object { mesh: 1 }),
            selected: Some(0),
        },
        h(&mut host),
    );
    assert_eq!(kinds(&store.take_events()), ["R{false,2,1}"]);
    shown(&mut store, &mut host);
    assert_eq!(kinds(&store.take_events()), ["R{true,2,1}"]);
    // The target suppresses the selection.
    assert_eq!(host.draws.last().unwrap().highlight.mesh, Some(1));
}

/// §6.2 trace 5: an idle rejected resize settles with no redraw.
#[test]
fn a_refused_resize_keeps_the_texture_and_settles() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    host.refuse_resize = true;
    store.update(
        5,
        8,
        ModelChange::Resize {
            width: 512,
            height: 512,
        },
        h(&mut host),
    );
    assert_eq!(
        kinds(&store.take_events()),
        ["Error{Some(8)}", "R{true,1,8}"]
    );
    assert!(!store.has_dirty());
}

/// §6.2 trace 6: an invalid highlight replacing a visible one.
#[test]
fn an_invalid_highlight_is_cleared_redrawn_and_reported() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    store.update(
        5,
        1,
        ModelChange::Highlight {
            target: Some(ModelTarget::Object { mesh: 0 }),
            selected: None,
        },
        h(&mut host),
    );
    shown(&mut store, &mut host);
    store.take_events();
    store.update(
        5,
        9,
        ModelChange::Highlight {
            target: Some(ModelTarget::Point {
                mesh: 0,
                face: 99,
                barycentric: [1.0, 0.0, 0.0],
                tolerance: 0.1,
            }),
            selected: None,
        },
        h(&mut host),
    );
    assert_eq!(
        kinds(&store.take_events()),
        ["Error{Some(9)}", "R{false,4,9}"]
    );
    shown(&mut store, &mut host);
    assert_eq!(kinds(&store.take_events()), ["R{true,4,9}"]);
    assert_eq!(
        host.draws.last().unwrap().highlight,
        GpuHighlight::default()
    );
}

#[test]
fn frozen_models_reject_requests_and_interaction() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.update(5, 1, ModelChange::Freeze, h(&mut host));
    store.take_events();
    store.update(5, 2, ModelChange::View(ModelView::default()), h(&mut host));
    assert_eq!(
        kinds(&store.take_events()),
        ["Error{Some(2)}", "R{true,1,2}"]
    );
    let hit = GlyphHit {
        id: 5,
        u: 0.5,
        v: 0.5,
    };
    assert!(!store.press(W, hit, (10.0, 10.0)));
    assert!(!store.wheel(5, 1, Instant::now()));
}

/// §6.2 trace 1 through the store: one `R{nil}` at the orbit's start, a
/// `View` at release, then `R{t}` once presented.
#[test]
fn an_orbit_reports_once_and_settles_with_a_view() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    let hit = GlyphHit {
        id: 5,
        u: 0.5,
        v: 0.5,
    };
    assert!(store.press(W, hit, (100.0, 100.0)));
    assert!(store.motion(W, (102.0, 101.0)), "consumed below threshold");
    assert!(!store.has_dirty(), "no orbit yet");
    assert!(store.motion(W, (110.0, 100.0)));
    assert!(store.motion(W, (120.0, 96.0)));
    assert_eq!(kinds(&store.take_events()), ["R{false,2,0}"]);
    let view = store.view(5).unwrap();
    // 20 px right, 4 px up from the press, 0.5 degree per pixel, wrapped.
    assert_eq!((view.yaw, view.pitch), (10.0, 358.0));
    shown(&mut store, &mut host);
    assert!(store.take_events().is_empty(), "interacting");
    store.release(W, Some(hit), Instant::now(), &mut host);
    assert_eq!(kinds(&store.take_events()), ["View{3}", "R{true,3,0}"]);
    assert!(host.picks.is_empty(), "an orbit does not pick");
}

#[test]
fn focus_loss_mid_orbit_keeps_the_orbit_and_reports_it() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    let hit = GlyphHit {
        id: 5,
        u: 0.5,
        v: 0.5,
    };
    store.press(W, hit, (0.0, 0.0));
    store.motion(W, (10.0, 0.0));
    store.cancel_capture();
    assert_eq!(kinds(&store.take_events()), ["R{false,2,0}", "View{2}"]);
    assert_eq!(store.view(5).unwrap().yaw, 5.0);
}

#[test]
fn wheel_zooms_and_settles_on_the_quiet_deadline() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    let t0 = Instant::now();
    assert!(store.wheel(5, 1, t0));
    assert!(store.wheel(5, 1, t0 + Duration::from_millis(100)));
    assert_eq!(kinds(&store.take_events()), ["R{false,2,0}"]);
    assert!((store.view(5).unwrap().zoom - 1.21).abs() < 1e-12);
    shown(&mut store, &mut host);
    store.tick(t0 + Duration::from_millis(200), h(&mut host));
    assert!(store.take_events().is_empty(), "quiet not yet elapsed");
    assert_eq!(
        store.next_deadline(t0),
        Some(t0 + Duration::from_millis(250))
    );
    store.tick(t0 + Duration::from_millis(260), h(&mut host));
    assert_eq!(kinds(&store.take_events()), ["View{3}", "R{true,3,0}"]);
}

#[test]
fn a_click_picks_the_presented_snapshot() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    let hit = GlyphHit {
        id: 5,
        u: 0.25,
        v: 0.5,
    };
    let t0 = Instant::now();
    store.press(W, hit, (5.0, 5.0));
    store.release(W, Some(hit), t0, &mut host);
    assert_eq!(host.picks.len(), 1);
    assert_eq!(host.picks[0].texel, (16, 16));
    assert_eq!(host.picks[0].size_px, (64, 32));
    // The model changes before the answer: the pick still names what was
    // presented (rev 1), so Lisp can drop it as stale.
    store.update(5, 4, ModelChange::View(ModelView::default()), h(&mut host));
    store.take_events();
    host.pick_answer = Some(PickPoll::Done(RawPick {
        hit: Some((3, [0.25, 0.25, 0.5])),
    }));
    store.tick(t0, h(&mut host));
    let events = store.take_events();
    assert_eq!(kinds(&events), ["Pick{1,0,true}"]);
    let ModelEvent::Pick { hit: Some(hit), .. } = &events[0].1 else {
        unreachable!()
    };
    // Global face 3 is mesh 1's face 1 (two faces per quad).
    assert_eq!((hit.mesh, hit.face), (1, 1));
    // Face 1 fans (v0, v2, v3) = (-1,-1), (1,1), (-1,1).
    assert_eq!(hit.point, [-0.5, 0.5, 0.0]);
}

#[test]
fn a_release_off_the_model_or_after_drag_does_not_pick() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    let hit = GlyphHit {
        id: 5,
        u: 0.5,
        v: 0.5,
    };
    store.press(W, hit, (5.0, 5.0));
    store.release(W, None, Instant::now(), &mut host);
    assert!(host.picks.is_empty());
}

#[test]
fn a_pick_times_out_and_its_late_answer_is_dropped() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    let hit = GlyphHit {
        id: 5,
        u: 0.5,
        v: 0.5,
    };
    let t0 = Instant::now();
    store.press(W, hit, (5.0, 5.0));
    store.release(W, Some(hit), t0, &mut host);
    store.tick(t0 + PICK_TIMEOUT, h(&mut host));
    assert_eq!(kinds(&store.take_events()), ["Error{None}"]);
    // A second click waits for the first mapping to return.
    store.press(W, hit, (5.0, 5.0));
    store.release(W, Some(hit), t0 + PICK_TIMEOUT, &mut host);
    assert_eq!(host.picks.len(), 1);
    host.pick_answer = Some(PickPoll::Done(RawPick { hit: None }));
    store.tick(t0 + PICK_TIMEOUT, h(&mut host));
    assert!(store.take_events().is_empty(), "late answer dropped");
    assert_eq!(host.picks.len(), 2, "the queued click starts");
}

#[test]
fn destroy_during_a_pick_and_a_load_releases_everything() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    let hit = GlyphHit {
        id: 5,
        u: 0.5,
        v: 0.5,
    };
    store.press(W, hit, (5.0, 5.0));
    store.release(W, Some(hit), Instant::now(), &mut host);
    store.free(5, h(&mut host));
    assert!(host.log.ends_with(&["free 5".to_owned()]));
    assert!(!store.contains(5));
    store.take_events();
    host.pick_answer = Some(PickPoll::Done(RawPick { hit: None }));
    store.tick(Instant::now(), h(&mut host));
    assert!(store.take_events().is_empty());
    // A load result arriving after free is dropped.
    store.loaded(
        LoadResult {
            id: 5,
            token: 1,
            outcome: Ok(scene()),
        },
        h(&mut host),
    );
    assert!(!store.contains(5));
}

#[test]
fn hiding_every_window_drops_and_reshowing_restores() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    store.window_gone(W);
    assert_eq!(kinds(&store.take_events()), ["R{false,1,0}"]);
    store.presented(W, &HashSet::from([5]), &HashSet::from([5]));
    assert_eq!(kinds(&store.take_events()), ["R{true,1,0}"]);
    // A frame without the glyph leaves; one carrying it undrawn keeps it.
    store.presented(W, &HashSet::new(), &HashSet::from([5]));
    assert!(store.take_events().is_empty());
    store.presented(W, &HashSet::new(), &HashSet::new());
    assert_eq!(kinds(&store.take_events()), ["R{false,1,0}"]);
}

#[test]
fn a_rescale_rebuilds_targets_and_bumps_the_revision() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    store.set_scale(2.0, &mut host);
    assert!(host.log.contains(&"resize 5 (64, 32)".to_owned()));
    assert_eq!(kinds(&store.take_events()), ["R{false,2,0}"]);
    shown(&mut store, &mut host);
    assert_eq!(kinds(&store.take_events()), ["R{true,2,0}"]);
}

#[test]
fn renderer_loss_then_a_new_renderer_reuploads() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.renderer_lost();
    assert!(!store.has_dirty());
    let mut fresh = FakeHost::default();
    store.upload_pending(&mut fresh);
    assert_eq!(fresh.log, ["upload 5"]);
    assert!(store.has_dirty());
    // Ready is reported once, not again on re-upload.
    assert_eq!(
        kinds(&store.take_events())
            .iter()
            .filter(|k| *k == "ready")
            .count(),
        1
    );
}

#[test]
fn highlight_resolution_follows_canvas_3d_precedence() {
    let scene = scene();
    let none = gpu_highlight(&scene, None, Some(1));
    assert_eq!(none.mesh, Some(1));
    let region = gpu_highlight(
        &scene,
        Some(&ModelTarget::Region {
            mesh: 1,
            faces: vec![0, 1],
        }),
        Some(0),
    );
    assert_eq!((region.mesh, region.faces), (None, Some(vec![2, 3])));
    let point = gpu_highlight(
        &scene,
        Some(&ModelTarget::Point {
            mesh: 0,
            face: 0,
            barycentric: [0.5, 0.0, 0.5],
            tolerance: 0.5,
        }),
        Some(1),
    );
    let p = point.point.unwrap();
    assert_eq!((point.mesh, p.mesh), (None, 0));
    assert!((p.radius as f64 - 0.5 / scene.radius()).abs() < 1e-7);
}
