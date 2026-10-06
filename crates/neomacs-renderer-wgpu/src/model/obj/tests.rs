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
