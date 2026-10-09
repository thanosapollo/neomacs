//! The header's generation byte and THE generation predicate (P3.1 C2.1),
//! and the byte map every later header commit builds on (P3.0 §3.1).

use super::*;

fn header_of(value: TaggedValue) -> *mut GcHeader {
    TaggedHeap::value_heap_addr(value).expect("a heap object") as *mut GcHeader
}

/// A new header is all zero outside `kind`: the reserved bytes, the
/// generation byte and `remembered` (the pdump image writes the same).
#[test]
fn a_new_header_zeroes_every_byte_but_kind() {
    let header = GcHeader::new(HeapObjectKind::VecLike);
    // SAFETY: the header is 16 plain bytes (a pointer and eight bytes).
    let bytes: [u8; 16] = unsafe { std::mem::transmute(header) };
    assert_eq!(bytes[1], u8::from(HeapObjectKind::VecLike));
    for (i, byte) in bytes.iter().enumerate() {
        if i != 1 {
            assert_eq!(*byte, 0, "byte {i}");
        }
    }
    let header = GcHeader::new(HeapObjectKind::Float);
    assert_eq!(header.generation, GenBits::NONE);
    assert!(!header.generation.permanent());
}

/// The predicate: a young object is black in no scope; an old object only
/// in a young collection; a permanent in every scope. Permanent implies
/// tenured, so a young-scope answer is exactly the `tenured` byte.
#[test]
fn black_by_generation_per_scope() {
    let mut young = GcHeader::new(HeapObjectKind::VecLike);
    assert!(!young.black_by_generation(CollectionScope::Young));
    assert!(!young.black_by_generation(CollectionScope::Full));

    // An old (promoted, not permanent) object: only P3.1's minors make one.
    young.tenured = true;
    let old = young;
    assert!(old.black_by_generation(CollectionScope::Young));
    assert!(!old.black_by_generation(CollectionScope::Full));

    let mut permanent = GcHeader::new(HeapObjectKind::String);
    permanent.make_permanent();
    assert!(permanent.tenured);
    assert!(permanent.generation.permanent());
    assert!(permanent.black_by_generation(CollectionScope::Young));
    assert!(permanent.black_by_generation(CollectionScope::Full));

    let heap = TaggedHeap::new();
    assert_eq!(heap.collection_scope(), CollectionScope::Young);
}

/// Explicit fixture promotion makes every survivor — boxed objects and
/// page slots of every class — tenured AND permanent, and nothing else;
/// the permanents then stay black across later cycles of both parities.
#[test]
fn explicit_permanent_survivors_stay_black_across_partition_cycles() {
    let mut heap = TaggedHeap::new();
    set_tagged_heap(&mut heap);
    heap.extend_dump_span(4096, 16);
    let table = heap.alloc_hash_table(crate::emacs_core::value::LispHashTable::new(
        crate::emacs_core::value::HashTableTest::Eq,
    ));
    let vector = heap.alloc_vector(vec![TaggedValue::NIL; 2]);
    let string = heap.alloc_string(crate::heap_types::LispString::from_utf8("perm"));
    let float = heap.alloc_float(1.5);
    let record = heap.alloc_record(vec![TaggedValue::T]);
    let lambda = heap.alloc_lambda(vec![TaggedValue::NIL]);
    let kept = [table, vector, string, float, record, lambda];
    let mut root = TaggedValue::NIL;
    for &value in &kept {
        root = heap.alloc_cons(value, root);
    }
    heap.collect_exact(std::iter::once(root));
    heap.make_survivors_permanent_for_test();
    for &value in &kept {
        let header = unsafe { &*header_of(value) };
        assert!(header.tenured, "{value:?}");
        assert!(header.generation.permanent(), "{value:?}");
        assert!(header.black_by_generation(CollectionScope::Full));
    }
    // A young object allocated after the promotion is neither.
    let young = heap.alloc_vector(vec![TaggedValue::NIL]);
    let young_header = unsafe { &*header_of(young) };
    assert!(!young_header.tenured);
    assert_eq!(young_header.generation, GenBits::NONE);
    // Later cycles leave the permanents alone (tenured-skip unchanged).
    let root2 = heap.alloc_cons(young, root);
    for _ in 0..3 {
        heap.collect_exact(std::iter::once(root2));
        for &value in &kept {
            assert!(heap.is_value_marked(value));
            assert!(unsafe { &*header_of(value) }.generation.permanent());
        }
    }
    assert!(heap.is_value_marked(young));
}

