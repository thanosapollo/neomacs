//! Exact lifecycle cancellation on the real bounded ThreadComms transport.
use super::{RenderApp, tests::make_test_app};
use crate::thread_comm::{
    AssetCommand, FrameRef, RenderCommand, ThreadComms, WindowCommand, WindowFullscreenMode,
};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

fn bundle(id: u64, adopt_primary: bool, live: Arc<AtomicBool>) -> RenderCommand {
    RenderCommand::Window(WindowCommand::RealizeFrame {
        frame: FrameRef::Frame(id), width: 901, height: 603,
        title: "transaction title".into(),
        geometry_hints: neovm_core::window::GuiFrameGeometryHints {
            base_width: 0, base_height: 0, min_width: 1, min_height: 1,
            width_inc: 1, height_inc: 1,
        },
        fullscreen: Some(WindowFullscreenMode::Maximized),
        visual: None, adopt_primary, reply: None, live,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(15),
    })
}

fn staged_owner() -> (crate::thread_comm::EmacsComms, RenderApp) {
    let (emacs, render) = ThreadComms::new().split();
    let mut app = make_test_app();
    app.comms = render;
    assert!(!app.comms.keep_alive_without_frames);
    for id in 0..64 {
        emacs.cmd_tx.try_send(RenderCommand::Asset(AssetCommand::SurfaceFree { id })).unwrap();
    }
    assert!(!app.process_startup_commands());
    assert_eq!(app.startup_commands.len(), 64);
    (emacs, app)
}

