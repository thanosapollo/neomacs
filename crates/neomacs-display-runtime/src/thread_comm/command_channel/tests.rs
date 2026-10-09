use super::*;
use crate::thread_comm::{AssetCommand, ConfigCommand, FrameRef, UiCommand, WindowCommand};
use neomacs_display_protocol::ImageId;

fn asset(id: u32) -> RenderCommand {
    RenderCommand::Asset(AssetCommand::ImageRetire {
        image: ImageId::new(id),
    })
}
fn asset_id(command: RenderCommand) -> ImageId {
    let RenderCommand::Asset(AssetCommand::ImageRetire { image }) = command else {
        panic!("not the queued asset")
    };
    image
}

#[test]
fn selective_startup_preserves_capacity_and_fifo_for_every_queued_asset() {
    let (tx, rx) = command_channel(64);
    tx.try_send(asset(1)).unwrap();
    tx.try_send(RenderCommand::Config(ConfigCommand::SetShowFps {
        enabled: true,
    }))
    .unwrap();
    tx.try_send(asset(2)).unwrap();
    tx.try_send(RenderCommand::Window(WindowCommand::DestroyWindow {
        frame: FrameRef::Frame(42),
    }))
    .unwrap();
    for id in 3..=62 {
        tx.try_send(asset(id)).unwrap();
    }
    assert_eq!(tx.len(), 64);
    assert!(matches!(tx.try_send(asset(63)), Err(TrySendError::Full(_))));
    assert!(matches!(
        rx.try_recv_startup().unwrap(),
        RenderCommand::Config(ConfigCommand::SetShowFps { enabled: true })
    ));
    assert!(matches!(
        rx.try_recv_startup().unwrap(),
        RenderCommand::Window(WindowCommand::DestroyWindow {
            frame: FrameRef::Frame(42)
        })
    ));
    assert!(matches!(rx.try_recv_startup(), Err(TryRecvError::Empty)));
    // Startup extraction frees only the consumed slots, never more storage.
    tx.try_send(asset(63)).unwrap();
    tx.try_send(asset(64)).unwrap();
    assert_eq!(rx.len(), 64);
    assert!(matches!(tx.try_send(asset(65)), Err(TrySendError::Full(_))));
    let ids: Vec<_> = rx.try_iter().map(asset_id).collect();
    assert_eq!(ids, (1..=64).map(ImageId::new).collect::<Vec<_>>());
}

#[test]
fn ordinary_receive_prioritizes_shutdown_without_reordering_assets() {
    let (tx, rx) = command_channel(4);
    tx.send(asset(1)).unwrap();
    tx.send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown))
        .unwrap();
    tx.send(asset(2)).unwrap();
    assert!(matches!(
        rx.recv().unwrap(),
        RenderCommand::Lifecycle(LifecycleCommand::Shutdown)
    ));
    assert_eq!(asset_id(rx.recv().unwrap()), ImageId::new(1));
    assert_eq!(asset_id(rx.recv().unwrap()), ImageId::new(2));
}

#[test]
fn selective_consumption_unblocks_a_backpressured_sender_without_asset_loss() {
    let (tx, rx) = command_channel(2);
    tx.send(asset(1)).unwrap();
    tx.send(RenderCommand::Config(ConfigCommand::SetShowFps { enabled: true }))
        .unwrap();
    let (done, result) = bounded(1);
    let worker = std::thread::spawn(move || {
        done.send(tx.send(asset(2))).unwrap();
    });
    assert!(matches!(
        rx.try_recv_startup().unwrap(),
        RenderCommand::Config(ConfigCommand::SetShowFps { enabled: true })
    ));
    assert!(result.recv_timeout(Duration::from_secs(2)).unwrap().is_ok());
    assert_eq!(asset_id(rx.recv().unwrap()), ImageId::new(1));
    assert_eq!(asset_id(rx.recv().unwrap()), ImageId::new(2));
    worker.join().unwrap();
    assert!(matches!(rx.recv(), Err(RecvError)));
}

#[test]
fn receiver_drop_releases_blocked_sender_and_returns_its_exact_command() {
    let (tx, rx) = command_channel(1);
    tx.send(asset(1)).unwrap();
    let (done, result) = bounded(1);
    let worker = std::thread::spawn(move || {
        done.send(tx.send(asset(2))).unwrap();
    });
    drop(rx);
    let Err(SendError(command)) = result.recv_timeout(Duration::from_secs(2)).unwrap() else {
        panic!("disconnected sender accepted a command")
    };
    assert_eq!(asset_id(command), ImageId::new(2));
    worker.join().unwrap();
}

