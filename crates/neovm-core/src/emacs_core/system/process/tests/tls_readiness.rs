use super::*;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime, pem::PemObject};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error, ServerConfig, ServerConnection,
    SignatureScheme,
};
use std::io::{Cursor, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
#[derive(Debug)]
struct AcceptAnyServerCertificate;

impl ServerCertVerifier for AcceptAnyServerCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
        ]
    }
}
// Real encrypted application data traverses a TCP socket. Handshake records
// use the established repository fixture's in-memory exchange; the peer then
// writes exactly one response and remains owned/open throughout every wait.
fn tls_peer(expected: &[u8]) -> (TlsStream, TcpStream) {
    tls_peer_with_close_notify(expected, false)
}

fn tls_peer_with_close_notify(expected: &[u8], close_notify: bool) -> (TlsStream, TcpStream) {
    let certificate = CertificateDer::from_pem_slice(include_bytes!(concat!(
        env!("CARGO_WORKSPACE_DIR"),
        "/test/lisp/net/network-stream-resources/cert.pem"
    )))
    .expect("test certificate");
    let private_key = PrivateKeyDer::from_pem_slice(include_bytes!(concat!(
        env!("CARGO_WORKSPACE_DIR"),
        "/test/lisp/net/network-stream-resources/key.pem"
    )))
    .expect("test private key");
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate], private_key)
        .expect("server certificate and key");
    let client_config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCertificate))
        .with_no_client_auth();
    let server_name = ServerName::try_from("localhost").expect("valid server name");
    let mut connection =
        ClientConnection::new(Arc::new(client_config), server_name).expect("TLS client connection");
    let mut server_connection =
        ServerConnection::new(Arc::new(server_config)).expect("TLS server connection");

    for _ in 0..10 {
        let mut client_wire = Vec::new();
        connection
            .write_tls(&mut client_wire)
            .expect("write client handshake records");
        if !client_wire.is_empty() {
            server_connection
                .read_tls(&mut Cursor::new(client_wire))
                .expect("read client handshake records");
            server_connection
                .process_new_packets()
                .expect("process client handshake records");
        }

        let mut server_wire = Vec::new();
        server_connection
            .write_tls(&mut server_wire)
            .expect("write server handshake records");
        if !server_wire.is_empty() {
            connection
                .read_tls(&mut Cursor::new(server_wire))
                .expect("read server handshake records");
            connection
                .process_new_packets()
                .expect("process server handshake records");
        }
        if !connection.is_handshaking() && !server_connection.is_handshaking() {
            break;
        }
    }
    assert!(
        !connection.is_handshaking(),
        "client handshake should finish"
    );
    assert!(
        !server_connection.is_handshaking(),
        "server handshake should finish"
    );
    // Flush post-handshake records (for example TLS 1.3 tickets and the
    // client's Finished) before constructing the application-data scenario.
    for _ in 0..4 {
        let mut client_wire = Vec::new();
        connection
            .write_tls(&mut client_wire)
            .expect("flush client post-handshake records");
        if !client_wire.is_empty() {
            server_connection
                .read_tls(&mut Cursor::new(client_wire))
                .expect("read client post-handshake records");
            server_connection
                .process_new_packets()
                .expect("process client post-handshake records");
        }

        let mut server_wire = Vec::new();
        server_connection
            .write_tls(&mut server_wire)
            .expect("flush server post-handshake records");
        if !server_wire.is_empty() {
            connection
                .read_tls(&mut Cursor::new(server_wire))
                .expect("read server post-handshake records");
            connection
                .process_new_packets()
                .expect("process server post-handshake records");
        }
        if !connection.wants_write() && !server_connection.wants_write() {
            break;
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    server_connection.writer().write_all(expected).unwrap();
    if close_notify {
        server_connection.send_close_notify();
    }
    let mut wire = Vec::new();
    server_connection.write_tls(&mut wire).unwrap();
    peer.write_all(&wire).unwrap();
    socket.set_nonblocking(true).unwrap();
    (TlsStream::from_test_connection(connection, socket), peer)
}

fn prepare() -> (Context, ProcessId, TcpStream, Vec<u8>) {
    crate::test_utils::init_test_tracing();
    let mut ev = Context::new();
    // This bare Context has no Lisp `unless` macro; filters use its `if` expansion.
    ev.eval_str(
        r#"(progn
        (setq process-adaptive-read-buffering nil read-process-output-max 1
              tls-output "" tls-other "" tls-other-at nil tls-tick-at nil timer-list nil tls-armed nil)
        (fset 'timer-event-handler (lambda (timer)
            (setq timer-list (delq timer timer-list))
            (apply (aref timer 5) (aref timer 6))))
        (fset 'tls-tick (lambda () (setq tls-tick-at (length tls-output))))
        (fset 'tls-filter (lambda (_proc text)
            (setq tls-output (concat tls-output text))
            (if tls-armed nil (setq tls-armed t timer-list (list tls-timer)))))
        (fset 'tls-other-filter (lambda (_proc text)
            (setq tls-other-at (length tls-output)
                  tls-other (concat tls-other text)))))"#,
    )
    .unwrap();
    ev.set_variable(
        "tls-timer",
        gnu_timer_before(Duration::from_millis(1), "tls-tick"),
    );
    ev.sync_process_read_config_from_visible_variables();
    let pid = ev.processes.create_process(
        "tls-ready".into(),
        Value::NIL,
        String::new(),
        vec![],
        ProcessCodingSystems::gnu_make_process_initial(),
    );
    let expected: Vec<_> = (0..256).map(|n| b'a' + (n % 26) as u8).collect();
    let (tls, peer) = tls_peer(&expected);
    let proc = ev.processes.get_mut(pid).unwrap();
    proc.kind = ProcessKind::Network;
    proc.command = Value::NIL;
    proc.input_disposition = ProcessInputDisposition::Connected;
    proc.filter = Value::symbol("tls-filter");
    proc.coding_decode = Value::symbol("utf-8-unix");
    proc.coding_encode = Value::symbol("utf-8-unix");
    proc.live_io.tls_stream = Some(tls);
    proc.gnutls_initstage = GnutlsInitStage::Ready;
    ev.processes.set_process_output_read_interest(pid, true);
    let ready = ev
        .processes
        .wait_for_backend_events(
            Duration::from_secs(1),
            ProcessWaitBackendInterest::ProcessesOnly,
        )
        .unwrap();
    assert!(
        ready.has_ready_process(pid),
        "real ciphertext must reach the descriptor"
    );
    ev.poll_ready_process_output_for_service_request(
        ready,
        &ProcessOutputServiceRequest::target_only(pid),
    )
    .unwrap();
    assert_eq!(output(&mut ev, "tls-output"), expected[..16]);
    let tls = ev
        .processes
        .get_mut(pid)
        .unwrap()
        .live_io
        .tls_stream
        .as_mut()
        .unwrap();
    assert!(
        tls.has_buffered_process_output(),
        "budget exit must retain real plaintext"
    );
    assert_eq!(
        tls.tcp_stream().peek(&mut [0; 1]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "peer must have no more ciphertext"
    );
    assert!(
        !ev.processes
            .wait_for_backend_events(Duration::ZERO, ProcessWaitBackendInterest::ProcessesOnly)
            .unwrap()
            .has_ready_process(pid),
        "FD-only backend must not rescue the remaining plaintext"
    );
    (ev, pid, peer, expected)
}

