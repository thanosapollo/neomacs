//! Exact-command lifecycle supplement to the existing PTY session.
//! One portable-pty child, existing reader/parser/grid; wait never runs on renderer.

use super::*;
use neovm_core::emacs_core::display_host::{TerminalCompletion, TerminalInvocation};
use portable_pty::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};

#[derive(Default)]
pub(super) struct Settlement {
    pub(super) child: Option<Result<ExitStatus, String>>,
    pub(super) output: Option<Result<(), String>>,
}

impl Settlement {
    pub(super) fn completion(&self) -> Option<TerminalCompletion> {
        let child = self.child.as_ref()?;
        let output = self.output.as_ref()?;
        Some(TerminalCompletion {
            exit_code: child
                .as_ref()
                .ok()
                .filter(|status| status.signal().is_none())
                .map(ExitStatus::exit_code),
            signal: child
                .as_ref()
                .ok()
                .and_then(|status| status.signal().map(str::to_owned)),
            wait_error: child.as_ref().err().cloned(),
            output_drained: output.is_ok(),
            read_error: output.as_ref().err().cloned(),
        })
    }
}

pub(super) struct AsyncOwner {
    cleanup: Sender<Cleanup>,
    pub(super) stop: Arc<AtomicBool>,
    #[cfg(test)]
    pub(super) pid: Option<u32>,
}

pub(super) struct Cleanup {
    // Keep master alive until reader exits; never abandon a join handle.
    _master: Option<Box<dyn MasterPty + Send>>,
    _writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    reader: Option<JoinHandle<()>>,
}

impl Cleanup {
    fn join(self) {
        if let Some(reader) = self.reader {
            if reader.join().is_err() {
                tracing::warn!("terminal reader panicked during cleanup");
            }
        }
        // Borrowed raw descriptor cannot be closed until the reader has joined.
        drop(self._writer);
        drop(self._master);
    }
}

pub(super) fn exact_environment(environment: &[String]) -> bool {
    // portable-pty as_command unconditionally adds SHELL, then applies envs.
    // Only an explicit first SHELL=value binding can override that injection.
    environment
        .iter()
        .find(|entry| entry.split('=').next() == Some("SHELL"))
        .is_some_and(|entry| entry.starts_with("SHELL="))
}

/// portable-pty silently falls back to HOME if its configured cwd is absent.
/// Pin the actual Linux directory inode across that check AND spawn, so a
/// rename/removal cannot silently select another directory. This file never
/// leaves the parent; no provider/dependency changes and no process-global cwd.
#[cfg(target_os = "linux")]
pub(super) fn pin_directory(directory: &str) -> std::io::Result<(std::fs::File, String)> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(directory)?;
    let path = format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd());
    if !std::fs::metadata(&path)?.is_dir() {
        return Err(std::io::Error::other(
            "pinned terminal cwd is not a directory",
        ));
    }
    Ok((file, path))
}

#[cfg(not(target_os = "linux"))]
pub(super) fn pin_directory(_directory: &str) -> std::io::Result<(std::fs::File, String)> {
    Err(std::io::Error::other(
        "exact-command cwd pinning currently requires Linux procfs",
    ))
}

pub(super) fn command(invocation: &TerminalInvocation) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(&invocation.executable);
    cmd.args(&invocation.argv);
    cmd.cwd(&invocation.directory);
    cmd.env_clear();
    let mut names = std::collections::HashSet::new();
    for entry in &invocation.environment {
        let (name, value) = entry
            .split_once('=')
            .map_or((entry.as_str(), None), |(name, value)| (name, Some(value)));
        if names.insert(name) {
            if let Some(value) = value {
                cmd.env(name, value);
            }
        }
    }
    cmd
}

