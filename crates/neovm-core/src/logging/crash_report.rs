//! Best-effort native panic reports, independent of tracing and its queues.
//!
//! Never format the panic payload: it can contain buffer text or credentials.
//! This is not a signal handler, a crash recovery mechanism, or an OOM guarantee.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
#[path = "crash_report/unix.rs"]
mod unix;

const MAX_REPORT_BYTES: usize = 256 * 1024;
const LIMIT_MARKER: &[u8] = b"\n[report size limit reached; subsequent panics omitted]\n";

fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos())
}

fn state_directory(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg.filter(|path| path.is_absolute())
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|path| path.join(".local/state"))
        })
        .map(|path| path.join("neomacs/crashes"))
}

/// Install once, during logging initialization and after daemon preparation.
/// A previous hook is always called, even if report creation/writing fails.
/// Non-Unix hosts keep the previous hook without claiming private file support.
pub(super) fn install(target: &'static str, build: &str) {
    #[cfg(unix)]
    {
        let started = timestamp();
        let pid = std::process::id();
        let directory = state_directory(
            std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        );
        let destination = directory
            .ok_or_else(|| io::Error::other("no absolute XDG_STATE_HOME or HOME"))
            .and_then(|path| unix::Destination::new(&path, pid, started));
        let executable = std::env::current_exe().ok();
        let identity = format!(
            "Neomacs native panic report v1\ninstance-start-unix-ns: {started}\npid: {pid}\nlogging-target: {target}\nversion: {}\nexecutable: {executable:?}\n{build}\n",
            env!("CARGO_PKG_VERSION"),
        );
        // Only a panic creates a file; ordinary launches leave no empty reports.
        // try_lock below avoids waiting for another panicking thread.
        let report = std::sync::Mutex::new(None::<LimitedReport>);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            match &destination {
                Ok(destination) => {
                    if let Ok(mut slot) = report.try_lock() {
                        let result = (|| -> io::Result<()> {
                            if slot.is_none() {
                                let mut file = LimitedReport::new(destination.create()?);
                                file.write_all(identity.as_bytes())?;
                                let _ = file.file.sync_data();
                                let _ = destination.sync();
                                *slot = Some(file);
                            }
                            if let Some(file) = slot.as_mut() {
                                if file.remaining == 0 {
                                    return Ok(());
                                }
                                let thread = std::thread::current();
                                // Thread IDs identify the worker without persisting arbitrary
                                // thread names, which can themselves contain user data.
                                let result = (|| -> io::Result<()> {
                                    writeln!(
                                        file,
                                        "\n=== PANIC ===\nunix-ns: {}\nthread: {:?}\nsite: {:?}\npayload: [omitted for privacy]",
                                        timestamp(),
                                        thread.id(),
                                        info.location()
                                    )?;
                                    // Preserve the site even if backtrace capture itself fails.
                                    let _ = file.file.sync_data();
                                    writeln!(
                                        file,
                                        "backtrace:\n{}",
                                        std::backtrace::Backtrace::force_capture()
                                    )
                                })();
                                file.finish()?;
                                if file.remaining != 0 {
                                    result?;
                                }
                            }
                            Ok(())
                        })();
                        // Ignore stderr failures; unlike eprintln!, this cannot panic
                        // merely because stderr is closed. The previous hook is unchanged.
                        let mut stderr = io::stderr().lock();
                        match result {
                            Ok(()) => {
                                let _ = writeln!(
                                    stderr,
                                    "Neomacs crash report: {}",
                                    destination.path().display()
                                );
                            }
                            Err(error) => {
                                let _ = writeln!(
                                    stderr,
                                    "Neomacs crash report incomplete/unavailable at {}: {error}",
                                    destination.path().display()
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    let _ = writeln!(
                        io::stderr().lock(),
                        "Neomacs crash report unavailable: {error}"
                    );
                }
            }
            previous(info);
        }));
    }
    #[cfg(not(unix))]
    let _ = (target, build);
}

struct LimitedReport {
    file: std::fs::File,
    remaining: usize,
    marked: bool,
}

impl LimitedReport {
    fn new(file: std::fs::File) -> Self {
        Self {
            file,
            remaining: MAX_REPORT_BYTES - LIMIT_MARKER.len(),
            marked: false,
        }
    }

    fn finish(&mut self) -> io::Result<()> {
        if self.remaining == 0 && !self.marked {
            self.marked = true;
            self.file.write_all(LIMIT_MARKER)?;
        }
        self.file.sync_data()
    }
}

impl Write for LimitedReport {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = bytes.len().min(self.remaining);
        let written = self.file.write(&bytes[..length])?;
        self.remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(all(test, unix))]
#[path = "crash_report/tests.rs"]
mod tests;
