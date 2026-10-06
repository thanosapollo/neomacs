//! TerminalView: manages a single terminal instance (Crosswords + PTY).
//!
//! Each TerminalView wraps a `rio_vt::crosswords::Crosswords`, spawns a PTY
//! child process (shell) via `portable-pty`, and runs a reader thread
//! to feed PTY output into the terminal state.

mod invocation;
mod transport;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use parking_lot::{FairMutex, Mutex};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::{Crosswords, CrosswordsSize};
use rio_vt::event::{EventListener, RioEvent, WindowId};
use rio_vt::performer::handler::Processor;

use super::content::TerminalContent;
use super::directory::{DirectoryUpdates, local_directory};
use super::{TerminalDisplayTarget, TerminalGridSize, TerminalId};

/// Scrollback history limit, matching the previous emulator default.
const SCROLLBACK_HISTORY: usize = 10_000;

/// Event listener that bridges rio-vt events to neomacs.
#[derive(Clone)]
pub struct NeomacsEventProxy {
    id: TerminalId,
    /// Signals that the terminal has new content to render.
    wakeup: Arc<std::sync::atomic::AtomicBool>,
    /// Signals that the terminal child process has exited.
    exited: Arc<std::sync::atomic::AtomicBool>,
    /// Replies the terminal wants written back to the PTY (DA/DSR/CPR).
    pending_writes: Arc<Mutex<Vec<u8>>>,
    exact_transport: Option<Arc<transport::Transport>>,
    /// Latest title not yet forwarded to the evaluator.
    pending_title: Arc<Mutex<Option<String>>>,
    pending_directory: Arc<Mutex<DirectoryUpdates>>,
    settlement: Arc<Mutex<invocation::Settlement>>,
    native_waker: Option<winit::event_loop::EventLoopProxy>,
    #[cfg(test)]
    cleanup_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl NeomacsEventProxy {
    fn new(id: TerminalId) -> Self {
        Self {
            id,
            wakeup: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pending_writes: Arc::new(Mutex::new(Vec::new())),
            exact_transport: None,
            pending_title: Arc::new(Mutex::new(None)),
            pending_directory: Arc::new(Mutex::new(DirectoryUpdates::default())),
            settlement: Arc::new(Mutex::new(invocation::Settlement::default())),
            native_waker: None,
            #[cfg(test)]
            cleanup_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Check and clear the wakeup flag.
    pub fn take_wakeup(&self) -> bool {
        self.wakeup
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// Check if wakeup is pending without consuming it.
    pub fn peek_wakeup(&self) -> bool {
        self.wakeup.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Check if the terminal child process has exited.
    pub fn is_exited(&self) -> bool {
        self.exited.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn completion(&self) -> Option<neovm_core::emacs_core::display_host::TerminalCompletion> {
        self.settlement.lock().completion()
    }

    #[cfg(test)]
    pub(crate) fn cleanup_complete(&self) -> bool {
        self.cleanup_finished
            .load(std::sync::atomic::Ordering::Acquire)
    }

    fn settle_output(&self, output: Result<(), String>) {
        let output = match &self.exact_transport {
            Some(transport) => transport.finish(output),
            None => output,
        };
        self.settlement.lock().output = Some(output);
        self.notify_wakeup();
    }

    fn notify_wakeup(&self) {
        if !self.wakeup.swap(true, std::sync::atomic::Ordering::AcqRel) {
            if let Some(waker) = &self.native_waker {
                waker.wake_up();
            }
        }
    }

    fn notify_exit(&self) {
        tracing::info!("Terminal {}: child process exited", self.id);
        self.exited
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Drain replies queued for write-back to the PTY.
    fn take_pending_writes(&self) -> Vec<u8> {
        std::mem::take(&mut *self.pending_writes.lock())
    }

    pub fn take_directory(&self) -> Option<String> {
        self.pending_directory.lock().take()
    }

    /// Called only after the native Processor updated its optional raw URI.
    fn observe_native_directory(
        &self,
        term: &Crosswords<Self>,
        last_uri: &mut Option<Vec<u8>>,
        local_host: Option<&str>,
    ) {
        if term.current_directory_uri != *last_uri {
            let directory = term
                .current_directory_uri
                .as_deref()
                .and_then(|uri| local_directory(uri, local_host));
            self.pending_directory.lock().observe(directory);
            last_uri.clone_from(&term.current_directory_uri);
        }
    }

    pub fn take_title(&self) -> Option<String> {
        self.pending_title.lock().take()
    }
}

impl EventListener for NeomacsEventProxy {
    fn event(&self) -> (Option<RioEvent>, bool) {
        (None, false)
    }

    fn send_event(&self, event: RioEvent, _window_id: WindowId) {
        match event {
            RioEvent::PtyWrite(_route, text) => {
                if let Some(transport) = &self.exact_transport {
                    transport.reply(text.as_bytes());
                } else {
                    self.pending_writes
                        .lock()
                        .extend_from_slice(text.as_bytes());
                }
            }
            RioEvent::Title(title) => {
                tracing::debug!("Terminal {}: title changed to '{}'", self.id, title);
                *self.pending_title.lock() = Some(title);
                self.notify_wakeup();
            }
            RioEvent::Bell => {
                tracing::debug!("Terminal {}: bell", self.id);
            }
            _ => {}
        }
    }
}

/// A single terminal instance.
pub struct TerminalView {
    pub id: TerminalId,
    pub target: TerminalDisplayTarget,
    /// The terminal state (shared with PTY reader).
    pub term: Arc<FairMutex<Crosswords<NeomacsEventProxy>>>,
    /// Event proxy for wakeup notifications.
    pub event_proxy: NeomacsEventProxy,
    /// Owns every operating-system resource backing this terminal. Keeping
    /// the child, master handles, and reader join handle in one object makes
    /// an incomplete teardown impossible through the normal API.
    pty_session: PtySession,
    /// Cached content from last extraction.
    pub last_content: Option<TerminalContent>,
    /// Whether content changed since last render.
    pub dirty: bool,
    /// Whether the Emacs side has been notified about process exit.
    pub exit_notified: bool,
    /// Floating position (only used in Floating mode).
    pub float_x: f32,
    pub float_y: f32,
    pub float_opacity: f32,
}

struct PtySession {
    master: Option<Box<dyn MasterPty + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    reader_thread: Option<JoinHandle<()>>,
    async_owner: Option<invocation::AsyncOwner>,
    exact_transport: Option<Arc<transport::Transport>>,
}

#[derive(Debug, thiserror::Error)]
pub enum TerminalShutdownError {
    #[error("failed to reap terminal child: {0}")]
    Reap(std::io::Error),
    #[error("terminal reader thread panicked during shutdown")]
    ReaderPanicked,
}

impl PtySession {
    #[cfg(test)]
    fn process_id(&self) -> Option<u32> {
        self.async_owner
            .as_ref()
            .and_then(|owner| owner.pid)
            .or_else(|| self.child.as_ref().and_then(|child| child.process_id()))
    }

    fn write(&self, data: &[u8]) -> std::io::Result<()> {
        if let Some(transport) = &self.exact_transport {
            return transport.input(data);
        }
        let writer = self.writer.as_ref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "terminal is shut down")
        })?;
        let mut writer = writer.lock();
        writer.write_all(data)?;
        writer.flush()
    }

    fn resize(&self, size: PtySize) -> std::io::Result<()> {
        self.master
            .as_ref()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "terminal is shut down")
            })?
            .resize(size)
            .map_err(|error| std::io::Error::other(error.to_string()))
    }

    fn shutdown(&mut self) -> Result<(), TerminalShutdownError> {
        if let Some(owner) = self.async_owner.take() {
            owner.retire(self);
            return Ok(()); // resource ownership transferred, not joined here
        }
        let reap_result = if let Some(mut child) = self.child.take() {
            match child.try_wait() {
                Ok(Some(_)) => Ok(()),
                Ok(None) | Err(_) => {
                    // A failed kill can race with natural exit. `wait` is the
                    // authoritative operation: it both observes that race and
                    // guarantees the child is reaped before teardown returns.
                    let _ = child.kill();
                    child
                        .wait()
                        .map(|_| ())
                        .map_err(TerminalShutdownError::Reap)
                }
            }
        } else {
            Ok(())
        };

        // Close the render thread's master/writer handles before joining. The
        // reader owns its cloned handles until the killed slave side reaches
        // EOF/EIO and the reader exits.
        self.writer.take();
        self.master.take();
        let join_result = self
            .reader_thread
            .take()
            .map(JoinHandle::join)
            .transpose()
            .map(|_| ())
            .map_err(|_| TerminalShutdownError::ReaderPanicked);

        reap_result.and(join_result)
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            tracing::warn!("Terminal PTY shutdown during drop failed: {error}");
        }
    }
}

