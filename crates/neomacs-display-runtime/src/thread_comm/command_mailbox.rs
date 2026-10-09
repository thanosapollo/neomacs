//! Bounded command admission with an independent, coalescing shutdown boundary.
//!
//! Ordinary commands retain FIFO order. During GPU startup only, the owner may
//! service daemon CPU commands past GPU work when its bounded staging is full.

use super::{LifecycleCommand, RenderCommand, COMMAND_CHANNEL_CAPACITY};
use crossbeam_channel::{Receiver, Sender, SendError, TryRecvError, TrySendError, bounded};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

struct State {
    commands: VecDeque<RenderCommand>,
    shutdown: bool,
    receiver_alive: bool,
    senders: usize,
}

struct Mailbox {
    state: Mutex<State>,
    space: Condvar,
    wake: Sender<()>,
}

/// Command sender. `try_send` returns rejected work to the caller; `send` applies
/// backpressure. Shutdown uses one independent bit, never queue/staging space.
pub struct RenderCommandSender {
    mailbox: Option<Arc<Mailbox>>,
    // Custom transports remain supported for embedders and host-only tests.
    custom: Option<Sender<RenderCommand>>,
}

pub struct RenderCommandReceiver {
    mailbox: Arc<Mailbox>,
    pub(crate) wake: Receiver<()>,
}

pub(super) fn channel() -> (RenderCommandSender, RenderCommandReceiver) {
    let (wake_tx, wake_rx) = bounded(1);
    let mailbox = Arc::new(Mailbox {
        state: Mutex::new(State {
            commands: VecDeque::with_capacity(COMMAND_CHANNEL_CAPACITY),
            shutdown: false,
            receiver_alive: true,
            senders: 1,
        }),
        space: Condvar::new(),
        wake: wake_tx,
    });
    (
        RenderCommandSender { mailbox: Some(Arc::clone(&mailbox)), custom: None },
        RenderCommandReceiver { mailbox, wake: wake_rx },
    )
}

impl From<Sender<RenderCommand>> for RenderCommandSender {
    fn from(custom: Sender<RenderCommand>) -> Self {
        Self { mailbox: None, custom: Some(custom) }
    }
}

impl Clone for RenderCommandSender {
    fn clone(&self) -> Self {
        if let Some(mailbox) = &self.mailbox {
            mailbox.state.lock().unwrap().senders += 1;
        }
        Self { mailbox: self.mailbox.clone(), custom: self.custom.clone() }
    }
}

impl RenderCommandSender {
    pub fn try_send(&self, command: RenderCommand) -> Result<(), TrySendError<RenderCommand>> {
        let Some(mailbox) = &self.mailbox else {
            return self.custom.as_ref().unwrap().try_send(command);
        };
        let mut state = mailbox.state.lock().unwrap();
        if !state.receiver_alive {
            return Err(TrySendError::Disconnected(command));
        }
        if matches!(&command, RenderCommand::Lifecycle(LifecycleCommand::Shutdown)) {
            state.shutdown = true;
        } else {
            if state.commands.len() == COMMAND_CHANNEL_CAPACITY {
                return Err(TrySendError::Full(command));
            }
            state.commands.push_back(command);
        }
        drop(state);
        let _ = mailbox.wake.try_send(());
        Ok(())
    }

    pub fn send(&self, command: RenderCommand) -> Result<(), SendError<RenderCommand>> {
        let Some(mailbox) = &self.mailbox else {
            return self.custom.as_ref().unwrap().send(command);
        };
        if matches!(&command, RenderCommand::Lifecycle(LifecycleCommand::Shutdown)) {
            return self.try_send(command).map_err(|error| SendError(error.into_inner()));
        }
        let mut state = mailbox.state.lock().unwrap();
        while state.receiver_alive && state.commands.len() == COMMAND_CHANNEL_CAPACITY {
            state = mailbox.space.wait(state).unwrap();
        }
        if !state.receiver_alive {
            return Err(SendError(command));
        }
        state.commands.push_back(command);
        drop(state);
        let _ = mailbox.wake.try_send(());
        Ok(())
    }
}

impl Drop for RenderCommandSender {
    fn drop(&mut self) {
        if let Some(mailbox) = &self.mailbox {
            mailbox.state.lock().unwrap().senders -= 1;
            let _ = mailbox.wake.try_send(());
        }
    }
}

impl RenderCommandReceiver {
    pub fn try_recv(&self) -> Result<RenderCommand, TryRecvError> {
        self.try_recv_startup(false, false)
    }

    pub(crate) fn take_shutdown(&self) -> bool {
        let mut state = self.mailbox.state.lock().unwrap();
        std::mem::take(&mut state.shutdown)
    }

