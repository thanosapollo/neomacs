use super::super::intern::{intern, intern_uninterned, intern_uninterned_lisp_string};
use super::super::marker::make_marker_value;
use super::*;
use crate::buffer::LispCharPos1;
use crate::emacs_core::builtins::{builtin_puthash, builtin_remhash};
use crate::emacs_core::value::{
    HashTableTest, LambdaData, LambdaParams, StringTextPropertyRun,
    set_string_text_properties_for_value,
};

// Independent of the production scope: even a RED assertion must not leak TLS
// roots into another test, and cleanup runs before the Context drops its heap.
struct BytecodeTestRootCleanup(usize);

impl BytecodeTestRootCleanup {
    fn new() -> Self {
        Self(crate::emacs_core::eval::save_scratch_gc_roots())
    }
}

impl Drop for BytecodeTestRootCleanup {
    fn drop(&mut self) {
        crate::emacs_core::eval::restore_scratch_gc_roots(self.0);
    }
}

fn bytecode_root_fixture() -> Value {
    let mut function =
        crate::emacs_core::bytecode::ByteCodeFunction::new(LambdaParams::simple(vec![]));
    function.gnu_bytecode_bytes = Some(crate::tagged::header::LispByteVec::owned(vec![b'A', b'B']));
    function.constants.ensure_owned().push(Value::fixnum(73));
    function.docstring = Some(crate::heap_types::LispString::from_utf8(
        "r028 documentation",
    ));
    Value::make_bytecode(function)
}

fn bytecode_temporary_slots(slots: &[Value]) -> [Value; 3] {
    [slots[1], slots[2], slots[4]]
}

fn bytecode_slot_ownership(ctx: &crate::emacs_core::Context, values: [Value; 3]) -> [bool; 3] {
    values.map(|value| ctx.tagged_heap.owns_heap_value_for_test(value))
}

#[test]
fn bytecode_literal_slots_restore_caller_roots_after_gc_and_panic() {
    use crate::emacs_core::eval::{
        push_scratch_gc_root, push_scratch_gc_root_slot, save_scratch_gc_roots, set_scratch_gc_root,
    };
    crate::test_utils::init_test_tracing();
    let mut ctx = crate::emacs_core::Context::new();
    ctx.gc_stress = false;
    let cleanup = BytecodeTestRootCleanup::new();
    let owner = bytecode_root_fixture();
    push_scratch_gc_root(owner);
    let sentinel = Value::cons(Value::fixnum(19), Value::fixnum(23));
    let sentinel_slot = push_scratch_gc_root_slot(sentinel);
    let caller_depth = save_scratch_gc_roots();
    let mut temporary = [Value::NIL; 3];
    let mut during = [false; 3];
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_bytecode_literal_slots_public(&owner, |slots| {
            temporary = bytecode_temporary_slots(slots);
            // Establish ownership before reading payloads, including after GC.
            assert_eq!(bytecode_slot_ownership(&ctx, temporary), [true; 3]);
            assert_eq!(print_value(&slots[1]), "\"AB\"");
            assert_eq!(print_value(&slots[2]), "[73]");
            assert_eq!(print_value(&slots[4]), "\"r028 documentation\"");
            ctx.gc_collect_exact();
            during = bytecode_slot_ownership(&ctx, temporary);
            assert_eq!(during, [true; 3]);
            assert!(ctx.tagged_heap.owns_heap_value_for_test(owner));
            assert!(ctx.tagged_heap.owns_heap_value_for_test(sentinel));
            std::panic::panic_any("r028 bytecode literal callback panic");
        });
    }));
    let payload = panic
        .as_ref()
        .err()
        .and_then(|p| p.downcast_ref::<&str>())
        .copied();
    let depth_after = save_scratch_gc_roots();
    ctx.gc_collect_exact();
    let after = bytecode_slot_ownership(&ctx, temporary);
    let caller_owned = [owner, sentinel].map(|v| ctx.tagged_heap.owns_heap_value_for_test(v));
    // The caller-owned slot must remain addressable, not merely have a similar depth.
    if depth_after >= caller_depth {
        set_scratch_gc_root(sentinel_slot, sentinel);
    }
    eprintln!(
        "r028 panic: payload={payload:?} caller_depth={caller_depth} depth_after={depth_after} during={during:?} after={after:?} caller_owned={caller_owned:?}"
    );
    drop(cleanup);
    assert_eq!(payload, Some("r028 bytecode literal callback panic"));
    assert_eq!(during, [true; 3]);
    assert_eq!(caller_owned, [true; 2]);
    // One aggregate assertion records both balance AND reclaimability on RED.
    assert_eq!((depth_after, after), (caller_depth, [false; 3]));
}

#[test]
fn bytecode_literal_slots_nested_panic_preserves_outer_prefix_and_recovers() {
    use crate::emacs_core::eval::{push_scratch_gc_root, save_scratch_gc_roots};
    crate::test_utils::init_test_tracing();
    let mut ctx = crate::emacs_core::Context::new();
    ctx.gc_stress = false;
    let cleanup = BytecodeTestRootCleanup::new();
    let outer = bytecode_root_fixture();
    push_scratch_gc_root(outer);
    let inner = bytecode_root_fixture();
    push_scratch_gc_root(inner);
    let sentinel = Value::cons(Value::fixnum(31), Value::NIL);
    push_scratch_gc_root(sentinel);
    let caller_depth = save_scratch_gc_roots();
    let mut outer_temporary = [Value::NIL; 3];
    let mut inner_temporary = [Value::NIL; 3];
    let mut inner_during = [false; 3];
    let mut inner_after = [true; 3];
    let mut outer_after_inner = [false; 3];
    let mut inner_depth_after = 0;
    let mut payload_ok = false;
    let returned = with_bytecode_literal_slots_public(&outer, |outer_slots| {
        outer_temporary = bytecode_temporary_slots(outer_slots);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_bytecode_literal_slots_public(&inner, |inner_slots| {
                inner_temporary = bytecode_temporary_slots(inner_slots);
                ctx.gc_collect_exact();
                inner_during = bytecode_slot_ownership(&ctx, inner_temporary);
                assert_eq!(inner_during, [true; 3]);
                assert_eq!(bytecode_slot_ownership(&ctx, outer_temporary), [true; 3]);
                std::panic::panic_any("r028 nested bytecode panic");
            });
        }));
        payload_ok = panic.as_ref().err().and_then(|p| p.downcast_ref::<&str>())
            == Some(&"r028 nested bytecode panic");
        inner_depth_after = save_scratch_gc_roots();
        ctx.gc_collect_exact();
        inner_after = bytecode_slot_ownership(&ctx, inner_temporary);
        outer_after_inner = bytecode_slot_ownership(&ctx, outer_temporary);
        // Continue through a fresh normal extent after the locally caught panic.
        with_bytecode_literal_slots_public(&inner, |slots| {
            ctx.gc_collect_exact();
            assert_eq!(
                bytecode_slot_ownership(&ctx, bytecode_temporary_slots(slots)),
                [true; 3]
            );
        });
        101
    });
    let depth_after = save_scratch_gc_roots();
    ctx.gc_collect_exact();
    let outer_after = bytecode_slot_ownership(&ctx, outer_temporary);
    let caller_owned =
        [outer, inner, sentinel].map(|v| ctx.tagged_heap.owns_heap_value_for_test(v));
    eprintln!(
        "r028 nested: payload_ok={payload_ok} caller_depth={caller_depth} inner_depth_after={inner_depth_after} final_depth={depth_after} inner_during={inner_during:?} inner_after={inner_after:?} outer_after_inner={outer_after_inner:?} outer_after={outer_after:?} caller_owned={caller_owned:?}"
    );
    drop(cleanup);
    assert!(payload_ok);
    assert_eq!(returned, Some(101));
    assert_eq!(inner_during, [true; 3]);
    assert_eq!(outer_after_inner, [true; 3]);
    assert_eq!(caller_owned, [true; 3]);
    assert_eq!((depth_after, outer_after), (caller_depth, [false; 3]));
    assert_eq!(
        (inner_depth_after, inner_after),
        (caller_depth + 5, [false; 3])
    );
}

#[test]
fn bytecode_literal_slots_normal_and_nonlocal_flow_returns_restore_prefix() {
    use crate::emacs_core::error::{Flow, FlowRef};
    use crate::emacs_core::eval::{push_scratch_gc_root, save_scratch_gc_roots};
    crate::test_utils::init_test_tracing();
    let mut ctx = crate::emacs_core::Context::new();
    ctx.gc_stress = false;
    let cleanup = BytecodeTestRootCleanup::new();
    let owner = bytecode_root_fixture();
    push_scratch_gc_root(owner);
    let sentinel = Value::cons(Value::fixnum(47), Value::NIL);
    push_scratch_gc_root(sentinel);
    let caller_depth = save_scratch_gc_roots();
    let mut temporary = [Value::NIL; 3];
    let normal = with_bytecode_literal_slots_public(&owner, |slots| {
        temporary = bytecode_temporary_slots(slots);
        ctx.gc_collect_exact();
        assert_eq!(bytecode_slot_ownership(&ctx, temporary), [true; 3]);
        211
    });
    let normal_depth = save_scratch_gc_roots();
    ctx.gc_collect_exact();
    let normal_after = bytecode_slot_ownership(&ctx, temporary);
    let flow: Option<Result<(), Flow>> = with_bytecode_literal_slots_public(&owner, |slots| {
        temporary = bytecode_temporary_slots(slots);
        ctx.gc_collect_exact();
        assert_eq!(bytecode_slot_ownership(&ctx, temporary), [true; 3]);
        Err(Flow::throw(Value::symbol("r028-tag"), Value::fixnum(307)))
    });
    let flow_depth = save_scratch_gc_roots();
    let flow_ok = matches!(flow.as_ref(), Some(Err(f)) if matches!(f.kind(), FlowRef::Throw(data)
        if data.tag == Value::symbol("r028-tag") && data.value == Value::fixnum(307)));
    ctx.gc_collect_exact();
    let flow_after = bytecode_slot_ownership(&ctx, temporary);
    let caller_owned = [owner, sentinel].map(|v| ctx.tagged_heap.owns_heap_value_for_test(v));
    eprintln!(
        "r028 returns: normal={normal:?} flow_ok={flow_ok} caller_depth={caller_depth} normal_depth={normal_depth} flow_depth={flow_depth} normal_after={normal_after:?} flow_after={flow_after:?} caller_owned={caller_owned:?}"
    );
    drop(cleanup);
    assert_eq!(normal, Some(211));
    assert!(flow_ok);
    assert_eq!((normal_depth, normal_after), (caller_depth, [false; 3]));
    assert_eq!((flow_depth, flow_after), (caller_depth, [false; 3]));
    assert_eq!(caller_owned, [true; 2]);
}

#[test]
fn bytecode_literal_slots_nonbytecode_does_not_call_or_change_roots() {
    use crate::emacs_core::eval::{push_scratch_gc_root, save_scratch_gc_roots};
    crate::test_utils::init_test_tracing();
    let mut ctx = crate::emacs_core::Context::new();
    let cleanup = BytecodeTestRootCleanup::new();
    let sentinel = Value::cons(Value::fixnum(59), Value::NIL);
    push_scratch_gc_root(sentinel);
    let caller_depth = save_scratch_gc_roots();
    let mut called = false;
    for input in [Value::NIL, Value::fixnum(17), sentinel] {
        assert_eq!(
            with_bytecode_literal_slots_public(&input, |_| {
                called = true;
                1
            }),
            None
        );
    }
    ctx.gc_collect_exact();
    let depth_after = save_scratch_gc_roots();
    let sentinel_owned = ctx.tagged_heap.owns_heap_value_for_test(sentinel);
    eprintln!(
        "r028 nonbytecode: called={called} caller_depth={caller_depth} depth_after={depth_after} sentinel_owned={sentinel_owned}"
    );
    drop(cleanup);
    assert!(!called);
    assert_eq!(depth_after, caller_depth);
    assert!(sentinel_owned);
}

#[test]
fn print_basic_values() {
    crate::test_utils::init_test_tracing();
    assert_eq!(print_value(&Value::NIL), "nil");
    assert_eq!(print_value(&Value::T), "t");
    assert_eq!(print_value(&Value::fixnum(42)), "42");
    assert_eq!(print_value(&Value::make_float(3.125)), "3.125");
    assert_eq!(print_value(&Value::make_float(1.0)), "1.0");
    assert_eq!(print_value(&Value::symbol("foo")), "foo");
    assert_eq!(print_value(&Value::symbol(".foo")), ".foo");
    assert_eq!(print_value(&Value::symbol("")), "##");
    assert_eq!(print_value(&Value::keyword(":bar")), ":bar");
}

