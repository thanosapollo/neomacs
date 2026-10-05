use super::*;

#[path = "status_fairness.rs"]
mod status_fairness;
use std::io::{self, Read};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

// A returning, always-readable source until all scripted bytes are delivered.
// No OS child, PTY device, sleeps or timing-dependent producer scheduling.
struct ScriptedReader {
    bytes: Vec<u8>,
    cursor: usize,
    chunk: usize,
    consumed: Arc<AtomicUsize>,
    eof: bool,
}

impl Read for ScriptedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cursor == self.bytes.len() {
            return if self.eof {
                Ok(0)
            } else {
                Err(io::ErrorKind::WouldBlock.into())
            };
        }
        let count = buffer
            .len()
            .min(self.chunk)
            .min(self.bytes.len() - self.cursor);
        buffer[..count].copy_from_slice(&self.bytes[self.cursor..self.cursor + count]);
        self.cursor += count;
        self.consumed.store(self.cursor, Ordering::SeqCst);
        Ok(count)
    }
}

fn context() -> Context {
    // This bare Context has no Lisp `unless` macro; filters use its `if` expansion.
    crate::test_utils::init_test_tracing();
    let mut ev = Context::new();
    ev.eval_str(
        r#"(progn
        (setq process-adaptive-read-buffering nil
              read-process-output-max 8192
              fair-output "" fair-other "" fair-other-at nil fair-tick-at nil)
        (fset 'fair-filter (lambda (_proc text)
            (setq fair-output (concat fair-output text))))
        (fset 'fair-other-filter (lambda (_proc text)
            (setq fair-other-at (length fair-output)
                  fair-other (concat fair-other text)))))"#,
    )
    .expect("install returning filters");
    ev
}

fn process(
    ev: &mut Context,
    name: &str,
    bytes: Vec<u8>,
    chunk: usize,
    filter: &str,
    eof: bool,
) -> (ProcessId, Arc<AtomicUsize>) {
    ev.sync_process_read_config_from_visible_variables();
    let pid = ev.processes.create_process(
        name.into(),
        Value::NIL,
        String::new(),
        vec![],
        ProcessCodingSystems::gnu_make_process_initial(),
    );
    let consumed = Arc::new(AtomicUsize::new(0));
    let proc = ev.processes.get_mut(pid).expect("scripted process");
    proc.filter = Value::symbol(filter);
    proc.coding_decode = Value::symbol("utf-8-unix");
    proc.coding_encode = Value::symbol("utf-8-unix");
    proc.live_io.pty_reader = Some(Box::new(ScriptedReader {
        bytes,
        cursor: 0,
        chunk,
        consumed: consumed.clone(),
        eof,
    }));
    (pid, consumed)
}

fn output(ev: &mut Context, name: &str) -> Vec<u8> {
    ev.eval_symbol(name)
        .expect("output variable")
        .as_lisp_string()
        .expect("output string")
        .as_bytes()
        .to_vec()
}

// Shared waits discover OS-backed sources, not bare scripted PTY readers.
// Use a real pipe for the pending-input journeys, with all bytes published
// before queueing input. readmax=1 keeps the original 16-byte read oracle.
fn ready_pipe(ev: &mut Context, name: &str, bytes: &[u8], filter: &str) -> ProcessId {
    use std::io::Write;
    ev.set_variable("read-process-output-max", Value::fixnum(1));
    let pid = builtin_make_pipe_process(
        ev,
        vec![
            Value::keyword(":name"),
            Value::string(name),
            Value::keyword(":filter"),
            Value::symbol(filter),
        ],
    )
    .unwrap()
    .as_process_id()
    .unwrap();
    ev.processes
        .get_mut(pid)
        .unwrap()
        .live_io
        .module_pipe_writer
        .as_mut()
        .unwrap()
        .write_all(bytes)
        .unwrap();
    assert!(ev.processes.live_process_ids().contains(&pid));
    assert!(
        ev.processes
            .wait_for_backend_events(Duration::ZERO, ProcessWaitBackendInterest::ProcessesOnly)
            .unwrap()
            .has_ready_process(pid)
    );
    pid
}

