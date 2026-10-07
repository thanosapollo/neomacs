//! Process-wide tracing/logging initialization.
//!
//! Two entry points:
//!
//! - [`init`] — for binary entry points. Takes a [`LogTarget`] that
//!   selects the default writer. See [`LogTarget`] for the per-variant
//!   policy and the `NEOMACS_LOG_FILE` override.  Returns a
//!   [`LoggingGuard`] that must be kept alive until process exit so
//!   diagnostics can drain their queues (best-effort at GUI process exit).
//!
//! - [`init_for_tests`] — thin wrapper around [`init`] with
//!   [`LogTarget::Test`] for unit/integration tests. Uses
//!   `with_test_writer` so output is captured per-test by the test
//!   harness and only appears on failure.
//!
//! Both honor `RUST_LOG` and bridge the `log` facade into `tracing`
//! (via `tracing_subscriber`'s built-in `LogTracer` install inside
//! `try_init`), so events from crates using the `log` facade
//! (e.g. `cosmic-text`, `wgpu`) flow into the tracing subscriber.

use std::sync::OnceLock;

#[cfg(unix)]
#[path = "logging/crash_report.rs"]
mod crash_report;
#[path = "logging/gui_stdout.rs"]
mod gui_stdout;

#[cfg(test)]
#[path = "logging/tests.rs"]
mod tests;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Where the default writer sends tracing output for a given binary.
///
/// The variants describe *writers*, not runtime kinds: any binary can
/// pick whichever variant is appropriate. The user-facing runtime
/// classification (GUI vs TUI vs build utility) is an argument to this
/// choice, not the choice itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTarget {
    /// Write to stdout.
    ///
    /// Used by:
    /// - build-time utilities (`neomacs-temacs`, `bootstrap-neomacs`),
    ///   whose stdout is captured by the `xtask` driver and surfaced
    ///   in build logs.
    ///
    /// With `NEOMACS_LOG_FILE=<path>`: stdout **and** file.
    Stdout,
    /// Bounded nonblocking stdout diagnostics for the user-facing GUI.
    ///
    /// Retains `RUST_LOG` filtering and stdout + optional file output.
    /// Whole records are rejected at 256 queued records or 64 KiB per record;
    /// admission loss is counted and reported on the next successful write.
    /// Guard drop requests best-effort draining but never waits for stdout.
    GuiStdout,
    /// Write to a file, never to stdout/stderr.
    ///
    /// Used by the TUI binary (`neomacs -nw` / `--batch` for user-
    /// interactive runs) where writing to stdout or stderr would
    /// corrupt the alt-screen the redisplay engine is drawing into.
    ///
    /// When `NEOMACS_LOG_FILE` is not set, the TUI runs silently (no
    /// file output). Set `NEOMACS_LOG_FILE=<path>` to opt in to file
    /// logging.
    File,
    /// Unit or integration test. Default writer: captured test writer
    /// (`tracing_subscriber::fmt::TestWriter`, visible only on test
    /// failure).
    ///
    /// With `NEOMACS_LOG_FILE=<path>`: test writer **and** file.
    Test,
}

/// Held by the binary's `main` function for the duration of the process.
/// For GUI logging, drop only requests best-effort stdout/file draining on
/// owned background workers; it never waits for or writes to a blocked sink.
/// Process exit may discard accepted but undelivered diagnostics. Other
/// targets retain the existing file appender's shutdown/flush policy.
///
/// Drop this only when the process is about to exit; dropping it earlier
/// will cause subsequent log lines to be lost from the file output.
#[must_use = "drop the LoggingGuard only at process exit; dropping early loses file log lines"]
pub struct LoggingGuard {
    _file: Option<FileGuard>,
    gui: Option<gui_stdout::GuiGuard>,
}

impl LoggingGuard {
    /// GUI records rejected at admission (full queue, oversized, or closed).
    /// This is not a delivery receipt: exit can discard accepted records.
    pub fn gui_rejected_records(&self) -> usize {
        self.gui
            .as_ref()
            .map_or(0, |guard| guard.rejected_records())
    }