#[test]
fn print_symbol_escapes_reader_sensitive_chars() {
    crate::test_utils::init_test_tracing();
    assert_eq!(print_value(&Value::symbol("a b")), "a\\ b");
    assert_eq!(print_value(&Value::symbol("a,b")), "a\\,b");
    assert_eq!(print_value(&Value::symbol("a,@b")), "a\\,@b");
    assert_eq!(print_value(&Value::symbol("a#b")), "a\\#b");
    assert_eq!(print_value(&Value::symbol("a'b")), "a\\'b");
    assert_eq!(print_value(&Value::symbol("a`b")), "a\\`b");
    assert_eq!(print_value(&Value::symbol("a\\b")), "a\\\\b");
    assert_eq!(print_value(&Value::symbol("a\"b")), "a\\\"b");
    assert_eq!(print_value(&Value::symbol("a(b")), "a\\(b");
    assert_eq!(print_value(&Value::symbol("a)b")), "a\\)b");
    assert_eq!(print_value(&Value::symbol("a[b")), "a\\[b");
    assert_eq!(print_value(&Value::symbol("a]b")), "a\\]b");
    assert_eq!(print_value(&Value::symbol("##")), "\\#\\#");
    assert_eq!(print_value(&Value::symbol("?a")), "\\?a");
    assert_eq!(print_value(&Value::symbol("a?b")), "a?b");
}

#[test]
fn print_symbol_escapes_numeric_looking_names_like_gnu() {
    crate::test_utils::init_test_tracing();
    assert_eq!(print_value(&Value::symbol("2")), "\\2");
    assert_eq!(print_value(&Value::symbol("+1")), "\\+1");
    assert_eq!(print_value(&Value::symbol("-1")), "\\-1");
    assert_eq!(print_value(&Value::symbol("1e2")), "\\1e2");
    assert_eq!(print_value(&Value::symbol("1.")), "\\1.");
    assert_eq!(print_value(&Value::symbol(".5")), "\\.5");
    assert_eq!(print_value(&Value::symbol("+.5")), "\\+.5");
    assert_eq!(print_value(&Value::symbol("1e+INF")), "\\1e+INF");
    assert_eq!(print_value(&Value::symbol("0.0e+NaN")), "\\0.0e+NaN");

    assert_eq!(print_value(&Value::symbol("1+")), "1+");
    assert_eq!(print_value(&Value::symbol("0x10")), "0x10");
    assert_eq!(print_value(&Value::symbol("+.")), "+.");
    assert_eq!(print_value(&Value::symbol("-.")), "-.");
    assert_eq!(print_value(&Value::symbol("+")), "+");
    assert_eq!(print_value(&Value::symbol("-")), "-");
}

#[test]
fn print_uninterned_symbols_follow_gnu_default_print_gensym_nil() {
    crate::test_utils::init_test_tracing();
    assert_eq!(print_value(&Value::symbol(intern_uninterned("foo"))), "foo");
    assert_eq!(
        print_value(&Value::symbol(intern_uninterned(":foo"))),
        ":foo"
    );
    assert_eq!(print_value(&Value::symbol(intern_uninterned(""))), "##");
}

#[test]
fn print_raw_unibyte_uninterned_symbol_bytes_match_gnu_encoding() {
    crate::test_utils::init_test_tracing();
    let raw_name = crate::heap_types::LispString::from_unibyte(vec![0xFF, b'a']);
    let sym = Value::symbol(intern_uninterned_lisp_string(&raw_name));
    assert_eq!(print_value_bytes(&sym), vec![0xC1, 0xBF, b'a']);
}

#[test]
fn print_uninterned_symbols_support_print_gensym_round_trip_syntax() {
    crate::test_utils::init_test_tracing();
    let options = PrintOptions::with_print_gensym(true);
    assert_eq!(
        print_value_with_options(&Value::symbol(intern_uninterned("foo")), options),
        "#:foo"
    );
    assert_eq!(
        print_value_with_options(&Value::symbol(intern_uninterned(":foo")), options),
        "#::foo"
    );
    assert_eq!(
        print_value_with_options(&Value::symbol(intern_uninterned("")), options),
        "#:"
    );
}

#[test]
fn print_gensym_raw_unibyte_symbol_bytes_match_gnu_encoding() {
    crate::test_utils::init_test_tracing();
    let options = PrintOptions::with_print_gensym(true);
    let raw_name = crate::heap_types::LispString::from_unibyte(vec![0xFF, b'a']);
    let sym = Value::symbol(intern_uninterned_lisp_string(&raw_name));
    assert_eq!(
        print_value_bytes_with_options(&sym, options),
        vec![b'#', b':', 0xC1, 0xBF, b'a']
    );
}

#[test]
fn print_float_nan_preserves_sign() {
    crate::test_utils::init_test_tracing();
    assert_eq!(print_value(&Value::make_float(f64::NAN)), "0.0e+NaN");
    let neg_nan = f64::from_bits(f64::NAN.to_bits() | (1_u64 << 63));
    assert_eq!(print_value(&Value::make_float(neg_nan)), "-0.0e+NaN");
}

#[test]
fn print_float_nan_payload_tag_round_trip_shape() {
    crate::test_utils::init_test_tracing();
    let tagged = f64::from_bits((0x7ffu64 << 52) | (1u64 << 51) | 1u64);
    assert_eq!(print_value(&Value::make_float(tagged)), "1.0e+NaN");

    let neg_tagged = f64::from_bits((1u64 << 63) | (0x7ffu64 << 52) | (1u64 << 51) | 2u64);
    assert_eq!(print_value(&Value::make_float(neg_tagged)), "-2.0e+NaN");
}

#[test]
fn float_output_format_zero_precision_matches_gnu_print_c() {
    crate::test_utils::init_test_tracing();
    assert_eq!(
        format_float_with_output_format(1.25, Some(Value::string("%.0f"))),
        "1"
    );
    assert_eq!(
        format_float_with_output_format(1.25, Some(Value::string("%.f"))),
        "1.0"
    );
    assert_eq!(
        format_float_with_output_format(1.25, Some(Value::string("%.e"))),
        "1e+00"
    );
    assert_eq!(
        format_float_with_output_format(1.25, Some(Value::string("%.0g"))),
        "1.25"
    );
}

#[test]
fn print_string() {
    crate::test_utils::init_test_tracing();
    assert_eq!(print_value(&Value::string("hello")), "\"hello\"");
}

#[test]
fn print_empty_char_table_uses_gnu_vector_shape() {
    crate::test_utils::init_test_tracing();
    let table = crate::emacs_core::chartable::make_char_table_with_extra_slots(
        Value::symbol("syntax-table"),
        Value::NIL,
        0,
    );
    let rendered = print_value(&table);
    assert!(rendered.starts_with("#^[nil nil syntax-table"));
}

#[test]
fn print_propertized_string_literal_shape() {
    crate::test_utils::init_test_tracing();
    let value = Value::string_with_text_properties(
        " ",
        vec![StringTextPropertyRun {
            start: 0,
            end: 1,
            plist: Value::list(vec![
                Value::symbol("display"),
                Value::list(vec![
                    Value::symbol("space"),
                    Value::keyword(":align-to"),
                    Value::list(vec![
                        Value::symbol("+"),
                        Value::symbol("header-line-indent-width"),
                        Value::fixnum(0),
                    ]),
                ]),
            ]),
        }],
    );
    assert_eq!(
        print_value(&value),
        r##"#(" " 0 1 (display (space :align-to (+ header-line-indent-width 0))))"##
    );
    assert_eq!(
        print_value_bytes(&value),
        br#"#(" " 0 1 (display (space :align-to (+ header-line-indent-width 0))))"#
    );
}

#[test]
fn print_propertized_string_properties_keep_buffer_context() {
    crate::test_utils::init_test_tracing();
    let mut buffers = crate::buffer::BufferManager::new();
    let buffer_id = buffers
        .find_buffer_by_name("*scratch*")
        .expect("scratch buffer");
    let value = Value::string_with_text_properties(
        "x",
        vec![StringTextPropertyRun {
            start: 0,
            end: 1,
            plist: Value::list(vec![Value::symbol("owner"), Value::make_buffer(buffer_id)]),
        }],
    );

    buffers.kill_buffer(buffer_id);

    assert_eq!(
        print_value_with_buffers(&value, &buffers),
        r#"#("x" 0 1 (owner #<killed buffer>))"#
    );
}

#[test]
fn print_propertized_string_parent_cycle_matches_gnu_default_cycle_path() {
    crate::test_utils::init_test_tracing();
    let text = Value::string("body");
    let parent = Value::list(vec![Value::symbol("section"), Value::NIL, text]);
    set_string_text_properties_for_value(
        text,
        vec![StringTextPropertyRun {
            start: 0,
            end: 4,
            plist: Value::list(vec![Value::keyword(":parent"), parent]),
        }],
    );

    assert_eq!(
        print_value(&Value::list(vec![text])),
        r#"(#("body" 0 4 (:parent (section nil #1))))"#
    );
    assert_eq!(
        print_value_bytes(&Value::list(vec![text])),
        br#"(#("body" 0 4 (:parent (section nil #1))))"#
    );
}

#[test]
fn print_circle_preprocess_traverses_string_text_property_plists_like_gnu() {
    crate::test_utils::init_test_tracing();
    let text = Value::string("body");
    let parent = Value::list(vec![Value::symbol("section"), Value::NIL, text]);
    set_string_text_properties_for_value(
        text,
        vec![StringTextPropertyRun {
            start: 0,
            end: 4,
            plist: Value::list(vec![Value::keyword(":parent"), parent]),
        }],
    );

    let options = PrintOptions::new(false, true, None, None);
    assert_eq!(
        print_value_with_options(&Value::list(vec![text]), options),
        r#"(#1=#("body" 0 4 (:parent (section nil #1#))))"#
    );
}

#[test]
fn print_string_keeps_non_bmp_visible() {
    crate::test_utils::init_test_tracing();
    assert_eq!(print_value(&Value::string("\u{10ffff}")), "\"\u{10ffff}\"");
}

#[test]
fn print_string_bytes_preserve_non_utf8_payloads() {
    crate::test_utils::init_test_tracing();
    assert_eq!(
        print_value_bytes(&Value::heap_string(
            crate::heap_types::LispString::from_emacs_bytes(vec![0xC1, 0xBF],)
        )),
        b"\"\\377\""
    );
}

#[test]
fn print_literal_private_use_unicode_does_not_masquerade_as_raw_byte() {
    crate::test_utils::init_test_tracing();
    let private_use = char::from_u32(0xE0FF).expect("private use scalar");
    assert_eq!(
        print_value_bytes(&Value::string(private_use.to_string())),
        format!("\"{}\"", private_use).into_bytes()
    );
}

/// Issue #131: Private-Use-Area characters (nerd-font icons live across
/// U+E000..U+F8FF) must extract as their real code points. neomacs reused
/// U+E300..U+E3FF as a unibyte "sentinel", so char access masked e.g.
/// U+E322 → 0x22 (`"`), corrupting glyphs and breaking byte-compiled `.elc`
/// syntax. This guards that whole range (the nerd-font weather/material icons).
///
/// (U+E080..U+E0FF is still used as the in-`String` storage for eight-bit raw
/// bytes, so genuine glyphs there remain ambiguous until the storage rework —
/// issue #131 Step B — and are intentionally not covered here.)
#[test]
fn private_use_chars_survive_char_extraction_issue_131() {
    crate::test_utils::init_test_tracing();
    use crate::emacs_core::builtins::{lisp_string_char_at, lisp_string_char_codes};
    for cp in [0xE300u32, 0xE322, 0xE325, 0xE379, 0xE39A, 0xE3FF] {
        let ch = char::from_u32(cp).expect("private use scalar");
        let value = Value::string(ch.to_string());
        let ls = value.as_lisp_string().expect("string value");
        assert_eq!(ls.schars(), 1, "U+{cp:04X} must be one character");
        assert_eq!(
            lisp_string_char_at(ls, 0),
            Some(cp),
            "aref of U+{cp:04X} must keep the real code point (issue #131)"
        );
        assert_eq!(lisp_string_char_codes(ls), vec![cp]);
        // The printer must emit the real glyph, not a masked low byte.
        assert_eq!(print_value(&value), format!("\"{ch}\""));
    }
}

#[test]
fn print_list() {
    crate::test_utils::init_test_tracing();
    let lst = Value::list(vec![Value::fixnum(1), Value::fixnum(2), Value::fixnum(3)]);
    assert_eq!(print_value(&lst), "(1 2 3)");
}

#[test]
fn print_stateful_record_preprocess_uses_record_storage() {
    crate::test_utils::init_test_tracing();
    let record = Value::make_record(vec![Value::symbol("foo"), Value::fixnum(1)]);
    let options = PrintOptions::new(false, false, Some(10), None);

    assert_eq!(print_value_stateful(&record, options), "#s(foo 1)");
}

