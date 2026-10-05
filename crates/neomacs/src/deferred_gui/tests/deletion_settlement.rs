//! Production deferred host construction and public core deletion; CPU only.
//! The renderer is deliberately not scheduled, not replaced by a draining mock.
use super::*;
use neomacs_display_protocol::GraphicalBackend;
use neomacs_display_runtime::render_thread::ImageRenderState;
use neovm_core::emacs_core::error::EvalError;
use neovm_core::window::{FrameDisplayIdentity, FrameVisibility};

// Observation adapter only: all lifecycle operations reach the exact production
// host. Keeping an Rc here lets assertions run before the evaluator/host drops.
struct ObservedHost(Rc<RefCell<PrimaryWindowDisplayHost>>);

impl DisplayHost for ObservedHost {
    fn gui_terminal(&self) -> Option<(u64, GraphicalDisplayIdentity)> {
        self.0.borrow().gui_terminal()
    }

    fn realize_gui_frame(&mut self, request: GuiFrameHostRequest) -> Result<(), String> {
        self.0.borrow_mut().realize_gui_frame(request)
    }

    fn resize_gui_frame(&mut self, request: GuiFrameHostRequest) -> Result<(), String> {
        self.0.borrow_mut().resize_gui_frame(request)
    }

    fn set_visual_config(&mut self, config: VisualConfig) -> Result<(), String> {
        self.0.borrow_mut().set_visual_config(config)
    }

    fn set_gui_frame_focus_redirects(
        &mut self,
        redirects: Vec<(FrameId, Option<FrameId>)>,
    ) -> Result<(), String> {
        self.0.borrow_mut().set_gui_frame_focus_redirects(redirects)
    }

    fn retire_gui_frame_alpha(&mut self, frame: FrameId) -> Result<(), String> {
        self.0.borrow_mut().retire_gui_frame_alpha(frame)
    }

    fn destroy_gui_frame(&mut self, frame: FrameId) -> Result<(), String> {
        self.0.borrow_mut().destroy_gui_frame(frame)
    }

    fn remove_gui_child_frame(&mut self, frame: FrameId) -> Result<(), String> {
        self.0.borrow_mut().remove_gui_child_frame(frame)
    }
}

fn production_host(comms: &EmacsComms, defaults: FrameDefaults) -> PrimaryWindowDisplayHost {
    display_host(
        comms,
        defaults,
        Default::default(),
        &bootstrap_tty_display_config(Interactivity::Batch),
        Arc::new(Mutex::new(PrimaryWindowSize {
            width: 800,
            height: 600,
        })),
        Rc::new(AsyncImageCatalog::new(
            comms.cmd_tx.clone(),
            None,
            Arc::new(ImageRenderState::default()),
            None,
        )),
        None,
        #[cfg(feature = "neo-term")]
        new_shared_terminals(),
    )
}

fn defaults(terminal_id: u64, identity: GraphicalDisplayIdentity) -> FrameDefaults {
    FrameDefaults {
        terminal_id,
        identity,
        metrics: BootstrapFrameMetrics {
            char_width: 10.0,
            char_height: 20.0,
            font_pixel_size: 16.0,
        },
        font: "monospace".into(),
        frame_id: Arc::new(AtomicU64::new(0)),
        initial: Rc::new(RefCell::new(None)),
        ready: HashMap::new(),
        leases: HashMap::new(),
        visual: RefCell::new(None),
    }
}

#[test]
fn deferred_production_host_keeps_renderer_opacity_arc_identity() {
    let (comms, render) = ThreadComms::new().split();
    let identity = GraphicalDisplayIdentity::named(GraphicalBackend::Wayland, "cpu-host").unwrap();
    let mut host = production_host(&comms, defaults(1, identity));
    assert!(Arc::ptr_eq(&host.frame_opacity, &comms.frame_opacity));
    assert!(Arc::ptr_eq(&host.frame_opacity, &render.frame_opacity));

    let frame = FrameId(42);
    host.set_gui_frame_alpha(frame, [0.75, 0.5], 0.0).unwrap();
    assert_eq!(
        render.frame_opacity.lock().unwrap().applied(frame.0),
        Some(0.5)
    );
    render.frame_opacity.lock().unwrap().focus(frame.0, true);
    assert_eq!(
        host.frame_opacity.lock().unwrap().applied(frame.0),
        Some(0.75)
    );
    host.retire_gui_frame_alpha(frame).unwrap();
    assert_eq!(comms.frame_opacity.lock().unwrap().applied(frame.0), None);
}

#[test]
fn deferred_core_delete_revokes_exact_lease_after_63_pending_opacity_failure() {
    deletion_with_pending_commands(63);
}

#[test]
fn deferred_core_delete_settles_normally_with_62_pending_commands() {
    deletion_with_pending_commands(62);
}

#[test]
fn deferred_core_delete_does_not_commit_when_64_pending_blocks_predelete_refresh() {
    deletion_with_pending_commands(64);
}

