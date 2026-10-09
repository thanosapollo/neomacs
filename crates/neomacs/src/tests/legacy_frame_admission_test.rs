//! Real capacity-64 transport regressions; source-only until the runtime gate.
use super::*;
use neomacs_display_runtime::thread_comm::{CommandSender, ThreadComms};
use neovm_core::emacs_core::DisplayHost;
use std::sync::atomic::Ordering;

fn host_for_transport(cmd_tx: &CommandSender) -> PrimaryWindowDisplayHost {
    PrimaryWindowDisplayHost {
        frame_opacity: Default::default(),
        deferred_frame: None,
        resources: Default::default(),
        system_fonts: Default::default(),
        tooltip_client: Default::default(),
        cmd_tx: cmd_tx.clone(),
        render_waker: None,
        font_sizing: FontSizing::gnu_x11_fallback(),
        primary_window_adopted: false,
        primary_frame_id: None,
        legacy_frame_leases: Default::default(),
        last_window_titles: Mutex::new(Default::default()),
        font_metrics: None,
        primary_window_size: shared_primary_window_size(800, 600),
        image_catalog: Rc::new(AsyncImageCatalog::new(
            cmd_tx.clone(), None, Arc::new(ImageRenderState::default()), None,
        )),
        #[cfg(feature = "video")]
        resolved_videos: Mutex::new(Default::default()),
        resolved_webkits: Mutex::new(Default::default()),
        resolved_surfaces: Mutex::new(Default::default()),
        render_capabilities: Arc::new(SharedRenderCapabilities::default()),
        requested_frame_shader: Mutex::new(None),
        #[cfg(feature = "neo-term")]
        terminal_state: super::super::super::TerminalHostState::new(new_shared_terminals()),
    }
}

fn request(id: u64) -> GuiFrameHostRequest {
    GuiFrameHostRequest {
        frame_id: FrameId(id), width: 901, height: 603,
        title: LispString::from_utf8("transaction title"),
        geometry_hints: GuiFrameGeometryHints {
            base_width: 0, base_height: 0, min_width: 1, min_height: 1,
            width_inc: 1, height_inc: 1,
        },
        fullscreen: Some(FrameFullscreen::Maximized),
    }
}

fn fill(sender: &CommandSender, count: u32) {
    for id in 0..count {
        sender.try_send(RenderCommand::Asset(AssetCommand::SurfaceFree { id })).unwrap();
    }
}

