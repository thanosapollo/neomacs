//! Original native dispatch/string/buffer/property controls, verbatim bodies.
use neovm_core::Value;
use neovm_core::emacs_core::error::{Flow,FlowKind,FlowResultExt as _};
type EvalResult = Result<Value, Flow>;
use crate::buffer::CharRange;
use neovm_core::case68_test_support::*;

fn put_string_property(
    table: &mut crate::buffer::text_props::TextPropertyTable,
    start: usize,
    end: usize,
    name: Value,
    value: Value,
) -> bool {
    table.put_property_in_char_range(CharRange::from_usize(start, end), name, value)
}

fn dispatch_builtin_pure(name: &str, args: Vec<Value>) -> Option<EvalResult> {
    super::dispatch_builtin_without_eval_state(name, args)
}

#[test]
fn pure_dispatch_typed_downcase_unicode_edge_payloads_match_oracle() {
    crate::test_utils::init_test_tracing();
    let cases = [
        (304, 304),
        (7305, 7305),
        (8490, 8490),
        (42955, 42955),
        (42956, 42956),
        (42958, 42958),
        (42962, 42962),
        (42964, 42964),
        (42970, 42970),
        (42972, 42972),
        (68944, 68944),
        (68965, 68965),
        (93856, 93856),
        (93880, 93880),
        (66560, 66600),
    ];

    for (input, expected) in cases {
        let result = dispatch_builtin_pure("downcase", vec![Value::fixnum(input)])
            .expect("builtin downcase should resolve")
            .expect("builtin downcase should evaluate");
        assert_eq!(
            result,
            Value::fixnum(expected),
            "downcase({input}) should equal {expected}"
        );
    }

    let dotted_i = dispatch_builtin_pure("downcase", vec![Value::char('\u{0130}')])
        .expect("builtin downcase should resolve")
        .expect("builtin downcase should evaluate");
    assert_eq!(dotted_i, Value::char('\u{0130}'));

    let kelvin = dispatch_builtin_pure("downcase", vec![Value::string("\u{212A}")])
        .expect("builtin downcase should resolve")
        .expect("builtin downcase should evaluate");
    assert_eq!(kelvin, Value::string("\u{212A}"));

    let dotted_i_string = dispatch_builtin_pure("downcase", vec![Value::string("\u{0130}")])
        .expect("builtin downcase should resolve")
        .expect("builtin downcase should evaluate");
    assert_eq!(dotted_i_string, Value::string("i\u{307}"));

    let preserve_latin = dispatch_builtin_pure("downcase", vec![Value::string("\u{A7CB}")])
        .expect("builtin downcase should resolve")
        .expect("builtin downcase should evaluate");
    assert_eq!(preserve_latin, Value::string("\u{A7CB}"));

    let preserve_cyrillic_sup = dispatch_builtin_pure("downcase", vec![Value::string("\u{10D50}")])
        .expect("builtin downcase should resolve")
        .expect("builtin downcase should evaluate");
    assert_eq!(preserve_cyrillic_sup, Value::string("\u{10D50}"));

    let preserve_adlam = dispatch_builtin_pure("downcase", vec![Value::string("\u{16EA0}")])
        .expect("builtin downcase should resolve")
        .expect("builtin downcase should evaluate");
    assert_eq!(preserve_adlam, Value::string("\u{16EA0}"));

    let negative = dispatch_builtin_pure("downcase", vec![Value::fixnum(-1)])
        .expect("builtin downcase should resolve")
        .expect_err("builtin downcase should reject negative integer designators");
    match negative.into_kind() {
        FlowKind::Signal(sig) => {
            assert_eq!(sig.symbol_name(), "wrong-type-argument");
            assert_eq!(
                sig.data,
                vec![Value::symbol("char-or-string-p"), Value::fixnum(-1)]
            );
        }
        other => panic!("unexpected flow: {other:?}"),
    }
}

