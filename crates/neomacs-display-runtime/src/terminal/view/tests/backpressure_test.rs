use super::super::transport::Transport;
use super::*;

#[test]
fn exact_ingress_is_atomic_bounded_nonwaiting_and_closed_by_cancel_or_eof() {
    let transport = Transport::new();
    let data = vec![b'x'; 64 * 1024];
    transport.input(&data).unwrap();
    assert_eq!(
        transport.input(b"paste").unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(transport.pressure().1, data.len());
    transport
        .stop
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        transport.input(b"x").unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert!(transport.finish(Err("terminal cancelled".into())).is_err());
    assert_eq!(transport.pressure().1, 0);
    let transport = Transport::new();
    assert!(transport.finish(Ok(())).is_ok());
    assert_eq!(
        transport.input(b"x").unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
}

#[test]
fn exact_reply_overflow_is_a_transport_error_not_successful_drain() {
    let transport = Transport::new();
    transport.input(&vec![b'x'; 64 * 1024]).unwrap();
    transport.reply(b"\x1b[1;1R");
    assert_eq!(transport.pressure().1, 0);
    assert_eq!(
        transport.input(b"x").unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert!(
        transport
            .finish(Ok(()))
            .unwrap_err()
            .contains("reply admission failed")
    );
}

#[cfg(unix)]
#[test]
fn exact_nonblocking_pump_preserves_partial_prefix_fifo_and_eagain_tail() {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    let (writer, mut reader) = UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    reader.set_nonblocking(true).unwrap();
    let send_buffer: libc::c_int = 4096;
    // SAFETY: live socket and correctly sized synchronous option value.
    assert_eq!(
        unsafe {
            libc::setsockopt(
                writer.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&send_buffer as *const libc::c_int).cast(),
                std::mem::size_of_val(&send_buffer) as libc::socklen_t,
            )
        },
        0
    );
    let transport = Transport::new();
    let prefix: Vec<u8> = (0..48 * 1024).map(|i| (i % 251) as u8).collect();
    let suffix = b"FIFO_SUFFIX".repeat(1024);
    transport.input(&prefix).unwrap();
    for _ in 0..64 {
        transport.pump(writer.as_raw_fd());
    }
    let (blocked, remaining) = transport.pressure();
    assert!(blocked > 0 && remaining > 0 && remaining < prefix.len());
    transport.input(&suffix).unwrap();
    let mut received = Vec::new();
    let expected: Vec<u8> = prefix.into_iter().chain(suffix).collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while received.len() < expected.len() {
        assert!(std::time::Instant::now() < deadline);
        transport.pump(writer.as_raw_fd());
        let mut bytes = [0; 4096];
        match reader.read(&mut bytes) {
            Ok(n) => received.extend_from_slice(&bytes[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("unexpected socket read: {other:?}"),
        }
    }
    assert_eq!(received, expected);
    assert_eq!(transport.pressure().1, 0);
}

#[cfg(unix)]
#[test]
fn exact_write_failure_is_latched_and_never_turns_into_eof_success() {
    let transport = Transport::new();
    transport.input(b"undeliverable").unwrap();
    assert!(!transport.pump(-1)); // deterministic EBADF, no foreign resource
    assert_eq!(
        transport.input(b"x").unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert!(
        transport
            .finish(Ok(()))
            .unwrap_err()
            .contains("PTY write failed")
    );
}