/// An ordinary session survivor is not a permanent merely because an image
/// was registered before its first collection. It remains collectible when
/// its last root disappears, in both legacy and generational modes.
#[test]
fn ordinary_first_partition_survivors_remain_collectible() {
    for generational in [false, true] {
        let mut heap = TaggedHeap::new();
        heap.generational.enabled = generational;
        set_tagged_heap(&mut heap);
        heap.extend_dump_span(4096, 16);
        let table = heap.alloc_hash_table(crate::emacs_core::value::LispHashTable::new(
            crate::emacs_core::value::HashTableTest::Eq,
        ));
        let vector = heap.alloc_vector(vec![TaggedValue::NIL; 2]);
        let string = heap.alloc_string(crate::heap_types::LispString::from_utf8("session"));
        let float = heap.alloc_float(1.5);
        let record = heap.alloc_record(vec![TaggedValue::T]);
        let lambda = heap.alloc_lambda(vec![TaggedValue::NIL]);
        let kept = [table, vector, string, float, record, lambda];
        let mut root = TaggedValue::NIL;
        for &value in &kept {
            root = heap.alloc_cons(value, root);
        }
        assert!(heap.is_partition_first_cycle());
        heap.collect_exact(std::iter::once(root));
        assert!(heap.dump_blackened);
        assert!(!heap.is_partition_first_cycle());
        assert!(heap.tenured_objects.is_null());
        for &value in &kept {
            assert!(heap.owns_heap_value_for_test(value));
            assert!(!unsafe { (*header_of(value)).generation.permanent() });
            assert!(!unsafe { (*header_of(value)).black_by_generation(CollectionScope::Full) });
        }
        for _ in 0..2 {
            heap.collect_exact(std::iter::once(root));
            for &value in &kept {
                assert!(heap.owns_heap_value_for_test(value));
                assert!(!unsafe { (*header_of(value)).generation.permanent() });
            }
        }
        // No allocation or stale header/payload access after reclamation.
        heap.collect_exact(std::iter::empty());
        assert!(!heap.owns_heap_value_for_test(root));
        for &value in &kept {
            assert!(!heap.owns_heap_value_for_test(value));
        }
        assert!(heap.all_objects.is_null());
        assert!(heap.tenured_objects.is_null());
        assert!(heap.generational.old_objects.is_null());
        assert_eq!(heap.generational.old_cons_count, 0);
        assert_eq!(heap.generational.old_bytes, 0);
    }
}

/// A mapped object's current edge retains its heap child, but neither that
/// child nor an unrelated session root is permanently owned by the image.
#[test]
fn first_partition_keeps_session_and_image_children_collectible() {
    for generational in [false, true] {
        for concurrent in [false, true] {
            let mut heap = TaggedHeap::new();
            heap.generational.enabled = generational;
            set_tagged_heap(&mut heap);
            let image = fake_image::FakeImage::leak(false);
            let mapped = image.register_cons(&mut heap);
            let image_child = heap.alloc_vector(vec![TaggedValue::fixnum(11)]);
            let session = heap.alloc_hash_table(crate::emacs_core::value::LispHashTable::new(
                crate::emacs_core::value::HashTableTest::Eq,
            ));
            let image_addr = TaggedHeap::value_heap_addr(image_child).unwrap();
            let session_addr = TaggedHeap::value_heap_addr(session).unwrap();
            assert!(crate::tagged::mutate::set_cons_car(mapped, image_child));
            assert!(heap.is_partition_first_cycle());
            if concurrent {
                heap.arm_first_cycle_concurrent();
                heap.concurrent_begin();
                heap.seed_root(session);
                heap.launch_concurrent_mark();
                while !heap.concurrent_mark_done() {
                    std::thread::yield_now();
                }
                heap.join_concurrent_mark();
                heap.reseed_runtime_and_remembered_roots();
                heap.seed_root(session);
                let before = heap.live_bytes();
                heap.incremental_drain_all();
                heap.incremental_finish(before, std::time::Instant::now());
                heap.finish_incremental_sweep_now();
                heap.finish_first_partition_cycle();
            } else {
                heap.collect_exact(std::iter::once(session));
            }
            assert!(heap.dump_blackened);
            for address in [image_addr, session_addr] {
                assert!(heap.owns_non_cons_object(address as *const u8));
                assert!(
                    !unsafe { (*(address as *const GcHeader)).generation.permanent() },
                    "heap survivor became permanent (generational={generational}, concurrent={concurrent})"
                );
            }
            heap.collect_exact(std::iter::empty());
            assert!(
                heap.owns_non_cons_object(image_addr as *const u8),
                "image edge keeps child"
            );
            assert!(
                !heap.owns_non_cons_object(session_addr as *const u8),
                "released session dies"
            );
            assert!(crate::tagged::mutate::set_cons_car(
                mapped,
                TaggedValue::NIL
            ));
            heap.collect_exact(std::iter::empty());
            assert!(
                !heap.owns_non_cons_object(image_addr as *const u8),
                "detached image child dies"
            );
        }
    }
}