impl TerminalView {
    /// Create a new terminal with the given grid dimensions.
    pub fn new(
        id: TerminalId,
        size: TerminalGridSize,
        target: TerminalDisplayTarget,
        shell: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_invocation(id, size, target, shell, None)
    }

    /// Exact argv/cwd/environment variant of the SAME PTY/parser/view.
    pub fn new_invocation(
        id: TerminalId,
        size: TerminalGridSize,
        target: TerminalDisplayTarget,
        shell: Option<&str>,
        invocation: Option<&neovm_core::emacs_core::display_host::TerminalInvocation>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_invocation_with_waker(id, size, target, shell, invocation, None)
    }

    pub(crate) fn new_invocation_with_waker(
        id: TerminalId,
        size: TerminalGridSize,
        target: TerminalDisplayTarget,
        shell: Option<&str>,
        invocation: Option<&neovm_core::emacs_core::display_host::TerminalInvocation>,
        native_waker: Option<winit::event_loop::EventLoopProxy>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut event_proxy = NeomacsEventProxy::new(id);
        event_proxy.native_waker = native_waker;
        if invocation.is_some() {
            event_proxy.exact_transport = Some(Arc::new(transport::Transport::new()));
        }
        let cols = size.cols.get();
        let rows = size.rows.get();

        let grid_size = CrosswordsSize::new(cols.max(1) as usize, rows.max(1) as usize);
        let term = Crosswords::new(
            grid_size,
            CursorShape::Block,
            event_proxy.clone(),
            WindowId::from(0),
            0,
            SCROLLBACK_HISTORY,
        );
        let term = Arc::new(FairMutex::new(term));

        // Create PTY and spawn shell using portable-pty.
        let pty_system = native_pty_system();
        let pty_size = PtySize {
            rows,
            cols,
            pixel_width: 8u16.saturating_mul(cols),
            pixel_height: 16u16.saturating_mul(rows),
        };
        let pty_pair = pty_system
            .openpty(pty_size)
            .map_err(|e| format!("Failed to create PTY: {}", e))?;

        let mut cmd = if let Some(invocation) = invocation {
            invocation::command(invocation)
        } else if let Some(shell_path) = shell {
            CommandBuilder::new(shell_path)
        } else {
            CommandBuilder::new_default_prog()
        };

        let pinned_directory = if let Some(invocation) = invocation {
            if !invocation::exact_environment(&invocation.environment) {
                return Err("portable-pty requires explicit SHELL=value for exact environment; keep canonical Eshell route".into());
            }
            let (file, path) = invocation::pin_directory(&invocation.directory)?;
            cmd.cwd(path);
            Some(file)
        } else {
            None
        };

        // Ensure TERM is set for the child shell process.
        // In neomacs, the display backend is GPU-based so TERM is typically unset.
        let term_env = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string());
        if invocation.is_some() {
            // Complete Eshell environment, including its explicit terminal TERM.
        } else if term_env.is_empty() {
            cmd.env("TERM", "xterm-256color");
        } else {
            cmd.env("TERM", &term_env);
        }

