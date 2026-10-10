//! Opt-in successful readiness and native replay on a private real compositor.
//! No fake windows/devices. Adapter results are gated, then forwarded intact.
use super::{RenderApp, frame_windows::FrameLifecycle, gpu_startup::PendingGpu, tests::make_test_app};
use crate::thread_comm::{EmacsComms, FrameRef, RenderCommand, ThreadComms, WindowCommand, WindowFullscreenMode};
use neovm_core::window::GuiFrameGeometryHints;
use std::{collections::VecDeque, os::unix::net::UnixStream, sync::{Arc, Weak, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use winit::{application::ApplicationHandler, event::WindowEvent, event_loop::{ActiveEventLoop, ControlFlow, EventLoop}, platform::wayland::{EventLoopBuilderExtWayland, WaylandConnection}, window::{Window, WindowId}};

type Adapters = Result<(wgpu::Instance, Vec<wgpu::Adapter>), String>;

fn hints() -> GuiFrameGeometryHints {
    GuiFrameGeometryHints { base_width: 24, base_height: 16, min_width: 24,
        min_height: 16, width_inc: 8, height_inc: 16 }
}

fn transaction(id: u64, primary: bool, mode: WindowFullscreenMode, live: Arc<AtomicBool>,
    reply: Option<crossbeam_channel::Sender<Result<(), String>>>) -> RenderCommand {
    RenderCommand::Window(WindowCommand::RealizeFrame {
        frame: FrameRef::Frame(id), width: 901, height: 603,
        title: format!("native replay {id}"), geometry_hints: hints(),
        fullscreen: Some(mode), visual: None, adopt_primary: primary, reply, live,
        deadline: Instant::now() + Duration::from_secs(30),
    })
}

fn native_mode_matches(window: &dyn Window, mode: WindowFullscreenMode) -> bool {
    match mode {
        WindowFullscreenMode::Fullscreen | WindowFullscreenMode::Fullboth => window.fullscreen().is_some(),
        WindowFullscreenMode::Maximized => window.fullscreen().is_none() && window.is_maximized(),
        WindowFullscreenMode::None => window.fullscreen().is_none() && !window.is_maximized(),
        _ => panic!("fixture only covers supported native states"),
    }
}

fn assert_native_cosmetics(window: &dyn Window, id: u64) {
    // Read native winit state, not the transaction or retained chrome fields.
    assert_eq!(window.title(), format!("native replay {id}"));
    let increments = super::state::window_size_from_emacs_pixels(8, 16)
        .to_physical::<u32>(window.scale_factor());
    assert_eq!(window.surface_resize_increments(), Some(increments));
}

// Submit a genuine buffer so the compositor maps otherwise frame-less fixture
// windows and acknowledges maximize/fullscreen. Never synthesize configure events.
fn present_blank(app: &RenderApp, id: WindowId) {
    let Some(gpu) = &app.gpu else { return; };
    let Some(native) = app.frame_windows.windows.values().find_map(|state| {
        state.lifecycle.native().filter(|native| native.window.id() == id)
    }) else { return; };
    if let wgpu::CurrentSurfaceTexture::Success(output)
        | wgpu::CurrentSurfaceTexture::Suboptimal(output) = native.surface.as_ref().expect("ready surface").get_current_texture()
    {
        let view = output.texture.create_view(&Default::default());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("legacy readiness fixture"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
        }
        gpu.queue.submit(Some(encoder.finish()));
        native.window.pre_present_notify();
        gpu.queue.present(output);
    }
}

struct Case {
    app: RenderApp,
    emacs: EmacsComms,
    mode: WindowFullscreenMode,
    adapter_reply: crossbeam_channel::Receiver<Adapters>,
    gate: Option<crossbeam_channel::Sender<Adapters>>,
    primary: Arc<AtomicBool>,
    secondary: Arc<AtomicBool>,
    abandoned: Arc<AtomicBool>,
    primary_id: WindowId,
    primary_weak: Weak<dyn Window>,
    identities: Option<(WindowId, WindowId)>,
    successful_passes: usize,
    deadline: Instant,
}

struct Probe {
    cases: VecDeque<(bool, WindowFullscreenMode)>,
    current: Option<Case>,
    done: Arc<AtomicBool>,
}

impl Probe {
    fn start_case(&mut self, owner: &dyn ActiveEventLoop) {
        let Some((before_native, mode)) = self.cases.pop_front() else {
            self.done.store(true, Ordering::Release);
            owner.exit();
            return;
        };
        let (emacs, render) = ThreadComms::new().split();
        let mut app = make_test_app();
        app.comms = render;
        let primary = Arc::new(AtomicBool::new(true));
        if before_native {
            emacs.cmd_tx.try_send(transaction(0x42, true, mode, primary.clone(), None)).unwrap();
            assert!(!app.process_startup_commands());
            assert_eq!(app.frame_windows.primary_window().unwrap().native_size(), (800, 600));
            assert!(matches!(app.frame_windows.primary_window().unwrap().lifecycle, FrameLifecycle::Pending { .. }));
        }
        app.handle_resumed(owner);
        let PendingGpu::Adapters { window, reply: adapter_reply } = app.gpu_startup.take().unwrap() else {
            panic!("genuine native creation must start adapter discovery");
        };
        let primary_id = window.id();
        let primary_weak = Arc::downgrade(&window);
        let (gate, receive) = crossbeam_channel::bounded(1);
        app.gpu_startup = Some(PendingGpu::Adapters { window, reply: receive });
        let host_size = app.frame_windows.primary_window().unwrap().native_size();
        if !before_native {
            assert_ne!(app.gpu_startup.as_ref().unwrap().window().title(), format!("native replay {}", 0x42));
            emacs.cmd_tx.try_send(transaction(0x42, true, mode, primary.clone(), None)).unwrap();
            assert!(!app.process_startup_commands());
            assert_eq!(app.frame_windows.primary_window().unwrap().native_size(), host_size);
        }
        assert!(app.gpu.is_none());
        assert_eq!(app.frame_windows.primary_frame_id(), Some(0x42));
        assert_eq!(app.gpu_startup.as_ref().unwrap().window_id(), primary_id);
        assert_native_cosmetics(app.gpu_startup.as_ref().unwrap().window(), 0x42);
        let secondary = Arc::new(AtomicBool::new(true));
        emacs.cmd_tx.try_send(transaction(0x43, false, mode, secondary.clone(), None)).unwrap();
        // Unlike legacy admission, this reply owns successful deferred readiness.
        // Drop it before actual native creation; settlement must retire only 0x44.
        let abandoned = Arc::new(AtomicBool::new(true));
        let (reply, ready) = crossbeam_channel::bounded(1);
        emacs.cmd_tx.try_send(transaction(0x44, false, mode, abandoned.clone(), Some(reply))).unwrap();
        drop(ready);
        assert!(!app.process_startup_commands());
        assert_eq!(app.frame_windows.pending_creates.len(), 2);
        assert_eq!(app.frame_windows.pending_creates[0].fullscreen, Some(mode));
        assert!(app.frame_windows.get(0x43).is_none());
        assert_eq!(app.gpu_startup.as_ref().unwrap().window_id(), primary_id);
        // Prevent the last-frame process policy from ending this multi-case
        // fixture; the transport transactions still use legacy reply=None.
        app.comms.keep_alive_without_frames = true;
        self.current = Some(Case { app, emacs, mode, adapter_reply, gate: Some(gate),
            primary, secondary, abandoned, primary_id, primary_weak, identities: None,
            successful_passes: 0, deadline: Instant::now() + Duration::from_secs(30) });
    }
}

impl ApplicationHandler for Probe {
    fn can_create_surfaces(&mut self, owner: &dyn ActiveEventLoop) {
        if self.current.is_none() { self.start_case(owner); }
    }

    fn window_event(&mut self, owner: &dyn ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(case) = &mut self.current {
            let redraw = matches!(event, WindowEvent::RedrawRequested);
            case.app.window_event(owner, id, event);
            if redraw { present_blank(&case.app, id); }
        }
    }

    fn about_to_wait(&mut self, owner: &dyn ActiveEventLoop) {
        let Some(case) = &mut self.current else { return; };
        assert!(Instant::now() < case.deadline, "native readiness/configure deadline exceeded");
        if case.gate.is_some() {
            match case.adapter_reply.try_recv() {
                Ok(result) => case.gate.take().unwrap().send(result).unwrap(),
                Err(crossbeam_channel::TryRecvError::Empty) => {},
                Err(error) => panic!("genuine adapter discovery disconnected: {error}"),
            }
        }
        case.app.handle_about_to_wait(owner);
        assert!(!case.app.lifecycle_flags.is_shutting_down());
        assert!(case.app.startup_error.is_none());
        let ready = [0x42, 0x43].into_iter().all(|id| case.app.frame_windows.get(id)
            .is_some_and(|state| matches!(state.lifecycle, FrameLifecycle::Active { .. })));
        if ready {
            assert!(case.app.frame_windows.pending_creates.is_empty());
            assert!(case.app.frame_windows.get(0x44).is_none(), "abandoned deferred readiness must retire its native frame");
            let primary = case.app.frame_windows.get(0x42).unwrap().window().unwrap().as_ref();
            let secondary = case.app.frame_windows.get(0x43).unwrap().window().unwrap().as_ref();
            assert_eq!(primary.id(), case.primary_id);
            assert_native_cosmetics(primary, 0x42);
            assert_native_cosmetics(secondary, 0x43);
            assert!(case.primary.load(Ordering::Acquire));
            assert!(case.secondary.load(Ordering::Acquire));
            assert!(Arc::ptr_eq(&case.app.frame_leases[&0x42], &case.primary));
            assert!(Arc::ptr_eq(&case.app.frame_leases[&0x43], &case.secondary));
            let identities = (primary.id(), secondary.id());
            if let Some(expected) = case.identities { assert_eq!(identities, expected); }
            case.identities = Some(identities);
            primary.request_redraw();
            secondary.request_redraw();
            if native_mode_matches(primary, case.mode) && native_mode_matches(secondary, case.mode) {
                assert!(native_mode_matches(primary, case.mode));
                assert!(native_mode_matches(secondary, case.mode));
                case.successful_passes += 1;
            }
            if case.successful_passes == 8 {
                // Repeated successful readiness settlement must not retire
                // admission-only identities. Exact lease cancellation must.
                case.secondary.store(false, Ordering::Release);
                case.app.handle_about_to_wait(owner);
                assert!(case.app.frame_windows.get(0x43).is_none());
                assert!(!case.app.frame_leases.contains_key(&0x43));
                assert_eq!(case.app.frame_windows.get(0x42).unwrap().window().unwrap().id(), case.primary_id);
                assert!(case.primary.load(Ordering::Acquire));
                case.primary.store(false, Ordering::Release);
                case.abandoned.store(false, Ordering::Release);
                assert!(!case.app.process_commands());
                case.app.frame_windows.process_destroys();
                assert!(case.app.frame_windows.get(0x42).is_none());
                assert_eq!(case.app.frame_windows.primary_frame_id(), None);
                assert!(case.app.frame_leases.is_empty());
                assert!(case.primary_weak.upgrade().is_none());
                assert!(case.emacs.cmd_tx.try_send(RenderCommand::Window(WindowCommand::RefreshFrameOpacity)).is_ok());
                assert!(!case.app.process_commands());
                assert!(case.app.frame_windows.get(0x42).is_none());
                case.app.handle_exiting();
                self.current = None;
                self.start_case(owner);
            }
        }
        owner.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(10)));
    }
}

