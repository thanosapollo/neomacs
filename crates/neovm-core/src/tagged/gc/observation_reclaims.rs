//! Bounded history of cleared permanent observation words, never Lisp roots.
//!
//! A clear is recorded after its atomic metadata update and before the Release
//! reclamation epoch publication. Readers snapshot under the journal mutex,
//! then validate the epoch as before. An overwritten cursor requires the old
//! full metadata scan; a current cursor visits only words that were cleared.
use super::collection_observed::CollectionObservedWord;
use std::sync::Mutex;

const CAPACITY: usize = 8192;

#[derive(Clone, Copy)]
pub(crate) struct ObservationReclaim {
    pub(crate) metadata: CollectionObservedWord,
    pub(crate) mask: u64,
}

struct Reclaims {
    entries: [Option<ObservationReclaim>; CAPACITY],
    next: u64,
    exhausted: bool,
}

impl Reclaims {
    const fn new() -> Self {
        Self { entries: [None; CAPACITY], next: 0, exhausted: false }
    }

    fn push(&mut self, entry: ObservationReclaim) {
        let Some(next) = self.next.checked_add(1) else {
            self.exhausted = true;
            return;
        };
        self.entries[self.next as usize % CAPACITY] = Some(entry);
        self.next = next;
    }

    fn since(&self, cursor: u64, out: &mut Vec<ObservationReclaim>) -> (u64, bool) {
        out.clear();
        let complete = !self.exhausted && cursor <= self.next
            && self.next - cursor <= CAPACITY as u64;
        if complete {
            for index in cursor..self.next {
                out.push(self.entries[index as usize % CAPACITY].expect("published reclaim"));
            }
        }
        (self.next, complete)
    }
}

static RECLAIMS: Mutex<Reclaims> = Mutex::new(Reclaims::new());

pub(super) fn record(metadata: CollectionObservedWord, mask: u64) {
    if mask != 0 {
        RECLAIMS.lock().unwrap_or_else(|e| e.into_inner()).push(ObservationReclaim { metadata, mask });
    }
}

pub(crate) fn observation_reclaim_cursor() -> u64 {
    RECLAIMS.lock().unwrap_or_else(|e| e.into_inner()).next
}

pub(crate) fn observation_reclaims_since(cursor: u64, out: &mut Vec<ObservationReclaim>) -> (u64, bool) {
    RECLAIMS.lock().unwrap_or_else(|e| e.into_inner()).since(cursor, out)
}

#[cfg(test)]
#[path = "tests/observation_reclaims_test.rs"]
mod tests;
