use super::*;
use std::fs::{self, DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "neomacs-crash-test-{}-{}",
            std::process::id(),
            timestamp()
        ));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn state_path_uses_absolute_xdg_or_home_only() {
    let home = Some(PathBuf::from("/home/example"));
    assert_eq!(
        state_directory(Some("/state".into()), home.clone()),
        Some("/state/neomacs/crashes".into())
    );
    for invalid in ["", "relative"] {
        assert_eq!(
            state_directory(Some(invalid.into()), home.clone()),
            Some("/home/example/.local/state/neomacs/crashes".into())
        );
    }
    assert!(state_directory(None, Some("relative".into())).is_none());
    assert!(state_directory(None, None).is_none());
}

#[test]
fn destination_is_private_exclusive_and_never_follows_links() {
    let dir = Directory::new();
    let path = dir.0.join("state/neomacs/crashes");
    let destination = unix::Destination::new(&path, 123, 456).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        fs::read_dir(&path).unwrap().count(),
        0,
        "no empty startup report"
    );
    let file = destination.create().unwrap();
    assert_eq!(file.metadata().unwrap().mode() & 0o777, 0o600);
    assert!(
        destination.create().is_err(),
        "never overwrite an existing report"
    );

    let victim = dir.0.join("user-file");
    fs::write(&victim, "untouched").unwrap();
    let collision = unix::Destination::new(&path, 123, 789).unwrap();
    symlink(&victim, collision.path()).unwrap();
    assert!(collision.create().is_err());
    assert_eq!(fs::read_to_string(victim).unwrap(), "untouched");
    let link = dir.0.join("linked-state");
    symlink(dir.0.join("state"), &link).unwrap();
    assert!(unix::Destination::new(&link.join("neomacs/crashes"), 123, 456).is_err());
}

#[test]
fn unsafe_permissions_and_parent_traversal_are_refused() {
    let dir = Directory::new();
    let path = dir.0.join("unsafe");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(unix::Destination::new(&path.join("crashes"), 1, 2).is_err());
    assert!(!path.join("crashes").exists());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        unix::Destination::new(&path, 1, 2).is_err(),
        "final directory must be private"
    );
    assert!(unix::Destination::new(&dir.0.join("../escape"), 1, 2).is_err());
    assert!(unix::Destination::new(Path::new("relative"), 1, 2).is_err());
}

#[test]
fn renamed_directory_remains_pinned_by_descriptor() {
    let dir = Directory::new();
    let original = dir.0.join("crashes");
    let destination = unix::Destination::new(&original, 1, 2).unwrap();
    let retained = dir.0.join("retained");
    fs::rename(&original, &retained).unwrap();
    let victim = dir.0.join("victim");
    fs::create_dir(&victim).unwrap();
    symlink(&victim, &original).unwrap();
    destination
        .create()
        .unwrap()
        .write_all(b"owned report")
        .unwrap();
    assert_eq!(fs::read_dir(victim).unwrap().count(), 0);
    assert_eq!(
        fs::read(retained.join("panic-2-1.txt")).unwrap(),
        b"owned report"
    );
}

#[test]
fn report_size_is_bounded_without_pruning_files() {
    let dir = Directory::new();
    let path = dir.0.join("report");
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let mut report = LimitedReport::new(file);
    assert!(report.write_all(&vec![b'x'; MAX_REPORT_BYTES * 2]).is_err());
    report.finish().unwrap();
    report.finish().unwrap();
    let contents = fs::read(path).unwrap();
    assert_eq!(contents.len(), MAX_REPORT_BYTES);
    assert!(contents.ends_with(LIMIT_MARKER));
}

const CHILD: &str = "NEOMACS_CRASH_REPORT_TEST_CHILD";
const PRIVATE_PAYLOAD: &str = "SECRET-BUFFER-CREDENTIAL-SENTINEL";

/// The same fixture works both under Cargo and a standalone source-import test.
fn child(root: &Path, xdg: &Path, mode: &str) -> (u32, String) {
    let identity = format!(
        "{}::child_process_reports_and_failure_paths",
        module_path!()
    );
    let (_, identity) = identity.split_once("::").unwrap();
    let stderr = root.join(format!("stderr-{mode}"));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", identity, "--nocapture"])
        .env(CHILD, mode)
        .env("XDG_STATE_HOME", xdg)
        .env("HOME", root.join("unused-home"))
        .env("RUST_BACKTRACE", "0")
        .env("SECRET_ENV_SENTINEL", PRIVATE_PAYLOAD)
        .stdout(Stdio::null())
        .stderr(fs::File::create(&stderr).unwrap())
        .spawn()
        .unwrap();
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("panic child did not finish");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        status.code(),
        Some(if mode == "hook-exit" { 77 } else { 101 }),
        "child must really panic, not pass with zero selected tests"
    );
    let stderr = fs::read_to_string(stderr).unwrap();
    assert_eq!(
        stderr.matches("PREVIOUS-HOOK-CALLED").count(),
        match mode {
            "repeat" => 2,
            "limit" => 257,
            _ => 1,
        }
    );
    (pid, stderr)
}