fn updated_hints() -> GuiFrameGeometryHints {
    GuiFrameGeometryHints { base_width: 36, base_height: 28, min_width: 72,
        min_height: 56, width_inc: 12, height_inc: 14 }
}

fn submit_pending_updates(emacs: &EmacsComms, target: Arc<AtomicBool>, other: Arc<AtomicBool>) {
    // Both identities and every update travel through the production transport.
    emacs.cmd_tx.try_send(transaction(0x43, false, WindowFullscreenMode::Maximized, target, None)).unwrap();
    emacs.cmd_tx.try_send(transaction(0x44, false, WindowFullscreenMode::None, other, None)).unwrap();
    for title in ["intermediate secondary", "latest secondary"] {
        emacs.cmd_tx.try_send(RenderCommand::Window(WindowCommand::SetFrameWindowTitle {
            frame: FrameRef::Frame(0x43), title: title.into(),
        })).unwrap();
    }
    emacs.cmd_tx.try_send(RenderCommand::Window(WindowCommand::SetFrameGeometryHints {
        frame: FrameRef::Frame(0x43), geometry_hints: updated_hints(),
    })).unwrap();
    emacs.cmd_tx.try_send(RenderCommand::Window(WindowCommand::SetWindowFullscreen {
        frame: FrameRef::Frame(0x43), mode: WindowFullscreenMode::Fullboth,
    })).unwrap();
}

