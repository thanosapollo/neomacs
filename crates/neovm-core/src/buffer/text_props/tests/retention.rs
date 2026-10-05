use super::*;

fn churn(table: &mut TextPropertyTable, cycles: usize) {
    let name = Value::symbol("retention-probe");
    for _ in 0..cycles {
        table.set_properties_for_object_char_len(
            char_range(8, 24),
            char_len(32),
            vec![(name, Value::fixnum(1))],
        );
        table.set_properties_for_object_char_len(char_range(0, 32), char_len(32), Vec::new());
    }
}

#[test]
fn fixed_size_property_churn_bounds_arena_slots() {
    let mut context = crate::emacs_core::Context::new();
    context.setup_thread_locals();
    let mut table = TextPropertyTable::new();
    for batch in 1..=5 {
        churn(&mut table, 20_000);
        let roots = save_scratch_gc_roots();
        table.for_each_root(push_scratch_gc_root);
        let epoch = context.eval_str("gcs-done").unwrap().as_fixnum().unwrap();
        context.gc_collect_exact();
        assert!(context.eval_str("gcs-done").unwrap().as_fixnum().unwrap() > epoch);
        restore_scratch_gc_roots(roots);
        let slots = table.intervals.nodes.len();
        let capacity = table.intervals.nodes.capacity();
        let live = table.intervals.runs().len();
        eprintln!(
            "cycles={} slots={slots} capacity={capacity} live={live} node_bytes={}",
            batch * 20_000,
            std::mem::size_of::<IntervalNode>()
        );
        assert_eq!(live, 1);
        assert!(
            slots <= 3,
            "fixed 32-character churn retained {slots} slots"
        );
        assert!(
            capacity <= 4,
            "fixed 32-character churn retained {capacity} allocated slots"
        );
        table.intervals.assert_invariants_for_test();
    }
}

#[test]
fn recycled_slots_and_retained_cow_snapshots_are_independent() {
    use std::rc::Rc;
    let _context = crate::emacs_core::Context::new();
    let mut current = Rc::new(TextPropertyTable::new());
    churn(Rc::make_mut(&mut current), 100);
    // Clone with free slots, then reuse them independently on both sides.
    let mut snapshot = Rc::clone(&current);
    assert!(Rc::ptr_eq(&current, &snapshot));
    let name = Value::symbol("retention-probe");
    Rc::make_mut(&mut current).set_properties_for_object_char_len(
        char_range(4, 28),
        char_len(32),
        vec![(name, Value::fixnum(7))],
    );
    assert!(!Rc::ptr_eq(&current, &snapshot));
    let frozen = current.clone();
    let frozen_runs = frozen.interval_plist_runs_for_test();
    churn(Rc::make_mut(&mut snapshot), 2_000);
    churn(Rc::make_mut(&mut current), 2_000);
    assert_eq!(frozen.interval_plist_runs_for_test(), frozen_runs);
    assert_eq!(get_at_char(&frozen, 16, name), Some(Value::fixnum(7)));
    assert_eq!(get_at_char(&snapshot, 16, name), None);
    assert_eq!(get_at_char(&current, 16, name), None);
    for table in [&current, &snapshot, &frozen] {
        table.intervals.assert_invariants_for_test();
        assert!(table.intervals.nodes.len() <= 5);
        assert!(table.intervals.nodes.capacity() <= 8);
    }
}

#[test]
fn trailing_prune_and_whole_delete_reuse_every_retired_slot() {
    let _context = crate::emacs_core::Context::new();
    let name = Value::symbol("retention-probe");
    let mut table = TextPropertyTable::new();
    for _ in 0..5_000 {
        set_chars(&mut table, 8, 24, vec![(name, Value::fixnum(1))]);
        // Seed both position and rightmost memos immediately before retirement.
        assert!(table.intervals.find_id(char_pos(16)).is_some());
        clear_chars(&mut table, 0, 32);
        assert!(table.is_empty());
        table.intervals.assert_invariants_for_test();
        assert!(table.intervals.nodes.len() <= 2);
        set_chars_for_object_len(&mut table, 4, 28, 32, vec![(name, Value::fixnum(2))]);
        delete_char_range(&mut table, 0, 32);
        assert_eq!(table.intervals.nodes.len(), 0);
        assert!(table.intervals.find_id(char_pos(0)).is_none());
        table.intervals.assert_invariants_for_test();
        assert!(table.intervals.nodes.capacity() <= 4);
    }
}

