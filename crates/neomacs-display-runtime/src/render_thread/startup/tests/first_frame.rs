//! Drive the real startup handler up to a refusing native constructor, never a GPU.
use super::*;
#[path = "staging.rs"]
mod staging;
use crate::thread_comm::{FrameRef, RenderCommand, ThreadComms, UiCommand, WindowCommand};
use neovm_core::window::GuiFrameGeometryHints;
use std::cell::Cell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use winit::error::{NotSupportedError, RequestError};

#[derive(Debug, Default)]
struct CpuLoop {
    constructors: Cell<usize>,
    constructor_size: Cell<Option<winit::dpi::PhysicalSize<u32>>>,
    exiting: Cell<bool>,
}
impl ActiveEventLoop for CpuLoop {
    fn create_proxy(&self) -> EventLoopProxy {
        panic!("CPU test must not start GPU preparation")
    }
    fn create_window(
        &self,
        attrs: winit::window::WindowAttributes,
    ) -> Result<Box<dyn winit::window::Window>, RequestError> {
        self.constructors.set(self.constructors.get() + 1);
        self.constructor_size
            .set(attrs.surface_size.map(|size| size.to_physical(1.0)));
        Err(RequestError::NotSupported(NotSupportedError::new(
            "CPU constructor boundary",
        )))
    }
    fn create_custom_cursor(
        &self,
        _: winit::cursor::CustomCursorSource,
    ) -> Result<winit::cursor::CustomCursor, RequestError> {
        panic!("native cursor")
    }
    fn available_monitors(&self) -> Box<dyn Iterator<Item = winit::monitor::MonitorHandle>> {
        Box::new(std::iter::empty())
    }
    fn primary_monitor(&self) -> Option<winit::monitor::MonitorHandle> {
        None
    }
    fn listen_device_events(&self, _: winit::event_loop::DeviceEvents) {}
    fn system_theme(&self) -> Option<winit::window::Theme> {
        None
    }
    fn set_control_flow(&self, _: ControlFlow) {}
    fn control_flow(&self) -> ControlFlow {
        ControlFlow::Wait
    }
    fn exit(&self) {
        self.exiting.set(true);
    }
    fn exiting(&self) -> bool {
        self.exiting.get()
    }
    fn owned_display_handle(&self) -> winit::event_loop::OwnedDisplayHandle {
        panic!("native display")
    }
    fn rwh_06_handle(&self) -> &dyn raw_window_handle::HasDisplayHandle {
        panic!("native display")
    }
}

fn transaction(id: u64, live: Arc<AtomicBool>, adopt_primary: bool) -> RenderCommand {
    let (reply, _) = crossbeam_channel::bounded(1);
    RenderCommand::Window(WindowCommand::RealizeFrame {
        frame: FrameRef::Frame(id),
        width: 901,
        height: 603,
        title: "replacement".into(),
        geometry_hints: GuiFrameGeometryHints {
            base_width: 0,
            base_height: 0,
            min_width: 1,
            min_height: 1,
            width_inc: 1,
            height_inc: 1,
        },
        fullscreen: None,
        visual: None,
        adopt_primary,
        reply: Some(reply),
        live,
        deadline: Instant::now() + Duration::from_secs(15),
    })
}

fn fixture() -> (
    StartingApp,
    crate::thread_comm::EmacsComms,
    Sender<Result<InitialWindowSize, String>>,
) {
    let (emacs, mut render) = ThreadComms::new().split();
    render.keep_alive_without_frames = true;
    render.native_window_waits = Some(Arc::new(
        crate::native_window_wait::NativeWindowWaits::default(),
    ));
    let app = RenderApp::new(
        render,
        0,
        0,
        "unowned dummy".into(),
        Default::default(),
        Arc::new((Default::default(), Default::default())),
        true,
        #[cfg(feature = "neo-term")]
        crate::terminal::new_shared_terminals(),
    );
    let (geometry, receiver) = crossbeam_channel::bounded(1);
    let mut startup = StartingApp::retained();
    startup.install(InitialWindowReceiver(receiver), app);
    startup.can_create_surfaces = true;
    (startup, emacs, geometry)
}