#[test]
fn print_hash_s_literal_shorthand() {
    crate::test_utils::init_test_tracing();
    let literal = Value::list(vec![
        Value::symbol("make-hash-table-from-literal"),
        Value::list(vec![
            Value::symbol("quote"),
            Value::list(vec![Value::symbol("x")]),
        ]),
    ]);
    assert_eq!(print_value(&literal), "#s(x)");
    assert_eq!(print_value_bytes(&literal), b"#s(x)");
}

#[test]
fn print_hash_table_object_uses_readable_hash_s_shape() {
    crate::test_utils::init_test_tracing();
    let table = Value::hash_table(HashTableTest::Equal);
    // GNU Emacs prints "test equal" for non-default test (default is eql).
    assert_eq!(print_value(&table), "#s(hash-table test equal)");
    assert_eq!(print_value_bytes(&table), b"#s(hash-table test equal)");
}

#[test]
fn print_hash_table_uses_live_slots_after_remhash_reinsert() {
    crate::test_utils::init_test_tracing();
    let table = Value::hash_table(HashTableTest::Eq);

    builtin_puthash(vec![Value::fixnum(1), Value::fixnum(10), table]).unwrap();
    builtin_puthash(vec![Value::fixnum(2), Value::fixnum(20), table]).unwrap();
    builtin_remhash(vec![Value::fixnum(1), table]).unwrap();
    builtin_puthash(vec![Value::fixnum(1), Value::fixnum(10), table]).unwrap();

    let rendered = print_value(&table);
    assert_eq!(rendered.matches("1 10").count(), 1);
    assert_eq!(rendered.matches("2 20").count(), 1);
    assert_eq!(print_value_bytes(&table), rendered.as_bytes());
}

#[test]
fn print_quote_shorthand_lists() {
    crate::test_utils::init_test_tracing();
    let quoted = Value::list(vec![Value::symbol("quote"), Value::symbol("foo")]);
    let function = Value::list(vec![Value::symbol("function"), Value::symbol("car")]);
    let quasiquoted = Value::list(vec![
        Value::symbol("`"),
        Value::list(vec![Value::symbol("a"), Value::symbol("b")]),
    ]);
    let unquoted = Value::list(vec![Value::symbol(","), Value::symbol("x")]);
    let unquote_splice = Value::list(vec![Value::symbol(",@"), Value::symbol("xs")]);

    assert_eq!(print_value(&quoted), "'foo");
    assert_eq!(print_value(&function), "#'car");
    assert_eq!(print_value(&quasiquoted), "`(a b)");
    assert_eq!(print_value(&unquoted), "(\\, x)");
    assert_eq!(print_value(&unquote_splice), "(\\,@ xs)");
}

#[test]
fn print_backquote_preserves_nested_unquote_shorthand_only_in_context() {
    crate::test_utils::init_test_tracing();
    let nested = Value::list(vec![
        Value::symbol("`"),
        Value::list(vec![
            Value::symbol("a"),
            Value::list(vec![Value::symbol(","), Value::symbol("x")]),
        ]),
    ]);

    assert_eq!(print_value(&nested), "`(a ,x)");
}

#[test]
fn print_dotted_pair() {
    crate::test_utils::init_test_tracing();
    let pair = Value::cons(Value::fixnum(1), Value::fixnum(2));
    assert_eq!(print_value(&pair), "(1 . 2)");
}

#[test]
fn print_vector() {
    crate::test_utils::init_test_tracing();
    let v = Value::vector(vec![Value::fixnum(1), Value::fixnum(2)]);
    assert_eq!(print_value(&v), "[1 2]");
}

#[test]
fn print_default_handles_circular_vector_like_gnu() {
    crate::test_utils::init_test_tracing();
    let vector = Value::vector(vec![Value::NIL]);
    assert!(vector.set_vector_slot(0, vector));

    assert_eq!(print_value(&vector), "[#0]");
    assert_eq!(print_value_bytes(&vector), b"[#0]");
}

#[test]
fn print_default_handles_circular_cons_like_gnu() {
    crate::test_utils::init_test_tracing();
    let cell = Value::cons(Value::NIL, Value::NIL);
    cell.set_cdr(cell);

    assert_eq!(print_value(&cell), "(nil . #0)");
    assert_eq!(print_value_bytes(&cell), b"(nil . #0)");
}

#[test]
fn print_default_bounded_circular_list_uses_gnu_tail_index() {
    crate::test_utils::init_test_tracing();
    let first = Value::cons(Value::fixnum(1), Value::NIL);
    let second = Value::cons(Value::fixnum(2), Value::NIL);
    first.set_cdr(second);
    second.set_cdr(first);

    let options = PrintOptions::new(false, false, None, Some(6));

    assert_eq!(print_value_stateful(&first, options), "(1 2 1 2 . #2)");
}

#[test]
fn print_default_tail_cycle_uses_gnu_tail_index() {
    crate::test_utils::init_test_tracing();
    let first = Value::cons(Value::symbol("a"), Value::NIL);
    let second = Value::cons(Value::symbol("b"), Value::NIL);
    let third = Value::cons(Value::symbol("c"), Value::NIL);
    first.set_cdr(second);
    second.set_cdr(third);
    third.set_cdr(second);

    let options = PrintOptions::new(false, false, None, Some(7));

    assert_eq!(print_value_stateful(&first, options), "(a b c b . #2)");
}

#[test]
fn print_level_applies_to_conses_not_vectorlike_objects_like_gnu() {
    crate::test_utils::init_test_tracing();
    let options = PrintOptions::new(false, false, Some(1), None);
    let nested_vector = Value::vector(vec![
        Value::vector(vec![Value::symbol("a"), Value::symbol("b")]),
        Value::vector(vec![Value::symbol("c"), Value::symbol("d")]),
    ]);
    let record = Value::make_record(vec![
        Value::symbol("foo"),
        Value::list(vec![Value::symbol("a"), Value::symbol("b")]),
        Value::vector(vec![Value::symbol("c"), Value::symbol("d")]),
    ]);

    assert_eq!(
        print_value_stateful(&nested_vector, options),
        "[[a b] [c d]]"
    );
    assert_eq!(print_value_stateful(&record, options), "#s(foo ... [c d])");
}

#[test]
fn print_circle_handles_self_referential_records() {
    crate::test_utils::init_test_tracing();
    let record = Value::make_record(vec![Value::symbol("foo"), Value::NIL]);
    record.with_record_data_mut(|slots| slots[1] = record);

    let options = PrintOptions::new(false, true, None, None);
    assert_eq!(
        print_value_stateful_with_buffers(&record, None, options),
        "#1=#s(foo #1#)"
    );
}

#[test]
fn print_number_table_cleanup_preserves_only_labeled_entries() {
    crate::test_utils::init_test_tracing();
    let table = Value::hash_table(HashTableTest::Eq);
    let retained = Value::cons(Value::symbol("shared"), Value::NIL);
    let retained_key = print_number_table_key(table, &retained).unwrap();
    let alias = Value::string("#$");
    let alias_key_value = Value::string("alias-key");
    let alias_key = print_number_table_key(table, &alias_key_value).unwrap();

    for i in 0..64 {
        let seen_once = Value::cons(Value::fixnum(i), Value::NIL);
        let key = print_number_table_key(table, &seen_once).unwrap();
        put_print_number_table_entry(table, key, seen_once, Value::T);
    }
    put_print_number_table_entry(table, retained_key.clone(), retained, Value::fixnum(7));
    put_print_number_table_entry(table, alias_key.clone(), alias_key_value, alias);

    remove_print_number_table_t_entries(table);

    let hash_table = table.as_hash_table().unwrap();
    assert_eq!(hash_table.data.len(), 2);
    assert_eq!(hash_table.key_snapshots().count(), 2);
    assert_eq!(
        hash_table.live_hash_keys_in_slot_order(),
        vec![&retained_key, &alias_key]
    );
    assert_eq!(hash_table.data.get(&retained_key), Some(&Value::fixnum(7)));
    assert_eq!(hash_table.data.get(&alias_key), Some(&alias));
    assert!(hash_table.data.values().all(|value| *value != Value::T));
}

#[test]
fn print_default_handles_self_referential_records_like_gnu() {
    crate::test_utils::init_test_tracing();
    let record = Value::make_record(vec![Value::symbol("foo"), Value::NIL]);
    record.with_record_data_mut(|slots| slots[1] = record);

    assert_eq!(print_value(&record), "#s(foo #0)");
    assert_eq!(print_value_bytes(&record), b"#s(foo #0)");
}

#[test]
fn print_default_handles_self_referential_bytecode_constants() {
    crate::test_utils::init_test_tracing();
    let mut function =
        crate::emacs_core::bytecode::ByteCodeFunction::new(LambdaParams::simple(vec![]));
    function.constants.ensure_owned().push(Value::NIL);
    let bytecode = Value::make_bytecode(function);
    bytecode.with_bytecode_data_mut_for_test(|data| data.constants[0] = bytecode);

    assert_eq!(print_value(&bytecode), "#[nil nil [#0] 0]");
    assert_eq!(print_value_bytes(&bytecode), b"#[nil nil [#0] 0]");

    let options = PrintOptions::new(false, true, None, None);
    assert_eq!(
        print_value_with_options(&bytecode, options),
        "#1=#[nil nil [#1#] 0]"
    );
}

#[test]
fn print_default_uses_gnu_depth_for_nested_bytecode_backrefs() {
    crate::test_utils::init_test_tracing();
    let record = Value::make_record(vec![Value::symbol("foo"), Value::NIL]);
    let mut function =
        crate::emacs_core::bytecode::ByteCodeFunction::new(LambdaParams::simple(vec![]));
    function.constants.ensure_owned().push(record);
    let bytecode = Value::make_bytecode(function);
    record.with_record_data_mut(|slots| slots[1] = bytecode);

    let wrapped = Value::list(vec![Value::T, record, Value::NIL]);
    assert_eq!(print_value(&wrapped), "(t #s(foo #[nil nil [#1] 0]) nil)");
}

#[test]
fn print_lambda() {
    crate::test_utils::init_test_tracing();
    let lam = Value::make_lambda(LambdaData {
        params: LambdaParams::simple(vec![intern("x"), intern("y")]).into(),
        body: vec![Value::list(vec![
            Value::symbol("+"),
            Value::symbol("x"),
            Value::symbol("y"),
        ])],
        env: None,
        docstring: None,
        doc_form: None,
        interactive: None,
    });
    assert_eq!(print_value(&lam), "#[(x y) ((+ x y)) nil]");
}

#[test]
fn print_lexical_closure_uses_gnu_vector_syntax() {
    crate::test_utils::init_test_tracing();
    let closure = Value::make_lambda(LambdaData {
        params: LambdaParams::simple(vec![intern("a"), intern("b")]).into(),
        body: vec![Value::list(vec![
            Value::symbol("+"),
            Value::symbol("a"),
            Value::symbol("b"),
            Value::symbol("x"),
        ])],
        env: Some(Value::list(vec![Value::cons(
            Value::symbol("x"),
            Value::fixnum(42),
        )])),
        docstring: None,
        doc_form: None,
        interactive: None,
    });

    assert_eq!(print_value(&closure), "#[(a b) ((+ a b x)) ((x . 42))]");
    assert_eq!(
        String::from_utf8(print_value_bytes(&closure)).expect("utf8"),
        "#[(a b) ((+ a b x)) ((x . 42))]"
    );
}

#[test]
fn print_recursive_closure_uses_backreference() {
    crate::test_utils::init_test_tracing();
    let binding = Value::cons(Value::symbol("f"), Value::NIL);
    let env = Value::list(vec![binding]);
    let closure = Value::make_lambda(LambdaData {
        params: LambdaParams::simple(vec![]).into(),
        body: vec![Value::symbol("f")],
        env: Some(env),
        docstring: None,
        doc_form: None,
        interactive: None,
    });
    binding.set_cdr(closure);

    assert_eq!(print_value(&closure), "#[nil (f) ((f . #0))]");
    assert_eq!(
        String::from_utf8(print_value_bytes(&closure)).expect("utf8"),
        "#[nil (f) ((f . #0))]"
    );
}

#[test]
fn print_terminal_handle_special_form() {
    crate::test_utils::init_test_tracing();
    let list = super::super::terminal::pure::builtin_terminal_list(vec![]).unwrap();
    let items = list_to_vec(&list).expect("terminal-list should return a list");
    let handle = items
        .first()
        .expect("terminal-list should contain one handle");

    let printed = print_value(handle);
    assert!(printed.starts_with("#<terminal "));
    assert!(printed.contains("on initial_terminal>"));
}