#[test]
fn pure_dispatch_typed_upcase_unicode_edge_payloads_match_oracle() {
    crate::test_utils::init_test_tracing();
    let cases = [
        (223, 7838),
        (305, 305),
        (7306, 7306),
        (8064, 8072),
        (8071, 8079),
        (8080, 8088),
        (8087, 8095),
        (8096, 8104),
        (8103, 8111),
        (8115, 8124),
        (8131, 8140),
        (8179, 8188),
        (42957, 42957),
        (68976, 68976),
        (68997, 68997),
        (93883, 93883),
        (93907, 93907),
        (97, 65),
    ];

    for (input, expected) in cases {
        let result = dispatch_builtin_pure("upcase", vec![Value::fixnum(input)])
            .expect("builtin upcase should resolve")
            .expect("builtin upcase should evaluate");
        assert_eq!(
            result,
            Value::fixnum(expected),
            "upcase({input}) should equal {expected}"
        );
    }

    let sharp_s = dispatch_builtin_pure("upcase", vec![Value::char('ß')])
        .expect("builtin upcase should resolve")
        .expect("builtin upcase should evaluate");
    assert_eq!(sharp_s, Value::char('\u{1E9E}'));

    let sharp_s_string = dispatch_builtin_pure("upcase", vec![Value::string("ß")])
        .expect("builtin upcase should resolve")
        .expect("builtin upcase should evaluate");
    assert_eq!(sharp_s_string, Value::string("SS"));

    let dotless_i_string = dispatch_builtin_pure("upcase", vec![Value::string("\u{0131}")])
        .expect("builtin upcase should resolve")
        .expect("builtin upcase should evaluate");
    assert_eq!(dotless_i_string, Value::string("\u{0131}"));

    let preserve_latin = dispatch_builtin_pure("upcase", vec![Value::string("\u{019B}")])
        .expect("builtin upcase should resolve")
        .expect("builtin upcase should evaluate");
    assert_eq!(preserve_latin, Value::string("\u{019B}"));

    let preserve_cyrillic_sup = dispatch_builtin_pure("upcase", vec![Value::string("\u{10D70}")])
        .expect("builtin upcase should resolve")
        .expect("builtin upcase should evaluate");
    assert_eq!(preserve_cyrillic_sup, Value::string("\u{10D70}"));

    let preserve_adlam = dispatch_builtin_pure("upcase", vec![Value::string("\u{16EBB}")])
        .expect("builtin upcase should resolve")
        .expect("builtin upcase should evaluate");
    assert_eq!(preserve_adlam, Value::string("\u{16EBB}"));

    let negative = dispatch_builtin_pure("upcase", vec![Value::fixnum(-1)])
        .expect("builtin upcase should resolve")
        .expect_err("builtin upcase should reject negative integer designators");
    match negative.into_kind() {
        FlowKind::Signal(sig) => {
            assert_eq!(sig.symbol_name(), "wrong-type-argument");
            assert_eq!(
                sig.data,
                vec![Value::symbol("char-or-string-p"), Value::fixnum(-1)]
            );
        }
        other => panic!("unexpected flow: {other:?}"),
    }
}

#[test]
fn pure_dispatch_typed_case_conversion_preserves_raw_unibyte_payloads() {
    crate::test_utils::init_test_tracing();
    let upper_raw = Value::heap_string(crate::heap_types::LispString::from_unibyte(vec![
        b'A', 0xFF,
    ]));
    let downcased = dispatch_builtin_pure("downcase", vec![upper_raw])
        .expect("builtin downcase should resolve")
        .expect("builtin downcase should evaluate");
    let downcased = downcased.as_lisp_string().expect("downcase string");
    assert!(!downcased.is_multibyte());
    assert_eq!(downcased.as_bytes(), &[b'a', 0xFF]);

    let lower_raw = Value::heap_string(crate::heap_types::LispString::from_unibyte(vec![
        b'a', 0xFF,
    ]));
    let upcased = dispatch_builtin_pure("upcase", vec![lower_raw])
        .expect("builtin upcase should resolve")
        .expect("builtin upcase should evaluate");
    let upcased = upcased.as_lisp_string().expect("upcase string");
    assert!(!upcased.is_multibyte());
    assert_eq!(upcased.as_bytes(), &[b'A', 0xFF]);
}

