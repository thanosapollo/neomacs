use super::*;

#[test]
fn native_spawn_boundary_captures_exact_invocation_and_current_buffer() {
    let host = RecordingTerminalDisplayHost::default();
    let mut eval = Context::new();
    let owner = eval.buffers.current_buffer_id().unwrap();
    eval.set_display_host(Box::new(host.clone()));
    eval.eval_str(r#"(neomacs-terminal-spawn 80 24 "/bin/echo" '("" "space λ" "; $()") "/literal directory/" '("SHELL=/bin/sh" "A=first=tail" "A=second" "UNSET"))"#).unwrap();
    let events = host.events.lock().unwrap();
    let TerminalHostEvent::Create(request) = &events[0] else {
        panic!("expected invocation");
    };
    assert_eq!(events.len(), 1);
    assert_eq!(
        request.target,
        TerminalDisplayTarget::Window { buffer: owner }
    );
    assert_eq!(request.shell, None);
    let invocation = request.invocation.as_ref().unwrap();
    assert_eq!(invocation.executable, "/bin/echo");
    assert_eq!(invocation.argv, ["", "space λ", "; $()"]);
    assert_eq!(invocation.directory, "/literal directory/");
    assert_eq!(
        invocation.environment,
        ["SHELL=/bin/sh", "A=first=tail", "A=second", "UNSET"]
    );
}

#[test]
fn native_cwd_capability_refuses_unsupported_inputs_without_terminal_admission() {
    let host = RecordingTerminalDisplayHost::default();
    let mut eval = Context::new();
    eval.set_display_host(Box::new(host.clone()));
    let temporary = tempfile::tempdir().unwrap();
    let absent = temporary.path().join("missing-directory");
    let file = temporary.path().join("regular-file");
    std::fs::write(&file, b"not a directory").unwrap();
    for form in [
        r#"(neomacs-terminal-cwd-pinning-p nil)"#.to_owned(),
        r#"(neomacs-terminal-cwd-pinning-p "relative")"#.to_owned(),
        r#"(neomacs-terminal-cwd-pinning-p "/ssh:host:/")"#.to_owned(),
        r#"(neomacs-terminal-cwd-pinning-p "/safe/../")"#.to_owned(),
        r#"(neomacs-terminal-cwd-pinning-p (string 0))"#.to_owned(),
        format!(
            "(neomacs-terminal-cwd-pinning-p {:?})",
            absent.to_str().unwrap()
        ),
        format!(
            "(neomacs-terminal-cwd-pinning-p {:?})",
            file.to_str().unwrap()
        ),
    ] {
        assert_eq!(eval.eval_str(&form).unwrap(), Value::NIL, "{form}");
    }
    assert!(host.events.lock().unwrap().is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn native_cwd_capability_supported_procfs_control_is_nonspawning() {
    let host = RecordingTerminalDisplayHost::default();
    let mut eval = Context::new();
    eval.set_display_host(Box::new(host.clone()));
    // This positive control deliberately requires usable Linux procfs.
    // Unavailable procfs is not silently counted as a passing positive.
    assert!(std::fs::metadata("/proc/self/fd").unwrap().is_dir());
    assert_eq!(
        eval.eval_str(r#"(neomacs-terminal-cwd-pinning-p "/")"#)
            .unwrap(),
        Value::T
    );
    assert!(host.events.lock().unwrap().is_empty());
    let mut no_host = Context::new();
    assert_eq!(
        no_host
            .eval_str(r#"(neomacs-terminal-cwd-pinning-p "/")"#)
            .unwrap(),
        Value::T
    );
}

#[test]
fn native_spawn_refuses_invalid_invocations_before_host_admission() {
    let host = RecordingTerminalDisplayHost::default();
    let mut eval = Context::new();
    eval.set_display_host(Box::new(host.clone()));
    for form in [
        r#"(neomacs-terminal-spawn 80 24 "echo" nil "/" '("SHELL=/bin/sh"))"#,
        r#"(neomacs-terminal-spawn 80 24 "/bin/echo" nil "/ssh:host:/" '("SHELL=/bin/sh"))"#,
        r#"(neomacs-terminal-spawn 80 24 "/bin/echo" nil "/safe/../ssh:host:/" '("SHELL=/bin/sh"))"#,
        r#"(neomacs-terminal-spawn 80 24 "/bin/echo" '(1) "/" '("SHELL=/bin/sh"))"#,
        r#"(neomacs-terminal-spawn 80 24 "/bin/echo" '("a" . "b") "/" '("SHELL=/bin/sh"))"#,
        r#"(neomacs-terminal-spawn 80 24 "/bin/echo" (list (string 0)) "/" '("SHELL=/bin/sh"))"#,
        r#"(neomacs-terminal-spawn 80 24 "/bin/echo" nil "/" '("SHELL=/bin/sh" "=bad"))"#,
        r#"(neomacs-terminal-spawn 80 24 "/bin/echo" nil "/" nil)"#,
        r#"(neomacs-terminal-spawn 80 24 "/bin/echo" nil "/" '("SHELL" "SHELL=/bin/sh"))"#,
        r#"(let ((argv (list "a"))) (setcdr argv argv) (neomacs-terminal-spawn 80 24 "/bin/echo" argv "/" '("SHELL=/bin/sh")))"#,
    ] {
        assert_eq!(
            eval.eval_str(&format!(
                "(condition-case nil (progn {form} 'unexpected) (error 'refused))"
            ))
            .unwrap(),
            Value::symbol("refused"),
            "{form}"
        );
    }
    assert!(host.events.lock().unwrap().is_empty());
}
