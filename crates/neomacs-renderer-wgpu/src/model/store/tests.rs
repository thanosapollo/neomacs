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
    refuse_render: bool,
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
        if self.refuse_render {
            return Err("device lost".to_owned());
        }
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
        layers: Vec::new(),
        unvalidated_highlight: None,
        background: [0.0, 0.0, 0.0, 1.0],
        logical: (64, 32),
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
    }
}

/// The fake as the trait object the store takes (coercion does not reach
/// inside an `Option`).
fn h(host: &mut FakeHost) -> Option<&mut dyn ModelGpuHost> {
    Some(host)
}

fn shown(store: &mut ModelStore, host: &mut FakeHost) {
    store.render_dirty(host);
    let picture = store.rendered_snapshots();
    store.presented(W, &picture);
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
    let picture = store.rendered_snapshots();
    store.presented(W, &picture);
    assert_eq!(kinds(&store.take_events()), ["R{true,1,0}"]);
}

/// Review MUST 6: a presented frame whose picture holds none of the model's
/// pixels (its glyph fully clipped, so the composite skipped it) is not a
/// showing, even though the frame still carries the glyph.
#[test]
fn a_full_frame_with_the_model_fully_clipped_leaves() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    store.presented(W, &BTreeMap::new());
    assert_eq!(kinds(&store.take_events()), ["R{false,1,0}"]);
}

/// A window picturing an older snapshot (a retained raster) is a member
/// behind the current rev, and a click there picks that snapshot's camera.
#[test]
fn a_presentation_reports_the_snapshot_its_pixels_came_from() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    let old = store.rendered(5).unwrap();
    store.update(
        5,
        1,
        ModelChange::View(ModelView {
            yaw: 40.0,
            pitch: 0.0,
            zoom: 1.0,
        }),
        h(&mut host),
    );
    store.render_dirty(&mut host);
    store.take_events();
    store.presented(W, &BTreeMap::from([(5, old)]));
    assert!(store.take_events().is_empty(), "behind: fence stays down");
    let hit = GlyphHit {
        id: 5,
        u: 0.5,
        v: 0.5,
    };
    store.press(W, hit, (1.0, 1.0));
    store.release(W, Some(hit), Instant::now(), &mut host);
    assert_eq!(host.picks.last().unwrap().view, old.view);
    shown(&mut store, &mut host);
    assert_eq!(kinds(&store.take_events()), ["R{true,2,1}"]);
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

// ----------------------------------------------------------------------
// Coloured highlight layers.

const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const RED: [f32; 3] = [1.0, 0.0, 0.0];

fn layer(target: ModelTarget, color: [f32; 3]) -> ModelHighlightLayer {
    ModelHighlightLayer { target, color }
}

#[test]
fn layers_resolve_in_order_with_their_colours() {
    let scene = scene();
    let resolved = gpu_layers(
        &scene,
        &[
            layer(
                ModelTarget::Region {
                    mesh: 1,
                    faces: vec![1],
                },
                GREEN,
            ),
            layer(ModelTarget::Object { mesh: 0 }, RED),
            layer(
                ModelTarget::Point {
                    mesh: 0,
                    face: 0,
                    barycentric: [0.5, 0.0, 0.5],
                    tolerance: 0.5,
                },
                [0.0, 0.0, 1.0],
            ),
        ],
    );
    assert_eq!(
        (resolved.mesh, resolved.point.is_none(), resolved.faces),
        (None, true, None)
    );
    assert_eq!(resolved.layers.len(), 3);
    assert_eq!(
        resolved.layers[0],
        GpuLayer {
            color: GREEN,
            paint: GpuPaint::Faces(vec![3]),
        }
    );
    assert_eq!(
        resolved.layers[1],
        GpuLayer {
            color: RED,
            paint: GpuPaint::Mesh(0),
        }
    );
    match &resolved.layers[2].paint {
        GpuPaint::Point(p) => {
            assert_eq!(p.mesh, 0);
            assert!((p.radius as f64 - 0.5 / scene.radius()).abs() < 1e-7);
        }
        other => panic!("expected a point layer, got {other:?}"),
    }
}

