//! Direct Rust regression for the first-partition char-table scan/store seam.
//!
//! The fixture sends ordinary heap objects through the actual mapped-veclike
//! scan path. It never needs a pdump file, Lisp evaluation, or a timing sleep.

use super::*;
use crate::emacs_core::chartable::{builtin_char_table_range, builtin_set_char_table_range};
use crate::emacs_core::intern::intern_uninterned;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Barrier, Condvar, Mutex};

const REPEATS: usize = 4096;

fn set_range(table: TaggedValue, range: TaggedValue, value: TaggedValue) {
    assert_eq!(
        builtin_set_char_table_range(vec![table, range, value], None).unwrap(),
        value,
    );
}

fn fixture_headers(table: TaggedValue) -> Vec<usize> {
    let mut values = vec![table];
    let mut cursor = 0;
    while cursor < values.len() {
        let value = values[cursor];
        cursor += 1;
        if let Some(object) = value.as_char_table_obj() {
            values.extend(
                object
                    .contents
                    .iter()
                    .copied()
                    .filter(|child| child.veclike_type() == Some(VecLikeType::SubCharTable)),
            );
        } else if let Some(object) = value.as_sub_char_table_obj() {
            values.extend(
                object
                    .contents
                    .as_slice()
                    .iter()
                    .copied()
                    .filter(|child| child.veclike_type() == Some(VecLikeType::SubCharTable)),
            );
        }
    }
    assert_eq!(
        values.len(),
        4,
        "the fixture must prebuild all three levels"
    );
    values
        .into_iter()
        .map(|value| value.as_veclike_ptr().unwrap() as usize)
        .collect()
}

#[test]
fn concurrent_mapped_chartable_scan_and_builtin_range_writes_are_atomic() {
    let mut heap = TaggedHeap::new();
    set_tagged_heap(&mut heap);
    let purpose_id = intern_uninterned("ctrace-purpose");
    let sentinel_id = intern_uninterned("ctrace-untouched-leaf");
    let purpose = TaggedValue::from_sym_id(purpose_id);
    let sentinel = TaggedValue::from_sym_id(sentinel_id);
    let table = TaggedValue::make_char_table(purpose, TaggedValue::fixnum(-1), 0);
    set_range(table, TaggedValue::fixnum(0), TaggedValue::fixnum(0));
    set_range(table, TaggedValue::fixnum(1), sentinel);
    let headers = fixture_headers(table);

    // This is the same confined manual-job pattern as the major worker
    // fixtures: begin a real owner-local mark, publish only retained addresses
    // and immutable ownership metadata, and join before reclamation. The worker
    // never gets the heap itself. The explicit phase mirror makes each builtin
    // use the real SATB preimage barrier while the job reads its slots.
    heap.concurrent_begin();
    heap.concurrent_mark_running = true;
    set_tagged_heap(&mut heap);
    let (exited, result) = std::sync::mpsc::channel();
    let deferred = Arc::new(Mutex::new(Vec::new()));
    let job = ConcurrentMarkJob {
        gray: MarkStack::default(),
        claims: ConcurrentClaimJob {
            major: true,
            parity: heap.mark_parity,
            pages: PageSnapshot::BaseSets {
                cons: heap.cons_blocks.iter().map(ConsBlock::base_addr).collect(),
                string: heap
                    .string_arena
                    .pages
                    .iter()
                    .map(|page| page.base_addr())
                    .collect(),
                float: heap
                    .float_arena
                    .pages
                    .iter()
                    .map(|page| page.base_addr())
                    .collect(),
                vector: heap
                    .vector_arena
                    .pages
                    .iter()
                    .map(|page| page.base_addr())
                    .collect(),
                bytecode: heap
                    .bytecode_arena
                    .pages
                    .iter()
                    .map(|page| page.base_addr())
                    .collect(),
            },
            dump_lo: heap.dump_addr_lo,
            dump_hi: heap.dump_addr_hi,
            drop_dump_children: false,
            str_claimed: Arc::new(AtomicUsize::new(0)),
            float_claimed: Arc::new(AtomicUsize::new(0)),
            vec_claimed: Arc::new(AtomicUsize::new(0)),
            bc_claimed: Arc::new(AtomicUsize::new(0)),
            subr_dropped: Arc::new(AtomicUsize::new(0)),
        },
        satb: heap.satb_shared.clone(),
        deferred: deferred.clone(),
        done: Arc::new(AtomicBool::new(false)),
        // The flat first-partition scan always completes before the loop
        // honors stop, so the job terminates without an idle wait.
        stop: Arc::new(AtomicBool::new(true)),
        wake: Arc::new((Mutex::new(()), Condvar::new())),
        exited,
        obarray: None,
        vectors: None,
        mapped_cons_ranges: None,
        mapped_veclikes: Some(headers.repeat(REPEATS)),
    };
    let start = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let worker_start = start.clone();
        let worker = scope.spawn(move || {
            worker_start.wait();
            run_concurrent_mark(job);
        });
        start.wait();
        for ordinal in 0..REPEATS {
            let value = TaggedValue::fixnum(ordinal as i64);
            // char0 writes the existing depth-3 slot; nil writes the default
            // field. No allocation, backing replacement, or topology change
            // is possible after the start handshake.
            set_range(table, TaggedValue::fixnum(0), value);
            set_range(table, TaggedValue::NIL, value);
        }
        worker.join().expect("the confined marker must finish");
    });
    let result = result.recv().expect("the marker must return its handoff");
    assert!(result.symbols.contains(&purpose_id));
    assert!(result.symbols.contains(&sentinel_id));
    assert!(result.promo.is_empty());
    assert_eq!(
        builtin_char_table_range(vec![table, TaggedValue::fixnum(0)], None).unwrap(),
        TaggedValue::fixnum((REPEATS - 1) as i64),
    );
    assert_eq!(
        builtin_char_table_range(vec![table, TaggedValue::NIL], None).unwrap(),
        TaggedValue::fixnum((REPEATS - 1) as i64),
    );
    assert_eq!(
        builtin_char_table_range(vec![table, TaggedValue::fixnum(1)], None).unwrap(),
        sentinel,
    );
    // The job has exited and the scoped worker was joined explicitly. Restore
    // the mirrors before ordinary heap teardown; Drop never joins this job.
    heap.concurrent_mark_running = false;
    set_tagged_heap(&mut heap);
    heap.collect_exact(std::iter::once(table));
}