struct PendingStateCase {
    app: RenderApp,
    emacs: EmacsComms,
    adapter_reply: crossbeam_channel::Receiver<Adapters>,
    gate: Option<crossbeam_channel::Sender<Adapters>>,
    leases: [Arc<AtomicBool>; 3],
    primary_id: WindowId,
    submitted: bool,
    identities: Option<[WindowId; 3]>,
    successful_passes: usize,
    deadline: Instant,
}

struct PendingStateProbe {
    pending_gpu_cases: VecDeque<bool>,
    current: Option<PendingStateCase>,
    done: Arc<AtomicBool>,
}

impl PendingStateProbe {
    fn start_case(&mut self, owner: &dyn ActiveEventLoop) {
        let Some(pending_gpu) = self.pending_gpu_cases.pop_front() else {
            self.done.store(true, Ordering::Release);
            owner.exit();
            return;
        };
        let (emacs, render) = ThreadComms::new().split();
        let mut app = make_test_app();
        app.comms = render;
        let leases: [Arc<AtomicBool>; 3] = std::array::from_fn(|_| Arc::new(AtomicBool::new(true)));
        emacs.cmd_tx.try_send(transaction(0x42, true, WindowFullscreenMode::None, leases[0].clone(), None)).unwrap();
        assert!(!app.process_startup_commands());
        assert_eq!(app.frame_windows.primary_window().unwrap().native_size(), (800, 600));
        app.handle_resumed(owner);
        let PendingGpu::Adapters { window, reply: adapter_reply } = app.gpu_startup.take().unwrap() else {
            panic!("genuine native creation must start adapter discovery");
        };
        let primary_id = window.id();
        let (gate, receive) = crossbeam_channel::bounded(1);
        app.gpu_startup = Some(PendingGpu::Adapters { window, reply: receive });
        app.comms.keep_alive_without_frames = true;
        if pending_gpu {
            let host_size = app.frame_windows.primary_window().unwrap().native_size();
            submit_pending_updates(&emacs, leases[1].clone(), leases[2].clone());
            assert!(!app.process_startup_commands());
            assert!(app.gpu.is_none());
            assert!(app.comms.cmd_rx.is_empty());
            assert!(app.startup_commands.is_empty());
            assert_eq!(app.frame_windows.pending_creates.len(), 2);
            let target = &app.frame_windows.pending_creates[0];
            assert_eq!(target.emacs_frame_id, 0x43);
            assert_eq!(target.title, "latest secondary");
            assert_eq!(target.geometry_hints, updated_hints());
            assert_eq!(target.fullscreen, Some(WindowFullscreenMode::Fullboth));
            let other = &app.frame_windows.pending_creates[1];
            assert_eq!(other.emacs_frame_id, 0x44);
            assert_eq!(other.title, format!("native replay {}", 0x44));
            assert_eq!(other.geometry_hints, hints());
            assert_eq!(other.fullscreen, Some(WindowFullscreenMode::None));
            assert!(app.frame_windows.get(0x43).is_none());
            assert!(app.frame_windows.get(0x44).is_none());
            assert_eq!(app.frame_windows.primary_window().unwrap().native_size(), host_size);
            let primary = app.gpu_startup.as_ref().unwrap();
            assert_eq!(primary.window_id(), primary_id);
            assert_native_cosmetics(primary.window(), 0x42);
            assert!(native_mode_matches(primary.window(), WindowFullscreenMode::None));
        }
        self.current = Some(PendingStateCase { app, emacs, adapter_reply, gate: Some(gate),
            leases, primary_id, submitted: pending_gpu, identities: None,
            successful_passes: 0, deadline: Instant::now() + Duration::from_secs(30) });
    }
}

