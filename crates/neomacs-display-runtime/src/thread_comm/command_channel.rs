//! Capacity-bounded command transport with CPU-only startup extraction.
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
        if pending.queue.len() == self.shared.capacity {
            return Err(TrySendError::Full(command));
        }
        // Publish and notify under the same lock, as in the frame mailbox.
        pending.queue.push_back(command);
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
        self.shared.pending.lock().unwrap().queue.len()
    }
}

fn startup_command(command: &RenderCommand) -> bool {
    matches!(
        command,
        RenderCommand::Lifecycle(LifecycleCommand::Shutdown)
            | RenderCommand::Window(_)
            | RenderCommand::Config(_)
            | RenderCommand::Clipboard(_)
    )
}

impl CommandReceiver {
    fn take(
        &self,
        select: impl FnOnce(&VecDeque<RenderCommand>) -> Option<usize>,
    ) -> Result<RenderCommand, TryRecvError> {
        let mut pending = self.shared.pending.lock().unwrap();
        if let Some(index) = select(&pending.queue) {
            let command = pending.queue.remove(index).unwrap();
            self.shared.space.notify_all();
            if !pending.queue.is_empty()
                && let Some(wake) = self.wake.upgrade()
            {
                let _ = wake.try_send(());
            }
            return Ok(command);
        }
        if self.wake.strong_count() == 0 {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }
    pub fn try_recv(&self) -> Result<RenderCommand, TryRecvError> {
        self.take(|queue| (!queue.is_empty()).then_some(0))
    }
    pub(crate) fn try_recv_startup(&self) -> Result<RenderCommand, TryRecvError> {
        self.take(|queue| queue.iter().position(startup_command))
    }
    pub(crate) fn has_startup_command(&self) -> bool {
        self.shared
            .pending
            .lock()
            .unwrap()
            .queue
            .iter()
            .any(startup_command)
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
        self.shared.pending.lock().unwrap().queue.len()
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
        let mut pending = self.shared.pending.lock().unwrap();
        pending.receiver_alive = false;
        pending.queue.clear();
        self.shared.space.notify_all();
    }
}