    /// Failed GUI sink write/flush operations observed by the worker.
    pub fn gui_write_failures(&self) -> usize {
        self.gui.as_ref().map_or(0, |guard| guard.write_failures())
    }
}

struct FileGuard {
    _direct: Option<tracing_appender::non_blocking::WorkerGuard>,
    stop: Option<crossbeam_channel::Sender<()>>,
    _shutdown_worker: Option<std::thread::JoinHandle<()>>,
}

impl FileGuard {
    fn new(worker: tracing_appender::non_blocking::WorkerGuard, target: LogTarget) -> Option<Self> {
        if target != LogTarget::GuiStdout {
            return Some(Self {
                _direct: Some(worker),
                stop: None,
                _shutdown_worker: None,
            });
        }
        // The existing file appender's destructor can print to stdout when its
        // queue is full. Keep even that shutdown work off the GUI caller.
        let (stop, stopped) = crossbeam_channel::bounded::<()>(1);
        match std::thread::Builder::new()
            .name("neomacs-log-shutdown".into())
            .spawn(move || {
                let _ = stopped.recv();
                drop(worker);
            }) {
            Ok(handle) => Some(Self {
                _direct: None,
                stop: Some(stop),
                _shutdown_worker: Some(handle),
            }),
            Err(e) => {
                // No subscriber has been installed yet, so there is no queued
                // file output when the failed spawn drops its captured guard.
                eprintln!(
                    "warning: could not start log shutdown worker: {e}; continuing without file output"
                );
                None
            }
        }
    }
}

impl Drop for FileGuard {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            let _ = stop.try_send(());
        }
    }
}

type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync + 'static>;

/// Initialize tracing for a binary entry point.
///
/// The `target` argument selects the default writer:
///
/// | target | default writer | with `NEOMACS_LOG_FILE=<path>` |
/// |---|---|---|
/// | [`LogTarget::Stdout`] | synchronous stdout (build utilities) | stdout + file |
/// | [`LogTarget::GuiStdout`] | bounded nonblocking stdout | stdout + file |
/// | [`LogTarget::File`] | silent (no file) | file at `<path>` |
/// | [`LogTarget::Test`] | captured test writer | test writer + file |
///
/// [`LogTarget::File`] produces no tracing file output unless `NEOMACS_LOG_FILE`
/// is set, so ordinary TUI logging is silent by default. On Unix, non-test
/// targets separately record private native panic reports in the XDG state
/// directory; this is independent of the tracing target and filter.
///
/// Behavior shared across all targets:
///
/// - Filter comes from `RUST_LOG`; defaults to `warn`.
/// - Bridges crates using the `log` facade into tracing (via
///   `tracing_subscriber`'s own `LogTracer` install inside
///   `try_init`).
/// - Idempotent — safe to call multiple times. Only the first call
///   sets up the global subscriber; subsequent calls return an empty
///   guard.
/// - If the configured log file fails to open, a warning is printed to
///   stderr and the function continues with the default writer only
///   (for [`LogTarget::Stdout`], [`LogTarget::GuiStdout`] and [`LogTarget::Test`]) or silently
///   (for [`LogTarget::File`]).
///
/// Legacy: `NEOMACS_LOG_TO_FILE=1` is still accepted and is equivalent
/// to setting `NEOMACS_LOG_FILE=neomacs-{pid}.log` in the current
/// directory. New call sites should prefer `NEOMACS_LOG_FILE`.
pub fn init(target: LogTarget) -> LoggingGuard {
    init_with_build_info(target, "Build provenance: not supplied")
}