#[test]
fn live_stream_yields_to_other_process_then_delivers_complete_ordered_output() {
    let mut ev = context();
    let expected: Vec<u8> = (0..4096).map(|n| b'a' + (n % 26) as u8).collect();
    let (a, consumed) = process(&mut ev, "fair-a", expected.clone(), 1, "fair-filter", false);
    let (b, _) = process(
        &mut ev,
        "fair-b",
        b"MARKER".to_vec(),
        6,
        "fair-other-filter",
        false,
    );
    let outcome = ev
        .poll_process_output_for_ids(vec![a, b], Some(a), false)
        .expect("service visit");
    assert!(outcome.has_target_process_activity());
    assert_eq!(
        consumed.load(Ordering::SeqCst),
        16,
        "a live source must yield after its read budget"
    );
    assert_eq!(
        ev.eval_symbol("fair-other-at").unwrap(),
        Value::fixnum(16),
        "other stream must progress before A finishes"
    );
    assert_eq!(output(&mut ev, "fair-other"), b"MARKER");
    for _ in 0..256 {
        ev.poll_process_output_for_ids(vec![a, b], Some(a), false)
            .expect("resume retained source");
    }
    assert_eq!(
        output(&mut ev, "fair-output"),
        expected,
        "every byte exactly once and in order"
    );
    assert_eq!(
        output(&mut ev, "fair-other"),
        b"MARKER",
        "marker exactly once"
    );
    assert!(ev.processes.get(a).unwrap().live_io.pty_reader.is_some());
}

#[test]
fn live_stream_byte_budget_and_split_character_state_survive_resumption() {
    let mut ev = context();
    let expected = vec![b'x'; 128 * 1024];
    let (pid, consumed) = process(
        &mut ev,
        "fair-bytes",
        expected.clone(),
        8192,
        "fair-filter",
        false,
    );
    ev.poll_process_output_for_ids(vec![pid], Some(pid), false)
        .unwrap();
    assert_eq!(
        consumed.load(Ordering::SeqCst),
        64 * 1024,
        "byte checkpoint must yield before the read count limit"
    );
    ev.poll_process_output_for_ids(vec![pid], Some(pid), false)
        .unwrap();
    assert_eq!(output(&mut ev, "fair-output"), expected);

    ev.set_variable("fair-output", Value::string(""));
    // 16 raw bytes end in the middle of a multibyte character. Raw input,
    // including reads producing no decoded text, must consume the allowance.
    let expected = "α€".repeat(100);
    let (pid, consumed) = process(
        &mut ev,
        "fair-unicode",
        expected.as_bytes().to_vec(),
        1,
        "fair-filter",
        false,
    );
    ev.poll_process_output_for_ids(vec![pid], Some(pid), false)
        .unwrap();
    assert_eq!(consumed.load(Ordering::SeqCst), 16);
    for _ in 0..32 {
        ev.poll_process_output_for_ids(vec![pid], Some(pid), false)
            .unwrap();
    }
    assert_eq!(output(&mut ev, "fair-output"), expected.as_bytes());
}