#[test]
fn cancelled_after_geometry_before_consumption_never_constructs_dummy_and_reuses_root() {
    cancellation_before_consumption(0);
}
#[test]
fn saturated_startup_after_geometry_cancellation_never_constructs_dummy_and_reuses_root() {
    cancellation_before_consumption(1);
}
#[test]
fn saturated_transport_after_geometry_cancellation_never_constructs_dummy_and_reuses_root() {
    cancellation_before_consumption(2);
}
fn cancellation_before_consumption(pressure: u8) {
    let saturated = pressure == 1;
    let (mut startup, emacs, geometry) = fixture();
    if pressure == 2 {
        for _ in 0..63 {
            emacs
                .cmd_tx
                .try_send(RenderCommand::Config(
                    crate::thread_comm::ConfigCommand::SetShowFps { enabled: false },
                ))
                .unwrap();
        }
    }
    if saturated {
        for _ in 0..64 {
            emacs
                .cmd_tx
                .try_send(RenderCommand::Ui(UiCommand::VisualBell {
                    frame: FrameRef::Frame(42),
                }))
                .unwrap();
        }
        if let Phase::Preparing { app, .. } = &mut startup.phase {
            assert!(!app.process_startup_commands());
            assert_eq!(app.startup_commands.len(), 64);
        }
    }
    let live = Arc::new(AtomicBool::new(true));
    emacs
        .cmd_tx
        .try_send(transaction(42, live.clone(), true))
        .unwrap();
    geometry
        .send(Ok(InitialWindowSize {
            width: 800,
            height: 600,
        }))
        .unwrap();
    live.store(false, Ordering::Release); // Precisely after geometry, before native consumption.
    let native = CpuLoop::default();
    startup.about_to_wait(&native);
    assert_eq!(
        native.constructors.get(),
        0,
        "revoked geometry is not native creation authority"
    );
    assert!(!native.exiting());
    let app = match &mut startup.phase {
        Phase::Preparing { app, .. } | Phase::Running(app) => app,
        Phase::Stopped => panic!("healthy retained loop discarded"),
    };
    assert!(
        app.frame_windows.primary_window().is_none(),
        "no unowned ID0 primary"
    );
    if saturated {
        assert!(!app.process_commands());
    } // Release the tested bounded staging boundary.
    assert!(!app.process_startup_commands());
    let replacement = Arc::new(AtomicBool::new(true));
    emacs
        .cmd_tx
        .try_send(transaction(43, replacement, false))
        .unwrap();
    assert!(!app.process_startup_commands());
    assert_eq!(app.frame_windows.primary_event_frame_id(), 43);
    assert_eq!(
        app.frame_windows.primary_window().unwrap().native_size(),
        (901, 603)
    );
    startup.about_to_wait(&native);
    assert_eq!(
        native.constructors.get(),
        1,
        "only the consumed live replacement reaches constructor"
    );
    assert_eq!(
        native.constructor_size.get(),
        Some(winit::dpi::PhysicalSize::new(901, 603)),
        "stale geometry must not resize the replacement"
    );
    assert!(!native.exiting());
}

#[test]
fn consumed_first_frame_revoked_before_surfaces_never_constructs() {
    let (mut startup, emacs, geometry) = fixture();
    let live = Arc::new(AtomicBool::new(true));
    emacs
        .cmd_tx
        .try_send(transaction(42, live.clone(), true))
        .unwrap();
    if let Phase::Preparing { app, .. } = &mut startup.phase {
        assert!(!app.process_startup_commands());
        assert_eq!(app.frame_windows.primary_event_frame_id(), 42);
    }
    geometry
        .send(Ok(InitialWindowSize {
            width: 800,
            height: 600,
        }))
        .unwrap();
    live.store(false, Ordering::Release);
    let native = CpuLoop::default();
    startup.about_to_wait(&native);
    assert_eq!(native.constructors.get(), 0);
    assert!(!native.exiting());
}

#[test]
fn missing_constructor_lease_never_runs_native_work() {
    let waits = crate::native_window_wait::NativeWindowWaits::default();
    let constructors = Cell::new(0);
    assert!(
        waits
            .run(0, || {
                constructors.set(constructors.get() + 1);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(constructors.get(), 0);
    assert!(!waits.terminal());
}

#[test]
fn full_staging_replacement_requires_automatic_callback_progress_probe() {
    let (mut startup, emacs, geometry) = fixture();
    for _ in 0..64 {
        emacs
            .cmd_tx
            .try_send(RenderCommand::Ui(UiCommand::VisualBell {
                frame: FrameRef::Frame(42),
            }))
            .unwrap();
    }
    let native = CpuLoop::default();
    startup.about_to_wait(&native);
    let live = Arc::new(AtomicBool::new(true));
    emacs
        .cmd_tx
        .try_send(transaction(42, live.clone(), true))
        .unwrap();
    geometry
        .send(Ok(InitialWindowSize {
            width: 800,
            height: 600,
        }))
        .unwrap();
    live.store(false, Ordering::Release);
    startup.about_to_wait(&native);
    assert_eq!(native.constructors.get(), 0, "orphan safety remains intact");
    emacs
        .cmd_tx
        .try_send(transaction(43, Arc::new(AtomicBool::new(true)), false))
        .unwrap();
    for _ in 0..3 {
        startup.about_to_wait(&native);
    }
    let app = match &startup.phase {
        Phase::Preparing { app, .. } | Phase::Running(app) => app,
        Phase::Stopped => panic!("healthy retained root stopped"),
    };
    assert_eq!(app.startup_commands.len(), 64, "real staging remains full");
    assert_eq!(
        app.comms.cmd_rx.len(),
        0,
        "both transactions consumed automatically"
    );
    assert_eq!(
        native.constructors.get(),
        1,
        "live replacement must progress without a test-only process_commands drain"
    );
    assert_eq!(
        native.constructor_size.get(),
        Some(winit::dpi::PhysicalSize::new(901, 603))
    );
    assert!(
        app.frame_leases.contains_key(&43),
        "exact replacement consumed"
    );
    assert!(
        !app.frame_leases.contains_key(&42),
        "revoked transaction not adopted"
    );
    assert!(!native.exiting());
}
