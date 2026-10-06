//! Real renderer dispatch + native parser replies; independently owned holder
//! must remain alive until reader join/master release, not rescue successful RED.
use super::*;
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn until(mut predicate: impl FnMut() -> bool, reason: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "{reason}");
        std::thread::yield_now();
    }
}

fn worker_exists(name: &str) -> bool {
    assert!(name.is_ascii());
    let prefix = &name[..name.len().min(15)];
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(Result::ok)
        .any(|task| {
            std::fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|comm| comm.trim() == prefix)
        })
}

// The isolated test process is the subreaper. Only this fixture's descendant is
// signalled through a captured pidfd and waitpid-reaped; no stale PID signalling.
struct Holder {
    release: Option<mpsc::Sender<()>>,
    guard: Option<std::thread::JoinHandle<()>>,
    released: Arc<std::sync::atomic::AtomicBool>,
}

impl Holder {
    fn new(root: std::path::PathBuf) -> (Self, mpsc::Receiver<libc::pid_t>) {
        let (release, receive) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watchdog_released = released.clone();
        let guard = std::thread::spawn(move || {
            // Installed BEFORE creation dispatch, including failure publication.
            // The helper publishes its PID atomically and then remains alive.
            let deadline = Instant::now() + Duration::from_secs(10);
            let pid = loop {
                if let Ok(pid) = std::fs::read_to_string(root.join("holder")) {
                    break pid.parse::<libc::pid_t>().unwrap();
                }
                if Instant::now() >= deadline {
                    return; // no helper handshake; caller's ready receive fails
                }
                std::thread::yield_now();
            };
            // SAFETY: acquire identity while the handshaken owned child is alive.
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            assert!(fd >= 0);
            // SAFETY: successful pidfd_open returns a new descriptor owned here.
            let descriptor = unsafe { std::fs::File::from_raw_fd(fd as i32) };
            let _ = ready_tx.send(pid);
            // Failure watchdog releases pressure only after the success deadline.
            // Success explicitly checks retirement BEFORE sending this release.
            let _ = receive.recv_timeout(Duration::from_secs(10));
            watchdog_released.store(true, std::sync::atomic::Ordering::Release);
            std::fs::write(root.join("EXIT"), b"exit").unwrap();
            // SAFETY: pidfd binds the still-owned fixture descendant's identity.
            let result = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    descriptor.as_raw_fd(),
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            };
            assert_eq!(result, 0, "owned holder was not alive before release");
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let mut status = 0;
                // SAFETY: exact fixture child, adopted by this private subreaper.
                let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                if result == pid {
                    break;
                }
                // ECHILD is permitted only before production reaps the direct
                // parent, when adoption has not yet occurred.
                assert!(
                    result == 0
                        || (result == -1
                            && std::io::Error::last_os_error().raw_os_error()
                                == Some(libc::ECHILD))
                );
                assert!(
                    Instant::now() < deadline,
                    "fixture descendant was not adopted/reaped"
                );
                std::thread::yield_now();
            }
        });
        (
            Self {
                release: Some(release),
                guard: Some(guard),
                released,
            },
            ready_rx,
        )
    }

    fn release(mut self) {
        let _ = self.release.take().unwrap().send(());
        self.guard
            .take()
            .unwrap()
            .join()
            .expect("holder signalled and reaped");
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(guard) = self.guard.take() {
            let _ = guard.join();
        }
    }
}