        // Exact invocation: all master duplicates share O_NONBLOCK. The existing
        // reader is the ONLY writer; never construct portable-pty's EOT-on-Drop
        // writer on this path. Legacy shell read/write semantics stay unchanged.
        let read_fd = if invocation.is_some() {
            Some(invocation::nonblocking_descriptor(&*pty_pair.master)?)
        } else {
            None
        };
        let pty_read_file = pty_pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("Failed to clone PTY reader: {}", e))?;
        let pty_writer = if invocation.is_some() {
            None
        } else {
            Some(Arc::new(Mutex::new(
                pty_pair
                    .master
                    .take_writer()
                    .map_err(|e| format!("Failed to take PTY writer: {}", e))?,
            )))
        };
        let waiter = if invocation.is_some() {
            Some(invocation::waiter(event_proxy.clone())?)
        } else {
            None
        };
        let pty_child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Failed to spawn PTY child: {}", e))?;
        // spawn has finished its chdir/exec handshake; release the pinned cwd.
        drop(pinned_directory);
        // Release the parent's slave: retaining it would prevent output EOF.
        drop(pty_pair.slave);
        let async_owner = waiter.as_ref().map(|(_, cleanup)| {
            invocation::AsyncOwner::new(
                &*pty_child,
                cleanup.clone(),
                event_proxy
                    .exact_transport
                    .as_ref()
                    .expect("exact transport")
                    .stop
                    .clone(),
            )
        });
        let mut session = PtySession {
            master: Some(pty_pair.master),
            child: Some(pty_child),
            writer: pty_writer.clone(),
            reader_thread: None,
            async_owner,
            exact_transport: event_proxy.exact_transport.clone(),
        };
        let stop = session.async_owner.as_ref().map(|owner| owner.stop.clone());
        if let Some((child_tx, _)) = waiter {
            // The worker's first operation is recv; it cannot retire its
            // receiver until this sender transfers the child. No child work,
            // callbacks or fallible operations precede receipt on that worker.
            child_tx
                .send(session.child.take().expect("spawned child is owned"))
                .expect("pre-spawn wait owner retains the child receiver");
        }

        // Spawn reader thread: reads from PTY, feeds into term via Processor
        let term_clone = Arc::clone(&term);
        let proxy_clone = event_proxy.clone();
        let writer_clone = pty_writer;
        // Kernel hostname only: no DNS and no per-output hostname lookup.
        let local_host = hostname::get()
            .ok()
            .and_then(|host| host.into_string().ok());
        let reader_thread = thread::Builder::new()
            .name(format!("neo-term-{}-pty", id))
            .spawn(move || {
                let mut reader = pty_read_file;
                let mut processor = Processor::default();
                let mut last_uri: Option<Vec<u8>> = None;
                let mut buf = [0u8; 4096];
                loop {
                    if stop
                        .as_ref()
                        .is_some_and(|stop| stop.load(std::sync::atomic::Ordering::Acquire))
                    {
                        proxy_clone.settle_output(Err("terminal cancelled".into()));
                        break;
                    }
                    if let Some(fd) = read_fd {
                        let pending = proxy_clone
                            .exact_transport
                            .as_ref()
                            .expect("exact transport")
                            .pump(fd);
                        match invocation::read_ready(fd, pending) {
                            Ok(false) => continue,
                            Ok(true) => {}
                            Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => {
                                continue;
                            }
                            Err(error) => {
                                proxy_clone.settle_output(Err(error.to_string()));
                                break;
                            }
                        }
                        // Cancellation may have arrived while readiness was polled.
                        if stop
                            .as_ref()
                            .is_some_and(|stop| stop.load(std::sync::atomic::Ordering::Acquire))
                        {
                            proxy_clone.settle_output(Err("terminal cancelled".into()));
                            break;
                        }
                    }
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            // Output EOF alone is NOT authoritative child exit.
                            if stop.is_some() {
                                proxy_clone.settle_output(Ok(()));
                            } else {
                                proxy_clone.notify_exit();
                            }
                            break;
                        }
                        Ok(n) => {
                            {
                                let mut term = term_clone.lock();
                                processor.advance(&mut *term, &buf[..n]);
                                // Consume only native parser metadata, never reparse PTY bytes.
                                // Ordinary high-output reads do not allocate or emit cwd events.
                                proxy_clone.observe_native_directory(
                                    &term,
                                    &mut last_uri,
                                    local_host.as_deref(),
                                );
                            }
                            // Answer the terminal's queries (DA/DSR/CPR)
                            let replies = proxy_clone.take_pending_writes();
                            if !replies.is_empty() {
                                let mut writer =
                                    writer_clone.as_ref().expect("legacy writer").lock();
                                if let Err(e) =
                                    writer.write_all(&replies).and_then(|_| writer.flush())
                                {
                                    tracing::warn!("Terminal {} PTY reply write failed: {}", id, e);
                                }
                            }
                            // Signal that content changed
                            proxy_clone.notify_wakeup();
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                            continue;
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // Non-blocking fd, wait and retry
                            std::thread::sleep(std::time::Duration::from_millis(10));
                            continue;
                        }
                        Err(e) => {
                            // Treat a dead PTY like an exited child so the
                            // render thread reports it to Emacs.
                            tracing::warn!("Terminal {} PTY read error: {}", id, e);
                            if stop.is_some() {
                                proxy_clone.settle_output(Err(e.to_string()));
                            } else {
                                proxy_clone.notify_exit();
                            }
                            break;
                        }
                    }
                }
            });
        let reader_thread = match reader_thread {
            Ok(reader) => reader,
            Err(error) => {
                event_proxy.settle_output(Err(format!("PTY reader spawn failed: {error}")));
                return Err(error.into()); // session Drop transfers owned child cleanup
            }
        };

        Ok(Self {
            id,
            target,
            term,
            event_proxy,
            pty_session: {
                session.reader_thread = Some(reader_thread);
                session
            },
            last_content: None,
            dirty: true,
            exit_notified: false,
            float_x: 0.0,
            float_y: 0.0,
            float_opacity: 1.0,
        })
    }

    /// Write input data to the terminal's PTY (keyboard input from user).
    /// Exact invocations acknowledge bounded FIFO admission, not delivery;
    /// WouldBlock rejects the entire input when full/busy, with no partial paste.
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.pty_session.write(data)
    }

    /// Resize the terminal grid and PTY.
    pub fn resize(&mut self, size: TerminalGridSize) {
        let cols = size.cols.get();
        let rows = size.rows.get();
        let grid_size = CrosswordsSize::new(cols as usize, rows as usize);
        let mut term = self.term.lock();
        term.resize(grid_size);
        drop(term);

        let pty_size = PtySize {
            rows,
            cols,
            pixel_width: 8u16.saturating_mul(cols),
            pixel_height: 16u16.saturating_mul(rows),
        };
        if let Err(e) = self.pty_session.resize(pty_size) {
            tracing::warn!("Terminal {} PTY resize failed: {}", self.id, e);
        }
        self.dirty = true;
    }

    /// Extract current content for rendering. Returns true if content changed.
    pub fn update_content(&mut self) -> bool {
        if self.event_proxy.take_wakeup() || self.dirty {
            let term = self.term.lock();
            self.last_content = Some(TerminalContent::from_term(&*term));
            self.dirty = false;
            true
        } else {
            false
        }
    }

    /// Get the last extracted content.
    pub fn content(&self) -> Option<&TerminalContent> {
        self.last_content.as_ref()
    }

    /// Extract text from a region of the terminal.
    pub fn get_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        let term = self.term.lock();
        super::content::extract_text(&*term, start_row, start_col, end_row, end_col)
    }

    /// Get all visible text.
    pub fn get_visible_text(&self) -> String {
        let term = self.term.lock();
        let cols = term.columns();
        let rows = term.screen_lines();
        super::content::extract_text(&*term, 0, 0, rows.saturating_sub(1), cols.saturating_sub(1))
    }

    /// Terminate and reap the PTY child, close all master handles, and join
    /// the reader thread. Safe to call repeatedly.
    pub fn shutdown(&mut self) -> Result<(), TerminalShutdownError> {
        self.pty_session.shutdown()
    }

    #[cfg(test)]
    pub(crate) fn child_process_id(&self) -> Option<u32> {
        self.pty_session.process_id()
    }

    #[cfg(test)]
    pub(crate) fn reply_pressure(&self) -> (usize, usize) {
        self.pty_session
            .exact_transport
            .as_ref()
            .expect("exact transport")
            .pressure()
    }
}