#[test]
fn sender_clones_disconnect_only_after_last_drop_and_pending_commands_are_drained() {
    let (tx, rx) = command_channel(2);
    let other = tx.clone();
    tx.send(asset(1)).unwrap();
    drop(tx);
    assert_eq!(asset_id(rx.recv().unwrap()), ImageId::new(1));
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(5)),
        Err(RecvTimeoutError::Timeout)
    ));
    other.send(asset(2)).unwrap();
    drop(other);
    assert_eq!(
        asset_id(rx.recv_timeout(Duration::from_secs(1)).unwrap()),
        ImageId::new(2)
    );
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
}

#[test]
fn tty_select_wake_is_rearmed_for_pending_work_and_stale_wakes_are_not_disconnects() {
    let (tx, rx) = command_channel(2);
    tx.send(RenderCommand::Ui(UiCommand::VisualBell {
        frame: FrameRef::Frame(42),
    }))
    .unwrap();
    tx.send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown))
        .unwrap();
    rx.available().recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        rx.try_recv().unwrap(),
        RenderCommand::Lifecycle(LifecycleCommand::Shutdown)
    ));
    rx.available().recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(rx.try_recv().unwrap(), RenderCommand::Ui(_)));
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    tx.send(asset(1)).unwrap();
    assert_eq!(asset_id(rx.try_recv().unwrap()), ImageId::new(1));
    rx.available().recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    drop(tx);
    assert!(rx.available().recv_timeout(Duration::from_secs(1)).is_err());
}

    #[test]
    fn shutdown_coalesces_outside_full_fifo_and_preserves_rejected_work() {
        let (sender, receiver) = command_channel(crate::thread_comm::COMMAND_CHANNEL_CAPACITY);
        for id in 0..crate::thread_comm::COMMAND_CHANNEL_CAPACITY as u32 {
            sender.try_send(RenderCommand::Asset(AssetCommand::SurfaceFree { id })).unwrap();
        }
        assert!(matches!(sender.try_send(RenderCommand::Asset(AssetCommand::SurfaceFree { id: 64 })),
            Err(TrySendError::Full(RenderCommand::Asset(AssetCommand::SurfaceFree { id: 64 })))));
        for _ in 0..1000 {
            sender.try_send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown)).unwrap();
        }
        assert!(receiver.take_shutdown());
        assert!(!receiver.take_shutdown());
        for id in 0..crate::thread_comm::COMMAND_CHANNEL_CAPACITY as u32 {
            assert!(matches!(receiver.recv().unwrap(), RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
        }
        assert!(receiver.is_empty());
    }

    #[test]
    fn cloned_producers_keep_stream_alive_and_disconnect_returns_ownership() {
        let (sender, receiver) = command_channel(crate::thread_comm::COMMAND_CHANNEL_CAPACITY);
        let other = sender.clone();
        drop(sender);
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
        other.send(RenderCommand::Config(ConfigCommand::SetShowFps { enabled: true })).unwrap();
        drop(other);
        assert!(matches!(receiver.recv().unwrap(), RenderCommand::Config(ConfigCommand::SetShowFps { enabled: true })));
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Disconnected)));
        let (sender, receiver) = command_channel(crate::thread_comm::COMMAND_CHANNEL_CAPACITY);
        drop(receiver);
        assert!(matches!(sender.try_send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown)),
            Err(TrySendError::Disconnected(RenderCommand::Lifecycle(LifecycleCommand::Shutdown)))));
        assert!(matches!(sender.send(RenderCommand::Asset(AssetCommand::SurfaceFree { id: 7 })),
            Err(SendError(RenderCommand::Asset(AssetCommand::SurfaceFree { id: 7 })))));
    }

    #[test]
    fn blocking_sender_resumes_on_space_or_owner_disconnect() {
        for disconnect in [false, true] {
            let (sender, receiver) = command_channel(crate::thread_comm::COMMAND_CHANNEL_CAPACITY);
            for id in 0..crate::thread_comm::COMMAND_CHANNEL_CAPACITY as u32 {
                sender.try_send(RenderCommand::Asset(AssetCommand::SurfaceFree { id })).unwrap();
            }
            let (started, start) = bounded(1);
            let (done, result) = bounded(1);
            let worker = std::thread::spawn(move || {
                started.send(()).unwrap();
                done.send(sender.send(RenderCommand::Asset(AssetCommand::SurfaceFree { id: 64 }))).unwrap();
            });
            start.recv().unwrap();
            assert!(matches!(result.try_recv(), Err(TryRecvError::Empty)));
            if disconnect {
                drop(receiver);
                assert!(matches!(result.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
                    Err(SendError(RenderCommand::Asset(AssetCommand::SurfaceFree { id: 64 })))));
            } else {
                assert!(matches!(receiver.try_recv().unwrap(), RenderCommand::Asset(AssetCommand::SurfaceFree { id: 0 })));
                assert!(result.recv_timeout(std::time::Duration::from_secs(1)).unwrap().is_ok());
                for id in 1..=crate::thread_comm::COMMAND_CHANNEL_CAPACITY as u32 {
                    assert!(matches!(receiver.try_recv().unwrap(), RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
                }
                assert!(receiver.is_empty());
            }
            worker.join().unwrap();
        }
    }

#[test]
fn independent_shutdown_retains_len_timeout_fifo_and_last_sender_disconnect() {
    let (tx, rx) = command_channel(2);
    tx.send(asset(1)).unwrap();
    tx.send(asset(2)).unwrap();
    for _ in 0..1000 {
        tx.send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown)).unwrap();
    }
    assert_eq!(tx.len(), 3);
    assert_eq!(rx.len(), 3);
    assert!(matches!(tx.try_send(asset(3)), Err(TrySendError::Full(_))));
    rx.available().recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(rx.recv_timeout(Duration::ZERO).unwrap(),
        RenderCommand::Lifecycle(LifecycleCommand::Shutdown)));
    assert_eq!(tx.len(), 2);
    rx.available().recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(asset_id(rx.try_recv().unwrap()), ImageId::new(1));
    rx.available().recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(asset_id(rx.try_recv().unwrap()), ImageId::new(2));
    assert_eq!(tx.len(), 0);
    assert!(rx.is_empty());
    assert!(matches!(rx.recv_timeout(Duration::ZERO), Err(RecvTimeoutError::Timeout)));
    tx.send(asset(3)).unwrap();
    tx.send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown)).unwrap();
    drop(tx);
    assert!(matches!(rx.recv_timeout(Duration::ZERO).unwrap(),
        RenderCommand::Lifecycle(LifecycleCommand::Shutdown)));
    assert_eq!(rx.try_iter().map(asset_id).collect::<Vec<_>>(), vec![ImageId::new(3)]);
    assert!(matches!(rx.recv_timeout(Duration::ZERO), Err(RecvTimeoutError::Disconnected)));
}