/// Like [`init`], with static build provenance for private native panic reports.
/// Pass only build metadata, never argv, environment dumps, or runtime user data.
/// The first initializer wins. Test targets leave the existing panic hook alone.
pub fn init_with_build_info(target: LogTarget, build: &str) -> LoggingGuard {
    static INIT: OnceLock<()> = OnceLock::new();
    let mut guard = LoggingGuard {
        _file: None,
        gui: None,
    };
    INIT.get_or_init(|| {
        #[cfg(unix)]
        let panic_target = match target {
            LogTarget::Stdout => Some("build/stdout"),
            LogTarget::GuiStdout => Some("gui"),
            LogTarget::File => Some("tty/batch/daemon"),
            LogTarget::Test => None,
        };
        #[cfg(unix)]
        if let Some(target) = panic_target {
            crash_report::install(target, build);
        }
        #[cfg(not(unix))]
        let _ = build;
        guard = init_inner(target);
    });
    guard
}

/// Initialize tracing for unit/integration tests.
///
/// Thin wrapper around [`init`] with [`LogTarget::Test`]. Idempotent — safe
/// to call from every `#[test]` function. The returned guard is discarded
/// because tests do not have a well-defined process shutdown boundary.
pub fn init_for_tests() {
    let _ = init(LogTarget::Test);
}

fn make_env_filter<S>() -> impl tracing_subscriber::layer::Filter<S> + Send + Sync + 'static
where
    S: tracing::Subscriber,
{
    use tracing_subscriber::filter::FilterExt;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let directives = std::env::var(EnvFilter::DEFAULT_ENV).ok();
    let keep_dumps = wants_cranelift_function_dumps(directives.as_deref());
    // Carry the user's level hint: without one the combined filter reports no
    // maximum, and the `log` bridge would pass every trace record of every
    // dependency on to the dynamic filter.
    let max_level = filter
        .max_level_hint()
        .unwrap_or(tracing_subscriber::filter::LevelFilter::TRACE);
    filter.and(
        tracing_subscriber::filter::filter_fn(move |meta| {
            keep_dumps || !is_cranelift_function_dump(meta)
        })
        .with_max_level_hint(max_level),
    )
}

/// `cranelift-jit` logs every function it defines, with its whole CLIF body,
/// at `info` (`cranelift_jit::backend`).  That is a debugging dump, not an
/// operational message: under the packaged `RUST_LOG=info` it formatted and
/// wrote megabytes of IR per session.  Treat it as debug output.
fn is_cranelift_function_dump(meta: &tracing::Metadata<'_>) -> bool {
    *meta.level() == tracing::Level::INFO && meta.target() == "cranelift_jit::backend"
}

/// Whether `RUST_LOG` asks for the dumps: it names cranelift, or some
/// directive selects `debug` or `trace`.
fn wants_cranelift_function_dumps(directives: Option<&str>) -> bool {
    let Some(directives) = directives else {
        return false;
    };
    directives.contains("cranelift")
        || directives.split(',').any(|directive| {
            let level = directive.rsplit('=').next().unwrap_or("").trim();
            level.eq_ignore_ascii_case("debug") || level.eq_ignore_ascii_case("trace")
        })
}

/// Resolve the log file path from environment.
///
/// - `NEOMACS_LOG_FILE=<path>` is the canonical way to set it.
/// - `NEOMACS_LOG_TO_FILE=1` is a legacy alias that maps to
///   `neomacs-{pid}.log` in the current directory.
/// - Returns `None` when neither env var is set.
fn resolve_env_log_file() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("NEOMACS_LOG_FILE") {
        let path = std::path::PathBuf::from(path);
        if path.as_os_str().is_empty() {
            return None;
        }
        return Some(path);
    }
    let legacy = std::env::var("NEOMACS_LOG_TO_FILE")
        .map(|v| v == "1")
        .unwrap_or(false);
    if legacy {
        let pid = std::process::id();
        return Some(std::path::PathBuf::from(format!("neomacs-{pid}.log")));
    }
    None
}

fn default_layer(target: LogTarget) -> Option<BoxedLayer> {
    match target {
        LogTarget::Stdout => Some(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout)
                .with_filter(make_env_filter())
                .boxed(),
        ),
        LogTarget::GuiStdout | LogTarget::File => None,
        LogTarget::Test => Some(
            tracing_subscriber::fmt::layer()
                .with_test_writer()
                .with_filter(make_env_filter())
                .boxed(),
        ),
    }
}