#[test]
fn newly_due_timer_and_command_input_progress_between_live_service_visits() {
    let mut ev = context();
    ev.set_variable(
        "fair-timer",
        gnu_timer_before(Duration::from_millis(1), "fair-timer-callback"),
    );
    ev.eval_str(
        r#"(progn
        (setq timer-list nil fair-timer-armed nil)
        (fset 'timer-event-handler (lambda (timer)
            (setq timer-list (delq timer timer-list))
            (apply (aref timer 5) (aref timer 6))))
        (fset 'fair-timer-callback (lambda () (setq fair-tick-at (length fair-output))))
        (fset 'fair-filter (lambda (_proc text)
            (setq fair-output (concat fair-output text))
            (if fair-timer-armed nil
                (setq fair-timer-armed t timer-list (list fair-timer))))))"#,
    )
    .unwrap();
    let pid = ready_pipe(
        &mut ev,
        "fair-timer-stream",
        &vec![b'x'; 4096],
        "fair-filter",
    );
    let (tx, rx) = crossbeam_channel::unbounded();
    ev.input_rx = Some(rx);
    tx.send(crate::keyboard::InputEvent::KeyPress {
        key: crate::keyboard::KeyEvent::char('j'),
        emacs_frame_id: 0,
    })
    .unwrap();

    // Timer becomes ripe during the filter, AFTER the scheduler's timer check.
    assert_eq!(
        ev.wait_for_command_input(Some(Instant::now() + Duration::from_secs(1)))
            .expect("first shared scheduler visit"),
        CommandInputWaitOutcome::InputPending
    );
    // For this one-byte ASCII pipe, delivered bytes equal consumed raw reads.
    assert_eq!(output(&mut ev, "fair-output").len(), 16);
    assert_eq!(ev.eval_symbol("fair-tick-at").unwrap(), Value::NIL);
    assert!(
        !ev.command_loop.keyboard.pending_input_events.is_empty(),
        "process output must return control to command input staging"
    );
    assert_eq!(
        ev.wait_for_command_input(Some(Instant::now() + Duration::from_secs(1)))
            .expect("next shared scheduler visit"),
        CommandInputWaitOutcome::InputPending
    );
    assert_eq!(
        ev.eval_symbol("fair-tick-at").unwrap(),
        Value::fixnum(16),
        "newly due timer must run before the stream is exhausted"
    );
    for _ in 0..256 {
        ev.poll_process_output_for_ids(vec![pid], Some(pid), false)
            .unwrap();
    }
    assert_eq!(output(&mut ev, "fair-output"), vec![b'x'; 4096]);
}

#[test]
fn associated_stderr_has_its_own_live_budget_and_keeps_target_accounting() {
    let mut ev = context();
    let (owner, _) = process(
        &mut ev,
        "fair-owner",
        b"OUT".to_vec(),
        3,
        "fair-filter",
        false,
    );
    let expected = vec![b'e'; 4096];
    let (stderr, consumed) = process(
        &mut ev,
        "fair-stderr",
        expected.clone(),
        1,
        "fair-other-filter",
        false,
    );
    ev.processes.get_mut(owner).unwrap().stderrproc = Value::make_process(stderr);
    ev.processes
        .get_mut(stderr)
        .unwrap()
        .live_io
        .stderr_pipe_owner = Some(owner);
    let outcome = ev
        .poll_process_output_for_ids(vec![owner], Some(owner), false)
        .unwrap();
    assert!(outcome.has_target_process_activity());
    assert_eq!(output(&mut ev, "fair-output"), b"OUT");
    assert_eq!(
        consumed.load(Ordering::SeqCst),
        16,
        "associated stderr must not replace stdout starvation with stderr starvation"
    );
    let outcome = ev
        .poll_process_output_for_ids(vec![stderr], Some(owner), false)
        .unwrap();
    assert!(outcome.has_any_process_activity());
    assert!(
        !outcome.has_target_process_activity(),
        "stderr bytes do not count as owner stdout"
    );
    for _ in 0..256 {
        ev.poll_process_output_for_ids(vec![stderr], Some(owner), false)
            .unwrap();
    }
    assert_eq!(output(&mut ev, "fair-other"), expected);
}

#[test]
fn deferred_stdout_live_drain_is_bounded_and_resumable() {
    let mut ev = context();
    let (pid, consumed) = process(
        &mut ev,
        "fair-deferred",
        vec![b'x'; 4096],
        1,
        "fair-filter",
        false,
    );
    let (outcome, disposition) = ev
        .poll_process_stdout_output_without_status_detailed(pid, Some(pid))
        .unwrap();
    assert!(outcome.has_target_process_activity());
    assert_eq!(disposition, ProcessOutputDrainDisposition::Output);
    assert_eq!(consumed.load(Ordering::SeqCst), 16);
    for _ in 0..256 {
        ev.poll_process_stdout_output_without_status_detailed(pid, Some(pid))
            .unwrap();
    }
    assert_eq!(output(&mut ev, "fair-output"), vec![b'x'; 4096]);
}