#[test]
fn mixed_edit_update_and_memo_semantics_match_character_model() {
    let _context = crate::emacs_core::Context::new();
    let name = Value::symbol("syntax-table");
    let mut table = TextPropertyTable::new();
    let mut expected = vec![None; 32];
    let mut seed = 0x81a3_769du64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as usize
    };
    let mut peak = 0;
    for step in 0..5_000 {
        let start = random() % expected.len();
        let end = start + 1 + random() % (expected.len() - start);
        let value = Value::fixnum((step % 7 + 1) as i64);
        match random() % 6 {
            0 => {
                set_chars_for_object_len(
                    &mut table,
                    start,
                    end,
                    expected.len(),
                    vec![(name, value)],
                );
                expected[start..end].fill(Some(value));
            }
            1 => {
                put_chars_for_object_len(&mut table, start, end, expected.len(), name, value);
                expected[start..end].fill(Some(value));
            }
            2 => {
                remove_chars(&mut table, start, end, name);
                expected[start..end].fill(None);
            }
            3 if expected.len() < 64 => {
                insert_chars_at(&mut table, start, 1);
                expected.insert(start, None);
            }
            4 if expected.len() > 8 => {
                delete_char_range(&mut table, start, start + 1);
                expected.remove(start);
            }
            _ => {
                clear_chars(&mut table, start, end);
                expected[start..end].fill(None);
            }
        }
        table.intervals.assert_invariants_for_test();
        assert!(table.debug_syntax_caches_consistent().is_ok());
        peak = peak.max(table.intervals.runs().len());
        assert!(table.intervals.nodes.len() <= peak + 2);
        for (pos, value) in expected.iter().enumerate() {
            assert_eq!(
                get_at_char(&table, pos, name),
                *value,
                "step {step}, pos {pos}"
            );
            assert_eq!(
                table.intervals.find_id(char_pos(pos)),
                table.intervals.find_id_uncached(char_pos(pos))
            );
        }
    }
}

#[test]
fn recycled_nodes_trace_only_reachable_plists_across_exact_gc() {
    let mut context = crate::emacs_core::Context::new();
    context.setup_thread_locals();
    let name = Value::symbol("retention-probe");
    let payload = Value::string("snapshot survives arena recycling");
    let mut table = TextPropertyTable::new();
    set_chars_for_object_len(&mut table, 8, 24, 32, vec![(name, payload)]);
    let snapshot = table.clone();
    set_chars_for_object_len(&mut table, 0, 32, 32, Vec::new());
    churn(&mut table, 1_000);
    let mut roots = Vec::new();
    table.trace_roots(&mut roots);
    assert_eq!(roots, vec![Value::NIL]);
    snapshot.trace_roots(&mut roots);
    assert_eq!(roots.len(), 4); // live nil / payload / nil partition in snapshot
    let saved = save_scratch_gc_roots();
    for root in &roots {
        push_scratch_gc_root(*root);
    }
    let epoch = context.eval_str("gcs-done").unwrap().as_fixnum().unwrap();
    context.gc_collect_exact();
    assert!(context.eval_str("gcs-done").unwrap().as_fixnum().unwrap() > epoch);
    assert_eq!(get_at_char(&snapshot, 16, name), Some(payload));
    assert_eq!(
        payload.as_lisp_string().unwrap().as_bytes(),
        b"snapshot survives arena recycling"
    );
    restore_scratch_gc_roots(saved);
    assert_eq!(save_scratch_gc_roots(), saved);
    table.intervals.assert_invariants_for_test();
    snapshot.intervals.assert_invariants_for_test();
}
