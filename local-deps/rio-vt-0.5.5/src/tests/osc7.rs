//! Native provider API regression source; no PTY/GPU backend is involved.
use crate::ansi::CursorShape;
use crate::crosswords::{Crosswords, CrosswordsSize};
use crate::event::{VoidListener, WindowId};
use crate::performer::handler::{Handler, Processor};
use crate::performer::parser::{Parser, Perform};
use std::path::PathBuf;

fn fixture() -> (Processor, Crosswords<VoidListener>) {
    (
        Processor::default(),
        Crosswords::new(
            CrosswordsSize::new(20, 5),
            CursorShape::Block,
            VoidListener,
            WindowId::from(0),
            0,
            100,
        ),
    )
}

#[test]
fn osc7_preserves_full_local_and_foreign_uris_without_changing_legacy_paths() {
    let (mut parser, mut term) = fixture();
    for uri in [
        "file://machine/private",
        "file://foreign/private",
        "ssh://foreign/private",
        "file://user:password@foreign/private?q#f",
    ] {
        parser.advance(&mut term, format!("\x1b]7;{uri}\x07").as_bytes());
        assert_eq!(term.current_directory_uri.as_deref(), Some(uri.as_bytes()));
        assert_eq!(
            term.current_directory.as_deref(),
            Some(std::path::Path::new("/private"))
        );
    }
}

#[test]
fn osc7_fragmented_st_requires_its_final_byte_for_optional_metadata() {
    let (mut parser, mut term) = fixture();
    parser.advance(&mut term, b"\x1b]7;file://machine/a%20");
    assert_eq!(term.current_directory_uri, None);
    parser.advance(&mut term, b"b\x1b");
    assert_eq!(term.current_directory_uri, None);
    // Legacy semantics still dispatch at ESC, before ST's backslash.
    assert_eq!(
        term.current_directory.as_deref(),
        Some(std::path::Path::new("/a%20b"))
    );
    parser.advance(&mut term, b"\\");
    assert_eq!(
        term.current_directory_uri.as_deref(),
        Some(b"file://machine/a%20b".as_slice())
    );
}

#[test]
fn osc7_metadata_preserves_unicode_bad_percent_and_invalid_utf8_for_consumer_policy() {
    let (mut parser, mut term) = fixture();
    for uri in [
        "file:///α%20b".as_bytes(),
        b"file:///a%GG",
        b"file:///a%00",
        b"file:///a\xff",
    ] {
        let mut bytes = b"\x1b]7;".to_vec();
        bytes.extend_from_slice(uri);
        bytes.push(7);
        parser.advance(&mut term, &bytes);
        assert_eq!(term.current_directory_uri.as_deref(), Some(uri));
    }
}

#[test]
fn osc7_discarded_controls_extra_params_and_cancelled_osc_clear_only_metadata() {
    let (mut parser, mut term) = fixture();
    for sequence in [
        b"\x1b]7;file:///a\0b\x07".as_slice(),
        b"\x1b]7;file:///ab;extra\x07",
        b"\x1b]7;file:///ab\x18",
        b"\x1b]7;file:///ab\x1bX",
    ] {
        parser.advance(&mut term, b"\x1b]7;file:///previous\x07");
        parser.advance(&mut term, sequence);
        assert_eq!(term.current_directory_uri, None, "{sequence:?}");
        assert_eq!(
            term.current_directory.as_deref(),
            Some(std::path::Path::new("/ab"))
        );
    }
    parser.advance(&mut term, b"\x1b]7;file:///recovered\x07");
    assert_eq!(
        term.current_directory_uri.as_deref(),
        Some(b"file:///recovered".as_slice())
    );
}

#[test]
fn osc7_legacy_handler_implementations_need_no_new_method() {
    #[derive(Default)]
    struct Legacy {
        path: Option<PathBuf>,
    }
    impl Handler for Legacy {
        fn set_current_directory(&mut self, path: PathBuf) {
            self.path = Some(path);
        }
    }
    let mut legacy = Legacy::default();
    Processor::default().advance(&mut legacy, b"\x1b]7;file:///legacy;extra\x07");
    assert_eq!(
        legacy.path.as_deref(),
        Some(std::path::Path::new("/legacy"))
    );
}

#[test]
fn osc7_legacy_perform_dispatch_still_occurs_once_at_esc() {
    #[derive(Default)]
    struct Legacy {
        osc_count: usize,
    }
    impl Perform for Legacy {
        fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {
            self.osc_count += 1;
        }
    }
    let mut parser = Parser::default();
    let mut legacy = Legacy::default();
    parser.advance(&mut legacy, b"\x1b]7;file:///legacy\x1b");
    assert_eq!(legacy.osc_count, 1);
    parser.advance(&mut legacy, b"\\");
    assert_eq!(legacy.osc_count, 1);
}
