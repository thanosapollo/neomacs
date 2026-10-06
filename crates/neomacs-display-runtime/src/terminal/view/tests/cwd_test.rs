//! Actual native Processor → Crosswords → production metadata projection.
//! CPU-only tests: no fake terminal backend, PTY, shell, device or renderer.
use super::*;

fn native_fixture(
    id: u32,
) -> (
    Processor,
    Crosswords<NeomacsEventProxy>,
    NeomacsEventProxy,
    Option<Vec<u8>>,
) {
    let proxy = NeomacsEventProxy::new(TerminalId::new(id).unwrap());
    let term = Crosswords::new(
        CrosswordsSize::new(20, 5),
        CursorShape::Block,
        proxy.clone(),
        WindowId::from(0),
        0,
        100,
    );
    (Processor::default(), term, proxy, None)
}

fn feed(
    fixture: &mut (
        Processor,
        Crosswords<NeomacsEventProxy>,
        NeomacsEventProxy,
        Option<Vec<u8>>,
    ),
    bytes: &[u8],
) {
    fixture.0.advance(&mut fixture.1, bytes);
    fixture
        .2
        .observe_native_directory(&fixture.1, &mut fixture.3, Some("machine"));
}

#[test]
fn native_cwd_fragmented_osc_preserves_full_uri_and_decodes_unicode() {
    let mut f = native_fixture(71);
    feed(&mut f, b"\x1b]7;file://machine/home/%CE");
    assert_eq!(f.2.take_directory(), None);
    feed(&mut f, b"%B1%20b\x1b");
    // The optional metadata waits for the complete ST, not just its ESC.
    assert_eq!(f.2.take_directory(), None);
    feed(&mut f, b"\\");
    assert_eq!(
        f.1.current_directory_uri.as_deref(),
        Some(b"file://machine/home/%CE%B1%20b".as_slice())
    );
    assert_eq!(f.2.take_directory().as_deref(), Some("/home/α b"));
}

#[test]
fn native_cwd_foreign_authority_cannot_cancel_into_a_local_path() {
    let mut f = native_fixture(72);
    feed(&mut f, b"\x1b]7;file://machine/private\x07");
    feed(&mut f, b"\x1b]7;file://foreign/private\x07");
    assert_eq!(
        f.1.current_directory.as_deref(),
        Some(std::path::Path::new("/private"))
    );
    assert_eq!(f.2.take_directory(), None);
    feed(&mut f, b"\x1b]7;file:///private\x07");
    assert_eq!(f.2.take_directory().as_deref(), Some("/private"));
}

#[test]
fn native_cwd_raw_invalid_evidence_is_not_normalized_into_permission() {
    for sequence in [
        b"\x1b]7;file:///a%GG\x07".as_slice(),
        b"\x1b]7;file:///a%00\x07",
        b"\x1b]7;file:///a%FF\x07",
        b"\x1b]7;file:///a\xff\x07",
        b"\x1b]7;file:///a\0b\x07",
        b"\x1b]7;file:///a;extra\x07",
        b"\x1b]7;file:///a\x18",
        b"\x1b]7;file:///a\x1bX",
    ] {
        let mut f = native_fixture(73);
        feed(&mut f, b"\x1b]7;file:///pending\x07");
        feed(&mut f, sequence);
        assert_eq!(f.2.take_directory(), None, "{sequence:?}");
    }
}

#[test]
fn native_cwd_latest_only_no_metadata_and_cd_back() {
    let mut f = native_fixture(74);
    feed(&mut f, b"plain terminal output");
    assert_eq!(f.2.take_directory(), None);
    feed(&mut f, b"\x1b]7;file:///a\x07\x1b]7;file:///b\x07");
    assert_eq!(f.2.take_directory().as_deref(), Some("/b"));
    for _ in 0..32 {
        feed(&mut f, b"\x1b]7;file://localhost/b\x07output\r\n");
    }
    assert_eq!(f.2.take_directory(), None);
    feed(&mut f, b"\x1b]7;file:///a\x07");
    assert_eq!(f.2.take_directory().as_deref(), Some("/a"));
}

#[test]
fn native_cwd_dot_segments_cannot_expose_magic_paths() {
    for path in [
        "/tmp/../ssh:host:/work",
        "/./ssh:host:/work",
        "/tmp/../sudo::/work",
        "/./sudo::/work",
        "/tmp/../:/work",
        "/./:/work",
        "/tmp/%2e%2E/ssh:host:/work",
        "/%2E/sudo::/work",
        "/tmp/.%2e/:/work",
        "/tmp/%2e./ssh:host:/work",
        "/tmp%2F%2e%2e%2Fsudo::/work",
    ] {
        let mut f = native_fixture(77);
        feed(&mut f, b"\x1b]7;file:///pending\x07");
        let uri = format!("file://{path}");
        feed(&mut f, format!("\x1b]7;{uri}\x07").as_bytes());
        // The real provider retains raw evidence; product policy rejects it
        // and cancels the undelivered valid report, without URL normalization.
        assert_eq!(f.1.current_directory_uri.as_deref(), Some(uri.as_bytes()));
        assert_eq!(f.2.take_directory(), None, "{path}");
        feed(&mut f, b"\x1b]7;file:///home/%CE%B1/.hidden/%252e%252e\x07");
        assert_eq!(
            f.2.take_directory().as_deref(),
            Some("/home/α/.hidden/%2e%2e")
        );
    }
}

#[test]
fn native_cwd_terminal_occurrences_keep_independent_slots() {
    let mut old = native_fixture(75);
    let mut new = native_fixture(76);
    feed(&mut old, b"\x1b]7;file:///old\x07");
    feed(&mut new, b"\x1b]7;file:///new\x07");
    assert_eq!(new.2.take_directory().as_deref(), Some("/new"));
    assert_eq!(old.2.take_directory().as_deref(), Some("/old"));
    assert_eq!(new.2.take_directory(), None);
}
