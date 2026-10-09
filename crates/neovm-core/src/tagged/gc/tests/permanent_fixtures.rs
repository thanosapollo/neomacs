//! Explicit permanent-world fixtures; never a production lifetime policy.

use super::*;

impl TaggedHeap {
    /// Deliberately make this fixture's allocated survivors permanent.
    ///
    /// Call only after its full trace/sweep has completed, or when every
    /// handed-out allocation is intentionally part of the permanent fixture
    /// (the allocation-region closure test). Ordinary session-lifetime tests
    /// must not use this helper. The test owns the only mutator; assert that
    /// no worker, mark, deferred sweep, or old sweep cursor can race the walk.
    pub(super) fn make_survivors_permanent_for_test(&mut self) {
        #[cfg(debug_assertions)]
        crate::tagged::mutate::debug_assert_no_heap_mut_closure();
        assert!(!self.concurrent_mark_running());
        assert!(!self.mark_in_progress());
        assert!(!self.sweep_in_progress());
        assert!(!self.generational.major_in_progress);
        assert!(self.generational.old_sweep_pending.is_null());
        self.close_alloc_regions();
        self.join_old_boxes_for_partition_world_stopped();
        let mut tail: *mut GcHeader = std::ptr::null_mut();
        let mut obj = self.all_objects;
        while !obj.is_null() {
            unsafe {
                (*obj).make_permanent();
                // A weak hash table being tenured becomes permanent-black and the
                // main mark will never re-touch it; record it so the weak sweep
                // keeps re-evaluating its entries every GC (GNU sweeps every weak
                // table every GC). See `permanent_weak_hash_tables`.
                if (*obj).kind == HeapObjectKind::VecLike {
                    let vptr = obj as *mut VecLikeHeader;
                    if (*vptr).type_tag == VecLikeType::HashTable {
                        let ht_ptr = vptr as *mut HashTableObj;
                        if (*ht_ptr).table.weakness.is_some()
                            && !self.permanent_weak_hash_tables_set.contains(&ht_ptr)
                        {
                            self.permanent_weak_hash_tables_set.insert(ht_ptr);
                            self.permanent_weak_hash_tables.push(ht_ptr);
                        }
                    }
                }
                tail = obj;
                obj = (*obj).next;
            }
        }
        if !tail.is_null() {
            // Splice: [all_objects .. tail] -> front of tenured_objects.
            unsafe {
                (*tail).next = self.tenured_objects;
            }
            self.tenured_objects = self.all_objects;
            self.all_objects = std::ptr::null_mut();
        }
        // Arena objects are not on either boxed list. Full permanent pages
        // retire; partial pages stay available for later young allocations.
        self.promote_arena_pages_and_retire_full();
        self.scan_permanents_for_young_children();
        self.recompute_old_bytes_world_stopped();
    }
}
