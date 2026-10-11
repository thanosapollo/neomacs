use std::{path::PathBuf, process::Command};

#[test]
fn batch_q_load_path_has_no_duplicate_bundled_subdirectories() {
    let home = tempfile::tempdir().expect("disposable startup home");
    let root = neomacs_infra::crate_root!().join("../..");
    let binary = std::env::var_os("NEOMACS_GUI_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_neomacs")));
    let output = Command::new(binary)
        .current_dir(&root)
        .env("NEOMACS_RUNTIME_ROOT", &root)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_STATE_HOME", home.path().join("state"))
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env("XDG_RUNTIME_DIR", home.path().join("run"))
        .env("RUST_LOG", "off")
        .env_remove("EMACSLOADPATH")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env_remove("NEOMACS_DIAGNOSTICS_PORT")
        .args([
            "-Q", "--batch", "--eval",
            "(let ((seen nil) (duplicates 0)) (dolist (p load-path) (if (member p seen) (setq duplicates (1+ duplicates)) (push p seen))) (if (> duplicates 0) (error \"Duplicate startup load-path entries: %d\" duplicates)) (if (< (length load-path) 2) (error \"Startup did not expand subdirs.el\")) (if (not (locate-library \"ntlm\")) (error \"Bundled subdirectory library unavailable\")) (princ \"startup-load-path-expanded-once\"))",
        ])
        .output()
        .expect("run clean batch startup");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "startup-load-path-expanded-once"
    );
}

#[test]
#[ignore = "requires release executable with matching pdump"]
fn batch_error_exits_nonzero_and_reports_only_to_stderr() {
    let root = neomacs_infra::crate_root!().join("../..");
    let binary = std::env::var_os("NEOMACS_GUI_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/release/neomacs"));
    let output = Command::new(binary)
        .current_dir(&root)
        .env("RUST_LOG", "off")
        .args([
            "-Q", "--batch", "--eval",
            "(add-hook 'kill-emacs-hook (lambda () (princ (if (and (get-buffer \"*Messages*\") (with-current-buffer \"*Messages*\" (string-match-p \"batch-startup-probe-error\" (buffer-string)))) \"error-was-logged\" \"error-not-logged\"))))",
            "--eval", "(error \"batch-startup-probe-error\")",
        ])
        .output()
        .expect("run batch startup");
    assert_eq!(output.status.code(), Some(255), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .lines()
            .any(|line| line == "batch-startup-probe-error"),
        "{stderr}"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "error-not-logged");
}