#[test]
fn legacy_real_transport_final_slot_is_one_create_or_adoption_bundle() {
    for adopt in [true, false] {
        let (emacs, render) = ThreadComms::new().split();
        let mut host = host_for_transport(&emacs.cmd_tx);
        host.primary_window_adopted = !adopt;
        host.primary_frame_id = (!adopt).then_some(FrameId(0x41));
        fill(&emacs.cmd_tx, 63);
        host.realize_gui_frame(request(0x42)).unwrap();
        assert!(host.legacy_frame_leases.contains_key(&FrameId(0x42)));
        assert!(emacs.cmd_tx.try_send(RenderCommand::Config(ConfigCommand::SetShowFps { enabled: true })).is_err());
        // Failure after successful bundle admission (e.g. Lisp rollback) has
        // exact cancellation authority even with no free ordinary slot.
        host.destroy_gui_frame(FrameId(0x42)).unwrap();
        assert!(!host.legacy_frame_leases.contains_key(&FrameId(0x42)));
        assert_eq!(host.primary_frame_id, (!adopt).then_some(FrameId(0x41)));
        for id in 0..63 {
            assert!(matches!(render.cmd_rx.try_recv().unwrap(),
                RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
        }
        assert!(matches!(render.cmd_rx.try_recv().unwrap(),
            RenderCommand::Window(WindowCommand::RealizeFrame {
                frame: FrameRef::Frame(0x42), adopt_primary,
                fullscreen: Some(WindowFullscreenMode::Maximized), live, title, ..
            }) if adopt_primary == adopt && title == "transaction title" && !live.load(Ordering::Acquire)));
        assert!(render.cmd_rx.is_empty());
    }
}

#[test]
fn legacy_host_real_transport_admission_has_no_readiness_owner() {
    let (emacs, render) = ThreadComms::new().split();
    let mut host = host_for_transport(&emacs.cmd_tx);
    for id in [0x42, 0x43] {
        host.realize_gui_frame(request(id)).unwrap();
        assert!(matches!(render.cmd_rx.try_recv().unwrap(),
            RenderCommand::Window(WindowCommand::RealizeFrame {
                frame, reply: None, live, ..
            }) if frame.raw_id() == id && live.load(Ordering::Acquire)));
        assert_eq!(host.poll_gui_frame_ready(FrameId(id)), Some(Ok(())));
        assert!(host.legacy_frame_leases[&FrameId(id)].load(Ordering::Acquire));
    }
    assert!(render.cmd_rx.is_empty());
}

#[test]
fn legacy_real_transport_full_bundle_rejects_without_publishing_host_identity() {
    let (emacs, render) = ThreadComms::new().split();
    let mut host = host_for_transport(&emacs.cmd_tx);
    fill(&emacs.cmd_tx, 64);
    assert!(host.realize_gui_frame(request(0x42)).is_err());
    host.destroy_gui_frame(FrameId(0x42)).unwrap();
    assert!(!host.primary_window_adopted);
    assert_eq!(host.primary_frame_id, None);
    assert!(host.legacy_frame_leases.is_empty());
    assert!(host.last_window_titles.lock().unwrap().is_empty());
    for id in 0..64 {
        assert!(matches!(render.cmd_rx.try_recv().unwrap(),
            RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
    }
    assert!(render.cmd_rx.is_empty());
}

#[test]
fn legacy_lisp_delete_at_final_slot_retires_exact_lease_before_opacity_error() {
    let (emacs, render) = ThreadComms::new().split();
    let mut eval = create_bootstrap_evaluator_cached_with_features(BOOTSTRAP_CORE_FEATURES)
        .expect("cached bootstrap evaluator");
    bootstrap_buffers(&mut eval, 800, 600, gui_display());
    let id = eval.frame_manager().selected_frame().unwrap().id;
    let mut host = host_for_transport(&emacs.cmd_tx);
    host.realize_gui_frame(request(id.0)).unwrap();
    let live = host.legacy_frame_leases[&id].clone();
    assert!(matches!(render.cmd_rx.try_recv().unwrap(),
        RenderCommand::Window(WindowCommand::RealizeFrame { frame, .. }) if frame.raw_id() == id.0));
    eval.set_display_host(Box::new(host));
    let survivor = eval.eval_str("(x-create-frame nil)").expect("second live GUI frame");
    let survivor_id = FrameId(survivor.as_frame_id().unwrap());
    let mut survivor_live = None;
    while let Ok(command) = render.cmd_rx.try_recv() {
        if let RenderCommand::Window(WindowCommand::RealizeFrame { frame, live, .. }) = command {
            assert_eq!(frame.raw_id(), survivor_id.0);
            survivor_live = Some(live);
        }
    }
    let survivor_live = survivor_live.expect("second frame transaction");
    fill(&emacs.cmd_tx, 63);
    eval.set_variable("retired-frame", Value::make_frame(id.0));
    // Focus synchronization fills slot64. Opacity retirement then rejects,
    // but the Lisp deletion has already cancelled the exact native lease.
    assert!(eval.eval_str("(delete-frame retired-frame t)").is_err());
    assert!(eval.frame_manager().get(id).is_none());
    assert!(!live.load(Ordering::Acquire));
    assert!(eval.frame_manager().get(survivor_id).is_some());
    assert!(survivor_live.load(Ordering::Acquire));
    for id in 0..63 {
        assert!(matches!(render.cmd_rx.try_recv().unwrap(),
            RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
    }
    assert!(matches!(render.cmd_rx.try_recv().unwrap(),
        RenderCommand::Window(WindowCommand::RefreshFrameOpacity)));
    assert!(render.cmd_rx.is_empty());
}

#[test]
fn legacy_lisp_failed_realization_rolls_back_admitted_final_slot_identity() {
    use std::cell::RefCell;
    struct FailAfterAdmission {
        host: PrimaryWindowDisplayHost,
        admitted: Rc<RefCell<Option<(FrameId, Arc<AtomicBool>)>>>,
    }
    impl DisplayHost for FailAfterAdmission {
        fn realize_gui_frame(&mut self, request: GuiFrameHostRequest) -> Result<(), String> {
            let id = request.frame_id;
            self.host.realize_gui_frame(request)?;
            *self.admitted.borrow_mut() = Some((id, self.host.legacy_frame_leases[&id].clone()));
            Err("failure after atomic admission".into())
        }
        fn resize_gui_frame(&mut self, _: GuiFrameHostRequest) -> Result<(), String> {
            Ok(())
        }
        fn destroy_gui_frame(&mut self, id: FrameId) -> Result<(), String> {
            self.host.destroy_gui_frame(id)
        }
    }
    for adopt in [true, false] {
        let (emacs, render) = ThreadComms::new().split();
        let mut eval = create_bootstrap_evaluator_cached_with_features(BOOTSTRAP_CORE_FEATURES)
            .expect("cached bootstrap evaluator");
        bootstrap_buffers(&mut eval, 800, 600, gui_display());
        let before = eval.frame_manager().frame_list();
        let mut host = host_for_transport(&emacs.cmd_tx);
        host.primary_window_adopted = !adopt;
        host.primary_frame_id = (!adopt).then_some(before[0]);
        let admitted = Rc::new(RefCell::new(None));
        eval.set_display_host(Box::new(FailAfterAdmission { host, admitted: admitted.clone() }));
        fill(&emacs.cmd_tx, 63);
        assert!(eval.eval_str("(x-create-frame '((fullscreen . maximized)))").is_err());
        let (id, live) = admitted.borrow().clone().expect("real host admitted bundle before error");
        assert_eq!(eval.frame_manager().frame_list(), before);
        assert!(eval.frame_manager().get(id).is_none());
        assert!(!live.load(Ordering::Acquire));
        for id in 0..63 {
            assert!(matches!(render.cmd_rx.try_recv().unwrap(),
                RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
        }
        assert!(matches!(render.cmd_rx.try_recv().unwrap(),
            RenderCommand::Window(WindowCommand::RealizeFrame {
                frame, live, adopt_primary, fullscreen: Some(WindowFullscreenMode::Maximized), ..
            }) if frame.raw_id() == id.0 && adopt_primary == adopt && !live.load(Ordering::Acquire)));
        assert!(render.cmd_rx.is_empty());
    }
}

#[test]
fn legacy_real_transport_redisplay_retries_silently_rejected_title_once() {
    let (emacs, render) = ThreadComms::new().split();
    let mut eval = create_bootstrap_evaluator_cached_with_features(BOOTSTRAP_CORE_FEATURES)
        .expect("cached bootstrap evaluator");
    bootstrap_buffers(&mut eval, 800, 600, gui_display());
    let id = eval.frame_manager().selected_frame().unwrap().id;
    let mut host = host_for_transport(&emacs.cmd_tx);
    host.realize_gui_frame(request(id.0)).unwrap();
    render.cmd_rx.try_recv().unwrap();
    eval.set_display_host(Box::new(host));
    eval.eval_str(r#"(setq frame-title-format "redisplay retry")"#).unwrap();
    fill(&emacs.cmd_tx, 64);
    sync_live_gui_frame_titles(&mut eval);
    render.cmd_rx.try_recv().unwrap();
    sync_live_gui_frame_titles(&mut eval);
    sync_live_gui_frame_titles(&mut eval);
    for id in 1..64 {
        assert!(matches!(render.cmd_rx.try_recv().unwrap(),
            RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
    }
    assert!(matches!(render.cmd_rx.try_recv().unwrap(),
        RenderCommand::Window(WindowCommand::SetFrameWindowTitle {
            frame: FrameRef::Primary, title,
        }) if title == "redisplay retry"));
    assert!(render.cmd_rx.is_empty());
}

#[test]
fn legacy_real_transport_rejected_title_retries_identical_once() {
    let (emacs, render) = ThreadComms::new().split();
    let mut host = host_for_transport(&emacs.cmd_tx);
    let id = FrameId(0x42);
    let title = LispString::from_utf8("retry identical title");
    fill(&emacs.cmd_tx, 64);
    assert!(host.set_gui_frame_title(id, title.clone()).is_err());
    assert!(!host.last_window_titles.lock().unwrap().contains_key(&id));
    render.cmd_rx.try_recv().unwrap();
    host.set_gui_frame_title(id, title.clone()).unwrap();
    host.set_gui_frame_title(id, title).unwrap();
    for id in 1..64 {
        assert!(matches!(render.cmd_rx.try_recv().unwrap(),
            RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
    }
    assert!(matches!(render.cmd_rx.try_recv().unwrap(),
        RenderCommand::Window(WindowCommand::SetFrameWindowTitle {
            frame: FrameRef::Frame(0x42), title,
        }) if title == "retry identical title"));
    assert!(render.cmd_rx.is_empty());
}