#[test]
fn unserviceable_startup_backlog_is_not_disconnected_or_spuriously_pollable() {
    let (tx, rx) = command_channel(2);
    tx.send(asset(1)).unwrap();
    tx.send(RenderCommand::Config(ConfigCommand::SetShowFps { enabled: true })).unwrap();
    assert!(!rx.has_startup_command_for(false));
    assert!(rx.has_startup_command_for(true));
    assert!(matches!(rx.try_recv_startup_for(true, false), Err(TryRecvError::Empty)));
    drop(tx);
    assert!(matches!(rx.try_recv_startup_for(true, false), Err(TryRecvError::Empty)));
    assert!(matches!(rx.try_recv_startup_for(true, true).unwrap(),
        RenderCommand::Config(ConfigCommand::SetShowFps { enabled: true })));
    assert!(!rx.has_startup_command_for(true));
    assert!(matches!(rx.try_recv_startup_for(true, true), Err(TryRecvError::Empty)));
    assert_eq!(asset_id(rx.try_recv().unwrap()), ImageId::new(1));
    assert!(matches!(rx.try_recv_startup_for(true, false), Err(TryRecvError::Disconnected)));
}

#[test]
fn integration_source_keeps_one_transport_and_one_command_per_tty_wake() {
    let transport = include_str!("../command_channel.rs");
    let communication = include_str!("../../thread_comm.rs");
    let tty = include_str!("../../tty_input.rs");
    let startup = include_str!("../../render_thread/command_processing.rs");
    let lifetime = include_str!("../../../../neovm-core/src/emacs_core/system/process/builtins.rs");
    assert!(communication.contains("mod command_channel;"));
    assert!(!communication.contains("mod command_mailbox;"));
    assert!(transport.contains("wake: Weak<Sender<()>>"));
    assert!(transport.contains("self.shared.space.notify_all()"));
    assert!(transport.contains("pub fn recv_timeout"));
    assert!(transport.contains("pub fn try_iter"));
    let selected = tty.split("recv(comms.cmd_rx.available())").nth(1).unwrap()
        .split("recv(rx)").next().unwrap();
    assert_eq!(selected.matches("comms.cmd_rx.try_recv()").count(), 1);
    assert!(!selected.contains("loop {"));
    assert!(startup.contains("self.comms.cmd_rx.take_shutdown()"));
    assert!(startup.contains("self.comms.cmd_rx.has_startup_command_for(self.comms.keep_alive_without_frames)"));
    assert!(startup.contains("self.handle_terminal_with_waker(c, _waker.clone())"));
    assert!(lifetime.contains("with_processes_rooted"));
    assert!(lifetime.contains("encode_and_send_process_input_in_context"));
}