fn deletion_with_pending_commands(pending: usize) {
    let mut eval = Context::new();
    let buffer = eval.buffer_manager_mut().create_buffer("*cpu-delete*");
    eval.buffer_manager_mut().set_current(buffer);
    let identity =
        GraphicalDisplayIdentity::named(GraphicalBackend::Wayland, "cpu-delete").unwrap();
    let terminal =
        neovm_core::emacs_core::terminal::pure::register_graphical_terminal(identity.clone());
    let survivor = eval
        .frame_manager_mut()
        .create_frame_on_terminal("survivor", terminal, 800, 600, buffer);
    let victim = eval
        .frame_manager_mut()
        .create_frame_on_terminal("victim", terminal, 800, 600, buffer);
    for id in [survivor, victim] {
        let frame = eval.frame_manager_mut().get_mut(id).unwrap();
        frame.initial = false;
        frame.visibility = FrameVisibility::Visible;
        frame.set_window_system(Some(Value::symbol("neo")));
        frame.set_display_identity(FrameDisplayIdentity::Graphical(identity.clone()));
    }
    assert!(eval.frame_manager_mut().select_frame(survivor));
    eval.set_variable("test-deleting-frame", Value::make_frame(victim.0));
    eval.set_variable("delete-frame-functions", Value::NIL);
    eval.set_variable("after-delete-frame-functions", Value::NIL);

    let (comms, render) = ThreadComms::new().split();
    let victim_live = Arc::new(AtomicBool::new(true));
    let survivor_live = Arc::new(AtomicBool::new(true));
    let (victim_ready_tx, victim_ready_rx) = crossbeam_channel::bounded(1);
    let (_survivor_ready_tx, survivor_ready_rx) = crossbeam_channel::bounded(1);
    let mut defaults = defaults(terminal, identity);
    defaults.leases.insert(victim, Arc::clone(&victim_live));
    defaults.leases.insert(survivor, Arc::clone(&survivor_live));
    defaults.ready.insert(victim, victim_ready_rx);
    defaults.ready.insert(survivor, survivor_ready_rx);
    let mut host = production_host(&comms, defaults);
    host.primary_frame_id = Some(victim);
    assert!(Arc::ptr_eq(&host.frame_opacity, &render.frame_opacity));
    assert!(Arc::ptr_eq(
        &host.deferred_frame.as_ref().unwrap().leases[&victim],
        &victim_live,
    ));
    // Seed a renderer-visible policy without consuming the saturation budget.
    render
        .frame_opacity
        .lock()
        .unwrap()
        .accept(victim.0, [0.7, 0.4], 0.0);
    let observed = Rc::new(RefCell::new(host));
    eval.set_display_host(Box::new(ObservedHost(Rc::clone(&observed))));
    // Installation config is not part of the pending deletion schedule.
    for _ in render.cmd_rx.try_iter() {}
    for _ in 0..pending {
        comms
            .cmd_tx
            .try_send(RenderCommand::Window(WindowCommand::RefreshFrameOpacity))
            .unwrap();
    }
    assert_eq!(render.cmd_rx.len(), pending);

    // No command is dequeued until all settlement assertions have completed.
    let result = eval.eval_str("(delete-frame test-deleting-frame t)");
    let host = observed.borrow();
    let defaults = host.deferred_frame.as_ref().unwrap();
    assert!(eval.frame_manager().get(survivor).is_some());
    assert_eq!(eval.frame_manager().selected_frame().unwrap().id, survivor);
    assert!(survivor_live.load(Ordering::Acquire));
    assert!(defaults.leases.contains_key(&survivor));
    assert!(defaults.ready.contains_key(&survivor));
    assert!(Arc::ptr_eq(&host.frame_opacity, &comms.frame_opacity));

    if pending == 64 {
        assert_error_contains(result, "failed to refresh frame highlight");
        assert!(eval.frame_manager().get(victim).is_some());
        assert!(victim_live.load(Ordering::Acquire));
        assert!(defaults.leases.contains_key(&victim));
        assert!(defaults.ready.contains_key(&victim));
        assert_eq!(host.primary_frame_id, Some(victim));
        assert_eq!(
            render.frame_opacity.lock().unwrap().applied(victim.0),
            Some(0.4)
        );
    } else {
        if pending == 63 {
            assert_error_contains(result, "failed to retire frame opacity");
        } else {
            assert!(result.unwrap().is_nil());
        }
        assert!(eval.frame_manager().get(victim).is_none());
        assert!(
            !victim_live.load(Ordering::Acquire),
            "exact native lease remained live"
        );
        assert!(!defaults.leases.contains_key(&victim));
        assert!(!defaults.ready.contains_key(&victim));
        assert!(matches!(
            victim_ready_tx.try_send(Ok(())),
            Err(crossbeam_channel::TrySendError::Disconnected(_)),
        ));
        assert_eq!(host.primary_frame_id, None);
        assert_eq!(render.frame_opacity.lock().unwrap().applied(victim.0), None);
    }
    // 62 succeeds with two refreshes; 63 fails the second; 64 fails the first.
    // All three reach the real production capacity without consuming it.
    assert_eq!(render.cmd_rx.len(), 64);
    assert!(matches!(
        comms
            .cmd_tx
            .try_send(RenderCommand::Window(WindowCommand::RefreshFrameOpacity)),
        Err(crossbeam_channel::TrySendError::Full(_)),
    ));
}

fn assert_error_contains(result: Result<Value, EvalError>, expected: &str) {
    let EvalError::Signal { symbol, data, .. } =
        result.expect_err("bounded refresh must report failure")
    else {
        panic!("expected an ordinary signal");
    };
    assert_eq!(symbol, neovm_core::emacs_core::intern::intern("error"));
    let message = data
        .first()
        .and_then(|value| value.as_str_owned())
        .expect("error message");
    assert!(message.contains(expected), "{message}");
    assert!(
        message.contains("full"),
        "queue pressure must remain truthful: {message}"
    );
}