#[test]
fn terminal_notification_drains_all_streams_before_sentinel_even_over_live_budget() {
    let mut ev = context();
    ev.eval_str(
        r#"(progn
        (setq fair-sentinel-observation nil fair-sentinel-count 0)
        (fset 'fair-sentinel (lambda (_proc _event)
            (setq fair-sentinel-count (1+ fair-sentinel-count))
            (setq fair-sentinel-observation (list (length fair-output) (length fair-other))))))"#,
    )
    .unwrap();
    let (owner, _) = process(
        &mut ev,
        "fair-exit",
        vec![b'o'; 4096],
        1,
        "fair-filter",
        true,
    );
    let (stderr, _) = process(
        &mut ev,
        "fair-exit-stderr",
        vec![b'e'; 4096],
        1,
        "fair-other-filter",
        true,
    );
    ev.processes.get_mut(owner).unwrap().stderrproc = Value::make_process(stderr);
    ev.processes.get_mut(owner).unwrap().sentinel = Value::symbol("fair-sentinel");
    ev.processes
        .get_mut(stderr)
        .unwrap()
        .live_io
        .stderr_pipe_owner = Some(owner);
    ev.processes
        .set_child_status_pending(owner, process_status_exit_value(0));
    let outcome = ev
        .run_process_status_notification(owner, Some(owner))
        .unwrap();
    assert!(outcome.has_target_process_activity());
    assert_eq!(output(&mut ev, "fair-output"), vec![b'o'; 4096]);
    assert_eq!(output(&mut ev, "fair-other"), vec![b'e'; 4096]);
    assert_eq!(
        ev.eval_symbol("fair-sentinel-observation").unwrap(),
        Value::list(vec![Value::fixnum(4096), Value::fixnum(4096)])
    );
    ev.poll_process_output_for_ids(vec![owner, stderr], Some(owner), false)
        .unwrap();
    assert_eq!(output(&mut ev, "fair-output"), vec![b'o'; 4096]);
    assert_eq!(output(&mut ev, "fair-other"), vec![b'e'; 4096]);
    assert_eq!(
        ev.eval_symbol("fair-sentinel-count").unwrap(),
        Value::fixnum(1)
    );
}

#[cfg(unix)]
#[test]
fn unread_pipe_bytes_remain_level_ready_after_a_budgeted_service_visit() {
    use std::io::Write;
    let mut ev = context();
    ev.set_variable("read-process-output-max", Value::fixnum(1));
    let pid = builtin_make_pipe_process(
        &mut ev,
        vec![
            Value::keyword(":name"),
            Value::string("fair-level-pipe"),
            Value::keyword(":filter"),
            Value::symbol("fair-filter"),
        ],
    )
    .unwrap()
    .as_process_id()
    .unwrap();
    let expected: Vec<u8> = (0..64).map(|n| b'a' + (n % 26) as u8).collect();
    ev.processes
        .get_mut(pid)
        .unwrap()
        .live_io
        .module_pipe_writer
        .as_mut()
        .unwrap()
        .write_all(&expected)
        .unwrap();
    let request = ProcessOutputServiceRequest::target_only(pid);
    for _ in 0..4 {
        let events = ev
            .processes
            .wait_for_backend_events(Duration::ZERO, ProcessWaitBackendInterest::ProcessesOnly)
            .expect("registered pipe backend");
        assert!(
            events.has_ready_process(pid),
            "unread bytes must retain readiness without a new write"
        );
        ev.poll_ready_process_output_for_service_request(events, &request)
            .unwrap();
    }
    assert_eq!(output(&mut ev, "fair-output"), expected);
    let outcome = ev
        .poll_process_output_for_service_request(&request)
        .unwrap();
    assert!(!outcome.has_any_process_activity());
    assert_eq!(output(&mut ev, "fair-output"), expected);
}

#[test]
fn finite_response_is_delivered_before_pending_input_without_spurious_replay() {
    let mut ev = context();
    let pid = ready_pipe(&mut ev, "fair-finite", b"READY", "fair-filter");
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
    assert_eq!(output(&mut ev, "fair-output"), b"READY");
    assert_eq!(
        ev.wait_for_command_input(Some(Instant::now() + Duration::from_secs(1)))
            .unwrap(),
        CommandInputWaitOutcome::InputPending
    );
    assert_eq!(output(&mut ev, "fair-output"), b"READY");
    let outcome = ev
        .poll_process_output_for_ids(vec![pid], Some(pid), false)
        .unwrap();
    assert!(!outcome.has_any_process_activity());
}