/// Layers and the single target replace each other; both bump readiness
/// like any highlight, and `nil` layers clear everything.
#[test]
fn layers_replace_the_target_and_are_revision_qualified() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    store.update(
        5,
        1,
        ModelChange::Highlight {
            target: Some(ModelTarget::Object { mesh: 1 }),
            selected: Some(0),
        },
        h(&mut host),
    );
    shown(&mut store, &mut host);
    store.take_events();
    let layers = vec![
        layer(ModelTarget::Object { mesh: 1 }, GREEN),
        layer(ModelTarget::Object { mesh: 0 }, RED),
    ];
    store.update(
        5,
        2,
        ModelChange::HighlightLayers(layers.clone()),
        h(&mut host),
    );
    assert_eq!(kinds(&store.take_events()), ["R{false,3,2}"]);
    shown(&mut store, &mut host);
    assert_eq!(kinds(&store.take_events()), ["R{true,3,2}"]);
    let draw = host.draws.last().unwrap().highlight.clone();
    assert_eq!((draw.mesh, draw.layers.len()), (None, 2));
    // The same layers again change nothing visible: settled, no redraw.
    let renders = host.draws.len();
    store.update(5, 3, ModelChange::HighlightLayers(layers), h(&mut host));
    shown(&mut store, &mut host);
    assert_eq!(kinds(&store.take_events()), ["R{true,3,3}"]);
    assert_eq!(host.draws.len(), renders);
    // A target request drops the layers again.
    store.update(
        5,
        4,
        ModelChange::Highlight {
            target: None,
            selected: Some(1),
        },
        h(&mut host),
    );
    shown(&mut store, &mut host);
    let draw = host.draws.last().unwrap().highlight.clone();
    assert_eq!((draw.mesh, draw.layers.len()), (Some(1), 0));
    // Empty layers clear the selection too.
    store.update(5, 5, ModelChange::HighlightLayers(vec![]), h(&mut host));
    shown(&mut store, &mut host);
    assert_eq!(
        host.draws.last().unwrap().highlight,
        GpuHighlight::default()
    );
    assert_eq!(
        kinds(&store.take_events()),
        ["R{false,4,4}", "R{true,4,4}", "R{false,5,5}", "R{true,5,5}"]
    );
}

/// One bad layer is the applied fallback for the whole request: no
/// highlight, one `Error`, whether malformed, out of range or too many.
#[test]
fn an_invalid_layer_clears_every_layer_and_reports_once() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    let good = layer(ModelTarget::Object { mesh: 0 }, GREEN);
    let cases = [
        vec![
            good.clone(),
            layer(
                ModelTarget::Region {
                    mesh: 1,
                    faces: vec![99],
                },
                RED,
            ),
        ],
        vec![good.clone(), layer(ModelTarget::Object { mesh: 7 }, RED)],
        vec![
            good.clone(),
            layer(ModelTarget::Object { mesh: 1 }, [2.0, 0.0, 0.0]),
        ],
        vec![good.clone(); MODEL_MAX_HIGHLIGHT_LAYERS + 1],
    ];
    let mut req = 0;
    for layers in cases {
        req += 1;
        store.update(
            5,
            req,
            ModelChange::HighlightLayers(vec![good.clone()]),
            h(&mut host),
        );
        shown(&mut store, &mut host);
        assert_eq!(host.draws.last().unwrap().highlight.layers.len(), 1);
        store.take_events();
        req += 1;
        store.update(5, req, ModelChange::HighlightLayers(layers), h(&mut host));
        let events = kinds(&store.take_events());
        assert_eq!(
            events
                .iter()
                .filter(|k| k.starts_with("Error"))
                .collect::<Vec<_>>(),
            [&format!("Error{{Some({req})}}")],
            "{events:?}"
        );
        shown(&mut store, &mut host);
        assert_eq!(
            host.draws.last().unwrap().highlight,
            GpuHighlight::default()
        );
    }
}