#[test]
fn full_session_page_is_not_retired_by_first_partition() {
    let mut heap = TaggedHeap::new();
    set_tagged_heap(&mut heap);
    heap.extend_dump_span(4096, 16);
    let mut root = TaggedValue::NIL;
    for i in 0..FLOAT_PAGE_SLOTS {
        let value = heap.alloc_float(i as f64);
        root = heap.alloc_cons(value, root);
    }
    heap.collect_exact(std::iter::once(root));
    assert!(heap.dump_blackened);
    assert_eq!(heap.float_arena.pages.len(), 1);
    assert!(
        !heap.float_arena.pages[0].retired,
        "runtime page must stay collectible"
    );
    heap.collect_exact(std::iter::empty());
    assert!(
        heap.float_arena.pages.iter().all(|p| p.allocated == 0),
        "released runtime slots reclaimed"
    );
}

// These controls use actual young-scope cycles, not collect_exact (a major).
// The cons is the sole explicit root; no owner store occurs after partition.
fn first_partition_then_true_minors(concurrent: bool, black_births: bool) {
    let mut heap = TaggedHeap::new();
    heap.generational.enabled = true;
    set_tagged_heap(&mut heap);
    let image = fake_image::FakeImage::leak(false);
    image.register_cons(&mut heap);
    let vector = heap.alloc_vector(vec![TaggedValue::fixnum(71)]);
    let string = heap.alloc_string(crate::heap_types::LispString::from_utf8("minor-child"));
    let string_owner = heap.alloc_cons(string, TaggedValue::NIL);
    let root = heap.alloc_cons(vector, string_owner);
    let mut born = Vec::new();
    assert!(heap.is_partition_first_cycle());
    if concurrent {
        heap.arm_first_cycle_concurrent();
        heap.concurrent_begin();
        heap.seed_root(root);
        heap.launch_concurrent_mark();
        if black_births {
            // Allocate during a real launched major, covering both explicit
            // header logs and used-prefix cons/float region logs.
            let v = heap.alloc_vector(vec![TaggedValue::fixnum(83)]);
            let f = heap.alloc_float(4.25);
            let tail = heap.alloc_cons(f, TaggedValue::NIL);
            let owner = heap.alloc_cons(v, tail);
            born.extend([owner, tail, v, f]);
            assert!(!heap.current_mutator_gc().black_born.is_empty());
        }
        while !heap.concurrent_mark_done() {
            std::thread::yield_now();
        }
        heap.join_concurrent_mark();
        assert!(
            heap.sweep_stats().last_concurrent_str_claimed >= 1,
            "the worker, not just termination, must claim the string"
        );
        if black_births {
            assert!(!heap.current_mutator_gc().black_born_regions.is_empty());
        }
        heap.reseed_runtime_and_remembered_roots();
        heap.seed_root(root);
        // Deliberately do not seed black births: P-all must consume their
        // constructor/log closure even if no fresh header claim occurs.
        let before = heap.live_bytes();
        heap.incremental_drain_all();
        heap.incremental_finish(before, std::time::Instant::now());
        heap.finish_incremental_sweep_now();
        heap.finish_first_partition_cycle();
    } else {
        heap.collect_exact(std::iter::once(root));
    }
    assert!(heap.dump_blackened);
    assert!(heap.value_is_old_for_test(root));
    for cycle in 0..3 {
        heap.begin_minor_collection();
        assert!(heap.is_minor_collection(), "must exercise the minor skip");
        assert_eq!(heap.collection_scope(), CollectionScope::Young);
        heap.seed_root(root);
        if black_births {
            heap.seed_root(born[0]);
        }
        heap.complete_minor_collection();
        heap.finish_incremental_sweep_now();
        // Check the ownership oracle BEFORE accessing possibly freed payloads.
        for value in [root, string_owner, vector, string]
            .into_iter()
            .chain(born.iter().copied())
        {
            assert!(
                heap.owns_heap_value_for_test(value),
                "rooted child reclaimed by true minor {cycle}: bits={:#x}",
                value.bits()
            );
            assert!(heap.value_is_old_for_test(value));
        }
        assert!(!unsafe { (*header_of(vector)).generation.permanent() });
        assert!(!unsafe { (*header_of(string)).generation.permanent() });
        assert_eq!(unsafe { (*root.xcons_ptr()).load_car() }, vector);
        assert_eq!(
            vector.as_vector_data().unwrap().as_slice()[0],
            TaggedValue::fixnum(71)
        );
        assert_eq!(
            unsafe { (*string.as_string_ptr().unwrap()).data.as_bytes() },
            b"minor-child"
        );
        if black_births {
            assert_eq!(
                born[2].as_vector_data().unwrap().as_slice()[0],
                TaggedValue::fixnum(83)
            );
            assert_eq!(born[3].as_float(), Some(4.25));
        }
        heap.assert_object_arenas_coherent();
    }
    assert!(heap.current_mutator_gc().black_born.is_empty());
    assert!(heap.current_mutator_gc().black_born_regions.is_empty());
    heap.collect_exact(std::iter::empty());
    for value in [root, string_owner, vector, string]
        .into_iter()
        .chain(born.iter().copied())
    {
        assert!(
            !heap.owns_heap_value_for_test(value),
            "ordinary old must die in a full cycle"
        );
    }
    assert_eq!(heap.generational.old_cons_count, 0);
    assert_eq!(heap.generational.old_bytes, 0);
    assert!(heap.generational.old_objects.is_null());
    assert!(heap.tenured_objects.is_null());
    heap.assert_object_arenas_coherent();
}

