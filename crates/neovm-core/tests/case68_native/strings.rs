//! Original native strings case/property controls, verbatim bodies.
use neovm_core::{Context,Value};
use crate::buffer::CharRange;
use crate::heap_types::LispString;
use neovm_core::case68_test_support::{builtin_concat,builtin_format_wrapper_strict_slice,builtin_format_message_slice};

fn put_string_property(
    table: &mut crate::buffer::text_props::TextPropertyTable,
    start: usize,
    end: usize,
    name: Value,
    value: Value,
) -> bool {
    table.put_property_in_char_range(CharRange::from_usize(start, end), name, value)
}

#[test]
fn format_percent_s_takes_a_symbols_properties_from_its_name_string() {
    crate::test_utils::init_test_tracing();
    let mut eval = crate::emacs_core::eval::Context::new();
    let rendered = eval
        .eval_str(
            r#"(prin1-to-string
                 (let* ((name (propertize "p15-fmt-symbol" 'fontified nil))
                        (sym (intern name)))
                   (list (text-properties-at 0 (format "%s" sym))
                         ;; `%S' prints a fresh representation in GNU too, so it
                         ;; carries nothing -- the two conversions must differ.
                         (text-properties-at 0 (format "%S" sym))
                         ;; The offset case the package hits: the symbol lands in
                         ;; the middle of a longer message.
                         (text-properties-at 6 (format "find: %s" sym)))))"#,
        )
        .unwrap();
    // One property only: with several, GNU's own plist ORDER differs between
    // these two calls, so ordering is not a contract worth pinning.
    assert_eq!(
        rendered.as_str_owned().as_deref(),
        Some("((fontified nil) nil (fontified nil))"),
    );
}

#[test]
fn substring_copies_text_properties_through_gnu_add_properties_order() {
    crate::test_utils::init_test_tracing();

    let mut eval = crate::emacs_core::eval::Context::new();
    let result = eval
        .eval_str(
            r#"(let* ((s (propertize "abcdef" 'face 'bold 'tag 'source))
                      (sub (substring s 1 5)))
                 (text-properties-at 0 sub))"#,
        )
        .expect("evaluation succeeds");

    assert_eq!(
        crate::emacs_core::print::print_value(&result),
        "(tag source face bold)"
    );
}

#[test]
fn concat_preserves_multibyte_text_properties_as_char_intervals() {
    crate::test_utils::init_test_tracing();

    let source = Value::string("é");
    let mut table = crate::buffer::text_props::TextPropertyTable::new();
    put_string_property(
        &mut table,
        0,
        1,
        Value::symbol("face"),
        Value::symbol("bold"),
    );
    crate::emacs_core::value::set_string_text_properties_table_for_value(source, table);

    let result = builtin_concat(vec![Value::string("x"), source, Value::string("z")])
        .expect("concat should preserve string properties");
    let props = crate::emacs_core::value::get_string_text_properties_table_for_value(result)
        .expect("result should carry text properties");
    let intervals = props.intervals_snapshot();

    assert_eq!(intervals.len(), 1);
    assert_eq!((intervals[0].start, intervals[0].end), (1, 2));
    assert_eq!(
        intervals[0].properties.get(&Value::symbol("face")),
        Some(&Value::symbol("bold"))
    );
}

#[test]
fn concat_applies_source_plists_in_gnu_add_properties_order() {
    crate::test_utils::init_test_tracing();

    let mut eval = crate::emacs_core::eval::Context::new();
    let result = eval
        .eval_str(
            r#"(let* ((source (propertize "a" 'face 'bold 'tag 'source))
                      (result (concat source)))
                 (text-properties-at 0 result))"#,
        )
        .expect("evaluation succeeds");

    assert_eq!(
        crate::emacs_core::print::print_value(&result),
        "(tag source face bold)"
    );
}

#[test]
fn concat_all_unibyte_strings_with_properties_stays_unibyte_like_gnu() {
    crate::test_utils::init_test_tracing();

    let raw = Value::heap_string(LispString::from_unibyte(vec![0xff]));
    let suffix = Value::heap_string(LispString::from_unibyte(vec![b'a']));
    let mut table = crate::buffer::text_props::TextPropertyTable::new();
    put_string_property(
        &mut table,
        0,
        1,
        Value::symbol("face"),
        Value::symbol("bold"),
    );
    crate::emacs_core::value::set_string_text_properties_table_for_value(raw, table);

    let result = builtin_concat(vec![raw, suffix]).expect("concat should preserve unibyte storage");
    let string = result.as_lisp_string().expect("concat returns a string");
    let props = crate::emacs_core::value::get_string_text_properties_table_for_value(result)
        .expect("result should carry text properties");
    let intervals = props.intervals_snapshot();

    assert!(!string.is_multibyte());
    assert_eq!(string.as_bytes(), &[0xff, b'a']);
    assert_eq!(intervals.len(), 1);
    assert_eq!((intervals[0].start, intervals[0].end), (0, 1));
    assert_eq!(
        intervals[0].properties.get(&Value::symbol("face")),
        Some(&Value::symbol("bold"))
    );
}

#[test]
fn format_preserves_multibyte_text_properties_as_char_intervals() {
    crate::test_utils::init_test_tracing();

    let source = Value::string("éz");
    let mut table = crate::buffer::text_props::TextPropertyTable::new();
    put_string_property(
        &mut table,
        0,
        1,
        Value::symbol("face"),
        Value::symbol("bold"),
    );
    crate::emacs_core::value::set_string_text_properties_table_for_value(source, table);

    let mut ctx = crate::emacs_core::eval::Context::new();
    let result = builtin_format_wrapper_strict_slice(&mut ctx, &[Value::string("%4s"), source])
        .expect("format should preserve string properties");
    let props = crate::emacs_core::value::get_string_text_properties_table_for_value(result)
        .expect("result should carry text properties");
    let intervals = props.intervals_snapshot();

    assert_eq!(result.as_utf8_str(), Some("  éz"));
    assert_eq!(intervals.len(), 1);
    assert_eq!((intervals[0].start, intervals[0].end), (2, 3));
    assert_eq!(
        intervals[0].properties.get(&Value::symbol("face")),
        Some(&Value::symbol("bold"))
    );
}

#[test]
fn format_reverses_percent_s_text_property_plist_order_like_gnu_add_properties() {
    crate::test_utils::init_test_tracing();

    let source = Value::string("key");
    let mut table = crate::buffer::text_props::TextPropertyTable::new();
    put_string_property(
        &mut table,
        0,
        3,
        Value::symbol("face"),
        Value::symbol("help-key-binding"),
    );
    put_string_property(
        &mut table,
        0,
        3,
        Value::symbol("font-lock-face"),
        Value::symbol("help-key-binding"),
    );
    crate::emacs_core::value::set_string_text_properties_table_for_value(source, table);

    let mut ctx = crate::emacs_core::eval::Context::new();
    let result = builtin_format_wrapper_strict_slice(&mut ctx, &[Value::string("%s ok"), source])
        .expect("format should preserve string properties");
    let props = crate::emacs_core::value::get_string_text_properties_table_for_value(result)
        .expect("result should carry text properties");
    let intervals = props.intervals_snapshot();
    let ordered_keys: Vec<_> = intervals[0]
        .ordered_properties()
        .map(|(name, _)| name.as_symbol_name().unwrap().to_string())
        .collect();

    assert_eq!(result.as_utf8_str(), Some("key ok"));
    assert_eq!(
        ordered_keys,
        vec!["face".to_string(), "font-lock-face".to_string()]
    );
}

#[test]
fn downcase_greek_final_sigma() {
    // GNU `casefiddle.c` `case_character`: a down-cased capital sigma becomes
    // the final form ς at the end of a word (preceding char is a word
    // constituent, following one is not), σ otherwise.
    crate::test_utils::init_test_tracing();
    let mut ev = crate::test_utils::runtime_startup_context();
    let cases = [
        (r#"(downcase "ΑΣ")"#, "ας"),       // Σ ends the word → ς
        (r#"(downcase "ΣΑ")"#, "σα"),       // Σ starts the word → σ
        (r#"(downcase "ΑΣΑ")"#, "ασα"),     // medial Σ → σ
        (r#"(downcase "Σ")"#, "σ"),         // lone Σ (no preceding word) → σ
        (r#"(downcase "ΟΔΟΣ")"#, "οδος"),   // word ends Σ → ς
        (r#"(downcase "ΑΣ ΒΣ")"#, "ας βς"), // both at word ends → ς
        (r#"(downcase "ΑΣ_")"#, "ας_"),     // _ is a non-word boundary → ς
        // case-symbols-as-words makes _ a word constituent → not at word end → σ
        (
            r#"(let ((case-symbols-as-words t)) (downcase "ΑΣ_"))"#,
            "ασ_",
        ),
        // upcase is unaffected.
        (r#"(upcase "ας")"#, "ΑΣ"),
    ];
    for (form, expected) in cases {
        let result = ev.eval_str(form).expect("eval");
        assert_eq!(result.as_utf8_str(), Some(expected), "form: {form}");
    }
}

#[test]
fn format_maps_format_string_properties_across_conversion_fields_like_gnu() {
    crate::test_utils::init_test_tracing();
    let mut ev = crate::emacs_core::eval::Context::new();
    let cases: &[(&str, &str)] = &[
        // xref's first pass: build the width-specific line format.
        (
            r#"(format #("%%%dd:" 0 4 (face xref-line-number) 5 6 (face shadow)) 1)"#,
            r##"#("%1d:" 0 2 (face xref-line-number) 3 4 (face shadow))"##,
        ),
        // xref's second pass: this is the one that used to lose the face.
        (
            r#"(format #("%1d:" 0 2 (face xref-line-number) 3 4 (face shadow)) 3)"#,
            r##"#("3:" 0 1 (face xref-line-number) 1 2 (face shadow))"##,
        ),
        (
            r#"(format #("%%%dd:" 0 4 (face xref-line-number) 5 6 (face shadow)) 2)"#,
            r##"#("%2d:" 0 2 (face xref-line-number) 3 4 (face shadow))"##,
        ),
        // Padding belongs to the field, so the property covers it too.
        (
            r#"(format #("%2d:" 0 3 (face xref-line-number) 3 4 (face shadow)) 7)"#,
            r##"#(" 7:" 0 2 (face xref-line-number) 2 3 (face shadow))"##,
        ),
        (
            r#"(format #("%5dX" 0 2 (face f2)) 7)"#,
            r##"#("    7X" 0 5 (face f2))"##,
        ),
        // A boundary that STARTS inside the spec also lands at the field end.
        (
            r#"(format #("a%dbb" 2 4 (face f6)) 9)"#,
            r##"#("a9bb" 2 3 (face f6))"##,
        ),
        // `%%` is not a conversion field: its discarded `%` has nothing to
        // jump over, so a boundary between the two `%`s stays put and the
        // property collapses to nothing.
        (r#"(format #("%%x" 0 1 (face f1)))"#, r#""%x""#),
    ];
    for (form, expected) in cases {
        let result = ev.eval_str(form).expect("eval");
        assert_eq!(
            crate::emacs_core::print::print_value(&result),
            *expected,
            "form: {form}"
        );
    }
}

#[test]
fn format_without_directives_returns_the_format_string_unchanged_like_gnu() {
    crate::test_utils::init_test_tracing();
    let mut ctx = crate::emacs_core::eval::Context::new();

    let preserved = ctx
        .eval_str(
            r#"(format "%S" (text-properties-at
                 0 (format (propertize "no directives here" 'f1 1 'f2 2 'f3 3))))"#,
        )
        .expect("format of a directive-free string should succeed");
    assert_eq!(
        preserved.as_utf8_str(),
        Some("(f1 1 f2 2 f3 3)"),
        "nothing was formatted, so GNU copies no properties and the supplied \
         order survives"
    );

    // GNU returns the very same object in that case.
    let identical = ctx
        .eval_str(r#"(let ((s (propertize "no directives here" 'f1 1))) (eq s (format s)))"#)
        .expect("eq check should succeed");
    assert!(!identical.is_nil(), "GNU returns args[0] itself");

    // The additive transfer that `4b6132cea` fixed must still reverse when the
    // format actually formats something (ac-helm / ac-php parity).
    let reversed = ctx
        .eval_str(
            r#"(format "%S" (text-properties-at
                 0 (format (propertize "X%sY" 'f1 1 'f2 2 'f3 3) "a")))"#,
        )
        .expect("format with a directive should succeed");
    assert_eq!(
        reversed.as_utf8_str(),
        Some("(f3 3 f2 2 f1 1)"),
        "a real conversion still copies the format plist with GNU's \
         additive-prepend order"
    );
}

#[test]
fn format_exact_percent_s_reuses_the_string_argument_like_gnu() {
    crate::test_utils::init_test_tracing();

    let source = Value::string("key");
    let mut table = crate::buffer::text_props::TextPropertyTable::new();
    put_string_property(
        &mut table,
        0,
        3,
        Value::symbol("face"),
        Value::symbol("help-key-binding"),
    );
    put_string_property(
        &mut table,
        0,
        3,
        Value::symbol("font-lock-face"),
        Value::symbol("help-key-binding"),
    );
    crate::emacs_core::value::set_string_text_properties_table_for_value(source, table);

    let mut ctx = crate::emacs_core::eval::Context::new();
    let formatted =
        builtin_format_wrapper_strict_slice(&mut ctx, &[Value::string("%s"), source]).unwrap();
    let message = builtin_format_message_slice(&mut ctx, &[Value::string("%s"), source]).unwrap();

    assert_eq!(formatted, source);
    assert_eq!(message, source);
}