impl ApplicationHandler for PendingStateProbe {
    fn can_create_surfaces(&mut self, owner: &dyn ActiveEventLoop) {
        if self.current.is_none() { self.start_case(owner); }
    }

    fn window_event(&mut self, owner: &dyn ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(case) = &mut self.current {
            let redraw = matches!(event, WindowEvent::RedrawRequested);
            case.app.window_event(owner, id, event);
            if redraw { present_blank(&case.app, id); }
        }
    }

    fn about_to_wait(&mut self, owner: &dyn ActiveEventLoop) {
        let Some(case) = &mut self.current else { return; };
        assert!(Instant::now() < case.deadline, "pending-state native configure deadline exceeded");
        if case.gate.is_some() {
            match case.adapter_reply.try_recv() {
                Ok(result) => case.gate.take().unwrap().send(result).unwrap(),
                Err(crossbeam_channel::TryRecvError::Empty) => {},
                Err(error) => panic!("genuine adapter discovery disconnected: {error}"),
            }
        }
        let submitted_now = !case.submitted && case.app.gpu.is_some();
        if submitted_now {
            // No secondary exists yet. This single production owner pass must
            // drain realize + updates before constructing either native window.
            assert!(case.app.frame_windows.pending_creates.is_empty());
            assert!(case.app.frame_windows.get(0x43).is_none());
            assert!(case.app.frame_windows.get(0x44).is_none());
            submit_pending_updates(&case.emacs, case.leases[1].clone(), case.leases[2].clone());
            case.submitted = true;
        }
        case.app.handle_about_to_wait(owner);
        assert!(!case.app.lifecycle_flags.is_shutting_down());
        assert!(case.app.startup_error.is_none());
        if submitted_now {
            assert!(case.app.comms.cmd_rx.is_empty());
            assert!(case.app.frame_windows.pending_creates.is_empty());
            for id in [0x43, 0x44] {
                assert!(matches!(case.app.frame_windows.get(id).unwrap().lifecycle,
                    FrameLifecycle::Active { .. }), "same owner pass must construct the secondary");
            }
        }
        if case.submitted && [0x42, 0x43, 0x44].into_iter().all(|id| case.app.frame_windows.get(id)
            .is_some_and(|state| matches!(state.lifecycle, FrameLifecycle::Active { .. }))) {
            assert!(case.app.frame_windows.pending_creates.is_empty());
            assert!(case.app.comms.cmd_rx.is_empty());
            assert!(case.app.startup_commands.is_empty());
            let primary = case.app.frame_windows.get(0x42).unwrap().window().unwrap().as_ref();
            let target = case.app.frame_windows.get(0x43).unwrap().window().unwrap().as_ref();
            let other = case.app.frame_windows.get(0x44).unwrap().window().unwrap().as_ref();
            assert_eq!(primary.id(), case.primary_id);
            assert_native_cosmetics(primary, 0x42);
            assert_native_cosmetics(other, 0x44);
            assert_eq!(target.title(), "latest secondary");
            let increments = super::state::window_size_from_emacs_pixels(12, 14)
                .to_physical::<u32>(target.scale_factor());
            assert_eq!(target.surface_resize_increments(), Some(increments));
            let identities = [primary.id(), target.id(), other.id()];
            assert_ne!(identities[0], identities[1]);
            assert_ne!(identities[0], identities[2]);
            assert_ne!(identities[1], identities[2]);
            if let Some(expected) = case.identities { assert_eq!(identities, expected); }
            case.identities = Some(identities);
            assert_eq!(case.app.frame_leases.len(), 3);
            for (id, live) in [0x42, 0x43, 0x44].into_iter().zip(&case.leases) {
                assert!(Arc::ptr_eq(&case.app.frame_leases[&id], live));
                assert!(live.load(Ordering::Acquire));
            }
            for window in [primary, target, other] { window.request_redraw(); }
            if native_mode_matches(primary, WindowFullscreenMode::None)
                && native_mode_matches(target, WindowFullscreenMode::Fullboth)
                && native_mode_matches(other, WindowFullscreenMode::None) {
                case.successful_passes += 1;
            }
            if case.successful_passes == 8 {
                case.app.handle_exiting();
                self.current = None;
                self.start_case(owner);
            }
        }
        owner.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(10)));
    }
}

