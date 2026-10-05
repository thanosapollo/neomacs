use super::*;
use neovm_core::emacs_core::display_host::TerminalInvocation;
use portable_pty::ExitStatus;

fn invocation(executable: &str, argv: &[&str], directory: &str) -> TerminalInvocation {
    TerminalInvocation {
        executable: executable.into(),
        argv: argv.iter().map(|arg| (*arg).into()).collect(),
        directory: directory.into(),
        environment: vec![
            "SHELL=/bin/sh".into(),
            "TERM=xterm-256color".into(),
            "NATIVE_LITERAL=α = ; $()".into(),
        ],
    }
}

fn settled(proxy: &NeomacsEventProxy) -> neovm_core::emacs_core::display_host::TerminalCompletion {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(completion) = proxy.completion() {
            return completion;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "native child/drain never settled"
        );
        std::thread::yield_now();
    }
}

#[test]
fn exact_command_builder_preserves_argv_cwd_and_first_environment_binding() {
    let mut request = invocation(
        "/bin/echo",
        &["", "space λ", "; $(touch NEVER)"],
        "/literal directory/",
    );
    request.environment = vec![
        "A=first=tail".into(),
        "A=second".into(),
        "B".into(),
        "B=forbidden".into(),
    ];
    let command = super::super::invocation::command(&request);
    assert_eq!(
        command.get_argv(),
        &vec!["/bin/echo", "", "space λ", "; $(touch NEVER)"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        command.get_cwd(),
        Some(&std::ffi::OsString::from("/literal directory/"))
    );
    assert_eq!(
        command.get_env("A"),
        Some(std::ffi::OsStr::new("first=tail"))
    );
    assert_eq!(command.get_env("B"), None);
    assert_eq!(command.iter_full_env_as_str().count(), 1);
}

#[test]
fn completion_requires_authoritative_wait_and_parser_drain_in_either_order() {
    let proxy = NeomacsEventProxy::new(TerminalId::new(801).unwrap());
    proxy.settle_output(Ok(()));
    assert!(proxy.peek_wakeup());
    assert!(proxy.completion().is_none(), "EOF is not exit");
    proxy.settlement.lock().child = Some(Ok(ExitStatus::with_exit_code(17)));
    assert_eq!(proxy.completion().unwrap().exit_code, Some(17));
    let proxy = NeomacsEventProxy::new(TerminalId::new(802).unwrap());
    proxy.settlement.lock().child = Some(Ok(ExitStatus::with_signal("native signal")));
    assert!(proxy.completion().is_none(), "wait is not drain");
    proxy.settle_output(Ok(()));
    let completion = proxy.completion().unwrap();
    assert_eq!(completion.exit_code, None);
    assert_eq!(completion.signal.as_deref(), Some("native signal"));
    assert!(completion.output_drained);
}

#[test]
fn completion_does_not_turn_wait_or_read_errors_into_exit_success() {
    let proxy = NeomacsEventProxy::new(TerminalId::new(803).unwrap());
    proxy.settlement.lock().child = Some(Err("wait failed".into()));
    proxy.settle_output(Err("read failed".into()));
    let completion = proxy.completion().unwrap();
    assert_eq!(completion.exit_code, None);
    assert!(!completion.output_drained);
    assert_eq!(completion.wait_error.as_deref(), Some("wait failed"));
    assert_eq!(completion.read_error.as_deref(), Some("read failed"));
}

#[test]
#[cfg(target_os = "linux")]
fn real_native_invocation_preserves_literals_environment_directory_and_final_output() {
    let directory = tempfile::tempdir().unwrap();
    let request = invocation(
        "/bin/sh",
        &[
            "-c",
            "printf 'ARGV:%s|%s\\nENV:%s\\n' \"$1\" \"$2\" \"$NATIVE_LITERAL\"; pwd; printf 'FINAL_NATIVE_MARKER\\n'; exit 17",
            "explicit-fixture",
            "space λ",
            "; $(touch NEVER)",
        ],
        directory.path().to_str().unwrap(),
    );
    let mut view = TerminalView::new_invocation(
        TerminalId::new(804).unwrap(),
        TerminalGridSize::new(160, 12).unwrap(),
        TerminalDisplayTarget::Floating,
        None,
        Some(&request),
    )
    .unwrap();
    let completion = settled(&view.event_proxy);
    assert_eq!(completion.exit_code, Some(17));
    assert!(completion.signal.is_none() && completion.wait_error.is_none());
    assert!(completion.output_drained && completion.read_error.is_none());
    let text = view.get_visible_text();
    assert!(text.contains("ARGV:space λ|; $(touch NEVER)"), "{text}");
    assert!(text.contains("ENV:α = ; $()"), "{text}");
    assert!(text.contains(directory.path().to_str().unwrap()), "{text}");
    assert!(text.contains("FINAL_NATIVE_MARKER"), "{text}");
    assert!(!directory.path().join("NEVER").exists());
    view.shutdown().unwrap();
}

#[test]
#[cfg(target_os = "linux")]
fn real_native_signal_is_not_fabricated_as_exit_code_or_eof_success() {
    let request = invocation(
        "/bin/sh",
        &["-c", "printf 'BEFORE_SIGNAL\\n'; kill -TERM $$"],
        "/",
    );
    let mut view = TerminalView::new_invocation(
        TerminalId::new(805).unwrap(),
        TerminalGridSize::new(80, 5).unwrap(),
        TerminalDisplayTarget::Floating,
        None,
        Some(&request),
    )
    .unwrap();
    let completion = settled(&view.event_proxy);
    assert_eq!(completion.exit_code, None);
    assert!(completion.signal.is_some());
    assert!(completion.output_drained);
    assert!(view.get_visible_text().contains("BEFORE_SIGNAL"));
    view.shutdown().unwrap();
}

#[test]
#[cfg(target_os = "linux")]
fn exact_native_spawn_refuses_missing_cwd_and_implicit_shell_environment() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let request = invocation("/bin/echo", &["must not run"], missing.to_str().unwrap());
    assert!(
        TerminalView::new_invocation(
            TerminalId::new(807).unwrap(),
            TerminalGridSize::new(80, 5).unwrap(),
            TerminalDisplayTarget::Floating,
            None,
            Some(&request)
        )
        .is_err()
    );
    let mut request = invocation("/bin/echo", &[], "/");
    for environment in [vec![], vec!["SHELL".into(), "SHELL=/bin/sh".into()]] {
        request.environment = environment;
        assert!(
            TerminalView::new_invocation(
                TerminalId::new(808).unwrap(),
                TerminalGridSize::new(80, 5).unwrap(),
                TerminalDisplayTarget::Floating,
                None,
                Some(&request)
            )
            .is_err()
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn production_cwd_pin_preserves_directory_inode_across_rename() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let initial = root.path().join("initial");
    let renamed = root.path().join("renamed");
    std::fs::create_dir(&initial).unwrap();
    let (file, pinned) =
        super::super::invocation::pin_directory(initial.to_str().unwrap()).unwrap();
    std::fs::rename(&initial, &renamed).unwrap();
    let before = file.metadata().unwrap();
    let after = std::fs::metadata(&pinned).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert!(after.is_dir());
    assert!(!initial.exists());
}

#[test]
#[cfg(target_os = "linux")]
fn native_cancel_transfers_owned_cleanup_without_renderer_wait_or_join() {
    let request = invocation(
        "/bin/sh",
        &["-c", "trap '' HUP; printf 'CANCEL_READY\\n'; exec sleep 60"],
        "/",
    );
    let mut view = TerminalView::new_invocation(
        TerminalId::new(806).unwrap(),
        TerminalGridSize::new(80, 5).unwrap(),
        TerminalDisplayTarget::Floating,
        None,
        Some(&request),
    )
    .unwrap();
    let pid = view.child_process_id().unwrap();
    assert!(wait_for_reader_thread("neo-term-806-pty"));
    assert!(wait_for_reader_thread("neo-term-806-wait"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !view.get_visible_text().contains("CANCEL_READY") {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let proxy = view.event_proxy.clone();
    let start = std::time::Instant::now();
    view.shutdown().unwrap();
    assert!(
        start.elapsed() < std::time::Duration::from_millis(100),
        "renderer waited for child cleanup"
    );
    view.shutdown().unwrap();
    let completion = settled(&proxy);
    assert!(
        !completion.output_drained,
        "cancellation is not successful drain"
    );
    assert!(!process_exists(pid), "owned child was not reaped");
    while reader_thread_exists("neo-term-806-pty") || reader_thread_exists("neo-term-806-wait") {
        assert!(
            std::time::Instant::now() < deadline,
            "owned worker was abandoned"
        );
        std::thread::yield_now();
    }
}