#[test]
fn child_process_reports_and_failure_paths() {
    if let Ok(mode) = std::env::var(CHILD) {
        let hook_exit = mode == "hook-exit";
        std::panic::set_hook(Box::new(move |_| {
            let _ = writeln!(io::stderr().lock(), "PREVIOUS-HOOK-CALLED");
            if hook_exit {
                std::process::exit(77);
            }
        }));
        install(
            if mode == "repeat" {
                "gui"
            } else {
                "tty/batch/daemon"
            },
            "Git commit: fixture-source\nBuild: fixture-profile",
        );
        let caught = match mode.as_str() {
            "repeat" => 1,
            "limit" => 256,
            _ => 0,
        };
        for _ in 0..caught {
            let _ = std::panic::catch_unwind(|| panic!("{PRIVATE_PAYLOAD}"));
        }
        let worker = std::thread::Builder::new()
            .name(PRIVATE_PAYLOAD.into())
            .spawn(|| panic!("{PRIVATE_PAYLOAD}"))
            .unwrap();
        std::panic::resume_unwind(worker.join().unwrap_err());
    }
    let root = Directory::new();
    let state = root.0.join("state");
    let before = timestamp();
    let first = child(&root.0, &state, "repeat");
    let second = child(&root.0, &state, "single");
    let after = timestamp();
    assert_ne!(first.0, second.0);
    let directory = state.join("neomacs/crashes");
    let mut files: Vec<_> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "one distinct report per instance");
    for (pid, stderr) in [first, second] {
        let path = files
            .iter()
            .find(|path| path.to_string_lossy().ends_with(&format!("-{pid}.txt")))
            .unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert!(stderr.contains(&format!("Neomacs crash report: {}", path.display())));
        assert!(text.contains(&format!("pid: {pid}\n")));
        for expected in [
            "fixture-source",
            "fixture-profile",
            "executable:",
            "thread: ThreadId(",
            "tests.rs",
            "backtrace:",
            "child_process_reports_and_failure_paths",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert!(!text.contains(PRIVATE_PAYLOAD));
        assert!(!text.contains("SECRET_ENV_SENTINEL"));
        let times: Vec<_> = text
            .lines()
            .filter_map(|line| line.strip_prefix("unix-ns: "))
            .map(|n| n.parse::<u128>().unwrap())
            .collect();
        assert_eq!(
            times.len(),
            if text.contains("logging-target: gui") {
                2
            } else {
                1
            }
        );
        assert!(times.iter().all(|time| (before..=after).contains(time)));
        assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
    }

    for mode in ["hook-exit", "limit"] {
        let (pid, _) = child(&root.0, &state, mode);
        let file = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.to_string_lossy().ends_with(&format!("-{pid}.txt")))
            .unwrap();
        let contents = fs::read(&file).unwrap();
        if mode == "limit" {
            assert_eq!(contents.len(), MAX_REPORT_BYTES);
            assert!(contents.ends_with(LIMIT_MARKER));
        } else {
            let text = String::from_utf8(contents).unwrap();
            assert!(
                text.contains("=== PANIC ===") && text.contains("backtrace:"),
                "report must precede previous hook exit"
            );
        }
    }

    let victim = root.0.join("victim");
    fs::create_dir(&victim).unwrap();
    let unsafe_state = root.0.join("unsafe-state");
    symlink(&victim, &unsafe_state).unwrap();
    let (_, stderr) = child(&root.0, &unsafe_state, "unsafe");
    assert!(stderr.contains("crash report unavailable"));
    assert_eq!(fs::read_dir(&victim).unwrap().count(), 0);
    assert!(
        !root.0.join("unused-home").exists(),
        "do not silently fall back after unsafe configured path"
    );

    // Root can bypass DAC; the normal unprivileged test proves real EACCES.
    if unsafe { libc::geteuid() } != 0 {
        let unwritable = root.0.join("unwritable");
        DirBuilder::new().mode(0o500).create(&unwritable).unwrap();
        let (_, stderr) = child(&root.0, &unwritable, "unwritable");
        fs::set_permissions(&unwritable, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(stderr.contains("crash report unavailable"));
        assert_eq!(fs::read_dir(unwritable).unwrap().count(), 0);
    }
}
