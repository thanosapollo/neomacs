use super::*;

const WIDE: ObjLimits = ObjLimits {
    max_vertices: 1000,
    max_triangles: 1000,
};

fn parse(text: &str) -> Result<ObjMesh, ObjError> {
    parse_obj(text.as_bytes(), WIDE)
}

#[test]
fn triangle_with_comments_and_ignored_records() {
    let mesh = parse(
        "# header\nmtllib x.mtl\no part\nv 0 0 0\nv 1 0 0 # trailing\nv 0 1 0 1.0\n\
         vt 0 0\nvn 0 0 1\ng grp\nusemtl m\ns 1\nf 1/1/1 2//1 3\n",
    )
    .unwrap();
    assert_eq!(mesh.vertices[1], [1.0, 0.0, 0.0]);
    assert_eq!(mesh.vertices[2], [0.0, 1.0, 0.0], "extra w ignored");
    assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
}

#[test]
fn polygons_fan_in_file_order_and_number_faces_from_zero() {
    let mesh =
        parse("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 2 0\nf 1 2 3 4 5\nf 3 4 5\n").unwrap();
    assert_eq!(
        mesh.triangles,
        vec![[0, 1, 2], [0, 2, 3], [0, 3, 4], [2, 3, 4]],
        "fan (p0,pi,pi+1), faces in file order"
    );
}

#[test]
fn negative_indices_are_relative_to_vertices_defined_so_far() {
    let mesh = parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\nv 5 5 5\nf -4 -3 -1\n").unwrap();
    assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 1, 3]]);
}

#[test]
fn tabs_and_crlf_separate_words() {
    let mesh = parse("v\t0 0 0\r\nv 1\t0 0\r\nv 0 1 0\r\nf 1 2 3\r\n").unwrap();
    assert_eq!(mesh.triangles.len(), 1);
}

#[test]
fn strict_gnosis_number_syntax() {
    for good in ["1", "-1", "+1", "1.", ".5", "1.5e3", "1E-2", "-0.0"] {
        assert!(parse_number(good, 1).is_ok(), "{good}");
    }
    for bad in [
        "inf", "nan", "1e", "e5", ".", "1_0", "0x1", "1.5.2", "--1", "",
    ] {
        assert!(
            matches!(parse_number(bad, 1), Err(ObjError::BadNumber { .. })),
            "{bad}"
        );
    }
    assert_eq!(
        parse_number("1000001", 1),
        Err(ObjError::NumberOutOfBounds { line: 1 })
    );
    assert!(parse_number("1e6", 1).is_ok());
}

#[test]
fn each_error_names_its_line() {
    let cases: &[(&str, ObjError)] = &[
        ("v 0 0\n", ObjError::ShortVertex { line: 1 }),
        (
            "v 0 0 x\n",
            ObjError::BadNumber {
                line: 1,
                text: "x".into(),
            },
        ),
        ("v 0 0 0\nv 1 0 0\nf 1 2\n", ObjError::ShortFace { line: 3 }),
        (
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 +3\n",
            ObjError::BadIndex {
                line: 4,
                text: "+3".into(),
            },
        ),
        (
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 4\n",
            ObjError::IndexOutOfRange { line: 4 },
        ),
        (
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n",
            ObjError::IndexOutOfRange { line: 4 },
        ),
        (
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf -4 1 2\n",
            ObjError::IndexOutOfRange { line: 4 },
        ),
        ("v 0 0 0\n", ObjError::NoTriangles),
    ];
    for (text, expected) in cases {
        assert_eq!(&parse(text).unwrap_err(), expected, "{text:?}");
    }
    assert_eq!(
        parse_obj(b"v 0 0 0\n\xff\n", WIDE).unwrap_err(),
        ObjError::NotUtf8 { line: 2 }
    );
}