/// Manages all terminal instances.
pub struct TerminalManager {
    pub terminals: HashMap<TerminalId, TerminalView>,
    pub palettes: HashMap<
        neomacs_display_protocol::types::DisplayFrameId,
        neomacs_display_protocol::neo_term_palette::NeoTermPalette,
    >,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            terminals: HashMap::new(),
            palettes: HashMap::new(),
        }
    }

    /// Destroy a terminal.
    pub fn destroy(&mut self, id: TerminalId) -> Result<bool, TerminalShutdownError> {
        let Some(mut view) = self.terminals.remove(&id) else {
            return Ok(false);
        };
        view.shutdown()?;
        Ok(true)
    }

    /// Get a terminal by ID.
    pub fn get(&self, id: TerminalId) -> Option<&TerminalView> {
        self.terminals.get(&id)
    }

    /// Get a mutable terminal by ID.
    pub fn get_mut(&mut self, id: TerminalId) -> Option<&mut TerminalView> {
        self.terminals.get_mut(&id)
    }

    /// Update all terminals (extract content if changed). Returns IDs that changed.
    pub fn update_all(&mut self) -> Vec<TerminalId> {
        let mut changed = Vec::new();
        for (id, view) in &mut self.terminals {
            if view.update_content() {
                changed.push(*id);
            }
        }
        changed
    }

    /// Get all terminal IDs.
    pub fn ids(&self) -> Vec<TerminalId> {
        let mut ids: Vec<_> = self.terminals.keys().copied().collect();
        ids.sort_unstable_by_key(|id| id.get());
        ids
    }

    /// Number of active terminals.
    pub fn len(&self) -> usize {
        self.terminals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terminals.is_empty()
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