#[test]
#[ignore = "requires explicitly selected private genuine compositor socket and real GPU"]
fn legacy_native_pending_secondary_replays_latest_updates_and_isolates_identities() {
    let socket = std::env::var("NEOMACS_TEST_WAYLAND_SOCKET").expect("selected compositor required");
    let connection = WaylandConnection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
    let mut builder = EventLoop::builder();
    builder.with_any_thread(true).with_wayland_connection(connection);
    let event_loop = builder.build().unwrap();
    let done = Arc::new(AtomicBool::new(false));
    event_loop.run_app(PendingStateProbe { pending_gpu_cases: VecDeque::from([true, false]),
        current: None, done: done.clone() }).unwrap();
    assert!(done.load(Ordering::Acquire));
}

#[test]
#[ignore = "requires explicitly selected private genuine compositor socket and real GPU"]
fn legacy_native_ready_settlement_replays_before_and_after_pending_gpu_and_keeps_exact_leases() {
    let socket = std::env::var("NEOMACS_TEST_WAYLAND_SOCKET").expect("selected compositor required");
    let connection = WaylandConnection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
    let mut builder = EventLoop::builder();
    builder.with_any_thread(true).with_wayland_connection(connection);
    let event_loop = builder.build().unwrap();
    let mut cases = VecDeque::new();
    for before_native in [true, false] {
        for mode in [WindowFullscreenMode::None, WindowFullscreenMode::Maximized,
                     WindowFullscreenMode::Fullscreen, WindowFullscreenMode::Fullboth] {
            cases.push_back((before_native, mode));
        }
    }
    let done = Arc::new(AtomicBool::new(false));
    event_loop.run_app(Probe { cases, current: None, done: done.clone() }).unwrap();
    assert!(done.load(Ordering::Acquire));
}