/// Layers requested while loading are validated at upload, like a target.
#[test]
fn layers_requested_while_loading_are_validated_at_upload() {
    let mut host = FakeHost::default();
    let mut store = ModelStore::new();
    store.entries.insert(5, loading_entry(1));
    store.update(
        5,
        1,
        ModelChange::HighlightLayers(vec![
            layer(ModelTarget::Object { mesh: 1 }, GREEN),
            layer(
                ModelTarget::Point {
                    mesh: 0,
                    face: 0,
                    barycentric: [1.0, 0.0, 0.0],
                    tolerance: 0.1,
                },
                RED,
            ),
        ]),
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
    assert_eq!(kinds(&store.take_events()), ["ready"]);
    shown(&mut store, &mut host);
    assert_eq!(host.draws.last().unwrap().highlight.layers.len(), 2);
}

// ----------------------------------------------------------------------
// Review MUST 10: orbit capture and wheel burst are independent reasons.

fn centre_hit() -> GlyphHit {
    GlyphHit {
        id: 5,
        u: 0.5,
        v: 0.5,
    }
}

/// A wheel burst that goes quiet while an orbit is still held settles
/// nothing: one `View` and the readiness follow the release (here outside
/// the glyph), not the wheel deadline.
#[test]
fn a_wheel_quiet_deadline_during_a_held_orbit_does_not_settle_it() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    let t0 = Instant::now();
    assert!(store.press(W, centre_hit(), (100.0, 100.0)));
    assert!(store.motion(W, (110.0, 100.0)));
    assert_eq!(kinds(&store.take_events()), ["R{false,2,0}"]);
    assert!(store.wheel(5, 1, t0));
    // The wheel's revision is presented before its quiet deadline.
    shown(&mut store, &mut host);
    assert!(store.take_events().is_empty(), "interacting");
    store.tick(t0 + WHEEL_QUIET + Duration::from_millis(10), h(&mut host));
    assert!(
        store.take_events().is_empty(),
        "the orbit is still held: no View, no readiness"
    );
    assert!(store.motion(W, (120.0, 100.0)), "the capture survives");
    shown(&mut store, &mut host);
    assert!(store.take_events().is_empty(), "still interacting");
    // Released outside the glyph: the orbit ends, and with it the last
    // active reason.
    store.release(W, None, t0 + Duration::from_millis(400), &mut host);
    assert_eq!(kinds(&store.take_events()), ["View{4}", "R{true,4,0}"]);
    assert!(host.picks.is_empty());
}

/// An orbit released during a wheel burst does not end the burst: the
/// wheel's quiet deadline still stands, and its expiry settles both.
#[test]
fn releasing_an_orbit_during_a_wheel_burst_waits_for_the_quiet_deadline() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    let t0 = Instant::now();
    assert!(store.wheel(5, 1, t0));
    assert_eq!(kinds(&store.take_events()), ["R{false,2,0}"]);
    assert!(store.press(W, centre_hit(), (100.0, 100.0)));
    assert!(store.motion(W, (110.0, 100.0)));
    // Everything is presented before the wheel deadline.
    shown(&mut store, &mut host);
    assert!(store.take_events().is_empty(), "interacting");
    store.release(W, None, t0 + Duration::from_millis(50), &mut host);
    assert!(
        store.take_events().is_empty(),
        "the wheel burst is still active: no View, no readiness"
    );
    assert_eq!(
        store.next_deadline(t0 + Duration::from_millis(50)),
        Some(t0 + WHEEL_QUIET),
        "the wheel deadline survives the release"
    );
    store.tick(t0 + Duration::from_millis(100), h(&mut host));
    assert!(store.take_events().is_empty(), "quiet not yet elapsed");
    store.tick(t0 + WHEEL_QUIET, h(&mut host));
    assert_eq!(kinds(&store.take_events()), ["View{3}", "R{true,3,0}"]);
    assert_eq!(store.next_deadline(t0 + WHEEL_QUIET), None);
}

