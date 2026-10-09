//! Buffer registry reads and the state a pdump load has to rebuild.
//!
//! The dump carries live buffers only, while its values can still name a
//! buffer that was killed before the dump. GNU dumps such a killed `struct
//! buffer` as an ordinary vectorlike that lives only through references.
//! Here the restored registry slot has to say so, or the object made for
//! that id would be a root for the rest of the session.

use super::*;
use crate::buffer::BufferId;

impl TaggedHeap {
    /// The object already made for buffer `id`, if any, for a root walk:
    /// a Rust holder that names a buffer by id (where GNU holds the buffer
    /// object) visits it so a killed buffer it names stays referenced.
    /// Makes nothing and grays nothing; the walk's visit marks it.
    pub fn buffer_object_for_trace(&self, id: BufferId) -> Option<TaggedValue> {
        match self.buffer_registry.get(id.0 as usize).copied()? {
            RegistrySlot::Live(value) | RegistrySlot::Killed(value) => Some(value),
            RegistrySlot::Vacant | RegistrySlot::Reclaimed => None,
        }
    }

    /// After a pdump load: every issued id below `next_id` that `is_live`
    /// rejects names a killed buffer. Demote an object the load already
    /// made for one to `Killed`, and mark a slot with no object `Reclaimed`
    /// so an object made for it later is `Killed` from the start. Such ids
    /// have no killed record to drop.
    pub fn note_restored_buffers(&mut self, next_id: u64, is_live: impl Fn(BufferId) -> bool) {
        for raw in 1..next_id {
            let id = BufferId(raw);
            if is_live(id) {
                continue;
            }
            let slot = self.buffer_registry_slot_mut(id);
            match *slot {
                RegistrySlot::Live(value) => {
                    *slot = RegistrySlot::Killed(value);
                    self.process_registry.cold.killed_buffer_ids.push(id);
                }
                RegistrySlot::Vacant => *slot = RegistrySlot::Reclaimed,
                RegistrySlot::Killed(_) | RegistrySlot::Reclaimed => {}
            }
        }
    }
}
