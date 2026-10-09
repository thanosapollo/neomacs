//! The heap's process objects, one per `ProcessId`.
//!
//! GNU marks a live process through `Vprocess_alist`. `delete-process`
//! (process.c `Fdelete_process`, `remove_process`) takes it off that list,
//! after which the process vectorlike lives only through references and the
//! vector sweep (alloc.c `sweep_vectors`) frees it once there are none. The
//! registry slots follow the killed-buffer ones (see [`RegistrySlot`]).

use super::*;
use crate::emacs_core::process::ProcessId;

/// Keep the original inline process-map footprint while retaining the indexed
/// slots and placing incoming Rust-only ownership state outside the hot heap.
/// Moving extra fields to the end of a repr(Rust) heap does not pin its layout.
#[repr(C)]
#[derive(Default)]
pub(super) struct ProcessRegistry {
    pub(super) slots: Vec<RegistrySlot>,
    pub(super) cold: Box<HeapOwnershipState>,
}

/// One allocation per heap, independent of census and concurrent-claims knobs.
/// The worker is still exclusively this heap's; it spawns only on first send.
#[derive(Default)]
pub(super) struct HeapOwnershipState {
    pub(super) gc_worker: GcWorker,
    /// Killed/deleted ids whose objects may still be referenced. Pruning visits
    /// these ids, not every registry slot ever issued.
    pub(super) killed_buffer_ids: Vec<crate::buffer::BufferId>,
    pub(super) deleted_process_ids: Vec<ProcessId>,
    /// Reclaimed ids waiting for the evaluator's cycle-completed drain.
    /// Plain data, never marked.
    pub(super) pending_buffer_reclaims: Vec<crate::buffer::BufferId>,
    pub(super) pending_process_reclaims: Vec<ProcessId>,
}

// Couple the replacement carrier to the original map, without inventing a
// new heap size or accepting shifted JIT offsets. cold_gc.rs and jit_state.rs
// retain all independent production layout pins.
#[cfg(all(target_arch = "x86_64", target_pointer_width = "64"))]
const _: () = {
    use std::mem::align_of;
    type OriginalProcessRegistry = FxHashMap<ProcessId, TaggedValue>;
    assert!(size_of::<ProcessRegistry>() == size_of::<OriginalProcessRegistry>());
    assert!(align_of::<ProcessRegistry>() == align_of::<OriginalProcessRegistry>());
};

impl TaggedHeap {
    /// The process object of `id`, if one exists. A deleted process's object
    /// is not a root, so reading it during a mark is a weak read, grayed
    /// exactly as in [`Self::buffer_value`].
    pub fn process_value(&mut self, id: ProcessId) -> Option<TaggedValue> {
        match self.process_registry.slots.get(id as usize).copied()? {
            RegistrySlot::Live(value) => Some(value),
            RegistrySlot::Killed(value) => {
                self.shade_weak_read(value);
                Some(value)
            }
            RegistrySlot::Vacant | RegistrySlot::Reclaimed => None,
        }
    }

    /// Record `value` as the object of `id`; rooted unless the process was
    /// already deleted.
    pub fn register_process_value(&mut self, id: ProcessId, value: TaggedValue) {
        let slot = self.process_registry_slot_mut(id);
        let deleted = matches!(*slot, RegistrySlot::Killed(_) | RegistrySlot::Reclaimed);
        *slot = if deleted {
            RegistrySlot::Killed(value)
        } else {
            RegistrySlot::Live(value)
        };
        if deleted {
            self.process_registry.cold.deleted_process_ids.push(id);
        }
    }

    /// `delete-process` (and the `delete-exited-processes` reap): stop
    /// rooting the object of `id`. GNU `remove_process` (process.c) takes the
    /// process off `Vprocess_alist`, after which the vector sweep frees it
    /// once nothing refers to it. A process deleted before any object was
    /// made for it has no handle to wait for: its record goes after the
    /// next cycle.
    pub fn note_process_deleted(&mut self, id: ProcessId) {
        let slot = self.process_registry_slot_mut(id);
        *slot = match *slot {
            RegistrySlot::Live(value) | RegistrySlot::Killed(value) => RegistrySlot::Killed(value),
            RegistrySlot::Vacant | RegistrySlot::Reclaimed => RegistrySlot::Reclaimed,
        };
        if matches!(*slot, RegistrySlot::Killed(_)) {
            self.process_registry.cold.deleted_process_ids.push(id);
        } else {
            self.process_registry.cold.pending_process_reclaims.push(id);
        }
    }

    /// Whether `id` names a deleted process that has no object (its record
    /// can go: no Lisp value can name the process).
    pub fn process_object_reclaimed(&self, id: ProcessId) -> bool {
        matches!(
            self.process_registry.slots.get(id as usize),
            Some(RegistrySlot::Reclaimed)
        )
    }

    fn process_registry_slot_mut(&mut self, id: ProcessId) -> &mut RegistrySlot {
        let idx = id as usize;
        if self.process_registry.slots.len() <= idx {
            self.process_registry
                .slots
                .resize(idx + 1, RegistrySlot::Vacant);
        }
        &mut self.process_registry.slots[idx]
    }

    /// Between mark termination and the sweep: forget every deleted process
    /// object the mark left unmarked and queue its id, so the evaluator
    /// drops the deleted record after the cycle, as
    /// [`Self::prune_unmarked_killed_buffers`] does for killed buffers.
    pub(super) fn prune_unmarked_deleted_processes(&mut self) {
        let mut ids = std::mem::take(&mut self.process_registry.cold.deleted_process_ids);
        ids.retain(|&id| {
            let idx = id as usize;
            let RegistrySlot::Killed(value) = self.process_registry.slots[idx] else {
                return false; // a stale duplicate
            };
            if self.is_value_marked(value) {
                return true;
            }
            self.process_registry.slots[idx] = RegistrySlot::Reclaimed;
            self.process_registry.cold.pending_process_reclaims.push(id);
            false
        });
        ids.sort_unstable();
        ids.dedup();
        self.process_registry.cold.deleted_process_ids = ids;
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn indexed_registry_cold_ownership_preserves_original_inline_footprint() {
        use std::mem::align_of;
        type OriginalProcessRegistry = FxHashMap<ProcessId, TaggedValue>;
        assert_eq!(
            size_of::<ProcessRegistry>(),
            size_of::<OriginalProcessRegistry>()
        );
        assert_eq!(
            align_of::<ProcessRegistry>(),
            align_of::<OriginalProcessRegistry>()
        );

        let mut first = ProcessRegistry::default();
        let second = ProcessRegistry::default();
        let cold = &*first.cold as *const HeapOwnershipState;
        assert_ne!(cold, &*second.cold as *const HeapOwnershipState);
        first.slots.resize(1024, RegistrySlot::Vacant);
        first.cold.deleted_process_ids.push(7);
        first.cold.pending_process_reclaims.push(9);
        assert_eq!(cold, &*first.cold as *const HeapOwnershipState);
        assert!(second.slots.is_empty());
        assert!(second.cold.deleted_process_ids.is_empty());
        assert!(second.cold.pending_process_reclaims.is_empty());
    }
}