#[test]
fn limits_fail_as_soon_as_exceeded() {
    let tight = ObjLimits {
        max_vertices: 3,
        max_triangles: 1,
    };
    assert_eq!(
        parse_obj(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nf 1 2 3\n", tight).unwrap_err(),
        ObjError::TooManyVertices { max: 3 }
    );
    let tight = ObjLimits {
        max_vertices: 10,
        max_triangles: 1,
    };
    assert_eq!(
        parse_obj(b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n", tight).unwrap_err(),
        ObjError::TooManyTriangles { max: 1 },
        "a quad is two triangles"
    );
}

fn scratch(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("neomacs-model-obj-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn file_digest_must_match_the_bytes_parsed() {
    use sha2::Digest;
    let bytes = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let path = scratch("digest.obj", bytes);
    let digest: [u8; 32] = sha2::Sha256::digest(bytes).into();
    let never = || false;
    assert!(read_obj_file(&path, Some(&digest), WIDE, &never).is_ok());
    let mut wrong = digest;
    wrong[0] ^= 1;
    assert_eq!(
        read_obj_file(&path, Some(&wrong), WIDE, &never).unwrap_err(),
        ObjError::DigestMismatch
    );
    assert!(read_obj_file(&path, None, WIDE, &never).is_ok());
}

#[test]
fn cancelled_load_stops() {
    let path = scratch("cancel.obj", b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
    let always = || true;
    assert_eq!(
        read_obj_file(&path, None, WIDE, &always).unwrap_err(),
        ObjError::Cancelled
    );
}

#[test]
fn missing_file_is_an_io_error() {
    let never = || false;
    assert!(matches!(
        read_obj_file(
            std::path::Path::new("/nonexistent/x.obj"),
            None,
            WIDE,
            &never
        ),
        Err(ObjError::Io(_))
    ));
}

/// Gnosis and canvas-3d read an index by numeric value, so zero padding of
/// any length is valid; only the value decides the range.
#[test]
fn zero_padded_indices_resolve_by_value() {
    let mesh = parse(
        "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0000000000000000001 00000000000000000000000002 -0000000000000000000001\n",
    )
    .unwrap();
    assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
    for face in [
        "f 1 2 00000000000000000000000",
        "f 1 2 -0000000000000000000000",
        "f 1 2 99999999999999999999999999",
        "f 1 2 -99999999999999999999999999",
    ] {
        assert_eq!(
            parse(&format!("v 0 0 0\nv 1 0 0\nv 0 1 0\n{face}\n")).unwrap_err(),
            ObjError::IndexOutOfRange { line: 4 },
            "{face}"
        );
    }
}

/// A face is rejected once it alone exceeds the remaining triangle allowance,
/// before the rest of its references are collected.
#[test]
fn oversized_polygon_stops_at_the_triangle_allowance() {
    let tight = ObjLimits {
        max_vertices: 10,
        max_triangles: 2,
    };
    // The bad reference after the fifth vertex is never parsed: the polygon
    // already needs 3 triangles there.
    assert_eq!(
        parse_obj(
            b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 2 0\nf 1 2 3 4 5 x\n",
            tight
        )
        .unwrap_err(),
        ObjError::TooManyTriangles { max: 2 }
    );
}

/// Run `read_obj_file` on a thread so a blocking open fails the test
/// instead of hanging it.
fn read_bounded(path: std::path::PathBuf) -> Result<ObjMesh, ObjError> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let never = || false;
        let _ = tx.send(read_obj_file(&path, None, WIDE, &never));
    });
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("read_obj_file blocked instead of rejecting a non-regular file")
}

#[test]
fn fifo_and_symlink_to_fifo_are_rejected_without_blocking() {
    let dir = scratch("fifo-anchor.obj", b"")
        .parent()
        .unwrap()
        .to_path_buf();
    let fifo = dir.join("pipe.obj");
    let _ = std::fs::remove_file(&fifo);
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
    let link = dir.join("link.obj");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&fifo, &link).unwrap();
    assert_eq!(read_bounded(fifo).unwrap_err(), ObjError::NotRegular);
    assert_eq!(read_bounded(link).unwrap_err(), ObjError::NotRegular);
    assert_eq!(read_bounded(dir).unwrap_err(), ObjError::NotRegular);
}

#[test]
fn oversized_file_is_rejected_before_reading() {
    let path = scratch("huge.obj", b"");
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_OBJ_FILE_BYTES + 1)
        .unwrap();
    let start = std::time::Instant::now();
    assert_eq!(read_bounded(path.clone()).unwrap_err(), ObjError::TooLarge);
    assert!(
        start.elapsed() < std::time::Duration::from_millis(500),
        "size checked from metadata, not by reading 100 MB: {:?}",
        start.elapsed()
    );
    std::fs::remove_file(path).unwrap();
}

// ----------------------------------------------------------------------
// Review MUST 2: cancellation reaches the parse, including long lines.

const TRIANGLE_HEAD: &[u8] = b"v 0 0 0\nv 1 0 0\nv 0 1 0\n";

/// `count` one-triangle face lines.
fn many_faces(count: usize) -> Vec<u8> {
    let mut body = TRIANGLE_HEAD.to_vec();
    for _ in 0..count {
        body.extend_from_slice(b"f 1 2 3\n");
    }
    body
}