fn output(ev: &mut Context, name: &str) -> Vec<u8> {
    ev.eval_symbol(name)
        .unwrap()
        .as_lisp_string()
        .unwrap()
        .as_bytes()
        .to_vec()
}

#[cfg(unix)]
fn other_stream(ev: &mut Context) -> ProcessId {
    let pid = builtin_make_pipe_process(
        ev,
        vec![
            Value::keyword(":name"),
            Value::string("tls-other"),
            Value::keyword(":filter"),
            Value::symbol("tls-other-filter"),
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
        .write_all(b"MARKER")
        .unwrap();
    pid
}

fn assert_drained(ev: &mut Context, pid: ProcessId, expected: &[u8]) {
    assert!(
        ev.eval_symbol("tls-other-at").unwrap().as_fixnum().unwrap() <= 32,
        "independent stream must progress while most TLS plaintext remains"
    );
    assert_eq!(
        output(ev, "tls-output"),
        expected,
        "complete exactly-once ordered response"
    );
    assert_eq!(
        ev.eval_symbol("tls-tick-at").unwrap(),
        Value::fixnum(16),
        "timer made due by the first filter must progress before the next chunk"
    );
    assert!(
        !ev.processes
            .get_mut(pid)
            .unwrap()
            .live_io
            .tls_stream
            .as_mut()
            .unwrap()
            .has_buffered_process_output(),
        "exact-budget final read must clear readiness without an extra read"
    );
    let start = Instant::now();
    let events = ev
        .processes
        .wait_for_backend_events_for_service(
            Duration::from_millis(20),
            ProcessWaitBackendInterest::ProcessesOnly,
            ProcessOutputServiceRequest::target_only(pid),
        )
        .unwrap();
    assert!(
        !events.has_ready_process(pid),
        "drained source cannot retain a logical wake"
    );
    assert!(
        start.elapsed() >= Duration::from_millis(10),
        "drained source must permit blocking, not spin"
    );
    assert_eq!(output(ev, "tls-output"), expected);
}

// Bounded command reads service successive fair batches until their deadline.
// Only unbounded reads yield to display maintenance after each callback batch.
fn wait_for_idle_command_input(ev: &mut Context, deadline: Instant) -> CommandInputWaitOutcome {
    loop {
        let outcome = ev.wait_for_command_input(Some(deadline)).unwrap();
        if outcome != CommandInputWaitOutcome::Interrupted {
            return outcome;
        }
    }
}

#[cfg(unix)]
#[test]
fn tls_unbounded_command_input_wait_yields_between_bounded_output_batches() {
    let (mut ev, pid, _peer, expected) = prepare();
    other_stream(&mut ev);
    while output(&mut ev, "tls-output").len() < expected.len() {
        let before = output(&mut ev, "tls-output").len();
        assert_eq!(
            ev.wait_for_command_input(None).unwrap(),
            CommandInputWaitOutcome::Interrupted
        );
        assert!(
            output(&mut ev, "tls-output").len() <= before + 16,
            "unbounded command reads yield after each bounded callback batch"
        );
    }
    assert_eq!(output(&mut ev, "tls-other"), b"MARKER");
    assert_drained(&mut ev, pid, &expected);
}

#[cfg(unix)]
#[test]
fn tls_buffered_response_resumes_inside_command_input_wait() {
    let (mut ev, pid, _peer, expected) = prepare();
    other_stream(&mut ev);
    assert_eq!(
        wait_for_idle_command_input(&mut ev, Instant::now() + Duration::from_millis(100)),
        CommandInputWaitOutcome::DeadlineElapsed
    );
    assert_eq!(output(&mut ev, "tls-other"), b"MARKER");
    assert_drained(&mut ev, pid, &expected);
}

#[cfg(unix)]
#[test]
fn tls_buffered_response_resumes_inside_sleep() {
    let (mut ev, pid, _peer, expected) = prepare();
    other_stream(&mut ev);
    ev.eval_str("(sleep-for 0.1)").unwrap();
    assert_eq!(output(&mut ev, "tls-other"), b"MARKER");
    assert_drained(&mut ev, pid, &expected);
}

#[cfg(unix)]
#[test]
fn tls_buffered_response_resumes_inside_other_target_wait_without_target_activity() {
    let (mut ev, pid, _peer, expected) = prepare();
    other_stream(&mut ev);
    let target = ev.processes.create_process(
        "tls-quiet-target".into(),
        Value::NIL,
        String::new(),
        vec![],
        ProcessCodingSystems::gnu_make_process_initial(),
    );
    let outcome = ev
        .wait_for_process_output(ProcessOutputWaitRequest::new(
            ProcessOutputWaitTiming::For(Duration::from_millis(100)),
            Some(target),
            false,
            true,
        ))
        .unwrap();
    assert_eq!(
        outcome,
        ProcessOutputWaitOutcome::NoProcessActivity,
        "TLS/marker bytes must not manufacture activity for another target"
    );
    assert_eq!(output(&mut ev, "tls-other"), b"MARKER");
    assert_drained(&mut ev, pid, &expected);
}

#[test]
fn tls_buffered_readiness_waits_for_completed_unsuspended_handshake() {
    let (mut ev, pid, _peer, expected) = prepare();
    ev.processes.get_mut(pid).unwrap().gnutls_initstage = GnutlsInitStage::Ready;
    // Real plaintext is buffered and the TCP descriptor has no ciphertext.
    // A pending handshake cannot consume it, even if the rustls client has
    // finished reading peer records but still owes its final outbound bytes.
    ev.processes.get_mut(pid).unwrap().gnutls_initstage = GnutlsInitStage::HandshakeTried;
    for suspended in [false, true] {
        if suspended {
            assert!(ev.processes.suspend_tls_handshake_polling(pid));
        }
        let events = ev
            .processes
            .wait_for_backend_events_for_service(
                Duration::ZERO,
                ProcessWaitBackendInterest::ProcessesOnly,
                ProcessOutputServiceRequest::any(None),
            )
            .unwrap();
        assert!(
            !events.has_ready_process(pid),
            "incomplete handshake plaintext must not bypass readiness suspension"
        );
    }
    // Completion alone must not override a nested wait's suspension owner.
    ev.processes.get_mut(pid).unwrap().gnutls_initstage = GnutlsInitStage::Ready;
    let events = ev
        .processes
        .wait_for_backend_events_for_service(
            Duration::ZERO,
            ProcessWaitBackendInterest::ProcessesOnly,
            ProcessOutputServiceRequest::any(None),
        )
        .unwrap();
    assert!(!events.has_ready_process(pid));
    ev.processes.resume_tls_handshake_polling(pid);
    let events = ev
        .processes
        .wait_for_backend_events_for_service(
            Duration::ZERO,
            ProcessWaitBackendInterest::ProcessesOnly,
            ProcessOutputServiceRequest::any(None),
        )
        .unwrap();
    assert!(events.has_ready_process(pid));
    assert_eq!(output(&mut ev, "tls-output"), expected[..16]);
    ev.poll_ready_process_output_for_service_request(
        events,
        &ProcessOutputServiceRequest::target_only(pid),
    )
    .unwrap();
    assert_eq!(output(&mut ev, "tls-output"), expected[..32]);
}

#[test]
fn tls_buffered_readiness_respects_interest_suspension_stop_and_deletion() {
    let (mut ev, pid, _peer, expected) = prepare();
    let target = ev.processes.create_process(
        "tls-control-target".into(),
        Value::NIL,
        String::new(),
        vec![],
        ProcessCodingSystems::gnu_make_process_initial(),
    );
    for (interest, request) in [
        (
            ProcessWaitBackendInterest::NotificationsOnly,
            ProcessOutputServiceRequest::any(None),
        ),
        (
            ProcessWaitBackendInterest::ProcessesOnly,
            ProcessOutputServiceRequest::none(),
        ),
        (
            ProcessWaitBackendInterest::ProcessesOnly,
            ProcessOutputServiceRequest::target_only(target),
        ),
    ] {
        let start = Instant::now();
        let events = ev
            .processes
            .wait_for_backend_events_for_service(Duration::from_millis(20), interest, request)
            .unwrap();
        assert!(!events.has_ready_process(pid));
        assert!(
            start.elapsed() >= Duration::from_millis(10),
            "excluded TLS cannot shorten this wait"
        );
    }
    for stopped in [false, true] {
        if stopped {
            ev.processes.get_mut(pid).unwrap().command = Value::T;
        } else {
            ev.processes.get_mut(pid).unwrap().filter = Value::T;
        }
        let events = ev
            .processes
            .wait_for_backend_events_for_service(
                Duration::ZERO,
                ProcessWaitBackendInterest::ProcessesOnly,
                ProcessOutputServiceRequest::any(None),
            )
            .unwrap();
        assert!(!events.has_ready_process(pid));
        assert_eq!(output(&mut ev, "tls-output"), expected[..16]);
        ev.processes.get_mut(pid).unwrap().command = Value::NIL;
        ev.processes.get_mut(pid).unwrap().filter = Value::symbol("tls-filter");
        let events = ev
            .processes
            .wait_for_backend_events_for_service(
                Duration::ZERO,
                ProcessWaitBackendInterest::ProcessesOnly,
                ProcessOutputServiceRequest::any(None),
            )
            .unwrap();
        assert!(
            events.has_ready_process(pid),
            "resume must rediscover plaintext, without new bytes"
        );
    }
    ev.processes.delete_process(pid);
    let events = ev
        .processes
        .wait_for_backend_events_for_service(
            Duration::ZERO,
            ProcessWaitBackendInterest::ProcessesOnly,
            ProcessOutputServiceRequest::any(None),
        )
        .unwrap();
    assert!(
        !events.has_ready_process(pid),
        "deletion cannot leave a stale logical claim"
    );
    assert_eq!(output(&mut ev, "tls-output"), expected[..16]);
}

#[cfg(unix)]
#[test]
fn prequeued_notification_harvests_pipe_and_writable_connect_with_buffered_tls() {
    let (mut ev, pid, _peer, expected) = prepare();
    ev.eval_str("(fset 'tls-other-filter (lambda (_p text) (if tls-other-at nil (setq tls-other-at (length tls-output))) (setq tls-other (concat tls-other text))))").unwrap();
    ev.service_timers_without_redisplay().unwrap();
    let other = other_stream(&mut ev);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    ev.eval_str("(setq tls-connect-count 0)").unwrap();
    let connect = ev.eval_str(&format!(
        "(make-network-process :name \"tls-connect\" :host \"127.0.0.1\" :service {} :nowait t :sentinel (lambda (_p event) (if (equal event \"open\\n\") (setq tls-connect-count (1+ tls-connect-count)))))",
        listener.local_addr().unwrap().port()
    )).unwrap().as_process_id().unwrap();
    let (_connect_peer, _) = listener.accept().unwrap();
    assert!(
        ev.processes
            .get(connect)
            .unwrap()
            .live_io
            .pending_network_connect
            .is_some()
    );
    for visit in 0..4 {
        ev.processes.wait_notifier().unwrap().notify().unwrap();
        if visit != 0 {
            ev.processes
                .get_mut(other)
                .unwrap()
                .live_io
                .module_pipe_writer
                .as_mut()
                .unwrap()
                .write_all(b"MARKER")
                .unwrap();
        }
        let events = ev
            .processes
            .wait_for_backend_events_for_service(
                Duration::from_secs(1),
                ProcessWaitBackendInterest::NotificationsAndProcesses,
                ProcessOutputServiceRequest::any(None),
            )
            .unwrap();
        assert!(events.has_notification_wakeup());
        assert!(events.has_ready_process(pid));
        assert!(
            events.has_ready_process(other),
            "a prequeued wake cannot hide the independent pipe"
        );
        if visit == 0 {
            assert!(events.writable_processes_ref().contains(&connect));
        }
        ev.poll_ready_process_output_for_service_request(
            events,
            &ProcessOutputServiceRequest::any(None),
        )
        .unwrap();
        assert_eq!(output(&mut ev, "tls-other"), b"MARKER".repeat(visit + 1));
    }
    assert_eq!(
        ev.eval_symbol("tls-connect-count").unwrap(),
        Value::fixnum(1)
    );
    assert!(
        ev.processes
            .get(connect)
            .unwrap()
            .live_io
            .pending_network_connect
            .is_none()
    );
    ev.processes.delete_process(connect);
    assert_eq!(
        wait_for_idle_command_input(&mut ev, Instant::now() + Duration::from_millis(100)),
        CommandInputWaitOutcome::DeadlineElapsed
    );
    assert_drained(&mut ev, pid, &expected);
    assert_eq!(
        ev.eval_symbol("tls-connect-count").unwrap(),
        Value::fixnum(1)
    );
}

#[cfg(unix)]
fn exact_budget_close_notify_journey(route: u8) {
    let (mut ev, pid, _idle_peer, _) = prepare();
    ev.eval_str(r#"(progn
        (setq tls-output "" tls-close-count 0 tls-close-observation nil tls-armed t delete-exited-processes t)
        (fset 'tls-close-sentinel (lambda (proc _event)
            (setq tls-close-count (1+ tls-close-count)
                  tls-close-observation (list tls-output (memq proc (process-list)))))))"#).unwrap();
    // Replace only this owned fixture's transport. The previous peer remains
    // alive; the new peer sends exactly one budget plus close_notify and holds TCP.
    let expected: Vec<u8> = (0..16).map(|n| b'a' + n).collect();
    let (tls, _closed_peer) = tls_peer_with_close_notify(&expected, true);
    ev.processes.set_process_output_read_interest(pid, false);
    let proc = ev.processes.get_mut(pid).unwrap();
    proc.live_io.tls_stream = Some(tls);
    proc.sentinel = Value::symbol("tls-close-sentinel");
    ev.processes.set_process_output_read_interest(pid, true);
    let events = ev
        .processes
        .wait_for_backend_events(
            Duration::from_secs(1),
            ProcessWaitBackendInterest::ProcessesOnly,
        )
        .unwrap();
    ev.poll_ready_process_output_for_service_request(
        events,
        &ProcessOutputServiceRequest::target_only(pid),
    )
    .unwrap();
    assert_eq!(output(&mut ev, "tls-output"), expected);
    assert_eq!(ev.eval_symbol("tls-close-count").unwrap(), Value::fixnum(0));
    assert_eq!(
        ev.processes
            .get(pid)
            .unwrap()
            .live_io
            .tls_stream
            .as_ref()
            .unwrap()
            .tcp_stream()
            .peek(&mut [0])
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );

    let other = other_stream(&mut ev);
    // The independent pipe stays level-ready across all eligibility checks and
    // the decisive shared-backend visit; no new TLS ciphertext/direct read occurs.
    for (interest, request) in [
        (
            ProcessWaitBackendInterest::NotificationsOnly,
            ProcessOutputServiceRequest::any(None),
        ),
        (
            ProcessWaitBackendInterest::ProcessesOnly,
            ProcessOutputServiceRequest::none(),
        ),
        (
            ProcessWaitBackendInterest::ProcessesOnly,
            ProcessOutputServiceRequest::target_only(other),
        ),
    ] {
        let events = ev
            .processes
            .wait_for_backend_events_for_service(Duration::ZERO, interest, request)
            .unwrap();
        assert!(!events.has_ready_process(pid));
    }
    for stopped in [false, true] {
        if stopped {
            ev.processes.get_mut(pid).unwrap().command = Value::T;
        } else {
            ev.processes.get_mut(pid).unwrap().filter = Value::T;
        }
        let events = ev
            .processes
            .wait_for_backend_events_for_service(
                Duration::ZERO,
                ProcessWaitBackendInterest::ProcessesOnly,
                ProcessOutputServiceRequest::any(None),
            )
            .unwrap();
        assert!(!events.has_ready_process(pid));
        ev.processes.get_mut(pid).unwrap().command = Value::NIL;
        ev.processes.get_mut(pid).unwrap().filter = Value::symbol("tls-filter");
    }
    for _ in 0..4 {
        let events = ev
            .processes
            .wait_for_backend_events_for_service(
                Duration::from_secs(1),
                ProcessWaitBackendInterest::ProcessesOnly,
                ProcessOutputServiceRequest::any(None),
            )
            .unwrap();
        assert!(events.has_ready_process(other));
        assert!(
            events.has_ready_process(pid),
            "exact-budget TLS EOF must not be hidden by a continuously ready stream"
        );
    }
    match route {
        0 => {
            ev.wait_for_command_input(Some(Instant::now() + Duration::from_millis(30)))
                .unwrap();
        }
        1 => {
            ev.eval_str("(sleep-for 0.03)").unwrap();
        }
        _ => {
            let target = ev.processes.create_process(
                "close-quiet".into(),
                Value::NIL,
                String::new(),
                vec![],
                ProcessCodingSystems::gnu_make_process_initial(),
            );
            assert_eq!(
                ev.wait_for_process_output(ProcessOutputWaitRequest::new(
                    ProcessOutputWaitTiming::For(Duration::from_millis(30)),
                    Some(target),
                    false,
                    true
                ))
                .unwrap(),
                ProcessOutputWaitOutcome::NoProcessActivity
            );
        }
    }
    assert_eq!(output(&mut ev, "tls-output"), expected);
    assert_eq!(ev.eval_symbol("tls-close-count").unwrap(), Value::fixnum(1));
    assert_eq!(
        ev.eval_symbol("tls-close-observation").unwrap(),
        Value::list(vec![
            Value::string(std::str::from_utf8(&expected).unwrap()),
            Value::NIL
        ])
    );
    assert!(!ev.processes.live_process_ids().contains(&pid));
    assert_eq!(output(&mut ev, "tls-other"), b"MARKER");
    ev.processes.delete_process(other);
    // A consumed EOF claim must retire, not become permanent logical readiness.
    let started = Instant::now();
    let events = ev
        .processes
        .wait_for_backend_events_for_service(
            Duration::from_millis(20),
            ProcessWaitBackendInterest::ProcessesOnly,
            ProcessOutputServiceRequest::any(None),
        )
        .unwrap();
    assert!(!events.has_ready_process(pid));
    assert!(started.elapsed() >= Duration::from_millis(10));
    assert_eq!(ev.eval_symbol("tls-close-count").unwrap(), Value::fixnum(1));
}

#[cfg(unix)]
#[test]
fn exact_budget_tls_close_notify_retires_once_in_command_wait() {
    exact_budget_close_notify_journey(0);
}
#[cfg(unix)]
#[test]
fn exact_budget_tls_close_notify_retires_once_in_sleep() {
    exact_budget_close_notify_journey(1);
}
#[cfg(unix)]
#[test]
fn exact_budget_tls_close_notify_retires_once_in_other_target_wait() {
    exact_budget_close_notify_journey(2);
}