// ----------------------------------------------------------------------
// Review MUST 9: renderer loss invalidates at once.

/// current -> renderer loss: the false level is published immediately (no
/// replacement renderer, no later upload needed), then a re-upload and a
/// presentation restore it.
#[test]
fn renderer_loss_publishes_the_false_level_at_once() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    assert_eq!(kinds(&store.take_events()), ["ready", "R{true,1,0}"]);
    let stale = store.rendered_snapshots();
    store.renderer_lost();
    assert_eq!(kinds(&store.take_events()), ["R{false,2,0}"]);
    // A presentation report arriving after the loss (no texture exists any
    // more) cannot rejoin the old provenance.
    store.presented(W, &stale);
    assert!(store.take_events().is_empty(), "no stale rejoin");
    assert!(!store.is_interactive(5));
    let mut fresh = FakeHost::default();
    store.upload_pending(&mut fresh);
    shown(&mut store, &mut fresh);
    assert_eq!(kinds(&store.take_events()), ["R{true,3,0}"]);
}

/// An orbit and a wheel burst active at the loss end with one `View` (the
/// applied view stays), the capture is gone, and no later deadline fires.
#[test]
fn renderer_loss_settles_an_active_interaction() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    let t0 = Instant::now();
    assert!(store.press(W, centre_hit(), (0.0, 0.0)));
    assert!(store.motion(W, (10.0, 0.0)));
    assert!(store.wheel(5, 1, t0));
    assert_eq!(kinds(&store.take_events()), ["R{false,2,0}"]);
    // Everything was presented while still interacting.
    shown(&mut store, &mut host);
    assert!(store.take_events().is_empty());
    store.renderer_lost();
    // The View names the interaction's last revision. No transient
    // R{t} for the presented revision: the provenance is dropped before
    // settling, and the fence was already down (no second R{nil}).
    assert_eq!(kinds(&store.take_events()), ["View{3}"]);
    assert!(!store.motion(W, (20.0, 0.0)), "the capture ended");
    store.tick(t0 + WHEEL_QUIET, None);
    assert!(store.take_events().is_empty(), "nothing left to settle");
    assert_eq!(store.next_deadline(t0), None);
}

// ----------------------------------------------------------------------
// Review MUST 1: live aggregate geometry caps (design §4.3).

/// A scene of `vertices` vertices (a fan of distinct points) and
/// `triangles` repetitions of one nondegenerate triangle.
fn geometry(vertices: usize, triangles: usize) -> Arc<ModelScene> {
    let mut points = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    points.extend((3..vertices).map(|i| [i as f64, 2.0, 0.0]));
    let mesh = crate::model::obj::ObjMesh {
        vertices: points,
        triangles: vec![[0, 1, 2]; triangles],
    };
    Arc::new(ModelScene::new(vec![mesh]).unwrap())
}

fn deliver(
    store: &mut ModelStore,
    id: u32,
    scene: &Arc<ModelScene>,
    host: Option<&mut dyn ModelGpuHost>,
) {
    store.entries.insert(id, loading_entry(1));
    store.loaded(
        LoadResult {
            id,
            token: 1,
            outcome: Ok(scene.clone()),
        },
        host,
    );
}

/// The reviewer's case: five valid 2,000,000-triangle scenes exceed the
/// 8,000,000 live triangle cap; with no renderer at all (no byte budget
/// runs) the fifth must still be refused, and freeing one admits it.
#[test]
fn the_live_triangle_cap_holds_without_a_renderer() {
    let big = geometry(3, crate::model::scene::MAX_MODEL_TRIANGLES);
    let mut store = ModelStore::new();
    for id in 1..=4 {
        deliver(&mut store, id, &big, None);
        assert!(store.entries[&id].scene.is_some(), "model {id} at the cap");
    }
    store.take_events();
    deliver(&mut store, 5, &big, None);
    assert_eq!(store.status(5), Some(ModelStatus::Failed));
    let events = store.take_events();
    assert_eq!(kinds(&events), ["failed"]);
    assert!(store.entries[&5].scene.is_none(), "not retained");
    // The refused one holds nothing; freeing a live one releases its share.
    store.free(5, None);
    store.free(1, None);
    deliver(&mut store, 6, &big, None);
    assert!(store.entries[&6].scene.is_some(), "admitted after a free");
}