#[test]
fn first_partition_stw_rooted_cons_children_survive_true_minors() {
    first_partition_then_true_minors(false, false);
}

#[test]
fn first_partition_worker_claimed_cons_children_survive_true_minors() {
    first_partition_then_true_minors(true, false);
}

#[test]
fn first_partition_black_birth_cons_children_survive_true_minors() {
    first_partition_then_true_minors(true, true);
}

/// The tri-state mark byte (P3.1 C2.2): 0 is unmarked at rest under both
/// parities; a parity value is marked only at that parity; a claim moves
/// the byte to the claiming parity from rest or from the other parity, once.
#[test]
fn the_mark_byte_is_tri_state() {
    let header = GcHeader::new(HeapObjectKind::VecLike);
    assert_eq!(header.raw_mark(), UNMARKED_AT_REST);
    assert!(!header.is_marked());
    for parity in [MarkParity::One, MarkParity::Two] {
        assert!(
            !header.is_marked_at(parity),
            "at rest is white at {parity:?}"
        );
    }
    assert!(header.mark_claim_at(MarkParity::One), "claim from rest");
    assert!(
        !header.mark_claim_at(MarkParity::One),
        "a second claim loses"
    );
    assert!(header.is_marked_at(MarkParity::One));
    assert!(!header.is_marked_at(MarkParity::Two));
    assert!(
        header.mark_claim_at(MarkParity::Two),
        "claim from the other parity"
    );
    assert_eq!(header.raw_mark(), MarkParity::Two.byte());
    let born = GcHeader::new_marked(HeapObjectKind::Float, MarkParity::One);
    assert!(born.is_marked_at(MarkParity::One));
    assert!(!born.is_marked_at(MarkParity::Two));
    assert_eq!(MarkParity::One.flip(), MarkParity::Two);
    assert_eq!(MarkParity::Two.flip(), MarkParity::One);
}

/// The heap's parity starts at `Two` so the bootstrap cycle runs at `One`
/// and alternates every collection. Tracing sets the cycle parity; the
/// generational sweep resets ordinary old survivors to rest (GEN-3), while
/// the legacy sweep leaves their cycle marks. Rest is white at both parities.
#[test]
fn parity_alternates_and_rest_is_white_under_both() {
    let mut heap = TaggedHeap::new();
    set_tagged_heap(&mut heap);
    assert_eq!(heap.mark_parity, MarkParity::Two);
    let v = heap.alloc_vector(vec![TaggedValue::NIL]);
    let header = header_of(v);
    assert_eq!(
        unsafe { (*header).raw_mark() },
        MarkParity::Two.byte(),
        "born at parity"
    );
    for parity in [MarkParity::One, MarkParity::Two] {
        heap.begin_stw_collection();
        heap.seed_root(v);
        heap.mark_all();
        assert_eq!(heap.mark_parity, parity);
        assert!(heap.owns_heap_value_for_test(v));
        assert!(
            unsafe { (*header).is_marked_at(parity) },
            "traced at the new parity before sweep",
        );
        heap.complete_collection();
        assert!(heap.owns_heap_value_for_test(v));
        if heap.generational_enabled() {
            assert!(heap.value_is_old_for_test(v));
            assert_eq!(
                unsafe { (*header).raw_mark() },
                UNMARKED_AT_REST,
                "ordinary old survivor rests only after sweep",
            );
        } else {
            assert!(unsafe { (*header).is_marked_at(parity) });
        }
    }
    // Fresh raw-pointer accesses only: no header borrow spans collection.
    unsafe { (*header).marked.store(UNMARKED_AT_REST, Ordering::Relaxed) };
    for parity in [heap.mark_parity.flip(), heap.mark_parity] {
        assert!(!unsafe { (*header).is_marked_at(parity) });
    }
    heap.collect_exact(std::iter::once(v));
    assert!(heap.owns_heap_value_for_test(v), "traced again from rest");
    assert!(heap.is_value_marked(v));
}