/// Start a cleanup/wait owner BEFORE spawn, so thread allocation failure owns
/// no child. The channel transfers the actual child, never its numeric PID.
pub(super) fn waiter(
    proxy: NeomacsEventProxy,
) -> std::io::Result<(Sender<Box<dyn Child + Send + Sync>>, Sender<Cleanup>)> {
    let (child_tx, child_rx) = channel::<Box<dyn Child + Send + Sync>>();
    let (cleanup_tx, cleanup_rx) = channel::<Cleanup>();
    thread::Builder::new()
        .name(format!("neo-term-{}-wait", proxy.id))
        .spawn(move || {
            let Ok(mut child) = child_rx.recv() else {
                return;
            };
            let mut early_cleanup = None;
            // The worker owns both signalling and reaping: no PID-only cloned
            // killer can race a separate wait/reap or touch a reused process ID.
            let result = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Err(error) => {
                        // Unknown wait state grants no signalling authority (for
                        // example ECHILD after an external reaper). Never signal
                        // a possibly reused PID or invent an exit code here.
                        break Err(error.to_string());
                    }
                    Ok(None) => {}
                }
                match cleanup_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                    Ok(cleanup) => {
                        early_cleanup = Some(cleanup);
                        let _ = child.kill();
                        break child.wait().map_err(|error| error.to_string());
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = child.kill();
                        break child.wait().map_err(|error| error.to_string());
                    }
                }
            };
            proxy.settlement.lock().child = Some(result);
            proxy.notify_wakeup();
            // Destruction releases the view immediately but transfers all resources
            // here until reader termination. Cancellation does not publish success.
            if let Some(cleanup) = early_cleanup.or_else(|| cleanup_rx.recv().ok()) {
                cleanup.join();
                #[cfg(test)]
                proxy.cleanup_finished.store(true, Ordering::Release);
            }
        })?;
    Ok((child_tx, cleanup_tx))
}

impl AsyncOwner {
    pub(super) fn new(_child: &dyn Child, cleanup: Sender<Cleanup>, stop: Arc<AtomicBool>) -> Self {
        Self {
            cleanup,
            stop,
            #[cfg(test)]
            pid: _child.process_id(),
        }
    }

    pub(super) fn retire(self, session: &mut PtySession) {
        self.stop.store(true, Ordering::Release);
        // No renderer kill/wait/join. The sole child owner signals only an
        // unreaped child on its worker and holds all resources through cleanup.
        let cleanup = Cleanup {
            _master: session.master.take(),
            _writer: session.writer.take(),
            reader: session.reader_thread.take(),
        };
        if let Err(error) = self.cleanup.send(cleanup) {
            // A panicked worker is not normal retirement. Preserve resources
            // on a replacement cleanup worker; never join on the renderer.
            let cleanup = Arc::new(Mutex::new(Some(error.0)));
            let worker_cleanup = cleanup.clone();
            let recovery = thread::Builder::new()
                .name("neo-term-cleanup-recovery".into())
                .spawn(move || {
                    worker_cleanup.lock().take().expect("owned cleanup").join();
                });
            if recovery.is_err() {
                // Emergency only: a panicked original owner PLUS allocation
                // failure cannot detach a reader or close its borrowed fd.
                // Normal retirement never joins on the renderer.
                cleanup.lock().take().expect("untransferred cleanup").join();
            }
            tracing::error!("terminal wait/cleanup worker was unavailable");
        }
    }
}

/// Exact-only: portable-pty clones share the master open-file description.
/// No portable writer is created; every input/reply goes through Transport.
/// Set flags before cloning/spawning, retaining O_NONBLOCK through reader join.
#[cfg(unix)]
pub(super) fn nonblocking_descriptor(master: &dyn MasterPty) -> std::io::Result<i32> {
    let fd = master
        .as_raw_fd()
        .ok_or_else(|| std::io::Error::other("PTY has no native fd"))?;
    // SAFETY: borrowed live master; these calls change flags, not fd ownership.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: same live master; preserve all unrelated open-file status flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

#[cfg(not(unix))]
pub(super) fn nonblocking_descriptor(_master: &dyn MasterPty) -> std::io::Result<i32> {
    Err(std::io::Error::other(
        "exact-command terminals currently require Unix cancellation",
    ))
}

#[cfg(unix)]
pub(super) fn read_ready(fd: i32, pending: bool) -> std::io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN | if pending { libc::POLLOUT } else { 0 },
        revents: 0,
    };
    // SAFETY: the sole cleanup owner keeps this master fd alive until the
    // reader has joined. The pollfd pointer is valid for the synchronous call.
    let result = unsafe { libc::poll(&mut descriptor, 1, 10) };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if descriptor.revents & libc::POLLNVAL != 0 {
        return Err(std::io::Error::other(
            "terminal reader descriptor became invalid",
        ));
    }
    // Write readiness is handled by the next nonblocking pump, not a read.
    // HUP/ERR still read remaining output through portable-pty's EOF/EIO policy.
    Ok(descriptor.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0)
}

#[cfg(not(unix))]
pub(super) fn read_ready(_fd: i32, _pending: bool) -> std::io::Result<bool> {
    Err(std::io::Error::other(
        "exact-command terminals currently require Unix cancellation",
    ))
}