fn open_file_layer(
    path: &std::path::Path,
) -> (
    Option<BoxedLayer>,
    Option<tracing_appender::non_blocking::WorkerGuard>,
) {
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(file) => {
            let (writer, worker_guard) = tracing_appender::non_blocking(file);
            let layer = tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_filter(make_env_filter())
                .boxed();
            (Some(layer), Some(worker_guard))
        }
        Err(e) => {
            eprintln!(
                "warning: could not open log file {}: {e}; continuing without file output",
                path.display(),
            );
            (None, None)
        }
    }
}

fn file_layer_for(
    _target: LogTarget,
) -> (
    Option<BoxedLayer>,
    Option<tracing_appender::non_blocking::WorkerGuard>,
) {
    let path = match resolve_env_log_file() {
        Some(path) => path,
        // No env override → no file layer for any target. TUI runs
        // silently by default; use NEOMACS_LOG_FILE to opt in.
        None => return (None, None),
    };
    open_file_layer(&path)
}

fn init_inner(target: LogTarget) -> LoggingGuard {
    // Note: `tracing_subscriber::fmt::Subscriber::try_init` (which the
    // `.try_init()` call below ultimately runs) installs the log→tracing
    // bridge itself via `LogTracer::init`, so we do NOT call
    // `tracing_log::LogTracer::init()` ourselves. Doing so was the
    // source of a
    //   "warning: tracing subscriber init failed: attempted to set a
    //    logger after the logging system was already initialized"
    // line that fired on every neomacs / neomacs-temacs / bootstrap-neomacs
    // invocation: the manual call set a global log facade, then the
    // subscriber try_init tried to set it again and the second call
    // failed via `log::SetLoggerError`, which `tracing_subscriber`
    // surfaces as a `TryInitError::Logger`.

    let (default, gui) = if target == LogTarget::GuiStdout {
        match gui_stdout::stdout() {
            Ok((writer, guard)) => (
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_filter(make_env_filter())
                        .boxed(),
                ),
                Some(guard),
            ),
            Err(e) => {
                // Do not fall back to synchronous GUI stdout on spawn failure.
                eprintln!(
                    "warning: could not start GUI logging worker: {e}; continuing without stdout diagnostics"
                );
                (None, None)
            }
        }
    } else {
        (default_layer(target), None)
    };
    let (mut file, file_guard) = file_layer_for(target);
    let file_guard = file_guard.and_then(|worker| FileGuard::new(worker, target));
    if file_guard.is_none() {
        file = None;
    }

    // Each layer carries its own `EnvFilter` (per-layer filter), so the
    // registry is not wrapped in a global filter. We combine the
    // (optional) default and file layers into a single `Vec<BoxedLayer>`:
    // `Vec<L>` implements `Layer<S>` when each `L: Layer<S>`, so a single
    // `.with(layers)` call stacks both atop the plain `Registry`. This
    // avoids the trait-bound explosion that occurs when chaining
    // `.with(Option<BoxedLayer>).with(Option<BoxedLayer>)`.
    let mut layers: Vec<BoxedLayer> = Vec::new();
    if let Some(layer) = default {
        layers.push(layer);
    }
    if let Some(layer) = file {
        layers.push(layer);
    }
    // GC trace is an explicit stderr diagnostic even for silent batch/TUI
    // logging. Keep this separate from RUST_LOG and the normal writers so
    // TRACE=1 alone exposes the generation labels without changing Lisp output.
    if std::env::var("NEOVM_GC_TRACE").as_deref() == Ok("1") {
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                metadata.is_event()
                    && metadata.target() == "neovm::gc"
                    && metadata.fields().field("kind").is_some()
                    && metadata.fields().field("cycle").is_some()
            }))
            .boxed();
        layers.push(layer);
    }
    let result = tracing_subscriber::registry().with(layers).try_init();
    if let Err(e) = result {
        eprintln!("warning: tracing subscriber init failed: {e}");
    }
    LoggingGuard {
        _file: file_guard,
        gui,
    }
}