    /// Shutdown is cancellation, not FIFO work. At full staging, frame
    /// transactions (and daemon CPU work) may bypass GPU backlog. Each CPU
    /// lane retains its order; neither lane adds a retry queue.
    pub(crate) fn try_recv_startup(
        &self,
        staging_full: bool,
        daemon: bool,
    ) -> Result<RenderCommand, TryRecvError> {
        let mut state = self.mailbox.state.lock().unwrap();
        if state.shutdown {
            state.shutdown = false;
            return Ok(RenderCommand::Lifecycle(LifecycleCommand::Shutdown));
        }
        let index = if !staging_full {
            (!state.commands.is_empty()).then_some(0)
        } else {
            state.commands.iter().position(|command| {
                matches!(command, RenderCommand::Window(super::WindowCommand::RealizeFrame { .. }))
                    || (daemon && matches!(command,
                        RenderCommand::Config(_) | RenderCommand::Window(_) | RenderCommand::Clipboard(_)
                    ))
            })
        };
        if let Some(index) = index {
            let command = state.commands.remove(index).unwrap();
            drop(state);
            self.mailbox.space.notify_all();
            return Ok(command);
        }
        // Staged or unserviceable queued work is not a disconnected stream.
        if state.senders == 0 && state.commands.is_empty() {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    pub fn recv(&self) -> Result<RenderCommand, crossbeam_channel::RecvError> {
        loop {
            match self.try_recv() {
                Ok(command) => return Ok(command),
                Err(TryRecvError::Disconnected) => return Err(crossbeam_channel::RecvError),
                Err(TryRecvError::Empty) => {
                    self.wake.recv().map_err(|_| crossbeam_channel::RecvError)?;
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        let state = self.mailbox.state.lock().unwrap();
        state.commands.is_empty() && !state.shutdown
    }
}

impl Drop for RenderCommandReceiver {
    fn drop(&mut self) {
        let commands = {
            let mut state = self.mailbox.state.lock().unwrap();
            state.receiver_alive = false;
            std::mem::take(&mut state.commands)
        };
        self.mailbox.space.notify_all();
        // Owner departure cancels queued replies/resources, as channel close
        // does; do not retain them merely because a producer still holds a clone.
        drop(commands);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread_comm::{AssetCommand, ConfigCommand};

    #[test]
    fn shutdown_coalesces_outside_full_fifo_and_preserves_rejected_work() {
        let (sender, receiver) = channel();
        for id in 0..COMMAND_CHANNEL_CAPACITY as u32 {
            sender.try_send(RenderCommand::Asset(AssetCommand::SurfaceFree { id })).unwrap();
        }
        assert!(matches!(sender.try_send(RenderCommand::Asset(AssetCommand::SurfaceFree { id: 64 })),
            Err(TrySendError::Full(RenderCommand::Asset(AssetCommand::SurfaceFree { id: 64 })))));
        for _ in 0..1000 {
            sender.try_send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown)).unwrap();
        }
        assert!(receiver.take_shutdown());
        assert!(!receiver.take_shutdown());
        for id in 0..COMMAND_CHANNEL_CAPACITY as u32 {
            assert!(matches!(receiver.recv().unwrap(), RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
        }
        assert!(receiver.is_empty());
    }

    #[test]
    fn cloned_producers_keep_stream_alive_and_disconnect_returns_ownership() {
        let (sender, receiver) = channel();
        let other = sender.clone();
        drop(sender);
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
        other.send(RenderCommand::Config(ConfigCommand::SetShowFps { enabled: true })).unwrap();
        drop(other);
        assert!(matches!(receiver.recv().unwrap(), RenderCommand::Config(ConfigCommand::SetShowFps { enabled: true })));
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Disconnected)));
        let (sender, receiver) = channel();
        drop(receiver);
        assert!(matches!(sender.try_send(RenderCommand::Lifecycle(LifecycleCommand::Shutdown)),
            Err(TrySendError::Disconnected(RenderCommand::Lifecycle(LifecycleCommand::Shutdown)))));
        assert!(matches!(sender.send(RenderCommand::Asset(AssetCommand::SurfaceFree { id: 7 })),
            Err(SendError(RenderCommand::Asset(AssetCommand::SurfaceFree { id: 7 })))));
    }

    #[test]
    fn blocking_sender_resumes_on_space_or_owner_disconnect() {
        for disconnect in [false, true] {
            let (sender, receiver) = channel();
            for id in 0..COMMAND_CHANNEL_CAPACITY as u32 {
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
                for id in 1..=COMMAND_CHANNEL_CAPACITY as u32 {
                    assert!(matches!(receiver.try_recv().unwrap(), RenderCommand::Asset(AssetCommand::SurfaceFree { id: actual }) if actual == id));
                }
                assert!(receiver.is_empty());
            }
            worker.join().unwrap();
        }
    }
}