/// Both caps at their exact boundary: 8,000,000 is admitted, one more is
/// not; a failed model releases its reservation; an admitted scene
/// uploads, a refused one never reaches the GPU.
#[test]
fn the_live_vertex_cap_is_exact_and_released_on_failure() {
    let big = geometry(crate::model::scene::MAX_MODEL_VERTICES, 1);
    let small = geometry(3, 1);
    let mut host = FakeHost::default();
    let mut store = ModelStore::new();
    for id in 1..=4 {
        deliver(&mut store, id, &big, h(&mut host));
        assert_eq!(store.status(id), Some(ModelStatus::Ready), "model {id}");
    }
    host.log.clear();
    deliver(&mut store, 5, &small, h(&mut host));
    assert_eq!(store.status(5), Some(ModelStatus::Failed));
    assert!(
        !host.log.iter().any(|l| l == "upload 5"),
        "refused before uploading: {:?}",
        host.log
    );
    // A model that fails later (its upload refused) gives its share back.
    store.free(5, h(&mut host));
    store.free(4, h(&mut host));
    host.refuse_upload = true;
    deliver(&mut store, 6, &big, h(&mut host));
    assert_eq!(store.status(6), Some(ModelStatus::Failed));
    host.refuse_upload = false;
    deliver(&mut store, 7, &big, h(&mut host));
    assert_eq!(
        store.status(7),
        Some(ModelStatus::Ready),
        "exactly 8,000,000"
    );
}

// ----------------------------------------------------------------------
// Round 2: a load freed while loading, and wheel input that changes no
// camera.

