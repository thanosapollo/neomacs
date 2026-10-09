//! Capacity-bounded command transport with CPU-only startup extraction.
//! Shutdown is one independent coalescing bit; len includes that pending bit.
//!
//! Deferred startup already applies window/configuration/clipboard commands
//! ahead of staged GPU work. Extract the same commands without moving or
//! discarding the GPU backlog when staging is full. Each class retains FIFO.

#[cfg(test)]
mod tests;

use super::{LifecycleCommand, RenderCommand};
use crossbeam_channel::{
    Receiver, RecvError, RecvTimeoutError, SendError, Sender, TryRecvError, TrySendError, bounded,
};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

struct Pending {
    queue: VecDeque<RenderCommand>,
    receiver_alive: bool,
    shutdown: bool,
}
struct Shared {
    pending: Mutex<Pending>,
    space: Condvar,
    capacity: usize,
}
#[derive(Clone)]
pub struct CommandSender {
    shared: Arc<Shared>,
    wake: Arc<Sender<()>>,
}
pub struct CommandReceiver {
    shared: Arc<Shared>,
    wake: Weak<Sender<()>>,
    available: Receiver<()>,
}

/// Create a finite transport. The wake channel carries no command payloads.
pub fn command_channel(capacity: usize) -> (CommandSender, CommandReceiver) {
    assert!(capacity > 0);
    let shared = Arc::new(Shared {
        pending: Mutex::new(Pending {
            queue: VecDeque::new(),
            receiver_alive: true,
            shutdown: false,
        }),
        space: Condvar::new(),
        capacity,
    });
    let (wake, available) = bounded(1);
    let wake = Arc::new(wake);
    (
        CommandSender {
            shared: shared.clone(),
            wake: wake.clone(),
        },
        CommandReceiver {
            shared,
            wake: Arc::downgrade(&wake),
            available,
        },
    )
}

impl CommandSender {
    pub fn try_send(&self, command: RenderCommand) -> Result<(), TrySendError<RenderCommand>> {
        let mut pending = self.shared.pending.lock().unwrap();
        if !pending.receiver_alive {
            return Err(TrySendError::Disconnected(command));
        }
        if matches!(&command, RenderCommand::Lifecycle(LifecycleCommand::Shutdown)) {
            // Cancellation is one coalescing bit, independent of FIFO capacity.
            pending.shutdown = true;
        } else {
            if pending.queue.len() == self.shared.capacity {
                return Err(TrySendError::Full(command));
            }
            pending.queue.push_back(command);
        }
        // Publish and notify under the same lock, as in the frame mailbox.
        let _ = self.wake.try_send(());
        Ok(())
    }
    pub fn send(&self, mut command: RenderCommand) -> Result<(), SendError<RenderCommand>> {
        loop {
            match self.try_send(command) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(command)) => return Err(SendError(command)),
                Err(TrySendError::Full(full)) => command = full,
            }
            let mut pending = self.shared.pending.lock().unwrap();
            while pending.receiver_alive && pending.queue.len() == self.shared.capacity {
                pending = self.shared.space.wait(pending).unwrap();
            }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        let pending = self.shared.pending.lock().unwrap();
        pending.queue.len() + usize::from(pending.shutdown)
    }
}

fn startup_command(command: &RenderCommand, daemon: bool) -> bool {
    matches!(command, RenderCommand::Window(super::WindowCommand::RealizeFrame { .. }))
        || (daemon && matches!(command,
            RenderCommand::Window(_) | RenderCommand::Config(_) | RenderCommand::Clipboard(_)
        ))
}

impl CommandReceiver {
    fn take(
        &self,
        select: impl FnOnce(&VecDeque<RenderCommand>) -> Option<usize>,
    ) -> Result<RenderCommand, TryRecvError> {
        let mut pending = self.shared.pending.lock().unwrap();
        if std::mem::take(&mut pending.shutdown) {
            self.rearm(&pending);
            return Ok(RenderCommand::Lifecycle(LifecycleCommand::Shutdown));
        }
        if let Some(index) = select(&pending.queue) {
            let command = pending.queue.remove(index).unwrap();
            self.shared.space.notify_all();
            self.rearm(&pending);
            return Ok(command);
        }
        // Unserviceable startup work still belongs to this stream.
        if pending.queue.is_empty() && self.wake.strong_count() == 0 {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }
    pub fn try_recv(&self) -> Result<RenderCommand, TryRecvError> {
        self.take(|queue| (!queue.is_empty()).then_some(0))
    }
    fn rearm(&self, pending: &Pending) {
        if (!pending.queue.is_empty() || pending.shutdown)
            && let Some(wake) = self.wake.upgrade()
        {
            let _ = wake.try_send(());
        }
    }
    pub(crate) fn take_shutdown(&self) -> bool {
        let mut pending = self.shared.pending.lock().unwrap();
        let shutdown = std::mem::take(&mut pending.shutdown);
        if shutdown {
            self.rearm(&pending);
        }
        shutdown
    }
    #[cfg(test)]
    pub(crate) fn try_recv_startup(&self) -> Result<RenderCommand, TryRecvError> {
        self.try_recv_startup_for(true, true)
    }
    pub(crate) fn try_recv_startup_for(
        &self,
        staging_full: bool,
        daemon: bool,
    ) -> Result<RenderCommand, TryRecvError> {
        self.take(|queue| {
            if staging_full {
                queue.iter().position(|command| startup_command(command, daemon))
            } else {
                (!queue.is_empty()).then_some(0)
            }
        })
    }
    pub(crate) fn has_startup_command_for(&self, daemon: bool) -> bool {
        let pending = self.shared.pending.lock().unwrap();
        pending.shutdown || pending.queue.iter().any(|command| startup_command(command, daemon))
    }
    pub fn recv(&self) -> Result<RenderCommand, RecvError> {
        loop {
            match self.try_recv() {
                Ok(command) => return Ok(command),
                Err(TryRecvError::Disconnected) => return Err(RecvError),
                Err(TryRecvError::Empty) => {}
            }
            // Recheck even on disconnect: a last command may have been admitted.
            let _ = self.available.recv();
        }
    }
    pub fn recv_timeout(&self, timeout: Duration) -> Result<RenderCommand, RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.try_recv() {
                Ok(command) => return Ok(command),
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
                Err(TryRecvError::Empty) => {}
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(RecvTimeoutError::Timeout);
            };
            match self.available.recv_timeout(remaining) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {}
                Err(RecvTimeoutError::Timeout) => return Err(RecvTimeoutError::Timeout),
            }
        }
    }
    pub fn try_iter(&self) -> impl Iterator<Item = RenderCommand> + '_ {
        std::iter::from_fn(|| self.try_recv().ok())
    }
    pub fn len(&self) -> usize {
        let pending = self.shared.pending.lock().unwrap();
        pending.queue.len() + usize::from(pending.shutdown)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(crate) fn available(&self) -> &Receiver<()> {
        &self.available
    }
}
impl Drop for CommandReceiver {
    fn drop(&mut self) {
        let commands = {
            let mut pending = self.shared.pending.lock().unwrap();
            pending.receiver_alive = false;
            pending.shutdown = false;
            std::mem::take(&mut pending.queue)
        };
        self.shared.space.notify_all();
        // Drop queued replies/resources outside the transport lock.
        drop(commands);
    }
}