#[test]
fn production_reply_pressure_keeps_renderer_serviceable_and_retires_before_slave_holder() {
    const KEY: &str = "NEOMACS_PRIVATE_REPLY_PRESSURE_CHILD";
    if std::env::var_os(KEY).is_none() {
        // Process-isolate PR_SET_CHILD_SUBREAPER, independent of libtest/Nextest
        // parallelism. Reject zero-selected-test success, not just exit zero.
        let name = format!(
            "{}::production_reply_pressure_keeps_renderer_serviceable_and_retires_before_slave_holder",
            module_path!().split_once("::").unwrap().1
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--color", "never"])
            .env(KEY, "1")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{text}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains(&format!("test {name} ... ok")), "{text}");
        let summary = text
            .lines()
            .find(|line| line.starts_with("test result:"))
            .unwrap();
        assert!(
            summary.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;"),
            "{summary}"
        );
        return;
    }
    // SAFETY: this is an isolated one-test process, never the user's editor.
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let root = tempfile::tempdir().unwrap();
    let (holder, holder_ready) = Holder::new(root.path().into());
    let script = r#"
import os, pathlib, signal, sys, termios, time, tty
root = pathlib.Path(sys.argv[1])
tty.setraw(0)
signal.signal(signal.SIGHUP, signal.SIG_IGN)
holder = os.fork()
if holder == 0:
    os.setsid()
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    (root / 'holder.tmp').write_text(str(os.getpid()))
    (root / 'holder.tmp').rename(root / 'holder')
    while True: time.sleep(1)
while not (root / 'holder').exists(): time.sleep(.001)
for batch in range(1024):
    while not (root / ('go-' + str(batch))).exists():
        if (root / 'EXIT').exists(): os._exit(17)
        time.sleep(.001)
    # Raw mode does not map LF to CRLF. Reset the column so a progress marker
    # cannot wrap across rows and masquerade as a stalled parser at batch 12.
    os.write(1, b'\x1b[6n' * 64 + ('BATCH' + str(batch) + '\r\n').encode())
while not (root / 'EXIT').exists(): time.sleep(.001)
os._exit(17)
"#;
    let id = TerminalId::new(1806).unwrap();
    let mut app = make_test_app();
    let request = neovm_core::emacs_core::display_host::TerminalInvocation {
        executable: "/usr/bin/python3".into(),
        argv: vec![
            "-c".into(),
            script.into(),
            root.path().to_str().unwrap().into(),
        ],
        directory: root.path().to_str().unwrap().into(),
        environment: vec!["SHELL=/bin/sh".into(), "TERM=xterm-256color".into()],
    };
    let reservation = app.shared_terminals.reserve(id).unwrap();
    app.handle_terminal(TerminalCommand::TerminalCreate {
        id,
        size: TerminalGridSize::new(80, 5).unwrap(),
        target: TerminalDisplayTarget::Floating,
        shell: None,
        invocation: Some(request),
    });
    reservation.commit();
    let view = app.terminal_manager.get(id).expect("production create");
    let proxy = view.event_proxy.clone();
    let parent_pid = view.child_process_id().unwrap();
    let holder_pid = holder_ready
        .recv_timeout(Duration::from_secs(10))
        .expect("native holder handshake");
    until(
        || worker_exists("neo-term-1806-pty") && worker_exists("neo-term-1806-wait"),
        "workers must actually start",
    );
    let mut pressured = false;
    for batch in 0..1024 {
        std::fs::write(root.path().join(format!("go-{batch}")), b"go").unwrap();
        let marker = format!("BATCH{batch}");
        until(
            || {
                app.terminal_manager
                    .get(id)
                    .unwrap()
                    .get_visible_text()
                    .contains(&marker)
            },
            "real parser batch",
        );
        until(
            || {
                let (blocked, pending) = app.terminal_manager.get(id).unwrap().reply_pressure();
                blocked > 0 || pending == 0
            },
            "reply pump must progress or demonstrate EAGAIN",
        );
        if app.terminal_manager.get(id).unwrap().reply_pressure().0 > 0 {
            pressured = true;
            break;
        }
    }
    assert!(pressured, "no actual parser-reply write pressure");
    let start = Instant::now();
    app.handle_terminal(TerminalCommand::TerminalWrite {
        id,
        data: b"accepted input".to_vec(),
    });
    app.handle_terminal(TerminalCommand::TerminalSetFloat {
        id,
        placement: TerminalFloatPlacement::new(24.0, 48.0, 0.75).unwrap(),
    });
    assert_eq!(app.terminal_manager.get(id).unwrap().float_x, 24.0);
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "reply pressure blocked renderer commands"
    );
    // Let the direct child exit naturally; the independently retained descendant
    // keeps the slave open. Native status cannot be mistaken for reader EOF.
    std::fs::write(root.path().join("EXIT"), b"exit").unwrap();
    until(
        || !std::path::Path::new(&format!("/proc/{parent_pid}")).exists(),
        "production wait must reap direct child",
    );
    assert!(
        proxy.completion().is_none(),
        "slave holder should prevent natural drain"
    );
    let start = Instant::now();
    let destruction = app.shared_terminals.begin_destroy(id).unwrap();
    app.handle_terminal(TerminalCommand::TerminalDestroy { id });
    destruction.commit();
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "renderer joined cleanup"
    );
    until(
        || proxy.cleanup_complete(),
        "reader join AND master release before holder release",
    );
    until(
        || !worker_exists("neo-term-1806-pty") && !worker_exists("neo-term-1806-wait"),
        "worker retirement",
    );
    let completion = proxy.completion().unwrap();
    assert_eq!(completion.exit_code, Some(17));
    assert!(completion.wait_error.is_none());
    assert!(!completion.output_drained);
    assert_eq!(completion.read_error.as_deref(), Some("terminal cancelled"));
    assert!(
        !holder.released.load(std::sync::atomic::Ordering::Acquire),
        "failure watchdog rescued retirement"
    );
    assert!(
        std::path::Path::new(&format!("/proc/{holder_pid}")).exists(),
        "holder released before retirement oracle"
    );
    holder.release(); // signals/reaps only after every successful retirement assertion
}