/// Pre-review MUST 1 (store level): the result of a load freed while it
/// was loading is drained with the store empty, emits nothing, and the
/// poll stops once it is.
#[test]
fn a_load_freed_while_loading_is_drained_and_stops_the_poll() {
    let dir = std::env::temp_dir().join(format!(
        "neomacs-model-store-freed-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("quad.obj");
    std::fs::write(&path, QUAD).unwrap();
    let mut store = ModelStore::new();
    store.create(
        5,
        vec![ModelMeshSpec { path, sha256: None }],
        ModelView::default(),
        [0.0, 0.0, 0.0, 1.0],
        (64, 32),
    );
    store.free(5, None);
    assert!(store.is_empty());
    let t0 = Instant::now();
    assert_eq!(
        store.next_deadline(t0),
        Some(t0 + POLL_INTERVAL),
        "the freed load's result is still owed"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while store.has_pending_loads() {
        assert!(Instant::now() < deadline, "the result never arrived");
        std::thread::sleep(Duration::from_millis(2));
        store.poll_loads(None);
    }
    assert!(store.take_events().is_empty());
    assert_eq!(store.next_deadline(Instant::now()), None);
}

/// Wheel input that changes no camera (a sub-notch event, or a notch at
/// the zoom clamp) starts no interaction, but keeps a burst already
/// active alive: it settles a quiet period after the last input
/// (display-lane open item; pre-review NIT 3).
#[test]
fn wheel_input_without_a_zoom_change_keeps_a_burst_but_starts_none() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    store.update(
        5,
        1,
        ModelChange::View(ModelView {
            yaw: 0.0,
            pitch: 0.0,
            zoom: 4.0,
        }),
        h(&mut host),
    );
    shown(&mut store, &mut host);
    store.take_events();
    let t0 = Instant::now();
    let ms = |n: u64| t0 + Duration::from_millis(n);
    // A notch at the clamp, and a sub-notch event: consumed, no burst.
    assert!(store.wheel(5, 1, ms(0)));
    assert!(store.wheel(5, 0, ms(0)));
    assert!(store.take_events().is_empty(), "no R{{nil}} at the clamp");
    assert_eq!(store.next_deadline(ms(0)), None, "no burst started");

    // A burst zooming out; sub-notch input at 100 ms keeps it active.
    assert!(store.wheel(5, -1, ms(0)));
    assert_eq!(kinds(&store.take_events()), ["R{false,3,1}"]);
    assert!(store.wheel(5, 0, ms(100)));
    assert_eq!(store.next_deadline(ms(100)), Some(ms(100) + WHEEL_QUIET));
    shown(&mut store, &mut host);
    store.tick(ms(0) + WHEEL_QUIET, h(&mut host));
    assert!(store.take_events().is_empty(), "the burst is still active");
    store.tick(ms(100) + WHEEL_QUIET, h(&mut host));
    assert_eq!(kinds(&store.take_events()), ["View{3}", "R{true,3,1}"]);
}

// ----------------------------------------------------------------------
// Round 3: OPUS-REVIEW NIT 3-4.

/// A highlight request with both a malformed target and an out-of-range
/// `selected` is one applied fallback: one `Error{req}`.
#[test]
fn a_doubly_invalid_highlight_reports_one_error() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    store.update(
        5,
        9,
        ModelChange::Highlight {
            target: Some(ModelTarget::Point {
                mesh: 0,
                face: 0,
                barycentric: [f64::NAN, 0.0, 0.0],
                tolerance: 0.1,
            }),
            selected: Some(99),
        },
        h(&mut host),
    );
    let events = kinds(&store.take_events());
    assert_eq!(
        events.iter().filter(|e| *e == "Error{Some(9)}").count(),
        1,
        "{events:?}"
    );
    shown(&mut store, &mut host);
    assert_eq!(
        host.draws.last().unwrap().highlight,
        GpuHighlight::default()
    );
}

/// A model failing mid-orbit ends its interaction with its `View` (design
/// §6.2: interacting lasts until its `View`), before `failed`.
#[test]
fn failing_mid_orbit_settles_the_interaction_with_a_view() {
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    shown(&mut store, &mut host);
    store.take_events();
    assert!(store.press(W, centre_hit(), (0.0, 0.0)));
    assert!(store.motion(W, (10.0, 0.0)));
    assert_eq!(kinds(&store.take_events()), ["R{false,2,0}"]);
    host.refuse_render = true;
    store.render_dirty(&mut host);
    let events = kinds(&store.take_events());
    let view = events.iter().position(|e| e.starts_with("View"));
    let failed = events.iter().position(|e| e == "failed");
    assert!(
        view.is_some() && failed.is_some() && view < failed,
        "{events:?}"
    );
    assert_eq!(
        events.iter().filter(|e| e.starts_with("View")).count(),
        1,
        "{events:?}"
    );
    assert!(!store.motion(W, (20.0, 0.0)), "the capture ended");
}

/// SOL-REVIEW-3 MUST 1, at the store: what a window composited while it
/// was hidden (its presentations are not showings) still lands in its
/// composition ring, so a blend after exposure holds the fence down for
/// that model although the window never joined; destroying the window
/// forgets it.
#[test]
fn a_blend_after_exposure_counts_what_the_window_composited_while_hidden() {
    const B: WindowKey = 2;
    let mut host = FakeHost::default();
    let mut store = loaded_store(&mut host);
    store.window_hidden(W);
    store.render_dirty(&mut host);
    let picture = store.rendered_snapshots();
    store.presented(W, &picture);
    store.presented(B, &picture);
    assert_eq!(kinds(&store.take_events()), ["ready", "R{true,1,0}"]);
    store.window_shown(W);
    store.presented_blended(W, &BTreeMap::new());
    assert_eq!(
        kinds(&store.take_events()),
        ["R{false,1,0}"],
        "W's ring holds the model it composited while hidden"
    );
    store.presented(W, &BTreeMap::new());
    assert_eq!(kinds(&store.take_events()), ["R{true,1,0}"]);
    store.window_destroyed(W);
    store.presented_blended(W, &BTreeMap::new());
    assert!(store.take_events().is_empty(), "W's ring died with it");
}
