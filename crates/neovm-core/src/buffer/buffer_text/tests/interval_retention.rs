use super::*;
use crate::emacs_core::Context;
use crate::gc_trace::GcTrace;

#[test]
fn indirect_buffer_property_churn_preserves_snapshot_and_bounds_slots() {
    let mut context = Context::new();
    context.setup_thread_locals();
    context
        .eval_str(
            "(progn (set-buffer (get-buffer-create \" interval-retention-base\"))
          (setq buffer-undo-list t)
          (insert \"01234567890123456789012345678901\"))",
        )
        .unwrap();
    let base = context.buffers.current_buffer_id().unwrap();
    let indirect = context
        .buffers
        .create_indirect_buffer(base, " interval-retention-indirect", false)
        .unwrap();
    let name = Value::symbol("retention-probe");
    context
        .eval_str("(set-text-properties 9 25 '(retention-probe 7))")
        .unwrap();
    let snapshot = context.buffers.get(base).unwrap().text.clone();
    let before = snapshot.full_text_string();
    // A Rust snapshot is not automatically enumerated by Context's collector;
    // root it across every evaluator entry, not only the explicit collection.
    let saved = crate::emacs_core::eval::save_scratch_gc_roots();
    let mut roots = Vec::new();
    snapshot.storage.borrow().text_props.trace_roots(&mut roots);
    for root in roots {
        crate::emacs_core::eval::push_scratch_gc_root(root);
    }
    context.buffers.set_current(indirect);
    for batch in 0..5 {
        for _ in 0..4_000 {
            context.eval_str("(progn (set-text-properties 9 25 '(retention-probe 1)) (set-text-properties 1 33 nil))").unwrap();
        }
        context.gc_collect_exact();
        let base_text = &context.buffers.get(base).unwrap().text;
        let indirect_text = &context.buffers.get(indirect).unwrap().text;
        assert!(base_text.shares_storage_with(indirect_text));
        let (slots, capacity) = base_text
            .storage
            .borrow()
            .text_props
            .arena_slot_counts_for_test();
        assert!(
            slots <= 3 && capacity <= 4,
            "batch {batch}: {slots}/{capacity}"
        );
        assert_eq!(snapshot.full_text_string(), before);
        assert_eq!(
            snapshot.text_props_get_property_at_char_pos(CharPos0::new(16), name),
            Some(Value::fixnum(7))
        );
        assert_eq!(
            base_text.text_props_get_property_at_char_pos(CharPos0::new(16), name),
            None
        );
        assert_eq!(
            context.buffers.get(base).unwrap().get_undo_list(),
            Value::symbol("t")
        );
        assert_eq!(
            context.buffers.get(indirect).unwrap().get_undo_list(),
            Value::symbol("t")
        );
        assert_eq!(base_text.full_text_string(), before);
    }
    crate::emacs_core::eval::restore_scratch_gc_roots(saved);
}

#[test]
fn buffer_text_snapshots_recycle_independent_arenas_in_both_modes() {
    use crate::buffer::text_snapshot::{TextSnapshotMode, set_text_snapshot_mode_override};
    let _context = Context::new();
    for mode in [TextSnapshotMode::Share, TextSnapshotMode::Copy] {
        set_text_snapshot_mode_override(Some(mode));
        let text = BufferText::from_str("01234567890123456789012345678901");
        let name = Value::symbol("retention-probe");
        text.text_props_set_properties_in_char_range(
            CharRange::new(CharPos0::new(8), CharPos0::new(24)),
            vec![(name, Value::fixnum(9))],
        );
        let snapshot = text.clone();
        let indirect = text.shared_clone();
        for _ in 0..2_000 {
            indirect.text_props_set_properties_in_char_range(
                CharRange::new(CharPos0::new(8), CharPos0::new(24)),
                vec![(name, Value::fixnum(1))],
            );
            indirect.text_props_set_properties_in_char_range(
                CharRange::new(CharPos0::ZERO, CharPos0::new(32)),
                Vec::new(),
            );
        }
        assert_eq!(
            snapshot.text_props_get_property_at_char_pos(CharPos0::new(16), name),
            Some(Value::fixnum(9))
        );
        assert_eq!(
            text.text_props_get_property_at_char_pos(CharPos0::new(16), name),
            None
        );
        assert!(text.shares_storage_with(&indirect));
        assert!(!text.shares_storage_with(&snapshot));
        for owner in [&text, &snapshot] {
            let (slots, capacity) = owner
                .storage
                .borrow()
                .text_props
                .arena_slot_counts_for_test();
            assert!(slots <= 3 && capacity <= 4);
        }
    }
    set_text_snapshot_mode_override(None);
}
