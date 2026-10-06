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
fn ordinary_receive_never_prioritizes_lifecycle_or_reorders_assets() {
    let (tx, rx) = command_channel(4);
    tx.send(asset(1)).unwrap();
    tx.send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown))
        .unwrap();
    tx.send(asset(2)).unwrap();
    assert_eq!(asset_id(rx.recv().unwrap()), ImageId::new(1));
    assert!(matches!(
        rx.recv().unwrap(),
        RenderCommand::Lifecycle(LifecycleCommand::Shutdown)
    ));
    assert_eq!(asset_id(rx.recv().unwrap()), ImageId::new(2));
}

#[test]
fn selective_consumption_unblocks_a_backpressured_sender_without_asset_loss() {
    let (tx, rx) = command_channel(2);
    tx.send(asset(1)).unwrap();
    tx.send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown))
        .unwrap();
    let (done, result) = bounded(1);
    let worker = std::thread::spawn(move || {
        done.send(tx.send(asset(2))).unwrap();
    });
    assert!(matches!(
        rx.try_recv_startup().unwrap(),
        RenderCommand::Lifecycle(LifecycleCommand::Shutdown)
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
    assert!(matches!(rx.try_recv().unwrap(), RenderCommand::Ui(_)));
    rx.available().recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        rx.try_recv().unwrap(),
        RenderCommand::Lifecycle(LifecycleCommand::Shutdown)
    ));
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    tx.send(asset(1)).unwrap();
    assert_eq!(asset_id(rx.try_recv().unwrap()), ImageId::new(1));
    rx.available().recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    drop(tx);
    assert!(rx.available().recv_timeout(Duration::from_secs(1)).is_err());
}
