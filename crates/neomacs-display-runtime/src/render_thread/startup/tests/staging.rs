use super::*;
use crate::thread_comm::{AssetCommand, ConfigCommand, LifecycleCommand};
use neomacs_display_protocol::ImageId;

fn asset(id: u32) -> RenderCommand {
    RenderCommand::Asset(AssetCommand::ImageRetire {
        image: ImageId::new(id),
    })
}
fn preparing_app(startup: &mut StartingApp) -> &mut RenderApp {
    let Phase::Preparing { app, .. } = &mut startup.phase else {
        panic!("preparing retained root")
    };
    app
}
fn queued_id(command: &RenderCommand) -> ImageId {
    let RenderCommand::Asset(AssetCommand::ImageRetire { image }) = command else {
        panic!("GPU asset changed type")
    };
    *image
}

#[test]
fn full_gpu_staging_and_transport_preserve_assets_while_exact_replacement_progresses() {
    let (mut startup, emacs, geometry) = fixture();
    startup.can_create_surfaces = false;
    for id in 1..=64 {
        emacs.cmd_tx.try_send(asset(id)).unwrap();
    }
    let native = CpuLoop::default();
    startup.about_to_wait(&native);
    assert_eq!(preparing_app(&mut startup).startup_commands.len(), 64);
    // The transactions sit behind more actual GPU-only commands, not just UI.
    for id in 65..=95 {
        emacs.cmd_tx.try_send(asset(id)).unwrap();
    }
    let live = Arc::new(AtomicBool::new(true));
    emacs
        .cmd_tx
        .try_send(transaction(42, live.clone(), true))
        .unwrap();
    for id in 96..=126 {
        emacs.cmd_tx.try_send(asset(id)).unwrap();
    }
    emacs
        .cmd_tx
        .try_send(transaction(43, Arc::new(AtomicBool::new(true)), false))
        .unwrap();
    assert_eq!(emacs.cmd_tx.len(), 64);
    assert!(
        emacs.cmd_tx.try_send(asset(127)).is_err(),
        "transport cap stays 64"
    );
    geometry
        .send(Ok(InitialWindowSize {
            width: 800,
            height: 600,
        }))
        .unwrap();
    live.store(false, Ordering::Release); // Geometry already sent; neither transaction consumed.
    startup.about_to_wait(&native);
    assert_eq!(
        native.constructors.get(),
        0,
        "no native permission before surfaces"
    );
    let app = preparing_app(&mut startup);
    assert_eq!(app.frame_windows.primary_event_frame_id(), 43);
    assert_eq!(
        app.frame_windows.primary_window().unwrap().native_size(),
        (901, 603)
    );
    assert!(!app.frame_leases.contains_key(&42));
    assert!(app.frame_leases.contains_key(&43));
    assert_eq!(
        app.startup_commands.len(),
        64,
        "no staging cap increase or drain"
    );
    assert_eq!(
        app.comms.cmd_rx.len(),
        62,
        "only the transactions were removed"
    );
    assert_eq!(
        app.startup_control_flow(),
        ControlFlow::Wait,
        "GPU-only pressure must not spin"
    );
    startup.can_create_surfaces = true;
    startup.about_to_wait(&native); // Production callback, no process_commands assistance.
    assert_eq!(native.constructors.get(), 1);
    assert_eq!(
        native.constructor_size.get(),
        Some(winit::dpi::PhysicalSize::new(901, 603))
    );
    assert!(!native.exiting());
    let Phase::Running(app) = &startup.phase else {
        panic!("root must survive refusing constructor")
    };
    let mut preserved: Vec<_> = app.startup_commands.iter().map(queued_id).collect();
    // Inspect payloads only after the automatic construction assertion. No GPU dispatch.
    preserved.extend(
        app.comms
            .cmd_rx
            .try_iter()
            .map(|command| queued_id(&command)),
    );
    assert_eq!(
        preserved,
        (1..=126).map(ImageId::new).collect::<Vec<_>>(),
        "all accepted GPU assets survive exactly once and in order"
    );
}

#[test]
fn full_staging_controls_apply_in_fifo_and_shader_asset_pressure_does_not_spin() {
    let (mut startup, emacs, _geometry) = fixture();
    let native = CpuLoop::default();
    for id in 1..=64 {
        emacs.cmd_tx.try_send(asset(id)).unwrap();
    }
    startup.about_to_wait(&native);
    assert_eq!(
        preparing_app(&mut startup).startup_control_flow(),
        ControlFlow::Wait
    );
    emacs.cmd_tx.try_send(asset(65)).unwrap();
    emacs
        .cmd_tx
        .try_send(RenderCommand::Config(ConfigCommand::SetExtraSpacing {
            line_spacing: 1.0,
            letter_spacing: 2.0,
        }))
        .unwrap();
    emacs.cmd_tx.try_send(asset(66)).unwrap();
    emacs
        .cmd_tx
        .try_send(RenderCommand::Config(ConfigCommand::SetExtraSpacing {
            line_spacing: 3.0,
            letter_spacing: 4.0,
        }))
        .unwrap();
    assert_eq!(
        preparing_app(&mut startup).startup_control_flow(),
        ControlFlow::Poll,
        "remaining CPU work schedules another finite pass"
    );
    startup.about_to_wait(&native);
    let app = preparing_app(&mut startup);
    assert_eq!(
        app.extra_line_spacing, 3.0,
        "CPU commands retain admission order"
    );
    assert_eq!(app.startup_commands.len(), 64);
    assert_eq!(app.comms.cmd_rx.len(), 2);
    assert_eq!(app.startup_control_flow(), ControlFlow::Wait);
    assert_eq!(native.constructors.get(), 0);
}

#[test]
fn full_staging_shutdown_behind_gpu_transport_is_consumed_by_actual_startup_callback() {
    let (mut startup, emacs, _geometry) = fixture();
    let native = CpuLoop::default();
    for id in 1..=64 {
        emacs.cmd_tx.try_send(asset(id)).unwrap();
    }
    startup.about_to_wait(&native);
    for id in 65..=127 {
        emacs.cmd_tx.try_send(asset(id)).unwrap();
    }
    emacs
        .cmd_tx
        .try_send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown))
        .unwrap();
    startup.about_to_wait(&native);
    assert!(matches!(startup.phase, Phase::Stopped));
    assert_eq!(native.constructors.get(), 0);
    assert!(
        !native.exiting(),
        "retained loop belongs to the root, not this connection"
    );
}
