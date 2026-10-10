use super::*;

fn install_status_observer(ev: &mut Context) {
    ev.eval_str(
        r#"(progn
        (setq fair-continued-count 0 fair-terminal-count 0 fair-status-observation nil)
        (fset 'fair-status-sentinel (lambda (proc _event)
            (cond ((eq (process-status proc) 'run)
                   (setq fair-continued-count (1+ fair-continued-count)
                         fair-status-observation (list (length fair-output) (length fair-other))))
                  ((memq (process-status proc) '(exit signal closed))
                   (setq fair-terminal-count (1+ fair-terminal-count)))))))"#,
    )
    .unwrap();
}

#[test]
fn pending_run_notification_bounds_both_streams_and_publishes_once() {
    let mut ev = context();
    install_status_observer(&mut ev);
    let expected = "α€".repeat(100);
    let (owner, stdout_reads) = process(
        &mut ev,
        "fair-continued",
        expected.as_bytes().to_vec(),
        1,
        "fair-filter",
        false,
    );
    let (stderr, stderr_reads) = process(
        &mut ev,
        "fair-continued-stderr",
        vec![b'e'; 4096],
        1,
        "fair-other-filter",
        false,
    );
    let proc = ev.processes.get_mut(owner).unwrap();
    proc.stderrproc = Value::make_process(stderr);
    proc.sentinel = Value::symbol("fair-status-sentinel");
    proc.status = process_status_stop_value(19);
    ev.processes
        .get_mut(stderr)
        .unwrap()
        .live_io
        .stderr_pipe_owner = Some(owner);
    ev.processes
        .set_child_status_pending(owner, process_status_run_value());
    let result = ev
        .run_process_status_notification(owner, Some(owner))
        .unwrap();
    assert!(result.has_target_process_activity());
    assert_eq!(stdout_reads.load(Ordering::SeqCst), 16);
    assert_eq!(stderr_reads.load(Ordering::SeqCst), 16);
    assert_eq!(
        ev.eval_symbol("fair-continued-count").unwrap(),
        Value::fixnum(1)
    );
    assert!(!ev.processes.get(owner).unwrap().status_notify_pending);
    assert!(process_status_is_run(
        &ev.processes.get(owner).unwrap().status
    ));
    assert!(
        ev.processes
            .get(owner)
            .unwrap()
            .coding_state
            .carryover_len()
            > 0,
        "partial UTF-8 survives notification settlement"
    );
    for _ in 0..256 {
        ev.poll_process_output_for_ids(vec![owner, stderr], Some(owner), false)
            .unwrap();
    }
    assert_eq!(output(&mut ev, "fair-output"), expected.as_bytes());
    assert_eq!(output(&mut ev, "fair-other"), vec![b'e'; 4096]);
    assert_eq!(
        ev.eval_symbol("fair-continued-count").unwrap(),
        Value::fixnum(1)
    );
    assert_eq!(
        ev.eval_symbol("fair-terminal-count").unwrap(),
        Value::fixnum(0)
    );
}