#[test]
fn pure_dispatch_typed_propertize_validates_and_returns_string() {
    crate::test_utils::init_test_tracing();
    let result = dispatch_builtin_pure(
        "propertize",
        vec![
            Value::string("x"),
            Value::symbol("face"),
            Value::symbol("bold"),
        ],
    )
    .expect("builtin propertize should resolve")
    .expect("builtin propertize should evaluate");
    assert_eq!(result, Value::string("x"));
}

#[test]
fn pure_dispatch_typed_propertize_non_string_signals_stringp() {
    crate::test_utils::init_test_tracing();
    let result = dispatch_builtin_pure("propertize", vec![Value::fixnum(1)])
        .expect("builtin propertize should resolve")
        .expect_err("propertize should reject non-string first arg");
    match result.into_kind() {
        FlowKind::Signal(sig) => {
            assert_eq!(sig.symbol_name(), "wrong-type-argument");
            assert_eq!(sig.data, vec![Value::symbol("stringp"), Value::fixnum(1)]);
        }
        other => panic!("unexpected flow: {other:?}"),
    }
}

#[test]
fn pure_dispatch_typed_propertize_odd_property_list_signals_arity() {
    crate::test_utils::init_test_tracing();
    let result = dispatch_builtin_pure(
        "propertize",
        vec![Value::string("x"), Value::symbol("face")],
    )
    .expect("builtin propertize should resolve")
    .expect_err("propertize should reject odd property argument count");
    match result.into_kind() {
        FlowKind::Signal(sig) => {
            assert_eq!(sig.symbol_name(), "wrong-number-of-arguments");
            assert_eq!(
                sig.data,
                vec![Value::symbol("propertize"), Value::fixnum(2)]
            );
        }
        other => panic!("unexpected flow: {other:?}"),
    }
}

#[test]
fn pure_dispatch_typed_propertize_accepts_non_symbol_property_keys() {
    crate::test_utils::init_test_tracing();
    let result = dispatch_builtin_pure(
        "propertize",
        vec![Value::string("x"), Value::fixnum(1), Value::symbol("v")],
    )
    .expect("builtin propertize should resolve")
    .expect("builtin propertize should evaluate");
    assert_eq!(result, Value::string("x"));
}

#[test]
fn pure_dispatch_typed_propertize_preserves_raw_unibyte_payload() {
    crate::test_utils::init_test_tracing();
    let raw = Value::heap_string(crate::heap_types::LispString::from_unibyte(vec![
        b'x', 0xFF,
    ]));
    let result = dispatch_builtin_pure(
        "propertize",
        vec![raw, Value::symbol("face"), Value::symbol("bold")],
    )
    .expect("builtin propertize should resolve")
    .expect("builtin propertize should evaluate");
    let result = result.as_lisp_string().expect("string");
    assert!(!result.is_multibyte());
    assert_eq!(result.as_bytes(), &[b'x', 0xFF]);
}

