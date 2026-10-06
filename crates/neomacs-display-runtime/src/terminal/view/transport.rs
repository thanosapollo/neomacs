//! Exact-invocation ingress; the existing reader is the sole PTY writer.
//! Admission is bounded and atomic, not a claim of physical delivery.
use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

const CAPACITY: usize = 64 * 1024;
const WRITE_CHUNK: usize = 4096;

struct Pending {
    bytes: VecDeque<u8>,
    closed: bool,
    error: Option<String>,
    #[cfg(test)]
    blocked_writes: usize,
}

pub(super) struct Transport {
    pub(super) stop: Arc<AtomicBool>,
    pending: Mutex<Pending>,
}

impl Transport {
    pub(super) fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            pending: Mutex::new(Pending {
                bytes: VecDeque::with_capacity(CAPACITY),
                closed: false,
                error: None,
                #[cfg(test)]
                blocked_writes: 0,
            }),
        }
    }

    fn admit(&self, pending: &mut Pending, data: &[u8]) -> std::io::Result<()> {
        if self.stop.load(Ordering::Acquire) || pending.closed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "terminal ingress closed",
            ));
        }
        if data.len() > CAPACITY - pending.bytes.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "terminal ingress full",
            ));
        }
        pending.bytes.extend(data);
        Ok(())
    }

    pub(super) fn input(&self, data: &[u8]) -> std::io::Result<()> {
        let Some(mut pending) = self.pending.try_lock() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "terminal ingress busy",
            ));
        };
        self.admit(&mut pending, data)
    }

    pub(super) fn reply(&self, data: &[u8]) {
        // Called only by Processor, never while the pump holds this mutex.
        let mut pending = self.pending.lock();
        if pending.closed || self.stop.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) = self.admit(&mut pending, data) {
            Self::fail(&mut pending, format!("PTY reply admission failed: {error}"));
        }
    }

    fn fail(pending: &mut Pending, error: String) {
        pending.error.get_or_insert(error);
        pending.closed = true;
        pending.bytes.clear(); // explicitly unsendable, never reported as delivered
    }

    /// One nonblocking syscall; consume ONLY the successful prefix in FIFO order.
    /// The master fd is borrowed through the cleanup owner's reader-join fence.
    #[cfg(unix)]
    pub(super) fn pump(&self, fd: i32) -> bool {
        let mut pending = self.pending.lock();
        if self.stop.load(Ordering::Acquire) || pending.closed || pending.bytes.is_empty() {
            return false;
        }
        let bytes = pending.bytes.as_slices().0;
        let bytes = &bytes[..bytes.len().min(WRITE_CHUNK)];
        // SAFETY: master remains owned until reader join; this valid slice is
        // held across the single synchronous write. O_NONBLOCK was set before
        // cloning, and no portable writer (including its EOT Drop) exists.
        let result = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if result > 0 {
            pending.bytes.drain(..result as usize);
        } else if result == 0 {
            Self::fail(&mut pending, "PTY write made no progress".into());
        } else {
            let error = std::io::Error::last_os_error();
            match error.kind() {
                std::io::ErrorKind::WouldBlock => {
                    #[cfg(test)]
                    {
                        pending.blocked_writes += 1;
                    }
                }
                std::io::ErrorKind::Interrupted => {}
                _ => Self::fail(&mut pending, format!("PTY write failed: {error}")),
            }
        }
        !pending.bytes.is_empty()
    }

    #[cfg(not(unix))]
    pub(super) fn pump(&self, _fd: i32) -> bool {
        false
    }

    /// Close admission at reader EOF/error. A transport failure conservatively
    /// uses the existing output-error slot, never a fabricated successful drain.
    pub(super) fn finish(&self, output: Result<(), String>) -> Result<(), String> {
        let mut pending = self.pending.lock();
        pending.closed = true;
        pending.bytes.clear();
        output.and_then(|()| pending.error.clone().map_or(Ok(()), Err))
    }

    #[cfg(test)]
    pub(super) fn pressure(&self) -> (usize, usize) {
        let pending = self.pending.lock();
        (pending.blocked_writes, pending.bytes.len())
    }
}
