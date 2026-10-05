use super::*;
use crate::emacs_core::eval::{Context, save_scratch_gc_roots};
use crate::emacs_core::string_pos_cache::{self, work};
use crate::heap_types::LispString;
use std::cell::Cell;

thread_local! {
    // Observe the payload handed to the production reader, not a model of it.
    static SOURCES: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

pub(super) fn observe_source(source: Value, payload: &LispString) {
    let original = source.as_lisp_string().unwrap();
    let copied = !std::ptr::eq(original, payload)
        || original.as_bytes().as_ptr() != payload.as_bytes().as_ptr();
    SOURCES.with(|sources| {
        let (calls, copies) = sources.get();
        sources.set((calls + 1, copies + usize::from(copied)));
    });
}

fn sequential_reads(token: &str, late_start: bool, explicit_end: bool) {
    let mut ctx = Context::new();
    ctx.setup_thread_locals();
    let forms = 4096;
    let form = format!("\"{token}\" ");
    let stride = form.chars().count();
    let source = Value::string(form.repeat(forms));
    let bytes = source.as_lisp_string().unwrap().sbytes();
    let chars = source.as_lisp_string().unwrap().schars();
    let mut pos = if late_start { forms / 2 * stride } else { 0 };
    let reads = (chars - pos) / stride;
    string_pos_cache::reset_string_pos_cache();
    work::reset();
    SOURCES.with(|sources| sources.set((0, 0)));
    let roots = save_scratch_gc_roots();
    for _ in 0..reads {
        let mut args = vec![source, Value::fixnum(pos as i64)];
        if explicit_end {
            args.push(Value::fixnum(chars as i64));
        }
        let pair = builtin_read_from_string(&mut ctx, args).unwrap();
        assert_eq!(
            pair.cons_car().as_lisp_string().map(|s| s.as_bytes()),
            Some(token.as_bytes())
        );
        assert_eq!(pair.cons_cdr().as_fixnum(), Some((pos + stride - 1) as i64));
        pos = pair.cons_cdr().xfixnum() as usize + 1;
        assert_eq!(save_scratch_gc_roots(), roots);
    }
    assert_eq!(pos, chars);
    assert_eq!(SOURCES.with(Cell::get), (reads, 0), "whole-input clone");
    let counted = work::snapshot();
    eprintln!("reader work: {reads} forms, {bytes} source bytes, {counted:?}");
    assert_eq!(
        counted.conversions,
        reads * 3,
        "bypassed positional authority"
    );
    assert!(
        counted.walked_bytes <= bytes * 2,
        "superlinear positional work: {counted:?}"
    );
    if token.is_ascii() {
        assert_eq!(counted.walked_bytes, 0);
    } else {
        assert!(
            counted.cache_hits >= reads,
            "sequential cache not reused: {counted:?}"
        );
        assert!(counted.walked_bytes > 0, "traversals were not counted");
    }
}

#[test]
fn read_from_string_sequential_ascii_borrows_without_prefix_work() {
    sequential_reads("a", false, false);
}

#[test]
fn read_from_string_sequential_greek_uses_linear_cached_work() {
    sequential_reads("α", false, false);
}

#[test]
fn read_from_string_late_start_and_explicit_end_use_linear_cached_work() {
    sequential_reads("αβ🙂", true, true);
}

#[test]
fn read_from_string_cache_survives_gc_and_rejects_same_address_mutation() {
    let mut ctx = Context::new();
    // Heap/pdump reset invalidates collection coverage. Activate afterward,
    // as runtime startup does, before populating the positional cache.
    string_pos_cache::reset_string_pos_cache();
    ctx.setup_thread_locals();
    let roots = save_scratch_gc_roots();
    let source = Value::string("aα \"β\" z");
    assert!(ctx.tagged_heap.owns_heap_value_for_test(source));
    // Only the positional cache roots SOURCE across the exact collection;
    // Rust locals are not Lisp roots.
    let pair = builtin_read_from_string(&mut ctx, vec![source, Value::fixnum(3)]).unwrap();
    assert_eq!(
        pair.cons_car().as_lisp_string().unwrap().as_bytes(),
        "β".as_bytes()
    );
    assert_eq!(pair.cons_cdr().as_fixnum(), Some(6));
    let old_data = source.as_lisp_string().unwrap().as_bytes().as_ptr();
    assert_eq!(save_scratch_gc_roots(), roots);
    let epoch = ctx.tagged_heap.gc_collections();
    ctx.gc_collect_exact();
    assert!(ctx.tagged_heap.gc_collections() > epoch);
    assert_eq!(save_scratch_gc_roots(), roots);
    assert!(ctx.tagged_heap.owns_heap_value_for_test(source));
    work::reset();
    let pair = builtin_read_from_string(&mut ctx, vec![source, Value::fixnum(3)]).unwrap();
    assert_eq!(pair.cons_cdr().as_fixnum(), Some(6));
    assert!(
        work::snapshot().cache_hits > 0,
        "covered GC lost warm entry"
    );
    // Keep identity, byte count and data pointer; move a multibyte character
    // across the old cached boundary. Only the mutation epoch can reject it.
    source.with_lisp_string_mut(|s| {
        s.mutate_bytes(|bytes| bytes.copy_from_slice("αa \"b\" β".as_bytes()));
    });
    assert_eq!(
        source.as_lisp_string().unwrap().as_bytes().as_ptr(),
        old_data
    );
    work::reset();
    let pair = builtin_read_from_string(&mut ctx, vec![source, Value::fixnum(3)]).unwrap();
    assert_eq!(pair.cons_car().as_lisp_string().unwrap().as_bytes(), b"b");
    assert_eq!(pair.cons_cdr().as_fixnum(), Some(6));
    assert_eq!(
        string_pos_cache::string_char_to_byte(source, source.as_lisp_string().unwrap(), 1),
        2
    );
    assert!(work::snapshot().walked_bytes > 0);
    assert_eq!(save_scratch_gc_roots(), roots);
    // Eviction must release the otherwise unreachable source, not leak it.
    string_pos_cache::reset_string_pos_cache();
    ctx.setup_thread_locals();
    let epoch = ctx.tagged_heap.gc_collections();
    ctx.gc_collect_exact();
    assert!(ctx.tagged_heap.gc_collections() > epoch);
    assert!(!ctx.tagged_heap.owns_heap_value_for_test(source));
    assert_eq!(save_scratch_gc_roots(), roots);
}

#[test]
fn read_from_string_borrowed_source_preserves_semantics_and_root_balance() {
    let mut ctx = Context::new();
    ctx.setup_thread_locals();
    let roots = save_scratch_gc_roots();
    for (text, start, end, expected, position) in [
        ("αβ 42 z", 3, 5, 42, 5),
        ("αβ 42 z", -4, -2, 42, 5),
        ("αβ 42", -2, 5, 42, 5),
    ] {
        let pair = builtin_read_from_string(
            &mut ctx,
            vec![
                Value::string(text),
                Value::fixnum(start),
                Value::fixnum(end),
            ],
        )
        .unwrap();
        assert_eq!(pair.cons_car().as_fixnum(), Some(expected));
        assert_eq!(pair.cons_cdr().as_fixnum(), Some(position));
        assert_eq!(save_scratch_gc_roots(), roots);
    }
    let source = Value::heap_string(LispString::from_unibyte(vec![0xff, b' ', b'4', b'2']));
    let pair = builtin_read_from_string(&mut ctx, vec![source, Value::fixnum(-2)]).unwrap();
    assert_eq!(pair.cons_car().as_fixnum(), Some(42));
    assert_eq!(pair.cons_cdr().as_fixnum(), Some(4));
    for args in [
        vec![Value::fixnum(3)],
        vec![Value::string("α"), Value::fixnum(2)],
        vec![Value::string("42"), Value::fixnum(2), Value::fixnum(1)],
        vec![Value::string(" ")],
        vec![Value::string("(")],
    ] {
        assert!(builtin_read_from_string(&mut ctx, args).is_err());
        assert_eq!(save_scratch_gc_roots(), roots, "reader error leaked roots");
    }
}

#[test]
fn read_from_string_borrowed_intervals_and_shorthands_survive_collection() {
    let mut ctx = Context::new();
    ctx.setup_thread_locals();
    ctx.obarray_mut().set_symbol_value(
        "read-symbol-shorthands",
        Value::list(vec![Value::cons(
            Value::string("perf:"),
            Value::string("reader-performance-"),
        )]),
    );
    let source = Value::string("(perf:symbol #(\"α\" 0 1 (face bold)))");
    crate::emacs_core::textprop::builtin_put_text_property(
        &mut ctx,
        vec![
            Value::fixnum(0),
            Value::fixnum(1),
            Value::symbol("help-echo"),
            Value::string("source interval"),
            source,
        ],
    )
    .unwrap();
    let pair = builtin_read_from_string(&mut ctx, vec![source]).unwrap();
    let value = pair.cons_car();
    assert_eq!(
        value.cons_car().as_symbol_name(),
        Some("reader-performance-symbol")
    );
    let literal = value.cons_cdr().cons_car();
    assert_eq!(literal.as_lisp_string().unwrap().as_bytes(), "α".as_bytes());
    assert!(get_string_text_properties_table_for_value(literal).is_some());
    assert!(get_string_text_properties_table_for_value(source).is_some());
    ctx.obarray_mut()
        .set_symbol_value("reader-perf-result", pair);
    ctx.obarray_mut()
        .set_symbol_value("reader-perf-input", source);
    ctx.gc_collect_exact();
    assert_eq!(literal.as_lisp_string().unwrap().as_bytes(), "α".as_bytes());
    let props = get_string_text_properties_table_for_value(literal).unwrap();
    assert_eq!(
        props.get_property_at_char_pos(crate::buffer::CharPos0::ZERO, Value::symbol("face")),
        Some(Value::symbol("bold"))
    );
}
