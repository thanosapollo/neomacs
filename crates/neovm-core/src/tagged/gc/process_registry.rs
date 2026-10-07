//! The heap's process objects, one per `ProcessId`.
//!
//! GNU marks a live process through `Vprocess_alist`. `delete-process`
//! (process.c `Fdelete_process`, `remove_process`) takes it off that list,
//! after which the process vectorlike lives only through references and the
//! vector sweep (alloc.c `sweep_vectors`) frees it once there are none. The
//! registry slots follow the killed-buffer ones (see [`RegistrySlot`]).

use super::*;
use crate::emacs_core::process::ProcessId;

impl TaggedHeap {
    /// The process object of `id`, if one exists. A deleted process's object
    /// is not a root, so reading it during a mark is a weak read, grayed
    /// exactly as in [`Self::buffer_value`].
    pub fn process_value(&mut self, id: ProcessId) -> Option<TaggedValue> {
        match self.process_registry.get(id as usize).copied()? {
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
            self.deleted_process_ids.push(id);
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
            self.deleted_process_ids.push(id);
        } else {
            self.pending_process_reclaims.push(id);
        }
    }

    /// Whether `id` names a deleted process that has no object (its record
    /// can go: no Lisp value can name the process).
    pub fn process_object_reclaimed(&self, id: ProcessId) -> bool {
        matches!(
            self.process_registry.get(id as usize),
            Some(RegistrySlot::Reclaimed)
        )
    }

    fn process_registry_slot_mut(&mut self, id: ProcessId) -> &mut RegistrySlot {
        let idx = id as usize;
        if self.process_registry.len() <= idx {
            self.process_registry.resize(idx + 1, RegistrySlot::Vacant);
        }
        &mut self.process_registry[idx]
    }

    /// Between mark termination and the sweep: forget every deleted process
    /// object the mark left unmarked and queue its id, so the evaluator
    /// drops the deleted record after the cycle, as
    /// [`Self::prune_unmarked_killed_buffers`] does for killed buffers.
    pub(super) fn prune_unmarked_deleted_processes(&mut self) {
        let mut ids = std::mem::take(&mut self.deleted_process_ids);
        ids.retain(|&id| {
            let idx = id as usize;
            let RegistrySlot::Killed(value) = self.process_registry[idx] else {
                return false; // a stale duplicate
            };
            if self.is_value_marked(value) {
                return true;
            }
            self.process_registry[idx] = RegistrySlot::Reclaimed;
            self.pending_process_reclaims.push(id);
            false
        });
        ids.sort_unstable();
        ids.dedup();
        self.deleted_process_ids = ids;
    }
}