#[test]
fn print_frame_handles_use_oracle_style_f_prefix() {
    crate::test_utils::init_test_tracing();
    let f1 = Value::make_frame(crate::window::FRAME_ID_BASE);
    let f2 = Value::make_frame(crate::window::FRAME_ID_BASE + 1);
    let legacy = Value::make_frame(7);

    assert_eq!(print_value(&f1), "#<frame F1 0x100000000>");
    assert_eq!(print_value_bytes(&f1), b"#<frame F1 0x100000000>");
    assert_eq!(print_value(&f2), "#<frame F2 0x100000001>");
    assert_eq!(print_value_bytes(&f2), b"#<frame F2 0x100000001>");
    assert_eq!(print_value(&legacy), "#<frame 7>");
}

#[test]
fn print_markers_use_gnu_style_handles() {
    crate::test_utils::init_test_tracing();
    let marker = make_marker_value(None, None, false);
    assert_eq!(print_value(&marker), "#<marker in no buffer>");

    let mut buffers = crate::buffer::BufferManager::new();
    let buffer_id = buffers
        .find_buffer_by_name("*scratch*")
        .expect("scratch buffer");
    let marker = make_marker_value(Some(buffer_id), Some(LispCharPos1::new(3)), false);
    assert_eq!(
        print_value_with_buffers(&marker, &buffers),
        "#<marker at 3 in *scratch*>"
    );

    buffers.kill_buffer(buffer_id);
    assert_eq!(
        print_value_with_buffers(&marker, &buffers),
        "#<marker in no buffer>"
    );
}

// ---------------------------------------------------------------------------
// Eval-driven printer regression tests (printer dynamic variables).
// ---------------------------------------------------------------------------

fn print_eval_one(src: &str) -> String {
    let mut ev = crate::emacs_core::Context::new();
    let result = ev.eval_str(src);
    crate::emacs_core::format_eval_result(&result)
}

