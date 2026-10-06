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

// ----------------------------------------------------------------------
// Round 3 (SOL-REVIEW-2 MUST 1, OPUS-REVIEW SHOULD 2): one huge token is
// processed in bounded, cancellable steps, an unfinished face line meets
// its reference allowance as it arrives, and errors quote a short prefix.

/// A file of `head`, then `fill` repeated `len` times, then `tail`.
fn huge_token_body(head: &[u8], fill: u8, len: usize, tail: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(head.len() + len + tail.len());
    body.extend_from_slice(head);
    body.resize(head.len() + len, fill);
    body.extend_from_slice(tail);
    body
}

/// Read `body` uncancelled, timing every cancellation poll. Answers the
/// outcome, the whole time, the longest stretch without a poll (from the
/// start to the first poll, between polls, and from the last poll to the
/// return) and the number of polls.
fn timed_polls(
    path: &std::path::Path,
    limits: ObjLimits,
) -> (
    Result<ObjMesh, ObjError>,
    std::time::Duration,
    std::time::Duration,
    usize,
) {
    let start = std::time::Instant::now();
    let last = std::cell::Cell::new(start);
    let gap = std::cell::Cell::new(std::time::Duration::ZERO);
    let polls = std::cell::Cell::new(0usize);
    let poll = || {
        let now = std::time::Instant::now();
        gap.set(gap.get().max(now - last.get()));
        last.set(now);
        polls.set(polls.get() + 1);
        false
    };
    let outcome = read_obj_file(path, None, limits, &poll);
    let end = std::time::Instant::now();
    gap.set(gap.get().max(end - last.get()));
    (outcome, end - start, gap.get(), polls.get())
}

/// The longest stretch without a cancellation poll is a small part of
/// the whole parse of `body` (best of three runs, so one scheduling
/// hiccup cannot fail it), and a flag raised at the middle poll stops the
/// parse promptly. Answers the uncancelled outcome.
fn assert_polled_throughout(name: &str, body: &[u8]) -> Result<ObjMesh, ObjError> {
    let path = scratch(name, body);
    let limits = ObjLimits {
        max_vertices: 1000,
        max_triangles: 1000,
    };
    let mut best = None;
    let mut outcome = None;
    let mut polls = 0;
    for _ in 0..3 {
        let (result, total, gap, count) = timed_polls(&path, limits);
        outcome = Some(result);
        polls = count;
        let ratio = gap.as_secs_f64() / total.as_secs_f64();
        if best.is_none_or(|(r, _, _)| ratio < r) {
            best = Some((ratio, gap, total));
        }
        if ratio * 8.0 < 1.0 {
            break;
        }
    }
    let (ratio, gap, total) = best.unwrap();
    assert!(
        ratio * 8.0 < 1.0,
        "{name}: {gap:?} without a cancellation poll in a {total:?} parse ({polls} polls)"
    );
    // Cancel at the middle poll: the parse stops there.
    let calls = std::cell::Cell::new(0usize);
    let tripped = std::cell::Cell::new(None::<std::time::Instant>);
    let cancelled = || {
        calls.set(calls.get() + 1);
        if calls.get() >= polls / 2 {
            tripped.set(tripped.get().or(Some(std::time::Instant::now())));
            true
        } else {
            false
        }
    };
    assert_eq!(
        read_obj_file(&path, None, limits, &cancelled).map(|m| m.vertices.len()),
        Err(ObjError::Cancelled),
        "{name}: cancelled at poll {} of {polls}",
        polls / 2
    );
    let stopped = tripped.get().unwrap().elapsed();
    assert!(
        stopped.as_secs_f64() * 8.0 < total.as_secs_f64(),
        "{name}: stopped {stopped:?} after the flag rose, in a {total:?} parse"
    );
    std::fs::remove_file(path).unwrap();
    outcome.unwrap()
}