#[test]
fn legacy_transaction_bypasses_full_gpu_staging_and_cancel_retires_exact_identity() {
    let (emacs, mut app) = staged_owner();
    let primary = Arc::new(AtomicBool::new(true));
    for id in 64..127 {
        emacs.cmd_tx.try_send(RenderCommand::Asset(AssetCommand::SurfaceFree { id })).unwrap();
    }
    emacs.cmd_tx.try_send(bundle(0x42, true, primary.clone())).unwrap();
    assert!(!app.process_startup_commands());
    assert_eq!(app.frame_windows.primary_frame_id(), Some(0x42));
    assert!(app.frame_leases.contains_key(&0x42));
    assert_eq!(app.startup_commands.len(), 64);
    // Only the transaction was extracted, so one slot is available. Fill it
    // with the exact cosmetic command that used to strand delete-frame.
    emacs.cmd_tx.try_send(RenderCommand::Window(WindowCommand::RefreshFrameOpacity)).unwrap();
    primary.store(false, Ordering::Release);
    assert!(!app.process_startup_commands());
    assert_eq!(app.frame_windows.primary_frame_id(), None);
    assert!(app.frame_windows.primary_window().is_none());
    assert!(!app.frame_leases.contains_key(&0x42));
    assert_eq!(app.startup_commands.len(), 64);
    for id in 64..127 {
        assert!(matches!(app.comms.cmd_rx.try_recv().unwrap(),
            RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
    }
    assert!(matches!(app.comms.cmd_rx.try_recv().unwrap(),
        RenderCommand::Window(WindowCommand::RefreshFrameOpacity)));
    assert!(app.comms.cmd_rx.is_empty());
    assert!(!app.lifecycle_flags.is_shutting_down());
}

#[test]
fn legacy_secondary_cancel_discards_preparation_without_gpu_or_retry_backlog() {
    let (emacs, mut app) = staged_owner();
    app.frame_windows.adopt_primary_frame_id(0x41);
    let survivor = Arc::new(AtomicBool::new(true));
    app.frame_leases.insert(0x41, survivor.clone());
    let secondary = Arc::new(AtomicBool::new(true));
    for id in 64..127 {
        emacs.cmd_tx.try_send(RenderCommand::Asset(AssetCommand::SurfaceFree { id })).unwrap();
    }
    emacs.cmd_tx.try_send(bundle(0x42, false, secondary.clone())).unwrap();
    assert!(!app.process_startup_commands());
    assert_eq!(app.frame_windows.pending_creates.len(), 1);
    assert_eq!(app.frame_windows.pending_creates[0].emacs_frame_id, 0x42);
    emacs.cmd_tx.try_send(RenderCommand::Window(WindowCommand::RefreshFrameOpacity)).unwrap();
    secondary.store(false, Ordering::Release);
    assert!(!app.process_startup_commands());
    assert!(app.frame_windows.pending_creates.is_empty());
    assert!(app.frame_windows.pending_destroys.is_empty());
    assert!(!app.frame_leases.contains_key(&0x42));
    assert_eq!(app.frame_windows.primary_frame_id(), Some(0x41));
    assert!(Arc::ptr_eq(&app.frame_leases[&0x41], &survivor));
    assert!(survivor.load(Ordering::Acquire));
    assert_eq!(app.startup_commands.len(), 64);
    assert!(app.gpu.is_none());
}

#[test]
fn legacy_pending_transaction_retains_native_state_without_host_resize() {
    use super::frame_windows::FrameLifecycle;
    for mode in [WindowFullscreenMode::None, WindowFullscreenMode::Maximized,
                 WindowFullscreenMode::Fullscreen, WindowFullscreenMode::Fullboth] {
        let (emacs, mut app) = staged_owner();
        let live = Arc::new(AtomicBool::new(true));
        let mut command = bundle(0x42, true, live.clone());
        if let RenderCommand::Window(WindowCommand::RealizeFrame { fullscreen, .. }) = &mut command {
            *fullscreen = Some(mode);
        }
        emacs.cmd_tx.try_send(command).unwrap();
        assert!(!app.process_startup_commands());
        let primary = app.frame_windows.primary_window().unwrap();
        assert_eq!(primary.native_size(), (800, 600));
        assert_eq!(primary.chrome().title, "transaction title");
        assert!(matches!(&primary.lifecycle,
            FrameLifecycle::Pending { fullscreen: Some(actual), geometry_hints: Some(_), .. }
            if *actual == mode));
        let secondary = Arc::new(AtomicBool::new(true));
        emacs.cmd_tx.try_send(bundle(0x43, false, secondary.clone())).unwrap();
        assert!(!app.process_startup_commands());
        assert_eq!(app.frame_windows.pending_creates[0].fullscreen,
            Some(WindowFullscreenMode::Maximized));
        assert_eq!(app.frame_windows.pending_creates[0].title, "transaction title");
        assert_eq!(app.frame_windows.pending_creates[0].width, 901);
        assert!(live.load(Ordering::Acquire));
        secondary.store(false, Ordering::Release);
        assert!(!app.process_startup_commands());
        assert!(app.frame_windows.pending_creates.is_empty());
        assert_eq!(app.frame_windows.primary_frame_id(), Some(0x42));
    }
}

#[test]
fn legacy_pending_secondary_updates_keep_latest_state_on_real_transport() {
    use super::frame_windows::FrameLifecycle;
    use neovm_core::window::GuiFrameGeometryHints;
    let updated = GuiFrameGeometryHints {
        base_width: 36, base_height: 28, min_width: 72, min_height: 56,
        width_inc: 12, height_inc: 14,
    };
    for startup_drain in [true, false] {
        let (emacs, render) = ThreadComms::new().split();
        let mut app = make_test_app();
        app.comms = render;
        app.comms.keep_alive_without_frames = true;
        let leases: Vec<_> = (0..3).map(|_| Arc::new(AtomicBool::new(true))).collect();
        emacs.cmd_tx.try_send(bundle(0x42, true, leases[0].clone())).unwrap();
        assert!(!app.process_startup_commands());
        for (id, live) in [(0x43, leases[1].clone()), (0x44, leases[2].clone())] {
            emacs.cmd_tx.try_send(bundle(id, false, live)).unwrap();
        }
        for title in ["intermediate secondary", "latest secondary"] {
            emacs.cmd_tx.try_send(RenderCommand::Window(WindowCommand::SetFrameWindowTitle {
                frame: FrameRef::Frame(0x43), title: title.into(),
            })).unwrap();
        }
        emacs.cmd_tx.try_send(RenderCommand::Window(WindowCommand::SetFrameGeometryHints {
            frame: FrameRef::Frame(0x43), geometry_hints: updated,
        })).unwrap();
        emacs.cmd_tx.try_send(RenderCommand::Window(WindowCommand::SetWindowFullscreen {
            frame: FrameRef::Frame(0x43), mode: WindowFullscreenMode::Fullboth,
        })).unwrap();
        // Realization and all updates share this drain, before process_creates.
        assert!(!if startup_drain { app.process_startup_commands() } else { app.process_commands() });
        assert!(app.comms.cmd_rx.is_empty());
        assert!(app.startup_commands.is_empty());
        assert_eq!(app.frame_windows.pending_creates.len(), 2);
        let target = &app.frame_windows.pending_creates[0];
        assert_eq!(target.emacs_frame_id, 0x43);
        assert_eq!(target.title, "latest secondary");
        assert_eq!(target.geometry_hints, updated);
        assert_eq!(target.fullscreen, Some(WindowFullscreenMode::Fullboth));
        assert_eq!((target.width, target.height), (901, 603));
        let other = &app.frame_windows.pending_creates[1];
        assert_eq!(other.emacs_frame_id, 0x44);
        assert_eq!(other.title, "transaction title");
        assert_eq!(other.geometry_hints.width_inc, 1);
        assert_eq!(other.geometry_hints.height_inc, 1);
        assert_eq!(other.fullscreen, Some(WindowFullscreenMode::Maximized));
        let primary = app.frame_windows.primary_window().unwrap();
        assert_eq!(app.frame_windows.primary_frame_id(), Some(0x42));
        assert_eq!(primary.chrome().title, "transaction title");
        assert_eq!(primary.native_size(), (800, 600));
        assert!(matches!(&primary.lifecycle, FrameLifecycle::Pending {
            geometry_hints: Some(hints), fullscreen: Some(WindowFullscreenMode::Maximized), ..
        } if hints.width_inc == 1 && hints.height_inc == 1));
        assert!(app.frame_windows.get(0x43).is_none());
        assert!(app.frame_windows.get(0x44).is_none());
        assert_eq!(app.frame_leases.len(), 3);
        for (id, live) in [0x42, 0x43, 0x44].into_iter().zip(&leases) {
            assert!(Arc::ptr_eq(&app.frame_leases[&id], live));
            assert!(live.load(Ordering::Acquire));
        }
    }
}

#[test]
fn legacy_readiness_replay_source_authority_is_exact_and_non_resizing() {
    let host = include_str!("../../../../neomacs/src/main.rs");
    let legacy = host.split("fn realize_gui_frame(&mut self, request: GuiFrameHostRequest)")
        .nth(1).unwrap().split("fn poll_gui_frame_ready").next().unwrap();
    assert!(legacy.contains("reply: None"));
    assert!(!legacy.contains("bounded(1)"));
    assert!(host.contains("reply: Some(reply)"));
    let dispatch = include_str!("../window_commands.rs");
    assert!(dispatch.contains("if let Some(reply) = reply"));
    assert!(dispatch.contains("self.frame_windows.await_ready(id, reply)"));
    assert!(dispatch.contains("pending.fullscreen = Some(mode)"));
    assert!(dispatch.contains("primary.replay_pending_native_state(pending.window())"));
    let manager = include_str!("../frame_windows.rs");
    let replay = manager.split("fn replay_pending_native_state").nth(1).unwrap()
        .split("fn set_title").next().unwrap();
    assert!(replay.contains("window.set_title(&chrome.title)"));
    assert!(replay.contains("apply_window_geometry_hints(window, hints)"));
    assert!(replay.contains("apply_native_fullscreen_mode(window, *mode)"));
    assert!(!replay.contains("request_surface_size"));
    assert!(!replay.contains("request_inner_size"));
    assert!(manager.contains("window_state.replay_pending_native_state(native.window.as_ref())"));
    assert!(manager.contains("if let Some(mode) = req.fullscreen"));
    assert!(manager.contains("if reply.send(result).is_err() && ready"));
    assert!(manager.contains("if reply.send(Ok(())).is_err()"));
    let creation = include_str!("../lifecycle.rs");
    assert!(creation.contains(".replay_pending_native_state(window.as_ref())"));
    let hints = include_str!("../geometry_hints.rs");
    assert!(hints.contains("window.set_min_surface_size"));
    assert!(hints.contains("window.set_surface_resize_increments"));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicitly selected private genuine compositor socket"]
fn legacy_native_final_slot_adoption_cancel_releases_exact_gated_window() {
    use super::gpu_startup::PendingGpu;
    use std::os::unix::net::UnixStream;
    use winit::{
        application::ApplicationHandler, event::WindowEvent,
        event_loop::{ActiveEventLoop, EventLoop},
        platform::wayland::{EventLoopBuilderExtWayland, WaylandConnection},
    };
    let socket = std::env::var("NEOMACS_TEST_WAYLAND_SOCKET").expect("selected compositor required");
    let connection = WaylandConnection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
    let mut builder = EventLoop::builder();
    builder.with_any_thread(true).with_wayland_connection(connection);
    let event_loop = builder.build().unwrap();
    struct Probe { done: Arc<AtomicBool> }
    impl ApplicationHandler for Probe {
        fn window_event(&mut self, _: &dyn ActiveEventLoop, _: winit::window::WindowId, _: WindowEvent) {}
        fn can_create_surfaces(&mut self, owner: &dyn ActiveEventLoop) {
            for cancel_before_dispatch in [false, true] {
                let (emacs, mut app) = staged_owner();
                let window: Arc<dyn winit::window::Window> = Arc::from(owner.create_window(Default::default()).unwrap());
                let id = window.id();
                let weak = Arc::downgrade(&window);
                let (gate, receive) = crossbeam_channel::bounded(1);
                app.gpu_startup = Some(PendingGpu::Adapters { window, reply: receive });
                let live = Arc::new(AtomicBool::new(true));
                for id in 64..127 {
                    emacs.cmd_tx.try_send(RenderCommand::Asset(AssetCommand::SurfaceFree { id })).unwrap();
                }
                emacs.cmd_tx.try_send(bundle(0x42, true, live.clone())).unwrap();
                if cancel_before_dispatch {
                    live.store(false, Ordering::Release);
                }
                assert!(!app.process_startup_commands());
                if !cancel_before_dispatch {
                    assert_eq!(app.frame_windows.primary_frame_id(), Some(0x42));
                    assert_eq!(app.gpu_startup.as_ref().unwrap().window_id(), id);
                    emacs.cmd_tx.try_send(RenderCommand::Window(WindowCommand::RefreshFrameOpacity)).unwrap();
                    live.store(false, Ordering::Release);
                    assert!(!app.process_startup_commands());
                }
                assert_eq!(app.frame_windows.primary_frame_id(), None);
                assert!(app.frame_windows.primary_window().is_none());
                assert!(app.frame_leases.is_empty());
                assert!(app.gpu_startup.is_none());
                assert!(weak.upgrade().is_none());
                assert!(gate.try_send(Err("late result".into())).is_err());
                assert!(app.gpu.is_none());
                assert!(!app.lifecycle_flags.is_shutting_down());
                // Draining retained work must not resurrect the cancelled ID.
                while let Ok(command) = app.comms.cmd_rx.try_recv() {
                    assert!(!matches!(command, RenderCommand::Window(WindowCommand::RealizeFrame { .. })));
                }
                assert!(!app.process_startup_commands());
                assert_eq!(app.frame_windows.primary_frame_id(), None);
                assert_eq!(app.startup_commands.len(), 64);
                app.handle_exiting();
            }
            self.done.store(true, Ordering::Release);
            owner.exit();
        }
    }
    let done = Arc::new(AtomicBool::new(false));
    event_loop.run_app(Probe { done: done.clone() }).unwrap();
    assert!(done.load(Ordering::Acquire));
}