#[test]
fn bool_vector_printing_honors_gnu_byte_escape_and_length_options() {
    crate::test_utils::init_test_tracing();

    assert_eq!(
        print_eval_one(
            "(let ((print-escape-newlines t)) \
               (prin1-to-string \
                 (bool-vector nil t nil t nil nil nil nil nil nil t t)))"
        ),
        r##"OK "#&12\"\\n\\f\"""##,
    );
    assert_eq!(
        print_eval_one(
            "(let ((print-escape-control-characters t)) \
               (prin1-to-string \
                 (bool-vector t nil nil nil nil nil nil nil \
                              t t t nil t t nil nil)))"
        ),
        r##"OK "#&16\"\\0017\"""##,
    );
    assert_eq!(
        print_eval_one(
            "(let ((print-length 1)) \
               (prin1-to-string \
                 (bool-vector nil t nil t nil nil nil nil nil nil t t)))"
        ),
        r##"OK "#&12\"
 ...\"""##,
    );
}

#[test]
fn print_integers_as_characters_uses_char_syntax_like_gnu() {
    crate::test_utils::init_test_tracing();
    // GNU: (?A ?\t). Letters print via graphic_base_p; tab via named_escape.
    assert_eq!(
        print_eval_one("(let ((print-integers-as-characters t)) (prin1-to-string (list 65 9)))"),
        "OK \"(?A ?\\\\t)\"",
    );
    // A broader spread, matching GNU exactly:
    //  - named escapes: 8 -> ?\b, 10 -> ?\n, 32 -> ?\s, 13 -> ?\r
    //  - graphic bases: 256 -> ?Ā, 955 -> ?λ, 59 -> ?\; (escaped by prin1)
    //  - left as integers: 0, 7, 11, 27, 127 (control), 8203 (Cf format)
    assert_eq!(
        print_eval_one(
            "(let ((print-integers-as-characters t)) \
             (prin1-to-string (list 65 9 10 32 0 1 127 ?\\( 7 11 27 ?\\; 256 955 8203)))"
        ),
        "OK \"(?A ?\\\\t ?\\\\n ?\\\\s 0 1 127 ?\\\\( 7 11 27 ?\\\\; ?Ā ?λ 8203)\"",
    );
    // princ-style output (no escapeflag): the self-delimiting `;` is NOT
    // backslash-escaped, but named escapes and `?` still apply. Exercise the
    // printer directly with `print_noescape` (the C `escapeflag = false` path).
    let princ_opts = PrintOptions {
        print_integers_as_characters: true,
        print_noescape: true,
        ..PrintOptions::default()
    };
    let list = Value::list(vec![
        Value::fixnum(65),
        Value::fixnum(9),
        Value::fixnum(';' as i64),
    ]);
    assert_eq!(print_value_with_options(&list, princ_opts), "(?A ?\\t ?;)",);
    // When the variable is nil, integers print as integers.
    assert_eq!(
        print_eval_one("(prin1-to-string (list 65 9))"),
        "OK \"(65 9)\"",
    );
}

#[test]
fn print_preprocess_fills_number_table_for_circular_structures_like_gnu() {
    crate::test_utils::init_test_tracing();
    // Root cause of the cl-print circular-list hang: `print--preprocess` was a
    // no-op stub, so cl-print never built `print-number-table` and recursed
    // forever.  GNU's `print--preprocess` fills the table when `print-circle'
    // is non-nil; a shared/circular object gets a negative-fixnum label.
    //
    // Circular list: l = (1 2 . l).  GNU: (gethash l print-number-table) = -1.
    assert_eq!(
        print_eval_one(
            "(let ((print-circle t) \
                   (print-number-table (make-hash-table :test 'eq)) \
                   (l (list 1 2))) \
               (setcdr (cdr l) l) \
               (print--preprocess l) \
               (and (< (gethash l print-number-table 0) 0) t))"
        ),
        "OK t",
    );
    // Circular vector: v = [v nil].  GNU labels the shared vector negatively.
    assert_eq!(
        print_eval_one(
            "(let ((print-circle t) \
                   (print-number-table (make-hash-table :test 'eq)) \
                   (v (make-vector 2 nil))) \
               (aset v 0 v) \
               (print--preprocess v) \
               (and (< (gethash v print-number-table 0) 0) t))"
        ),
        "OK t",
    );
    // With `print-circle' nil, GNU does nothing (the table stays empty).
    assert_eq!(
        print_eval_one(
            "(let ((print-circle nil) \
                   (print-number-table (make-hash-table :test 'eq)) \
                   (l (list 1 2))) \
               (setcdr (cdr l) l) \
               (print--preprocess l) \
               (gethash l print-number-table 'absent))"
        ),
        "OK absent",
    );
    // Acyclic, non-shared structure: no shared label is assigned (the head
    // gets the transient `t` status, which is not a number), but GNU records
    // every traversed candidate, so the three cons cells of (1 2 3) leave a
    // table count of 3.  cl-print only treats *numberp* entries as labels, so
    // an acyclic list prints without any `#N=` prefix.
    assert_eq!(
        print_eval_one(
            "(let* ((print-circle t) \
                    (print-number-table (make-hash-table :test 'eq)) \
                    (l (list 1 2 3))) \
               (print--preprocess l) \
               (list (hash-table-count print-number-table) \
                     (numberp (gethash l print-number-table))))"
        ),
        "OK (3 nil)",
    );
}

#[test]
fn print_circle_candidate_set_matches_gnu_for_bool_vectors_and_char_tables() {
    crate::test_utils::init_test_tracing();
    // GNU's `print_circle_candidate_p` matches CLOSUREP || CHAR_TABLE_P ||
    // SUB_CHAR_TABLE_P || HASH_TABLE_P || FONTP || RECORDP for non-vector
    // vectorlikes; `VECTORP` excludes bool-vectors (a distinct pseudovector).
    //
    // Bug (a), cosmetic over-labeling: a bool-vector printed twice under
    // `print-circle' must NOT be labeled, because GNU does not treat
    // bool-vectors as circle candidates.  GNU prints each bool-vector in
    // full (`#&3"\7"`, where \7 is the 0x07 bit-pack byte for 3 set bits)
    // with no `#N=' / `#N#' label: bytes `# & 3 " \x07 "` per element.
    assert_eq!(
        print_eval_one(
            "(let ((v (make-bool-vector 3 t))) \
               (let ((print-circle t)) (prin1-to-string (list v v))))"
        ),
        "OK \"(#&3\\\"\u{7}\\\" #&3\\\"\u{7}\\\")\"",
    );
    // Bug (b), functional round-trip break: a genuinely shared char-table
    // printed twice under `print-circle' MUST get a `#N=' / `#N#' label so
    // that reading the output back preserves shared identity.  GNU returns t.
    assert_eq!(
        print_eval_one(
            "(let* ((v (make-char-table 'test)) \
                    (r (read (let ((print-circle t)) \
                               (prin1-to-string (list v v)))))) \
               (eq (nth 0 r) (nth 1 r)))"
        ),
        "OK t",
    );
    // A non-shared char-table (printed once) must NOT be labeled: GNU only
    // labels objects that appear more than once.
    assert_eq!(
        print_eval_one(
            "(let ((v (make-char-table 'test))) \
               (let ((print-circle t)) \
                 (string-match \"#[0-9]+=\" (prin1-to-string v) nil t)))"
        ),
        "OK nil",
    );
}

#[test]
fn hash_table_printer_omits_default_eql_test_like_gnu() {
    crate::test_utils::init_test_tracing();
    // Default test (no :test arg) -> omitted.
    assert_eq!(
        print_eval_one("(prin1-to-string (make-hash-table))"),
        "OK \"#s(hash-table)\"",
    );
    // Explicit :test 'eql is still the default -> omitted (GNU compares the
    // test *name* symbol against `eql`).
    assert_eq!(
        print_eval_one("(prin1-to-string (make-hash-table :test 'eql))"),
        "OK \"#s(hash-table)\"",
    );
    // Non-default tests are still printed.
    assert_eq!(
        print_eval_one("(prin1-to-string (make-hash-table :test 'eq))"),
        "OK \"#s(hash-table test eq)\"",
    );
    assert_eq!(
        print_eval_one("(prin1-to-string (make-hash-table :test 'equal))"),
        "OK \"#s(hash-table test equal)\"",
    );
    // Data is still printed; the default test stays omitted.
    assert_eq!(
        print_eval_one(
            "(let ((h (make-hash-table :test 'eql))) (puthash 1 2 h) (prin1-to-string h))"
        ),
        "OK \"#s(hash-table data (1 2))\"",
    );
}

// ---------------------------------------------------------------------------
// `princ` / `%s` printer: GNU `print_object` cycle and depth bounds.
//
// Expected strings are GNU Emacs 31 output with
// `internal-make-interpreted-closure-function' nil, so closures keep their
// whole lexical environment, trailing `t' included, as in a bare Context.
// ---------------------------------------------------------------------------

// Public `princ` must publish its initially nil continuous history before
// rendering. These primitive-only forms need no bootstrap Lisp; GC runs
// between prints and assertions inspect returned fields, not a formatter.
fn assert_princ_history_fields(src: &str, strings: &[&str], fields: &[Value]) {
    let mut ctx = crate::emacs_core::Context::new();
    ctx.set_lexical_binding(true);
    let mut result = ctx.eval_str(src).expect("primitive princ history fixture");
    for (index, expected) in strings.iter().enumerate() {
        assert!(result.is_cons(), "missing string field {index}");
        assert_eq!(
            result.cons_car().as_utf8_str(),
            Some(*expected),
            "string field {index}"
        );
        result = result.cons_cdr();
    }
    for (index, expected) in fields.iter().enumerate() {
        assert!(result.is_cons(), "missing field {index}");
        assert_eq!(result.cons_car(), *expected, "field {index}");
        result = result.cons_cdr();
    }
    assert!(result.is_nil(), "unexpected trailing fields");
}

fn check_princ_continuous_history(buffer: bool, supplied: bool, continuous: bool) {
    let src = r#"(progn
  (let ((print-continuous-numbering nil)) (prin1-to-string nil))
  (let* ((print-circle t) (print-continuous-numbering CONTINUOUS)
         (caller TABLE) (print-number-table caller)
         (x (list 1)) (pair (list x x)) (s "")
         (sink SINK) first first-return second-return)
    (setq first-return (eq pair (princ pair sink)))
    (setq first SNAPSHOT s "")
    CLEAR
    (garbage-collect)
    (setq second-return (eq pair (princ pair sink)))
    (list first SNAPSHOT first-return second-return
          (or (eq caller nil) (eq caller print-number-table))
          (and print-continuous-numbering (hash-table-p print-number-table))
          (and print-continuous-numbering (hash-table-p print-number-table)
               (hash-table-count print-number-table)))))"#
        .replace("CONTINUOUS", if continuous { "t" } else { "nil" })
        .replace("TABLE", if supplied { "(make-hash-table :test 'eq)" } else { "nil" })
        .replace("SINK", if buffer { "(progn (set-buffer (get-buffer-create \" princ-history\")) (erase-buffer) (current-buffer))" }
                 else { "(lambda (ch) (setq s (concat s (string ch))))" })
        .replace("SNAPSHOT", if buffer { "(progn (set-buffer sink) (buffer-string))" } else { "s" })
        .replace("CLEAR", if buffer { "(erase-buffer)" } else { "nil" });
    assert_princ_history_fields(
        &src,
        &[
            "(#1=(1) #1#)",
            if continuous {
                "(#1# #1#)"
            } else {
                "(#1=(1) #1#)"
            },
        ],
        &[
            Value::T,
            Value::T,
            Value::T,
            if continuous { Value::T } else { Value::NIL },
            if continuous {
                Value::fixnum(1)
            } else {
                Value::NIL
            },
        ],
    );
}

#[test]
fn princ_continuous_history_callable_nil_across_gc() {
    check_princ_continuous_history(false, false, true);
}

#[test]
fn princ_continuous_history_callable_supplied_across_gc() {
    check_princ_continuous_history(false, true, true);
}

#[test]
fn princ_continuous_history_callable_disabled_across_gc() {
    check_princ_continuous_history(false, false, false);
}

#[test]
fn princ_continuous_history_buffer_nil_across_gc() {
    check_princ_continuous_history(true, false, true);
}

#[test]
fn princ_continuous_history_buffer_supplied_across_gc() {
    check_princ_continuous_history(true, true, true);
}

#[test]
fn princ_continuous_history_buffer_disabled_across_gc() {
    check_princ_continuous_history(true, false, false);
}

#[test]
fn princ_continuous_history_scope_reset() {
    assert_princ_history_fields(
        r#"(let* ((print-circle t) (print-continuous-numbering t)
       (caller (make-hash-table :test 'eq)) (print-number-table caller)
       (x (list 1)) (pair (list x x)) (s "")
       (sink (lambda (ch) (setq s (concat s (string ch))))) first second)
  (let ((print-number-table nil)) (princ pair sink) (setq first s))
  (setq s "")
  (garbage-collect)
  (let ((print-number-table nil)) (princ pair sink) (setq second s))
  (list first second (eq caller print-number-table) (hash-table-count caller)))"#,
        &["(#1=(1) #1#)", "(#1=(1) #1#)"],
        &[Value::T, Value::fixnum(0)],
    );
}

#[test]
fn princ_continuous_history_nonlocal_recovery() {
    assert_princ_history_fields(
        r#"(let* ((print-circle t) (print-continuous-numbering t)
       (print-number-table nil) (x (list 1)) (pair (list x x)) (s "")
       (sink (lambda (ch) (setq s (concat s (string ch))))) table stopped returned)
  (princ pair sink)
  (setq table print-number-table s "")
  (setq stopped (catch 'abort (princ pair (lambda (ch) (throw 'abort 'stopped)))))
  (garbage-collect)
  (setq returned (eq pair (princ pair sink)))
  (list s (eq stopped 'stopped) returned (eq table print-number-table)
        (and (hash-table-p print-number-table) (hash-table-count print-number-table))))"#,
        &["(#1# #1#)"],
        &[Value::T, Value::T, Value::T, Value::fixnum(1)],
    );
}

#[test]
fn princ_continuous_history_direct_impl_initializes_nil_table() {
    use crate::emacs_core::builtins::misc_eval::builtin_princ_impl;
    let mut ctx = crate::emacs_core::Context::new();
    let pair = ctx
        .eval_str(
            r#"(progn
        (setq print-circle t print-continuous-numbering t print-number-table nil)
        (set-buffer (get-buffer-create " princ-history-direct"))
        (setq princ-history-pair (let ((x (list 1))) (list x x))))"#,
        )
        .unwrap();
    let sink = ctx.eval_str("(current-buffer)").unwrap();
    assert_eq!(builtin_princ_impl(&mut ctx, vec![pair, sink]).unwrap(), pair);
    assert_eq!(
        ctx.eval_str("(buffer-string)").unwrap().as_utf8_str(),
        Some("(#1=(1) #1#)")
    );
    assert_eq!(
        ctx.eval_str("(hash-table-p print-number-table)").unwrap(),
        Value::T
    );
    assert_eq!(
        ctx.eval_str("(hash-table-count print-number-table)")
            .unwrap(),
        Value::fixnum(1)
    );
    ctx.eval_str("(progn (erase-buffer) (garbage-collect))")
        .unwrap();
    assert_eq!(builtin_princ_impl(&mut ctx, vec![pair, sink]).unwrap(), pair);
    assert_eq!(
        ctx.eval_str("(buffer-string)").unwrap().as_utf8_str(),
        Some("(#1# #1#)")
    );
}

// GNU only materializes nil history for a circle-preprocessing candidate.
// Inspect the variable itself; neither output formatting nor a truthy
// continuous-numbering flag is evidence that a history table was published.
fn check_princ_candidate_history(
    object: &str,
    output: &str,
    gensym: bool,
    candidate: bool,
    buffer: bool,
) {
    let src = r#"(let* ((print-circle t) (print-continuous-numbering t)
       (print-gensym GENSYM) (caller nil) (print-number-table caller)
       (x OBJECT) (s "") (sink SINK))
  (let ((returned (princ x sink)))
    (list SNAPSHOT (eq x returned) (hash-table-p print-number-table)
          (eq print-number-table nil)
          (or (eq caller nil) (eq caller print-number-table))
          (if (hash-table-p print-number-table)
              (hash-table-count print-number-table) nil))))"#
        .replace("GENSYM", if gensym { "t" } else { "nil" })
        .replace("OBJECT", object)
        .replace("SINK", if buffer {
            "(progn (set-buffer (get-buffer-create \" princ-candidate\")) (erase-buffer) (current-buffer))"
        } else { "(lambda (ch) (setq s (concat s (string ch))))" })
        .replace("SNAPSHOT", if buffer { "(buffer-string)" } else { "s" });
    // Gensym spelling is independent of history eligibility. GNU includes
    // `#:' here; the existing noescape renderer does not. Check the common
    // history label, return identity, and table contents without changing
    // that pre-existing renderer behavior in this regression.
    let src = if candidate && gensym {
        src.replace("(list (buffer-string)", "(list (substring (buffer-string) 0 3)")
            .replace("(list s", "(list (substring s 0 3)")
    } else {
        src
    };
    assert_princ_history_fields(
        &src,
        &[output],
        &[
            Value::T,
            if candidate { Value::T } else { Value::NIL },
            if candidate { Value::NIL } else { Value::T },
            Value::T,
            if candidate {
                Value::fixnum(if gensym { 1 } else { 0 })
            } else {
                Value::NIL
            },
        ],
    );
}

#[test]
fn princ_candidate_history_fixnum_stays_nil() {
    for buffer in [false, true] {
        check_princ_candidate_history("1", "1", false, false, buffer);
    }
}

#[test]
fn princ_candidate_history_nil_stays_nil() {
    for buffer in [false, true] {
        check_princ_candidate_history("nil", "nil", false, false, buffer);
    }
}

#[test]
fn princ_candidate_history_interned_symbol_stays_nil() {
    for buffer in [false, true] {
        check_princ_candidate_history("'princ-atom", "princ-atom", true, false, buffer);
    }
}

#[test]
fn princ_candidate_history_empty_string_stays_nil() {
    for buffer in [false, true] {
        check_princ_candidate_history("\"\"", "", false, false, buffer);
    }
}

#[test]
fn princ_candidate_history_empty_vector_stays_nil() {
    for buffer in [false, true] {
        check_princ_candidate_history("[]", "[]", false, false, buffer);
    }
}

#[test]
fn princ_candidate_history_gensym_disabled_stays_nil() {
    for buffer in [false, true] {
        check_princ_candidate_history(
            "(make-symbol \"princ-atom\")",
            "princ-atom",
            false,
            false,
            buffer,
        );
    }
}

#[test]
fn princ_candidate_history_public_buffer_atom_stays_nil() {
    check_princ_candidate_history("1", "1", false, false, true);
}

#[test]
fn princ_candidate_history_eligible_objects_publish_table() {
    for buffer in [false, true] {
        for (object, output, gensym) in [
            ("\"abc\"", "abc", false),
            ("[1]", "[1]", false),
            ("(list 1)", "(1)", false),
            ("(make-symbol \"princ-atom\")", "#1=", true),
        ] {
            check_princ_candidate_history(object, output, gensym, true, buffer);
        }
    }
}

#[test]
fn princ_candidate_history_circle_disabled_and_supplied_atom_controls() {
    assert_princ_history_fields(
        r#"(let* ((print-circle nil) (print-continuous-numbering t)
       (print-gensym nil) (caller nil) (print-number-table caller)
       (x [1]) (s "") (sink (lambda (ch) (setq s (concat s (string ch))))))
  (let ((returned (princ x sink)))
    (list s (eq x returned) (hash-table-p print-number-table)
          (eq print-number-table nil)
          (or (eq caller nil) (eq caller print-number-table))
          (if (hash-table-p print-number-table)
              (hash-table-count print-number-table) nil))))"#,
        &["[1]"],
        &[Value::T, Value::NIL, Value::T, Value::T, Value::NIL],
    );
    assert_princ_history_fields(
        r#"(let* ((print-circle t) (print-continuous-numbering t)
       (print-gensym nil) (caller (make-hash-table :test 'eq)) (print-number-table caller)
       (x 1) (s "") (sink (lambda (ch) (setq s (concat s (string ch))))))
  (let ((returned (princ x sink)))
    (list s (eq x returned) (hash-table-p print-number-table)
          (eq print-number-table nil)
          (or (eq caller nil) (eq caller print-number-table))
          (if (hash-table-p print-number-table)
              (hash-table-count print-number-table) nil))))"#,
        &["1"],
        &[Value::T, Value::T, Value::NIL, Value::T, Value::fixnum(0)],
    );
}

#[test]
fn princ_candidate_history_direct_impl_atom_stays_nil() {
    use crate::emacs_core::builtins::misc_eval::builtin_princ_impl;
    let mut ctx = crate::emacs_core::Context::new();
    ctx.eval_str(
        r#"(progn
        (setq print-circle t print-continuous-numbering t print-gensym nil print-number-table nil)
        (set-buffer (get-buffer-create " princ-candidate-direct")))"#,
    )
    .unwrap();
    let sink = ctx.eval_str("(current-buffer)").unwrap();
    let object = Value::fixnum(1);
    assert_eq!(
        builtin_princ_impl(&mut ctx, vec![object, sink]).unwrap(),
        object
    );
    assert_eq!(
        ctx.eval_str("(buffer-string)").unwrap().as_utf8_str(),
        Some("1")
    );
    assert_eq!(ctx.eval_str("print-number-table").unwrap(), Value::NIL);
    let caller = ctx
        .eval_str("(setq print-number-table (make-hash-table :test 'eq))")
        .unwrap();
    assert_eq!(
        builtin_princ_impl(&mut ctx, vec![object, sink]).unwrap(),
        object
    );
    assert_eq!(ctx.eval_str("print-number-table").unwrap(), caller);
    assert_eq!(
        ctx.eval_str("(hash-table-count print-number-table)")
            .unwrap(),
        Value::fixnum(0)
    );
}

// GNU primitive controls: effective current/destination bindings and history.
fn assert_princ_scope_result(result: Value, output: &str, history: bool) {
    let mut fields = result;
    assert_eq!(
        fields.cons_car().as_utf8_str(),
        Some(output),
        "scope output"
    );
    fields = fields.cons_cdr();
    for expected in [
        Value::T,
        Value::T,
        Value::T,
        Value::T,
        if history {
            Value::fixnum(1)
        } else {
            Value::NIL
        },
        Value::T,
    ] {
        assert!(fields.is_cons(), "missing scope field");
        assert_eq!(fields.cons_car(), expected, "scope field");
        fields = fields.cons_cdr();
    }
    assert!(fields.is_nil());
}

fn check_princ_effective_scope(case: usize, route: &str) {
    use crate::emacs_core::builtins::misc_eval::builtin_princ_impl;
    let (setup, circle, history, other) = match case {
        0 => (
            r#"(progn
  (setq print-circle t print-continuous-numbering t print-number-table nil)
  (setq scope-default-table (default-value 'print-number-table))
  (set-buffer (get-buffer-create " princ-scope-caller"))
  (make-local-variable 'print-circle) (setq print-circle nil)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-caller-table print-number-table)
  nil
  (make-local-variable 'print-circle) (setq print-circle nil)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-supplied-table print-number-table scope-destination (current-buffer))
  (setq scope-x (let ((x (list 1))) (list x x)) scope-s "")
  (erase-buffer)
  nil
  scope-x)"#,
            false,
            false,
            false,
        ),
        1 => (
            r#"(progn
  (setq print-circle t print-continuous-numbering t print-number-table nil)
  (setq scope-default-table (default-value 'print-number-table))
  (set-buffer (get-buffer-create " princ-scope-caller"))
  (make-local-variable 'print-circle) (setq print-circle t)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering nil)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-caller-table print-number-table)
  nil
  (make-local-variable 'print-circle) (setq print-circle t)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering nil)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-supplied-table print-number-table scope-destination (current-buffer))
  (setq scope-x (let ((x (list 1))) (list x x)) scope-s "")
  (erase-buffer)
  nil
  scope-x)"#,
            true,
            false,
            false,
        ),
        2 => (
            r#"(progn
  (setq print-circle nil print-continuous-numbering nil print-number-table nil)
  (setq scope-default-table (default-value 'print-number-table))
  (set-buffer (get-buffer-create " princ-scope-caller"))
  (make-local-variable 'print-circle) (setq print-circle t)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-caller-table print-number-table)
  nil
  (make-local-variable 'print-circle) (setq print-circle t)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-supplied-table print-number-table scope-destination (current-buffer))
  (setq scope-x (let ((x (list 1))) (list x x)) scope-s "")
  (erase-buffer)
  nil
  scope-x)"#,
            true,
            true,
            false,
        ),
        3 => (
            r#"(progn
  (setq print-circle t print-continuous-numbering t print-number-table (make-hash-table :test 'eq))
  (setq scope-default-table (default-value 'print-number-table))
  (set-buffer (get-buffer-create " princ-scope-caller"))
  (make-local-variable 'print-circle) (setq print-circle t)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-caller-table print-number-table)
  nil
  (make-local-variable 'print-circle) (setq print-circle t)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-supplied-table print-number-table scope-destination (current-buffer))
  (setq scope-x (let ((x (list 1))) (list x x)) scope-s "")
  (erase-buffer)
  nil
  scope-x)"#,
            true,
            true,
            false,
        ),
        4 => (
            r#"(progn
  (setq print-circle t print-continuous-numbering t print-number-table nil)
  (setq scope-default-table (default-value 'print-number-table))
  (set-buffer (get-buffer-create " princ-scope-caller"))
  (make-local-variable 'print-circle) (setq print-circle t)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-caller-table print-number-table)
  nil
  (make-local-variable 'print-circle) (setq print-circle t)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table (make-hash-table :test 'eq))
  (setq scope-supplied-table print-number-table scope-destination (current-buffer))
  (setq scope-x (let ((x (list 1))) (list x x)) scope-s "")
  (erase-buffer)
  nil
  scope-x)"#,
            true,
            true,
            false,
        ),
        5 => (
            r#"(progn
  (setq print-circle nil print-continuous-numbering nil print-number-table nil)
  (setq scope-default-table (default-value 'print-number-table))
  (set-buffer (get-buffer-create " princ-scope-caller"))
  (make-local-variable 'print-circle) (setq print-circle nil)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-caller-table print-number-table)
  (set-buffer (get-buffer-create " princ-scope-destination"))
  (make-local-variable 'print-circle) (setq print-circle t)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-supplied-table print-number-table scope-destination (current-buffer))
  (setq scope-x (let ((x (list 1))) (list x x)) scope-s "")
  (erase-buffer)
  (set-buffer (get-buffer-create " princ-scope-caller"))
  scope-x)"#,
            true,
            true,
            true,
        ),
        6 => (
            r#"(progn
  (setq print-circle t print-continuous-numbering t print-number-table nil)
  (setq scope-default-table (default-value 'print-number-table))
  (set-buffer (get-buffer-create " princ-scope-caller"))
  (make-local-variable 'print-circle) (setq print-circle t)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-caller-table print-number-table)
  (set-buffer (get-buffer-create " princ-scope-destination"))
  (make-local-variable 'print-circle) (setq print-circle nil)
  (make-local-variable 'print-continuous-numbering) (setq print-continuous-numbering t)
  (make-local-variable 'print-number-table) (setq print-number-table nil)
  (setq scope-supplied-table print-number-table scope-destination (current-buffer))
  (setq scope-x (let ((x (list 1))) (list x x)) scope-s "")
  (erase-buffer)
  (set-buffer (get-buffer-create " princ-scope-caller"))
  scope-x)"#,
            false,
            false,
            true,
        ),
        _ => unreachable!(),
    };
    let mut ctx = crate::emacs_core::Context::new();
    ctx.set_lexical_binding(true);
    let object = ctx.eval_str(setup).expect("primitive scope setup");
    let sink = ctx.eval_str("scope-destination").unwrap();
    let snapshot = if route == "callable" {
        "scope-s"
    } else {
        "(buffer-string)"
    };
    // GNU publishes a transient table when circle=t/continuous=nil. That
    // pre-existing renderer difference is not a continuous-history assertion.
    let table_check = if history {
        "(hash-table-p print-number-table)"
    } else {
        "(or (eq print-number-table nil) (eq print-continuous-numbering nil))"
    };
    let count = if history {
        "(if (hash-table-p print-number-table) (hash-table-count print-number-table) nil)"
    } else {
        "nil"
    };
    let caller_check = if other {
        r#"(progn (set-buffer (get-buffer-create " princ-scope-caller")) (eq scope-caller-table print-number-table))"#
    } else {
        "t"
    };
    let inspect = format!(
        r#"(progn (set-buffer scope-destination)
      (list {snapshot} (eq scope-x scope-returned) {table_check}
        (or (eq scope-supplied-table nil) (eq scope-supplied-table print-number-table))
        (eq scope-default-table (default-value 'print-number-table)) {count} {caller_check}))"#
    );
    let first = if circle { "(#1=(1) #1#)" } else { "((1) (1))" };
    let second = if history { "(#1# #1#)" } else { first };
    for output in [first, second] {
        if route == "direct" {
            assert_eq!(
                builtin_princ_impl(&mut ctx, vec![object, sink]).unwrap(),
                object
            );
            ctx.set_variable("scope-returned", object);
        } else {
            let stream = if route == "callable" {
                "(lambda (ch) (setq scope-s (concat scope-s (string ch))))"
            } else {
                "scope-destination"
            };
            ctx.eval_str(&format!("(setq scope-returned (princ scope-x {stream}))"))
                .unwrap();
        }
        let result = ctx.eval_str(&inspect).unwrap();
        assert_princ_scope_result(result, output, history);
        ctx.eval_str(r#"(progn (set-buffer scope-destination) (erase-buffer) (setq scope-s "") (garbage-collect))"#).unwrap();
        if other {
            ctx.eval_str(r#"(set-buffer (get-buffer-create " princ-scope-caller"))"#)
                .unwrap();
        }
    }
}

#[test]
fn princ_effective_scope_local_circle_disabled_callable() {
    check_princ_effective_scope(0, "callable");
}

#[test]
fn princ_effective_scope_local_circle_disabled_buffer() {
    check_princ_effective_scope(0, "buffer");
}

#[test]
fn princ_effective_scope_local_circle_disabled_direct() {
    check_princ_effective_scope(0, "direct");
}

#[test]
fn princ_effective_scope_local_continuous_disabled_callable() {
    check_princ_effective_scope(1, "callable");
}

#[test]
fn princ_effective_scope_local_continuous_disabled_buffer() {
    check_princ_effective_scope(1, "buffer");
}

#[test]
fn princ_effective_scope_local_continuous_disabled_direct() {
    check_princ_effective_scope(1, "direct");
}

#[test]
fn princ_effective_scope_local_flags_enabled_callable() {
    check_princ_effective_scope(2, "callable");
}

#[test]
fn princ_effective_scope_local_flags_enabled_buffer() {
    check_princ_effective_scope(2, "buffer");
}

#[test]
fn princ_effective_scope_local_flags_enabled_direct() {
    check_princ_effective_scope(2, "direct");
}

#[test]
fn princ_effective_scope_local_nil_table_default_supplied_callable() {
    check_princ_effective_scope(3, "callable");
}

#[test]
fn princ_effective_scope_local_nil_table_default_supplied_buffer() {
    check_princ_effective_scope(3, "buffer");
}

#[test]
fn princ_effective_scope_local_nil_table_default_supplied_direct() {
    check_princ_effective_scope(3, "direct");
}

#[test]
fn princ_effective_scope_local_supplied_table_default_nil_callable() {
    check_princ_effective_scope(4, "callable");
}

#[test]
fn princ_effective_scope_local_supplied_table_default_nil_buffer() {
    check_princ_effective_scope(4, "buffer");
}

#[test]
fn princ_effective_scope_local_supplied_table_default_nil_direct() {
    check_princ_effective_scope(4, "direct");
}

#[test]
fn princ_effective_scope_destination_enabled_caller_disabled_buffer() {
    check_princ_effective_scope(5, "buffer");
}

#[test]
fn princ_effective_scope_destination_enabled_caller_disabled_direct() {
    check_princ_effective_scope(5, "direct");
}

#[test]
fn princ_effective_scope_destination_disabled_caller_enabled_buffer() {
    check_princ_effective_scope(6, "buffer");
}

#[test]
fn princ_effective_scope_destination_disabled_caller_enabled_direct() {
    check_princ_effective_scope(6, "direct");
}

fn princ_eval(src: &str) -> String {
    let mut ev = crate::emacs_core::Context::new();
    ev.set_lexical_binding(true);
    crate::emacs_core::format_eval_result(&ev.eval_str(src))
}

/// SRC with X bound to N levels of WRAP around nil.
fn nested(n: usize, wrap: &str, body: &str) -> String {
    format!("(let ((x nil) (i 0)) (while (< i {n}) (setq x {wrap}) (setq i (1+ i))) {body})")
}

const CIRCULAR: &str = r#"OK (error "Apparently circular structure being printed")"#;

#[test]
fn princ_prints_a_closure_that_captures_itself_as_an_ancestor() {
    assert_eq!(
        princ_eval(r#"(let (f) (setq f (lambda () f)) (format "%s" f))"#),
        r##"OK "#[nil (f) ((f . #0) t)]""##
    );
}

#[test]
fn princ_prints_a_circular_list_captured_by_a_closure() {
    assert_eq!(
        princ_eval(r#"(let ((c (list 1))) (setcdr c c) (format "%s" (lambda () c)))"#),
        r##"OK "#[nil (c) ((c 1 1 . #2) t)]""##
    );
}

#[test]
fn princ_prints_a_closure_inside_the_vector_it_captures() {
    assert_eq!(
        princ_eval(
            r#"(let ((v (make-vector 3 nil))) (aset v 1 (lambda () v)) (format "%s" (aref v 1)))"#
        ),
        r##"OK "#[nil (v) ((v . [nil #0 nil]) t)]""##
    );
}

#[test]
fn princ_to_a_function_and_error_message_string_share_the_cycle_bound() {
    assert_eq!(
        princ_eval(
            r#"(let (f) (setq f (lambda () f))
                 (let ((s ""))
                   (princ f (lambda (ch) (setq s (concat s (string ch)))))
                   s))"#
        ),
        r##"OK "#[nil (f) ((f . #0) t)]""##
    );
    assert_eq!(
        princ_eval(
            r#"(let (f) (setq f (lambda () f)) (error-message-string (list 'user-error f)))"#
        ),
        r##"OK "#[nil (f) ((f . #0) t)]""##
    );
}

#[test]
fn princ_prints_tail_cycles_with_gnu_tortoise_index() {
    for (src, expected) in [
        (
            r#"(let ((c (list 1 2))) (setcdr (cdr c) c) (format "%s" c))"#,
            r#"OK "(1 2 1 2 . #2)""#,
        ),
        (
            r#"(let ((c (list 1 2 3 4 5))) (setcdr (nthcdr 4 c) (nthcdr 2 c)) (format "%s" c))"#,
            r#"OK "(1 2 3 4 5 . #2)""#,
        ),
        (
            r#"(let ((c (list 1))) (setcdr c c) (format "%s" c))"#,
            r#"OK "(1 . #0)""#,
        ),
        (
            r#"(let ((c (list 1))) (setcdr c c) (format "%s" (list 'quote c)))"#,
            r#"OK "'(1 . #0)""#,
        ),
    ] {
        assert_eq!(princ_eval(src), expected, "{src}");
    }
}

#[test]
fn princ_prints_enclosing_aggregates_by_depth() {
    for (src, expected) in [
        (
            r#"(let ((v (vector 1 nil))) (aset v 1 v) (format "%s" v))"#,
            r#"OK "[1 #0]""#,
        ),
        (
            r#"(let ((v (vector 1 nil))) (aset v 1 (list "s" v)) (format "%s" v))"#,
            r#"OK "[1 (s #0)]""#,
        ),
        (
            r#"(let ((r (record 'foo nil))) (aset r 1 r) (format "%s" r))"#,
            r##"OK "#s(foo #0)""##,
        ),
        (
            r#"(let ((c (list nil))) (setcar c c) (format "%s" c))"#,
            r#"OK "(#0)""#,
        ),
        (
            r#"(let ((c (list 'a))) (setcar c (list 'quote c)) (format "%s" c))"#,
            r#"OK "('#0)""#,
        ),
    ] {
        assert_eq!(princ_eval(src), expected, "{src}");
    }
}

#[test]
fn princ_signals_at_gnu_print_depth_instead_of_overflowing() {
    assert_eq!(
        princ_eval(&nested(199, "(list x)", r#"(length (format "%s" x))"#)),
        "OK 401"
    );
    for wrap in ["(list x)", "(vector x)", "(let ((y x)) (lambda () y))"] {
        assert_eq!(
            princ_eval(&nested(
                200,
                wrap,
                r#"(condition-case e (format "%s" x) (error e))"#
            )),
            CIRCULAR,
            "{wrap}"
        );
    }
    assert_eq!(
        princ_eval(&nested(
            10_000,
            "(let ((y x)) (lambda () y))",
            r#"(condition-case e (format "%s" x) (error e))"#
        )),
        CIRCULAR
    );
    assert_eq!(
        princ_eval(&nested(
            200,
            "(list x)",
            "(condition-case e (error-message-string (list 'user-error x)) (error e))"
        )),
        CIRCULAR
    );
    // `'X' is a tail call, so only the inner lists count toward the depth.
    assert_eq!(
        princ_eval(&nested(
            101,
            "(list 'quote (list x))",
            r#"(length (format "%s" x))"#
        )),
        "OK 306"
    );
}

#[test]
fn princ_with_print_circle_survives_deep_and_cyclic_data() {
    assert_eq!(
        princ_eval(&nested(
            10_000,
            "(list x)",
            r#"(let ((print-circle t)) (length (format "%s" x)))"#
        )),
        "OK 20003"
    );
    assert_eq!(
        princ_eval(
            r#"(let ((print-circle t) f) (setq f (lambda () f)) (stringp (format "%s" f)))"#
        ),
        "OK t"
    );
}

#[test]
fn princ_with_print_circle_labels_shared_objects_like_gnu() {
    for (src, expected) in [
        (
            r#"(let ((print-circle t) (v (vector nil))) (aset v 0 v) (format "%s" v))"#,
            r##"OK "#1=[#1#]""##,
        ),
        (
            r#"(let ((print-circle t) (x (list 1))) (format "%s" (list x x)))"#,
            r##"OK "(#1=(1) #1#)""##,
        ),
        (
            r#"(let ((print-circle t) (s "ab")) (format "%s" (list s s)))"#,
            r##"OK "(#1=ab #1#)""##,
        ),
        (
            r#"(let ((print-circle t) (x (list 1 2))) (setcdr (cdr x) x) (format "%s" x))"#,
            r##"OK "#1=(1 2 . #1#)""##,
        ),
        (
            r#"(let ((print-circle t) (x (list 1 2))) (format "%s" (list x (cdr x))))"#,
            r##"OK "((1 . #1=(2)) #1#)""##,
        ),
        (
            r#"(let ((print-circle t) (x (list 1 2))) (format "%s" (list x (cdr x) x)))"#,
            r##"OK "(#2=(1 . #1=(2)) #1# #2#)""##,
        ),
        (
            r#"(let ((print-circle t) (x 0) (i 0))
                 (while (< i 3) (setq x (list x x) i (1+ i)))
                 (format "%s" x))"#,
            r##"OK "(#2=(#1=(0 0) #1#) #2#)""##,
        ),
        (
            r#"(let ((print-circle t) (print-level 2) (x (list 1))) (format "%s" (list (list x) x)))"#,
            r##"OK "((#1=...) #1#)""##,
        ),
        (
            r#"(let ((print-circle t) (x (list 1))) (format "%s" (list (list 'quote x) x)))"#,
            r##"OK "('#1=(1) #1#)""##,
        ),
        (
            r#"(let ((print-circle t) (print-length 1) (x (list 1))) (format "%s" (list x x)))"#,
            r##"OK "(#1=(1) ...)""##,
        ),
        (
            r#"(let ((print-circle t) (x (make-string 0 ?a))) (format "%s" (list x x)))"#,
            r#"OK "( )""#,
        ),
        (
            r#"(let ((print-circle t) (r (record 'foo nil))) (aset r 1 r) (format "%s" r))"#,
            r##"OK "#1=#s(foo #1#)""##,
        ),
        (
            r#"(let ((print-circle t) (v (vector 1))) (format "%s" (vector v v (list v))))"#,
            r##"OK "[#1=[1] #1# (#1#)]""##,
        ),
        // Each `%s' argument is a separate print.
        (
            r#"(let ((print-circle t) (x (list 1))) (format "%s %s" x x))"#,
            r#"OK "(1) (1)""#,
        ),
        // GNU's preprocessing walks text properties that `princ' omits.
        (
            r#"(let ((print-circle t) (x (list 1))) (format "%s" (list (propertize "a" 'p x) x)))"#,
            r##"OK "(a #1=(1))""##,
        ),
        (
            r#"(let ((print-circle t) f) (setq f (lambda () f)) (format "%s" f))"#,
            r##"OK "#1=#[nil (f) ((f . #1#) t)]""##,
        ),
        (
            r#"(let ((print-circle t) (x (list 1)) (s ""))
                 (princ (list x x) (lambda (ch) (setq s (concat s (string ch)))))
                 s)"#,
            r##"OK "(#1=(1) #1#)""##,
        ),
        (
            r#"(let ((print-circle t) (x (list 1)))
                 (error-message-string (list 'user-error (list x x))))"#,
            r##"OK "(#1=(1) #1#)""##,
        ),
    ] {
        assert_eq!(princ_eval(src), expected, "{src}");
    }
}

/// `nil' N times, space-separated, for char-table expectations.
fn nils(n: usize) -> String {
    vec!["nil"; n].join(" ")
}

/// GNU's `print_object' prints hash tables and char-tables itself, under the
/// one `print-number-table' of the print: a table's label is consumed once
/// and its contents are labelled with everything else.  Each case starting
/// with `(prin1-to-string nil)' resets `print_number_index' as GNU's `print'
/// does, since `print-continuous-numbering' keeps counting from there.
#[test]
fn princ_with_print_circle_labels_hash_and_char_tables_in_one_print() {
    const PERSISTENT: &str = "(print-circle t) (print-continuous-numbering t)
                              (print-number-table (make-hash-table :test 'eq))";
    let ct = "(progn (put 'neo-ct 'char-table-extra-slots 1) (make-char-table 'neo-ct))";
    for (src, expected) in [
        (
            format!(
                r#"(progn (prin1-to-string nil)
                   (let ({PERSISTENT} (h (make-hash-table :test 'eq)))
                     (puthash 'a 1 h) (format "%s" (list h h))))"#
            ),
            r##"OK "(#1=#s(hash-table test eq data (a 1)) #1#)""##.to_string(),
        ),
        (
            r#"(let ((print-circle t) (h (make-hash-table :test 'eq)))
                 (puthash 'a h h) (format "%s" h))"#
                .to_string(),
            r##"OK "#1=#s(hash-table test eq data (a #1#))""##.to_string(),
        ),
        (
            format!(
                r#"(progn (prin1-to-string nil)
                   (let ({PERSISTENT} (h (make-hash-table :test 'eq)))
                     (puthash 'a h h) (format "%s" h)))"#
            ),
            r##"OK "#1=#s(hash-table test eq data (a #1#))""##.to_string(),
        ),
        (
            r#"(let ((print-circle t) (h (make-hash-table :test 'eq)) (x (list 1)))
                 (puthash 'a x h) (puthash 'b x h) (format "%s" (list x h)))"#
                .to_string(),
            r##"OK "(#1=(1) #s(hash-table test eq data (a #1# b #1#)))""##.to_string(),
        ),
        (
            r#"(let ((print-circle t) (h (make-hash-table :test 'equal)) (s "str"))
                 (puthash s s h) (format "%s" (list h s)))"#
                .to_string(),
            r##"OK "(#s(hash-table test equal data (#1=str #1#)) #1#)""##.to_string(),
        ),
        // GNU's `print_circle_candidate_p' excludes obarrays.
        (
            format!(
                r#"(progn (prin1-to-string nil)
                   (let ({PERSISTENT} (ob (obarray-make)))
                     (list (format "%s" (list ob ob)) (prin1-to-string (list ob ob)))))"#
            ),
            r#"OK ("(#<obarray n=0> #<obarray n=0>)" "(#<obarray n=0> #<obarray n=0>)")"#
                .to_string(),
        ),
        (
            format!(
                r#"(progn (prin1-to-string nil)
                   (let ({PERSISTENT} (ct {ct})) (format "%s" (list ct ct))))"#
            ),
            format!(r##"OK "(#1=#^[nil nil neo-ct {}] #1#)""##, nils(66)),
        ),
        (
            format!(
                r#"(let ((print-circle t) (ct {ct}))
                     (set-char-table-extra-slot ct 0 ct)
                     (list (format "%s" ct) (prin1-to-string ct)))"#
            ),
            format!(
                r##"OK ("#1=#^[nil nil neo-ct {0} #1#]" "#1=#^[nil nil neo-ct {0} #1#]")"##,
                nils(65)
            ),
        ),
        (
            format!(
                r#"(let ((print-circle t) (ct {ct}) (x (list "s")))
                     (set-char-table-extra-slot ct 0 x) (format "%s" (list x ct)))"#
            ),
            format!(r##"OK "(#1=(s) #^[nil nil neo-ct {} #1#])""##, nils(65)),
        ),
        // The ASCII sub-char-table is shared by the `ascii' slot and the
        // contents tree, and is labelled like any other shared object.
        (
            r#"(let ((print-circle t) (ct (make-char-table 'foo)) (x (list 1)))
                 (aset ct ?a x) (aset ct ?b x) (format "%s" (list x ct)))"#
                .to_string(),
            format!(
                r##"OK "(#1=(1) #^[nil nil foo #2=#^^[3 0 {} #1# #1# {}] #^^[1 0 #^^[2 0 #2# {}] {}] {}])""##,
                nils(97),
                nils(29),
                nils(31),
                nils(15),
                nils(63)
            ),
        ),
    ] {
        assert_eq!(princ_eval(&src), expected, "{src}");
    }
}

/// GNU `print_object' with `escapeflag' false: hash-table and char-table
/// elements print as by `princ', count toward `print-level' and honor
/// `print-length'.
#[test]
fn princ_prints_hash_and_char_table_elements_like_gnu() {
    for (src, expected) in [
        (
            r#"(let ((h (make-hash-table :test 'equal))) (puthash "k" "v" h) (format "%s" h))"#,
            r##"OK "#s(hash-table test equal data (k v))""##,
        ),
        (
            r#"(let ((h (make-hash-table :test 'eq :weakness 'key)))
                 (puthash 'a "x" h) (format "%s" h))"#,
            r##"OK "#s(hash-table test eq weakness key data (a x))""##,
        ),
        (
            r#"(format "%s" (list (make-hash-table :test 'eq) (make-hash-table)))"#,
            r##"OK "(#s(hash-table test eq) #s(hash-table))""##,
        ),
        (
            r#"(let ((print-level 1) (h (make-hash-table :test 'eq)))
                 (puthash 'a (list 1) h) (format "%s" (list h)))"#,
            r##"OK "(#s(hash-table test eq data (a ...)))""##,
        ),
        (
            r#"(let ((print-length 1) (h (make-hash-table :test 'eq)))
                 (puthash 'a 1 h) (puthash 'b 2 h) (format "%s" h))"#,
            r##"OK "#s(hash-table test eq data (a 1 ...))""##,
        ),
        (
            r#"(let ((h (make-hash-table :test 'eq))) (puthash 'a h h) (format "%s" h))"#,
            r##"OK "#s(hash-table test eq data (a #0))""##,
        ),
        (
            r#"(let ((ct (make-char-table 'foo)) (print-length 4))
                 (aset ct ?a "x") (format "%s" ct))"#,
            r##"OK "#^[nil nil foo #^^[3 0 nil nil nil ...] ...]""##,
        ),
    ] {
        assert_eq!(princ_eval(src), expected, "{src}");
    }
}

/// 60 levels of `(list x x)' are 120 conses but 2^60 leaves unshared; GNU
/// prints them in 636 characters with `print-circle'.
#[test]
fn princ_with_print_circle_prints_a_shared_dag_once() {
    assert_eq!(
        princ_eval(
            r#"(let ((print-circle t) (x 0) (i 0))
                 (while (< i 60) (setq x (list x x) i (1+ i)))
                 (length (format "%s" x)))"#
        ),
        "OK 636"
    );
}

/// GNU reads the print variables in the buffer current while printing: the
/// current buffer for a function stream, the target buffer for a buffer
/// stream, and the ` prin1' buffer (default values) for `format'.
#[test]
fn princ_honors_the_print_stream_buffers_print_bounds() {
    assert_eq!(
        princ_eval(
            r#"(let ((b (get-buffer-create "neo-princ-locals")))
                 (save-current-buffer
                   (set-buffer b)
                   (set (make-local-variable 'print-level) 1)
                   (set (make-local-variable 'print-length) 1)
                   (let ((s ""))
                     (princ '(1 2 (3)) (lambda (ch) (setq s (concat s (string ch)))))
                     (list s (format "%s" '(1 2 (3)))))))"#
        ),
        r#"OK ("(1 ...)" "(1 2 (3))")"#
    );
    assert_eq!(
        princ_eval(
            r#"(let ((b (get-buffer-create "neo-princ-target")))
                 (save-current-buffer (set-buffer b) (set (make-local-variable 'print-length) 1))
                 (princ '(1 2 (3)) b)
                 (save-current-buffer (set-buffer b) (buffer-string)))"#
        ),
        r#"OK "(1 ...)""#
    );
}

#[test]
fn princ_honors_print_level_and_print_length_like_gnu() {
    for (src, expected) in [
        (
            r#"(let ((print-level 2)) (format "%s" '(1 (2 (3 (4))))))"#,
            r#"OK "(1 (2 ...))""#,
        ),
        (
            r#"(let ((print-level 1)) (format "%s" [1 [2 [3]]]))"#,
            r#"OK "[1 [2 [3]]]""#,
        ),
        (
            r#"(let ((print-level 1)) (format "%s" (list 1 [2 (3)])))"#,
            r#"OK "(1 [2 ...])""#,
        ),
        (
            r#"(let ((print-level 0)) (format "%s" (list 1)))"#,
            r#"OK "...""#,
        ),
        (
            r#"(let ((print-level -1)) (format "%s" (list 1)))"#,
            r#"OK "...""#,
        ),
        (
            r#"(let ((print-level 1)) (format "%s" ''(1)))"#,
            r#"OK "'(1)""#,
        ),
        (
            r#"(let ((print-length 2)) (format "%s" '(1 2 3 4)))"#,
            r#"OK "(1 2 ...)""#,
        ),
        (
            r#"(let ((print-length 0)) (format "%s" (list 1 2)))"#,
            r#"OK "(...)""#,
        ),
        (
            r#"(let ((print-length 1)) (format "%s" (cons 1 2)))"#,
            r#"OK "(1 . 2)""#,
        ),
        (
            r#"(let ((print-length -1)) (format "%s" (list 1 2)))"#,
            r#"OK "(1 2)""#,
        ),
        (
            r#"(let ((print-length 2)) (format "%s" [1 2 3 4]))"#,
            r#"OK "[1 2 ...]""#,
        ),
        (
            r#"(let ((print-length 0)) (format "%s" [1 2]))"#,
            r#"OK "[...]""#,
        ),
        (
            r#"(let ((print-length 1)) (format "%s" (record 'r 1 2)))"#,
            r##"OK "#s(r ...)""##,
        ),
        (
            r#"(let ((c (list 1 2))) (setcdr (cdr c) c) (let ((print-length 3)) (format "%s" c)))"#,
            r#"OK "(1 2 1 ...)""#,
        ),
    ] {
        assert_eq!(princ_eval(src), expected, "{src}");
    }
}

#[test]
fn princ_shorthands_follow_gnu_print_quoted_rules() {
    for (src, expected) in [
        (r#"(format "%s" '(a (\, b)))"#, r#"OK "(a (, b))""#),
        (
            r#"(format "%s" '(\` (a (\, b) (\,@ c))))"#,
            r#"OK "`(a ,b ,@c)""#,
        ),
        (r#"(format "%s" '(quote x y))"#, r#"OK "(quote x y)""#),
        (r#"(format "%s" (list 'quote "a\200"))"#, r#"OK "'a\\200""#),
    ] {
        assert_eq!(princ_eval(src), expected, "{src}");
    }
}

const DEEP_DATA_CHILD_FORM: &str = "NEOVM_DEEP_DATA_CHILD_FORM";

/// Child half of the deep-data tests: eval the form and print the result.
/// A native stack overflow kills only this process.
#[test]
#[ignore = "run in a child process by the deep-data tests"]
fn deep_data_child() {
    let Ok(src) = std::env::var(DEEP_DATA_CHILD_FORM) else {
        return;
    };
    // An overflow here must not dump a core.
    let no_core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: setrlimit and prctl only access their arguments; this process
    // runs this test alone.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_CORE, &no_core) }, 0);
    assert_eq!(unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) }, 0);
    println!("RESULT={}", princ_eval(&src));
}

/// Eval each form in a child process on an 8 MiB stack, the usual main
/// thread's, and return the mismatches with GNU's result.
fn deep_data_mismatches(cases: &[(&str, &str)]) -> Vec<String> {
    crate::test_utils::init_test_tracing();
    let child = format!(
        "{}::deep_data_child",
        module_path!().split_once("::").expect("crate path").1
    );
    let mut failures = Vec::new();
    for (src, expected) in cases {
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                child.as_str(),
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(DEEP_DATA_CHILD_FORM, src)
            .env("RUST_MIN_STACK", (8 << 20).to_string())
            .output()
            .expect("spawn child test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail = &stderr[stderr.len().saturating_sub(2000)..];
            failures.push(format!("{src}: child {}\n{tail}", output.status));
        } else if !stdout.contains(&format!("RESULT={expected}\n")) {
            failures.push(format!("{src}: expected {expected}\n{stdout}"));
        }
    }
    failures
}

/// GNU `read0` keeps unfinished lists, vectors and quotations on its own
/// `read_stack`: data nested a million levels deep reads from a string or
/// a buffer without overflowing the native stack.
#[test]
fn deeply_nested_data_reads_like_gnu() {
    let failures = deep_data_mismatches(&[
        (
            r#"(let ((x (read (concat (make-string 1000000 ?\() (make-string 1000000 ?\)))))) (let ((d 0)) (while (consp x) (setq d (1+ d) x (car x))) d))"#,
            r#"OK 999999"#,
        ),
        (
            r#"(let ((x (read (concat (make-string 1000000 ?\[) (make-string 1000000 ?\]))))) (let ((d 0)) (while (and (vectorp x) (> (length x) 0)) (setq d (1+ d) x (aref x 0))) d))"#,
            r#"OK 999999"#,
        ),
        (
            r#"(let* ((r (read-from-string (concat (make-string 1000000 ?\() (make-string 1000000 ?\))))) (x (car r))) (list (let ((d 0)) (while (consp x) (setq d (1+ d) x (car x))) d) (cdr r)))"#,
            r#"OK (999999 2000000)"#,
        ),
        (
            r#"(save-current-buffer (set-buffer (get-buffer-create (generate-new-buffer-name "p"))) (insert (concat (make-string 1000000 ?\() (make-string 1000000 ?\)))) (goto-char 1) (let ((x (read (current-buffer)))) (list (let ((d 0)) (while (consp x) (setq d (1+ d) x (car x))) d) (point))))"#,
            r#"OK (999999 2000001)"#,
        ),
        (
            r#"(let ((x (read (concat (make-string 1000000 ?') "x"))) (d 0)) (while (consp x) (setq d (1+ d) x (car (cdr x)))) (list d x))"#,
            r#"OK (1000000 x)"#,
        ),
        (
            r#"(let ((x (read (concat (mapconcat #'identity (make-list 1000000 "(a . ") "") "nil" (make-string 1000000 ?\))))) (d 0)) (while (consp x) (setq d (1+ d) x (cdr x))) d)"#,
            r#"OK 1000000"#,
        ),
        (
            r#"(length (read (concat "(" (mapconcat #'identity (make-list 1000000 "1") " ") ")")))"#,
            r#"OK 1000000"#,
        ),
    ]);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `#N=` labels, records, closures and strings with text properties nested
/// fifty thousand levels deep read as in GNU, whose `read0` keeps them on its
/// own `read_stack`; substituting a label's placeholder must not recurse
/// once per level either.
#[test]
fn deeply_nested_labels_and_hash_syntax_read_like_gnu() {
    let failures = deep_data_mismatches(&[
        (
            r##"(let ((x (read (concat "#1=" (make-string 50000 ?\[) "nil" (make-string 50000 ?\])))) (n 0)) (while (vectorp x) (setq n (1+ n) x (aref x 0))) n)"##,
            r##"OK 50000"##,
        ),
        (
            r##"(let* ((v (read (concat "#1=" (make-string 50000 ?\[) "#1#" (make-string 50000 ?\])))) (x (aref v 0)) (n 1)) (while (null (eq x v)) (setq n (1+ n) x (aref x 0))) n)"##,
            r##"OK 50000"##,
        ),
        (
            r##"(let ((x (read (concat "#1=" (make-string 50000 ?\() "nil" (make-string 50000 ?\))))) (n 0)) (while (consp x) (setq n (1+ n) x (car x))) n)"##,
            r##"OK 50000"##,
        ),
        (
            r##"(let* ((v (read (concat "#1=" (make-string 50000 ?\() "#1#" (make-string 50000 ?\))))) (x (car v)) (n 1)) (while (null (eq x v)) (setq n (1+ n) x (car x))) n)"##,
            r##"OK 50000"##,
        ),
        (
            r##"(let ((x (read (concat (make-string 50000 ?\[) "#1=[a]" (make-string 50000 ?\])))) (n 0)) (while (vectorp x) (setq n (1+ n) x (aref x 0))) (list n x))"##,
            r##"OK (50001 a)"##,
        ),
        (
            r##"(let ((x (read (concat (apply #'concat (make-list 50000 "#s(r ")) "nil" (make-string 50000 ?\))))) (n 0)) (while (recordp x) (setq n (1+ n) x (aref x 1))) n)"##,
            r##"OK 50000"##,
        ),
        (
            r##"(let ((x (read (concat "#1=" (apply #'concat (make-list 50000 "#s(r ")) "nil" (make-string 50000 ?\))))) (n 0)) (while (recordp x) (setq n (1+ n) x (aref x 1))) n)"##,
            r##"OK 50000"##,
        ),
        (
            r##"(let* ((v (read (concat "#1=" (apply #'concat (make-list 50000 "#s(r ")) "#1#" (make-string 50000 ?\))))) (x (aref v 1)) (n 1)) (while (null (eq x v)) (setq n (1+ n) x (aref x 1))) n)"##,
            r##"OK 50000"##,
        ),
        (
            r##"(let ((x (read (concat "#1=" (apply #'concat (make-list 50000 "#[nil (")) "nil" (apply #'concat (make-list 50000 ") nil]"))))) (n 0)) (while x (setq n (1+ n) x (car (aref x 1)))) n)"##,
            r##"OK 50000"##,
        ),
        (
            r##"(let ((x (read (concat (apply #'concat (make-list 50000 "#(\"x\" 0 1 (p ")) "nil" (make-string 100000 ?\))))) (n 0)) (while (stringp x) (setq n (1+ n) x (get-text-property 0 'p x))) n)"##,
            r##"OK 50000"##,
        ),
        (
            r##"(let ((x (read (concat "#1=" (make-string 50000 ?') "x"))) (n 0)) (while (consp x) (setq n (1+ n) x (car (cdr x)))) (list n x))"##,
            r##"OK (50000 x)"##,
        ),
    ]);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A string's property runs are snapshots: read-label replacements must be
/// written back after walking the plists, including direct keys and values.
#[test]
fn read_labels_in_string_properties_preserve_identity() {
    for (src, expected) in [
        (
            r##"(let ((s (read "#1=#(\"x\" 0 1 (p #1#))"))) (eq (get-text-property 0 'p s) s))"##,
            "OK t",
        ),
        (
            r##"(let* ((v (read "#1=[#(\"x\" 0 1 (p #1#))]")) (s (aref v 0))) (eq (get-text-property 0 'p s) v))"##,
            "OK t",
        ),
        (
            r##"(let ((s (read "#1=#(\"x\" 0 1 (#1# value))"))) (list (eq (car (text-properties-at 0 s)) s) (get-text-property 0 s s)))"##,
            "OK (t value)",
        ),
        (
            r##"(let* ((v (read "#1=[#(\"xy\" 0 1 (p #1# q 7) 1 2 (p #1# q 8))]")) (s (aref v 0))) (list (eq (get-text-property 0 'p s) v) (eq (get-text-property 1 'p s) v) (get-text-property 0 'q s) (get-text-property 1 'q s)))"##,
            "OK (t t 7 8)",
        ),
        (
            r##"(let* ((s (read "#1=#(\"x\" 0 1 (p #(\"y\" 0 1 (p #1#))))")) (child (get-text-property 0 'p s))) (eq (get-text-property 0 'p child) s))"##,
            "OK t",
        ),
        (
            r##"(let ((s (read "#1=#(\"x\" 0 1 (p (#1#)))"))) (eq (car (get-text-property 0 'p s)) s))"##,
            "OK t",
        ),
    ] {
        assert_eq!(princ_eval(src), expected, "{src}");
    }
}