/// SOL-REVIEW-2 MUST 1: `f <40 MB of leading zeros>1 2 3`, a valid face
/// (zero padding is read by value), is not one uncancellable token.
#[test]
fn cancellation_is_polled_inside_one_huge_zero_padded_index() {
    let body = huge_token_body(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf ", b'0', 40 << 20, b"1 2 3\n");
    let mesh = assert_polled_throughout("huge-index.obj", &body).unwrap();
    assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
}

/// The same for one huge coordinate, whose digits are all significant:
/// its value is still the correctly rounded one.
#[test]
fn cancellation_is_polled_inside_one_huge_coordinate() {
    let body = huge_token_body(
        b"v 0.",
        b'3',
        40 << 20,
        b"7 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
    );
    let mesh = assert_polled_throughout("huge-coordinate.obj", &body).unwrap();
    let digits =
        std::str::from_utf8(&body[2..body.iter().position(|b| *b == b'7').unwrap() + 1]).unwrap();
    assert_eq!(mesh.vertices[0][0], digits.parse::<f64>().unwrap());
    assert_eq!(mesh.vertices[0][0], 1.0 / 3.0);
}

/// SOL-REVIEW-2 MUST 1: an unterminated face line fails at its reference
/// allowance as the references arrive, not once the whole line is in.
#[test]
fn an_unfinished_face_line_fails_at_its_reference_allowance() {
    let tight = ObjLimits {
        max_vertices: 10,
        max_triangles: 4,
    };
    let mut parser = ObjParser::new(tight);
    parser
        .feed(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf", &|| false)
        .unwrap();
    // Six references are four triangles: the seventh exceeds them.
    let mut outcome = Ok(());
    for _ in 0..1000 {
        outcome = parser.feed(b" 1 2 3", &|| false);
        if outcome.is_err() {
            break;
        }
    }
    assert_eq!(outcome, Err(ObjError::TooManyTriangles { max: 4 }));
}

/// OPUS-REVIEW SHOULD 2: an error quotes at most a short prefix of the
/// offending word, however long the word is.
#[test]
fn errors_quote_a_short_prefix_of_the_offending_word() {
    let long = "x".repeat(1 << 20);
    for (text, quoted) in [
        (format!("v 0 0 {long}\n"), "number"),
        (
            format!("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 {long}\n"),
            "index",
        ),
        (
            format!("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3{long}/7\n"),
            "index",
        ),
    ] {
        let error = parse(&text).unwrap_err();
        let shown = match &error {
            ObjError::BadNumber { text, .. } | ObjError::BadIndex { text, .. } => text.clone(),
            other => panic!("{quoted}: unexpected {other:?}"),
        };
        assert!(
            shown.len() <= 40 && shown.ends_with('…'),
            "{quoted}: quoted {} bytes",
            shown.len()
        );
        assert!(error.to_string().len() < 80, "{quoted}: {error}");
    }
    // A short word is quoted whole.
    assert_eq!(
        parse("v 0 0 0x1\n").unwrap_err(),
        ObjError::BadNumber {
            line: 1,
            text: "0x1".into()
        }
    );
    // A multi-byte character is never cut.
    let accented = format!("v 0 0 {}\n", "é".repeat(40));
    match parse(&accented).unwrap_err() {
        ObjError::BadNumber { text, .. } => {
            assert!(text.starts_with("éé") && text.ends_with('…'), "{text}")
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// The parser's numbers are Rust's correctly rounded `f64` of the whole
/// text, also for long digit runs (pseudo-random, reproducible).
#[test]
fn numbers_match_the_correctly_rounded_value_of_the_whole_text() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let mut checked = 0;
    for _ in 0..3000 {
        let mut text = String::new();
        if next(3) == 0 {
            text.push(if next(2) == 0 { '-' } else { '+' });
        }
        for _ in 0..next(4) {
            text.push('0');
        }
        let int_len = next(8);
        for _ in 0..int_len {
            text.push(char::from(b'0' + next(10) as u8));
        }
        let frac_len = match next(4) {
            0 => 0,
            1 => next(20),
            2 => next(900),
            _ => 700 + next(400),
        };
        if frac_len > 0 || int_len == 0 {
            text.push('.');
            for i in 0..frac_len.max(1) {
                // Runs of zeros and nines probe rounding at the cut.
                let digit = match next(5) {
                    0 => 0,
                    1 => 9,
                    _ => next(10) as u8,
                };
                let digit = if i + 1 == frac_len && next(4) == 0 {
                    5
                } else {
                    digit
                };
                text.push(char::from(b'0' + digit));
            }
        }
        if next(3) == 0 {
            text.push(if next(2) == 0 { 'e' } else { 'E' });
            if next(2) == 0 {
                text.push('-');
            }
            for _ in 0..next(3) {
                text.push('0');
            }
            text.push_str(&next(12).to_string());
        }
        let expected: f64 = text.parse().unwrap();
        match parse_number(&text, 1) {
            Ok(value) => {
                assert_eq!(value.to_bits(), expected.to_bits(), "{text}");
                checked += 1;
            }
            Err(ObjError::NumberOutOfBounds { .. }) => assert!(expected.abs() > 1e6, "{text}"),
            Err(e) => panic!("{text}: {e}"),
        }
    }
    assert!(checked > 1000, "{checked}");
    // Exponents far beyond any digit count saturate to the same value.
    assert_eq!(parse_number("1e-99999999999999999999", 1), Ok(0.0));
    assert_eq!(
        parse_number("-0.5e-000000000000000000000000000001", 1).map(f64::to_bits),
        Ok((-0.05f64).to_bits())
    );
    assert_eq!(
        parse_number("0e99999999999999999999", 1).map(f64::to_bits),
        Ok(0.0f64.to_bits())
    );
    assert_eq!(
        parse_number("-0", 1).map(f64::to_bits),
        Ok((-0.0f64).to_bits())
    );
    assert_eq!(
        parse_number("1e99999999999999999999", 1),
        Err(ObjError::NumberOutOfBounds { line: 1 })
    );
}

/// SOL-REVIEW-3 SHOULD 4: the public incremental parser polls once per
/// [`SCAN_WINDOW`] bytes whatever the feeds' sizes: uneven feeds (one byte
/// short of a window, then a whole window) inside one long comment, which
/// has no line or word checkpoint, still poll at every window boundary.
#[test]
fn the_incremental_parser_polls_every_window_for_uneven_feeds() {
    let polls = std::cell::Cell::new(0usize);
    let count = || {
        polls.set(polls.get() + 1);
        false
    };
    let mut parser = ObjParser::new(WIDE);
    parser.feed(b"#", &count).unwrap();
    let mut fed = 1usize;
    let filler = vec![b'x'; SCAN_WINDOW];
    for round in 0..8 {
        let len = if round % 2 == 0 {
            SCAN_WINDOW - 2
        } else {
            SCAN_WINDOW
        };
        parser.feed(&filler[..len], &count).unwrap();
        fed += len;
        assert_eq!(
            polls.get(),
            fed / SCAN_WINDOW,
            "polls after {fed} bytes in feeds of {len}"
        );
    }
}

/// The exact decimal expansion of `k * 2^-1075` (every binary64 halfway
/// point is one), from exact integer arithmetic.
fn exact_binary_fraction(k: u64) -> String {
    // k * 5^1075 in base 10^9 limbs, least significant first.
    let mut limbs: Vec<u64> = vec![
        k % 1_000_000_000,
        k / 1_000_000_000 % 1_000_000_000,
        k / 1_000_000_000_000_000_000,
    ];
    for _ in 0..1075 {
        let mut carry = 0u64;
        for limb in &mut limbs {
            let v = *limb * 5 + carry;
            *limb = v % 1_000_000_000;
            carry = v / 1_000_000_000;
        }
        if carry > 0 {
            limbs.push(carry);
        }
    }
    let mut digits = String::new();
    for limb in limbs.iter().rev() {
        digits.push_str(&format!("{limb:09}"));
    }
    let digits = digits.trim_start_matches('0');
    let digits = format!("{digits:0>1076}");
    let (int, frac) = digits.split_at(digits.len() - 1075);
    format!("{int}.{frac}")
}

/// Significant digits of a decimal text (no sign or exponent).
fn significant_digits(text: &str) -> usize {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    digits.trim_start_matches('0').trim_end_matches('0').len()
}

/// SOL-REVIEW-3 NIT 1: halfway points, subnormals and sticky tails beyond
/// [`MAX_SIGNIFICANT`] digits round as the whole text does. The longest
/// binary64 midpoint, `(2^54 - 3) * 2^-1075`, has 768 significant digits.
#[test]
fn halfway_subnormal_and_sticky_tail_vectors_round_exactly() {
    let longest = exact_binary_fraction((1 << 54) - 3);
    assert_eq!(significant_digits(&longest), 768);
    assert!(significant_digits(&longest) < MAX_SIGNIFICANT);
    let below_one = "0.99999999999999994448884876874217297881841659545898437500";
    let above_one = "1.00000000000000011102230246251565404236316680908203125";
    let mut vectors = vec![
        longest.clone(),
        // Ties to even: down to 1, up to 2^-1074, ...
        above_one.to_owned(),
        below_one.to_owned(),
        exact_binary_fraction(1),
        exact_binary_fraction(3),
        // The smallest subnormal, the largest one, and the smallest normal.
        exact_binary_fraction(2),
        exact_binary_fraction((1 << 53) - 2),
        exact_binary_fraction(1 << 53),
        "4.9406564584124654e-324".to_owned(),
        "2.4703282292062327e-324".to_owned(),
        "2.4703282292062328e-324".to_owned(),
    ];
    // A nonzero digit far past the kept prefix breaks each tie upward; a
    // tail of zeros does not; one short of the midpoint rounds down.
    for midpoint in [longest, above_one.to_owned(), exact_binary_fraction(1)] {
        let zeros = "0".repeat(MAX_SIGNIFICANT + 50);
        vectors.push(format!("{midpoint}{zeros}1"));
        vectors.push(format!("{midpoint}{zeros}"));
        let mut short = midpoint.clone().into_bytes();
        let last = short
            .iter()
            .rposition(|b| (b'1'..=b'9').contains(b))
            .unwrap();
        short[last] -= 1;
        vectors.push(format!("{}{}9", String::from_utf8(short).unwrap(), zeros));
        vectors.push(format!("-{midpoint}{zeros}1"));
    }
    for text in &vectors {
        let expected: f64 = text.parse().unwrap();
        assert_eq!(
            parse_number(text, 1).map(f64::to_bits),
            Ok(expected.to_bits()),
            "{}…",
            &text[..text.len().min(40)]
        );
    }
    // The sticky tail changes the result (the vectors are not vacuous).
    let tie = exact_binary_fraction(1);
    let zeros = "0".repeat(MAX_SIGNIFICANT + 50);
    assert_eq!(parse_number(&tie, 1), Ok(0.0));
    assert_eq!(
        parse_number(&format!("{tie}{zeros}1"), 1),
        Ok(f64::from_bits(1))
    );
}
