# Native crash reports

On Unix, Neomacs saves native Rust panic reports automatically, independently of
`RUST_LOG` and `NEOMACS_LOG_FILE`. This applies to the GUI, TTY, batch and daemon
frontends and to build/bootstrap utilities after logging initialization.
Ordinary Rust unit-test logging does not install the report hook.

Reports live in:

- `$XDG_STATE_HOME/neomacs/crashes/` when `XDG_STATE_HOME` is absolute;
- otherwise `$HOME/.local/state/neomacs/crashes/` when `HOME` is absolute.

The filename is `panic-<instance-start-unix-nanoseconds>-<pid>.txt`. One process
keeps one report, created only on its first panic. Each panic records its own
Unix timestamp, Rust thread ID, source location and forced native backtrace,
including when `RUST_BACKTRACE` is unset. The header includes the process ID,
logging target, executable path, version and the same build provenance used by
`neomacs --version` (unknown provenance stays explicitly unknown). A caught Rust
panic can produce a report even when the editor does not exit; a report alone
is not proof of process termination.

## Find and inspect a report

In Neomacs or GNU Emacs, use `C-x d` (`dired`) and enter the directory above.
Select the report matching the PID and time of the failed instance, then press
`RET`. This also works after restarting Neomacs. From a shell, for the usual
unset-or-absolute XDG configuration:

```sh
ls -lt -- "${XDG_STATE_HOME:-$HOME/.local/state}/neomacs/crashes/"
less -- /path/to/the/matching/panic-INSTANCE-PID.txt
```

If `XDG_STATE_HOME` is relative, use the HOME fallback instead. Timestamps are
nanoseconds since the Unix epoch, not local wall-clock time. To convert a record
timestamp with Python:

```sh
python3 -c 'import datetime,sys; print(datetime.datetime.fromtimestamp(int(sys.argv[1])/1e9, datetime.timezone.utc).isoformat())' UNIX_NANOSECONDS
```

The panic hook also writes the report path to stderr before invoking the
previous Rust panic hook. Desktop launchers need not expose stderr, so the
state directory is the persistent retrieval route; no popup or editor buffer
is opened during a panic. Include the matching report and `neomacs --version`
when reporting a bug, after reviewing them for private filesystem paths.

## Privacy, bounds and failure behavior

The report directory must be user-owned and private; newly created directories
are mode `0700` and reports are mode `0600` (subject to a stricter umask). Existing
unsafe directories are refused, not chmodded. Symlink path components and
foreign-owned or group/world-writable ancestors are refused; root-owned sticky
ancestors such as `/tmp` are allowed, but the report directory must still be
private. A preexisting report filename is never followed, overwritten or
appended to. Files are opened relative to a retained directory descriptor.

Reports deliberately omit the panic payload, thread names, buffer/chat text,
command-line arguments and environment values. Backtraces contain native code
symbols and source paths, not a dump of the Lisp heap, stack variables or
credentials. The previous Rust hook is preserved and may independently print
the panic payload to stderr; these privacy rules concern the new report only.
Nothing is uploaded.

Each process's file is capped at **256 KiB**, with an explicit limit marker.
Further panics after that limit are not recorded. Reports are not automatically
deleted or rotated: remove reviewed files yourself as needed. Total storage
therefore depends on the number of processes that panic, not just this per-file
limit. Concurrent panics may omit a record rather than wait for a report lock.

Persistence is synchronous, best effort, with file syncs and a directory sync
on first creation. An unwritable/unsafe state path or a write failure leaves the
previous hook operational; no fallback writes to a shared temporary file.
`/tmp/neomacs-first-panic.txt` is no longer used or modified. Storage failures,
resource exhaustion, a failing previous hook or a stuck filesystem can still
prevent complete diagnostics. The hook is not async-signal-safe and does not
capture SIGKILL/OOM kills, SIGSEGV, explicit aborts, or failures before logging
initialization. It does not recover buffers. On non-Unix hosts this private
file mechanism is unavailable and the existing Rust panic hook is unchanged.

## Focused tests

From the repository root:

```sh
cargo nextest run --locked -p neovm-core --lib -E 'test(logging::)'
```

The tests use disposable state directories and child processes, not a running
editor. They cover distinct instance identities, caught then fatal panics,
forced backtraces, payload omission, previous-hook chaining, permissions,
symlink/collision refusal, directory replacement, unavailable storage and the
file-size limit. The unwritable-directory DAC case requires a non-root test
user; root bypasses that permission control.