#[cfg(unix)]
#[test]
fn external_sigcont_live_output_yields_to_input_timer_and_independent_pipe() {
    use std::io::Write;
    let mut ev = context();
    ev.set_variable("read-process-output-max", Value::fixnum(1));
    install_status_observer(&mut ev);
    ev.eval_str(r#"(progn
        (setq timer-list nil fair-resume-armed nil)
        (fset 'timer-event-handler (lambda (timer) (setq timer-list (delq timer timer-list)) (apply (aref timer 5) (aref timer 6))))
        (fset 'fair-resume-tick (lambda () (setq fair-tick-at (length fair-output))))
        (fset 'fair-filter (lambda (_proc text)
            (setq fair-output (concat fair-output text))
            (if fair-resume-armed nil (setq fair-resume-armed t timer-list (list fair-resume-timer))))))"#).unwrap();
    ev.set_variable(
        "fair-resume-timer",
        gnu_timer_before(Duration::from_millis(1), "fair-resume-tick"),
    );
    let stderr = builtin_make_pipe_process(
        &mut ev,
        vec![
            Value::keyword(":name"),
            Value::string("fair-real-stderr"),
            Value::keyword(":filter"),
            Value::symbol("fair-other-filter"),
        ],
    )
    .unwrap()
    .as_process_id()
    .unwrap();
    ev.set_variable("fair-real-stderr", Value::make_process(stderr));
    // Stop before producing. stdout is queued above budget before stderr is
    // published, which is our real-pipe handshake, then the child stays alive
    // waiting for owned stdin (not a timing-dependent exit/continuation race).
    let owner = ev.eval_str(r#"(make-process :name "fair-real-continued" :connection-type 'pipe
        :command '("/bin/sh" "-c" "kill -STOP $$; printf '%4096s' '' | tr ' ' x; printf '%4096s' '' | tr ' ' e >&2; read gate")
        :coding 'utf-8-unix :filter 'fair-filter :sentinel 'fair-status-sentinel :stderr fair-real-stderr)"#).unwrap().as_process_id().unwrap();
    // LiveProcessIo::drop uses ChildOwnership::terminate_and_reap on unwind,
    // including the stopped-child case; never kill a captured PID after reaping.
    let os_pid = ev.processes.get(owner).unwrap().os_pid.unwrap() as i32;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ev.processes.check_child_status_change(owner) {
        assert!(Instant::now() < deadline, "real child must stop");
        std::thread::yield_now();
    }
    ev.run_process_status_notification(owner, Some(owner))
        .unwrap();
    assert_eq!(
        ProcessStatusSymbol::from_status_value(ev.processes.get(owner).unwrap().status),
        Some(ProcessStatusSymbol::Stop)
    );
    assert_eq!(unsafe { libc::kill(os_pid, libc::SIGCONT) }, 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        ev.processes.check_child_status_change(owner);
        let events = ev
            .processes
            .wait_for_backend_events(
                Duration::from_millis(5),
                ProcessWaitBackendInterest::ProcessesOnly,
            )
            .unwrap();
        if ev.processes.get(owner).unwrap().status_notify_pending
            && events.has_ready_process(stderr)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Continued plus published stderr handshake required"
        );
    }
    assert!(process_status_is_run(
        &ev.processes.get(owner).unwrap().pending_status
    ));
    assert!(output(&mut ev, "fair-output").is_empty());
    let other = ready_pipe(&mut ev, "fair-independent", b"MARKER", "fair-other-filter");
    // Independent output has its own variable: keep associated stderr order.
    ev.eval_str("(setq fair-independent-output \"\")").unwrap();
    ev.processes.get_mut(other).unwrap().filter = ev.eval_str("(lambda (_p text) (setq fair-independent-output (concat fair-independent-output text)))").unwrap();
    let (tx, rx) = crossbeam_channel::unbounded();
    ev.input_rx = Some(rx);
    tx.send(crate::keyboard::InputEvent::KeyPress {
        key: crate::keyboard::KeyEvent::char('j'),
        emacs_frame_id: 0,
    })
    .unwrap();
    assert_eq!(
        ev.wait_for_command_input(Some(Instant::now() + Duration::from_secs(1)))
            .unwrap(),
        CommandInputWaitOutcome::InputPending
    );
    // The continued sentinel observes only the first notification visit.
    assert_eq!(
        ev.eval_symbol("fair-status-observation").unwrap(),
        Value::list(vec![Value::fixnum(16), Value::fixnum(16)])
    );
    // The initial wait notifies status, then performs an ordinary Poll: stdout
    // gets two live allowances, stderr three (notification, associated stdout,
    // and its own admitted pipe visit). Neither stream may drain all 4096 bytes.
    assert_eq!(output(&mut ev, "fair-output").len(), 32);
    assert_eq!(output(&mut ev, "fair-other").len(), 48);
    assert_eq!(output(&mut ev, "fair-independent-output"), b"MARKER");
    assert_eq!(
        ev.eval_symbol("fair-continued-count").unwrap(),
        Value::fixnum(1)
    );
    // The notification filter arms this timer after the first timer phase.
    // Its callback stays deferred through that pass's ordinary poll, then
    // observes both live allowances when the next wait services timers.
    assert_eq!(ev.eval_symbol("fair-tick-at").unwrap(), Value::NIL);
    ev.wait_for_command_input(Some(Instant::now() + Duration::from_secs(1)))
        .unwrap();
    assert_eq!(ev.eval_symbol("fair-tick-at").unwrap(), Value::fixnum(32));
    for _ in 0..256 {
        ev.poll_process_output_for_ids(vec![owner, stderr], Some(owner), false)
            .unwrap();
    }
    assert_eq!(output(&mut ev, "fair-output"), vec![b'x'; 4096]);
    assert_eq!(output(&mut ev, "fair-other"), vec![b'e'; 4096]);
    assert_eq!(
        ev.eval_symbol("fair-continued-count").unwrap(),
        Value::fixnum(1)
    );
    ev.processes
        .get_mut(owner)
        .unwrap()
        .live_io
        .child
        .stdin_mut()
        .unwrap()
        .write_all(b"done\n")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while ev.eval_symbol("fair-terminal-count").unwrap() != Value::fixnum(1) {
        ev.poll_process_output_for_ids(vec![owner, stderr], Some(owner), true)
            .unwrap();
        assert!(
            Instant::now() < deadline,
            "owned child must exit and notify"
        );
        std::thread::yield_now();
    }
    assert_eq!(output(&mut ev, "fair-output"), vec![b'x'; 4096]);
    assert_eq!(output(&mut ev, "fair-other"), vec![b'e'; 4096]);
    assert_eq!(
        ev.eval_symbol("fair-continued-count").unwrap(),
        Value::fixnum(1)
    );
    ev.processes.delete_process(other);
    ev.processes.delete_process(stderr);
}