#[test]
fn replace_match_string_preserves_source_and_replacement_text_properties_like_gnu() {
    crate::test_utils::init_test_tracing();
    use crate::emacs_core::eval::Context;

    fn put_face(value: Value, start: usize, end: usize, face: &str) {
        let mut table = crate::emacs_core::value::get_string_text_properties_table_for_value(value)
            .unwrap_or_default();
        let _ = put_string_property(
            &mut table,
            start,
            end,
            Value::symbol("face"),
            Value::symbol(face),
        );
        crate::emacs_core::value::set_string_text_properties_table_for_value(value, table);
    }

    fn assert_face_run(
        interval: &crate::buffer::text_props::PropertyInterval,
        start: usize,
        end: usize,
        face: &str,
    ) {
        assert_eq!(interval.start, start);
        assert_eq!(interval.end, end);
        assert_eq!(
            interval.properties.get(&Value::symbol("face")),
            Some(&Value::symbol(face))
        );
    }

    let mut eval = Context::new();
    let source = Value::string("abcde");
    put_face(source, 0, 2, "a");
    put_face(source, 2, 5, "b");
    let replacement = Value::string("XY");
    put_face(replacement, 0, 2, "x");

    builtin_string_match(&mut eval, vec![Value::string("bc"), source]).expect("seed match data");
    let result = builtin_replace_match(&mut eval, vec![replacement, Value::T, Value::NIL, source])
        .expect("replace-match should preserve string intervals");

    assert_eq!(result.as_runtime_string_owned().as_deref(), Some("aXYde"));
    let table = crate::emacs_core::value::get_string_text_properties_table_for_value(result)
        .expect("result should keep text properties");
    let intervals = table.intervals_snapshot();
    assert_eq!(intervals.len(), 3);
    assert_face_run(&intervals[0], 0, 1, "a");
    assert_face_run(&intervals[1], 1, 3, "x");
    assert_face_run(&intervals[2], 3, 5, "b");

    let source_table = crate::emacs_core::value::get_string_text_properties_table_for_value(source)
        .expect("source should remain propertized");
    let source_intervals = source_table.intervals_snapshot();
    assert_eq!(source_intervals.len(), 2);
    assert_face_run(&source_intervals[0], 0, 2, "a");
    assert_face_run(&source_intervals[1], 2, 5, "b");
}

#[test]
fn replace_match_buffer_updates_live_match_data_like_gnu() {
    crate::test_utils::init_test_tracing();
    use crate::emacs_core::eval::Context;

    let mut eval = Context::new();
    {
        let buffer = eval.buffers.current_buffer_mut().expect("scratch buffer");
        buffer.insert("foo-42");
        buffer.goto_emacs_byte_pos(crate::buffer::EmacsBytePos::new(0));
    }

    builtin_re_search_forward(&mut eval, vec![Value::string("\\([a-z]+\\)-\\([0-9]+\\)")])
        .expect("seed buffer match data");
    builtin_replace_match(&mut eval, vec![Value::string("\\2-\\1")])
        .expect("replace-match should succeed");

    let buffer = eval.buffers.current_buffer().expect("scratch buffer");
    assert_eq!(
        buffer.buffer_substring_range(crate::buffer::EmacsByteRange::from_usize(
            0,
            buffer.total_emacs_byte_len().get(),
        )),
        "42-foo"
    );

    assert_eq!(
        builtin_match_beginning(&mut eval, vec![Value::fixnum(0)]).expect("match-beginning 0"),
        Value::fixnum(1)
    );
    assert_eq!(
        builtin_match_end(&mut eval, vec![Value::fixnum(0)]).expect("match-end 0"),
        Value::fixnum(7)
    );
    assert_eq!(
        builtin_match_beginning(&mut eval, vec![Value::fixnum(1)]).expect("match-beginning 1"),
        Value::fixnum(1)
    );
    assert_eq!(
        builtin_match_end(&mut eval, vec![Value::fixnum(1)]).expect("match-end 1"),
        Value::fixnum(1)
    );
    assert_eq!(
        builtin_match_beginning(&mut eval, vec![Value::fixnum(2)]).expect("match-beginning 2"),
        Value::fixnum(1)
    );
    assert_eq!(
        builtin_match_end(&mut eval, vec![Value::fixnum(2)]).expect("match-end 2"),
        Value::fixnum(7)
    );
}

#[test]
fn case_and_propertize_preserve_private_use_glyphs_issue_131() {
    crate::test_utils::init_test_tracing();
    let mut eval = crate::emacs_core::eval::Context::new();

    let result = eval
        .eval_str(
            r#"
            (mapcar (lambda (cp)
                      (let ((s (char-to-string cp)))
                        (and (= (aref (upcase s) 0) cp)
                             (= (aref (downcase s) 0) cp)
                             (= (aref (capitalize s) 0) cp)
                             (= (aref (upcase-initials s) 0) cp)
                             (= (aref (propertize s 'k 1) 0) cp))))
                    '(#xe080 #xe0a0 #xe0ff #xe380 #xe3a0 #xe3ff))
            "#,
        )
        .expect("case/propertize builtins should preserve real PUA glyphs");

    assert_eq!(result, Value::list(vec![Value::T; 6]));
}