/// One face line of `refs` references.
fn one_long_face(refs: usize) -> Vec<u8> {
    let mut body = TRIANGLE_HEAD.to_vec();
    body.extend_from_slice(b"f");
    for i in 0..refs {
        body.extend_from_slice([&b" 1"[..], b" 2", b" 3"][i % 3]);
    }
    body.push(b'\n');
    body
}

/// Read `body` with a cancel flag that rises only after the read loop has
/// polled it once per read step and once more: a flag raised then must
/// still stop the work promptly. Answers the outcome, how many polls ran
/// and the time from the first `true` to the return.
fn read_cancelled_late(
    name: &str,
    body: &[u8],
) -> (
    Result<ObjMesh, ObjError>,
    usize,
    Option<std::time::Duration>,
) {
    let path = scratch(name, body);
    let read_steps = body.len().div_ceil(1 << 20) + 1;
    let calls = std::cell::Cell::new(0usize);
    let tripped = std::cell::Cell::new(None::<std::time::Instant>);
    let cancelled = || {
        calls.set(calls.get() + 1);
        if calls.get() > read_steps + 1 {
            if tripped.get().is_none() {
                tripped.set(Some(std::time::Instant::now()));
            }
            true
        } else {
            false
        }
    };
    let limits = ObjLimits {
        max_vertices: 1000,
        max_triangles: 2_000_000,
    };
    let outcome = read_obj_file(&path, None, limits, &cancelled);
    let stopped = tripped.get().map(|t| t.elapsed());
    std::fs::remove_file(path).unwrap();
    (outcome, calls.get(), stopped)
}

/// Many short lines: the parse itself polls the flag.
#[test]
fn cancellation_is_polled_while_parsing_many_lines() {
    let body = many_faces(1_500_000);
    let (outcome, calls, stopped) = read_cancelled_late("cancel-lines.obj", &body);
    assert_eq!(
        outcome.map(|m| m.triangles.len()),
        Err(ObjError::Cancelled),
        "a flag raised after the read steps must stop the parse ({calls} polls)"
    );
    let stopped = stopped.unwrap();
    assert!(
        stopped < std::time::Duration::from_millis(250),
        "stopped {stopped:?} after the flag rose"
    );
}

/// One face line of 1,500,000 references (about 3 MB): checkpoints run
/// inside the line, not only between lines.
#[test]
fn cancellation_is_polled_inside_one_long_face_line() {
    let body = one_long_face(1_500_000);
    let (outcome, calls, stopped) = read_cancelled_late("cancel-long-line.obj", &body);
    assert_eq!(
        outcome.map(|m| m.triangles.len()),
        Err(ObjError::Cancelled),
        "a flag raised after the read steps must stop the long line ({calls} polls)"
    );
    let stopped = stopped.unwrap();
    assert!(
        stopped < std::time::Duration::from_millis(250),
        "stopped {stopped:?} after the flag rose"
    );
}

/// The incremental parser gives the same mesh whatever the chunking, and
/// polls inside one long line.
#[test]
fn the_incremental_parser_is_chunking_independent_and_polls_long_lines() {
    let text = b"# c\nv 0 0 0\nv 1 0 0 # x\r\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\nf -4 -2 -1";
    let whole = parse_obj(text, WIDE).unwrap();
    for step in 1..text.len() {
        let mut parser = ObjParser::new(WIDE);
        for piece in text.chunks(step) {
            parser.feed(piece, &|| false).unwrap();
        }
        assert_eq!(parser.finish(&|| false).unwrap(), whole, "step {step}");
    }
    // Line numbers survive chunking.
    let mut parser = ObjParser::new(WIDE);
    parser.feed(b"v 0 0 0\nv 1 ", &|| false).unwrap();
    assert_eq!(
        parser.feed(b"0\n", &|| false).unwrap_err(),
        ObjError::ShortVertex { line: 2 }
    );
    // Invalid UTF-8 split across chunks is still found, with its line.
    let mut parser = ObjParser::new(WIDE);
    parser.feed(b"v 0 0 0\n# \xc3", &|| false).unwrap();
    assert_eq!(
        parser.feed(b"\xa9\n# \xff\n", &|| false).unwrap_err(),
        ObjError::NotUtf8 { line: 3 }
    );
    // One long face: polled within the line, stopping at the first `true`.
    let body = one_long_face(1_500_000);
    let calls = std::cell::Cell::new(0usize);
    let late = || {
        calls.set(calls.get() + 1);
        calls.get() >= 3
    };
    let limits = ObjLimits {
        max_vertices: 1000,
        max_triangles: 2_000_000,
    };
    assert_eq!(
        parse_obj_cancellable(&body, limits, &late).unwrap_err(),
        ObjError::Cancelled
    );
    assert_eq!(calls.get(), 3);
}
