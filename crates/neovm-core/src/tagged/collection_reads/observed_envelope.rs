//! Exact local observation identities and the collector's conservative gate.
//!
//! This lives inside the existing mutator journal State, never in a Context
//! or a new TLS identity cache. Certificates and its ledger remain !Send;
//! only the executing mutator changes them. Shared permanent mark metadata
//! and the reclamation epoch are atomic. No old owner header is dereferenced.

use super::super::gc::{
    BarrierWindow, CompiledObservationGate, collection_observation_epoch,
    collection_observed_metadata,
};
use super::super::value::TAG_MASK;
use rustc_hash::FxHashSet;
use std::collections::BTreeSet;

const EMPTY_BOUNDS: (usize, usize) = (usize::MAX, 0);
const MAX_RECLAMATION_RETRIES: usize = 3;

pub(super) struct ObservedEnvelope {
    owners: FxHashSet<usize>,
    addresses: BTreeSet<usize>,
    epoch: Option<u64>,
    dump: BarrierWindow,
    full: (usize, usize),
    below: (usize, usize),
    above: (usize, usize),
    // Protocol bounds for a proven empty interval, never an owner hint or a
    // Lisp-state cache. Only this mutator's observed identities constrain it.
    excluded: Option<BarrierWindow>,
    // Reused buffer for the owners one reclamation pass removes.
    reclaimed: Vec<usize>,
}

impl ObservedEnvelope {
    pub(super) fn new() -> Self {
        Self {
            owners: FxHashSet::default(),
            addresses: BTreeSet::new(),
            epoch: Some(collection_observation_epoch()),
            dump: BarrierWindow::NONE,
            full: EMPTY_BOUNDS,
            below: EMPTY_BOUNDS,
            above: EMPTY_BOUNDS,
            excluded: None,
            reclaimed: Vec::new(),
        }
    }

    /// Refresh once per reclamation epoch or current dump-span change. An
    /// observer never initializes this ledger merely to reject an empty gate.
    pub(super) fn refresh(&mut self, dump: BarrierWindow) -> bool {
        let epoch = collection_observation_epoch();
        if self.epoch == Some(epoch) {
            if self.dump == dump {
                return false;
            }
            self.rebuild(dump);
            return true;
        }
        self.refresh_reclaimed(dump);
        true
    }

    /// The scanner cannot accept a completed concurrent clearing batch under
    /// its old epoch. Ordinary stop-the-world/cooperative batches are stable
    /// on their first pass. Continuous other-mutator reclamation is bounded:
    /// retain a conservative surviving superset and leave the epoch dirty so
    /// the next cold access retries. A false bit only removes a dead identity,
    /// never a live rooted dependency whose sticky mark survives collection.
    #[cold]
    #[inline(never)]
    fn refresh_reclaimed(&mut self, dump: BarrierWindow) {
        for _ in 0..MAX_RECLAMATION_RETRIES {
            let before = collection_observation_epoch();
            self.retain_owners(dump, collection_observed_metadata);
            let after = collection_observation_epoch();
            if before == after {
                self.epoch = Some(after);
                return;
            }
        }
        self.epoch = None;
    }

    /// Insert even when another mutator already set the process-wide mark.
    /// New observations update both buckets and the ordered address ledger;
    /// only reclamation or a dump-span change scans all local identities.
    /// A newly observed owner closes any gap covering it before the caller
    /// publishes its shared mark or snapshots the dependency's revision.
    pub(super) fn insert(&mut self, bits: usize) -> bool {
        if !self.owners.insert(bits) {
            return false;
        }
        let address = bits & !TAG_MASK;
        if self.excluded.is_some_and(|gap| gap.covers(address)) {
            self.excluded = None;
        }
        self.addresses.insert(address);
        self.include(address);
        true
    }

    /// A completed local publication remains valid throughout a stable
    /// reclamation epoch. The ledger is changed only under State's exclusive
    /// borrow, and its insertion/mark/gate publication contains no Lisp call
    /// or safepoint. Another read cannot see the insertion before completion.
    /// Shared marks clear only on death, followed by Release epoch advancement
    /// before reuse/resumption; this Acquire check rejects every such change.
    #[inline]
    pub(super) fn already_published(&self, bits: usize) -> bool {
        self.epoch == Some(collection_observation_epoch()) && self.owners.contains(&bits)
    }

    /// Compare with an epoch already acquired by the current pointer read.
    /// That read contains no callback or safepoint before its State borrow.
    pub(super) fn was_refreshed_at(&self, epoch: u64) -> bool {
        self.epoch == Some(epoch)
    }

    pub(super) fn full_bounds(&self) -> (usize, usize) {
        self.full
    }

    pub(super) fn ranges(&self) -> [BarrierWindow; 2] {
        [
            BarrierWindow::span(self.below.0, self.below.1),
            BarrierWindow::span(self.above.0, self.above.1),
        ]
    }

    pub(super) fn gate(&self) -> CompiledObservationGate {
        CompiledObservationGate {
            observed: self.ranges(),
            excluded: self.excluded,
        }
    }

    /// Exclude an interval containing no dependency owned by this mutator.
    /// Another mutator's sticky mark does not create a local certificate: the
    /// existing !Send journal contract requires shared writes separately.
    /// The ordinary dump is indivisible, even when none of it was observed.
    #[cold]
    #[inline(never)]
    pub(super) fn exclude_unobserved(&mut self, bits: usize) -> bool {
        let address = bits & !TAG_MASK;
        if self.owners.contains(&bits)
            || self.addresses.contains(&address)
            || self.dump.covers(address)
        {
            return false;
        }
        let mut lo = self
            .addresses
            .range(..address)
            .next_back()
            .map_or(0, |previous| previous + 1);
        let mut hi = self
            .addresses
            .range(address..)
            .next()
            .copied()
            .unwrap_or(usize::MAX);
        if self.dump.len() != 0 {
            if address < self.dump.lo() {
                hi = hi.min(self.dump.lo());
            } else {
                lo = lo.max(self.dump.lo() + self.dump.len());
            }
        }
        let gap = BarrierWindow::span(lo, hi);
        debug_assert!(gap.covers(address));
        self.excluded = Some(gap);
        true
    }

    /// Keep the owners `live` accepts; the result equals `rebuild` over the
    /// survivors. Every epoch advance follows a death of some observed owner,
    /// so this runs on most incremental sweep slices: with an unchanged dump
    /// span only the dead addresses leave the ordered ledger and the bounds are
    /// re-read from its ends, instead of re-sorting every survivor.
    fn retain_owners(&mut self, dump: BarrierWindow, live: impl Fn(usize) -> bool) {
        if self.dump != dump {
            self.owners.retain(|&bits| live(bits));
            self.rebuild(dump);
            return;
        }
        let reclaimed = &mut self.reclaimed;
        reclaimed.clear();
        self.owners.retain(|&bits| {
            let keep = live(bits);
            if !keep {
                reclaimed.push(bits);
            }
            keep
        });
        if self.reclaimed.is_empty() {
            return;
        }
        for &bits in &self.reclaimed {
            let address = bits & !TAG_MASK;
            // Owners are keyed by tagged bits, the ledger by address.
            if !(0..=TAG_MASK).any(|tag| self.owners.contains(&(address | tag))) {
                self.addresses.remove(&address);
            }
        }
        self.reclaimed.clear();
        self.recompute_bounds();
    }

    #[cold]
    #[inline(never)]
    fn rebuild(&mut self, dump: BarrierWindow) {
        // Removing dead identities cannot introduce an observed address into
        // an empty gap. A different dump span can, so only that change clears
        // the exclusion; collection alone keeps the proved gap available.
        if self.dump != dump {
            self.excluded = None;
        }
        self.dump = dump;
        // Bulk construction sorts once instead of inserting node by node.
        self.addresses = self.owners.iter().map(|&bits| bits & !TAG_MASK).collect();
        self.recompute_bounds();
    }

    /// `full`, `below` and `above` from the ordered ledger: every address,
    /// those below the dump span (all of them without a dump), and those
    /// above it. Addresses inside the dump span belong to neither side.
    fn recompute_bounds(&mut self) {
        let dump = self.dump;
        self.full = span_bounds(self.addresses.iter());
        if dump.len() == 0 {
            self.below = self.full;
            self.above = EMPTY_BOUNDS;
            return;
        }
        self.below = span_bounds(self.addresses.range(..dump.lo()));
        // A span reaching the top of the address space covers every address
        // from its start, leaving nothing above it.
        self.above = match dump.lo().checked_add(dump.len()) {
            Some(end) => span_bounds(self.addresses.range(end..)),
            None => EMPTY_BOUNDS,
        };
    }

    fn include(&mut self, address: usize) {
        include(&mut self.full, address);
        if self.dump.len() == 0 || address < self.dump.lo() {
            include(&mut self.below, address);
        } else if !self.dump.covers(address) {
            include(&mut self.above, address);
        }
    }
}

/// Bounds of an ascending address run, `EMPTY_BOUNDS` when it is empty.
fn span_bounds<'a>(mut addresses: impl DoubleEndedIterator<Item = &'a usize>) -> (usize, usize) {
    match addresses.next() {
        None => EMPTY_BOUNDS,
        Some(&first) => {
            let last = addresses.next_back().copied().unwrap_or(first);
            (first, last + 1)
        }
    }
}

#[inline]
fn include(bounds: &mut (usize, usize), address: usize) {
    bounds.0 = bounds.0.min(address);
    bounds.1 = bounds.1.max(address + 1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tagged::value::{TAG_CONS, TAG_STRING, TAG_VECLIKE};
    use proptest::prelude::*;

    /// The ledger and bounds a from-scratch scan of `owners` yields: the
    /// definition `retain_owners` must match after any reclamation.
    fn scanned(owners: &FxHashSet<usize>, dump: BarrierWindow) -> String {
        let mut addresses = BTreeSet::new();
        let (mut full, mut below, mut above) = (EMPTY_BOUNDS, EMPTY_BOUNDS, EMPTY_BOUNDS);
        for &bits in owners {
            let address = bits & !TAG_MASK;
            addresses.insert(address);
            include(&mut full, address);
            if dump.len() == 0 || address < dump.lo() {
                include(&mut below, address);
            } else if !dump.covers(address) {
                include(&mut above, address);
            }
        }
        format!("{addresses:?} {full:?} {below:?} {above:?}")
    }

    fn state(envelope: &ObservedEnvelope) -> String {
        format!(
            "{:?} {:?} {:?} {:?}",
            envelope.addresses, envelope.full, envelope.below, envelope.above
        )
    }

    fn dump_window(choice: u8) -> BarrierWindow {
        match choice % 5 {
            0 => BarrierWindow::NONE,
            1 => BarrierWindow::span(0x4000, 0x8000),
            2 => BarrierWindow::span(0x10, 0x20),
            3 => BarrierWindow::ALL,
            // Wraps past the top of the address space into [0, 0x2000).
            _ => BarrierWindow::from_lo_len(usize::MAX - 0xfff, 0x3000),
        }
    }

    /// Owner bits: low slots straddle the ordinary dump spans, high slots
    /// sit inside the wrapped span's top part, and every third owner reuses
    /// its predecessor's address under another tag.
    fn owner_bits(slot: usize, tag: u8, high: bool) -> usize {
        let tag = [TAG_CONS, TAG_STRING, TAG_VECLIKE][usize::from(tag)];
        let address = if high {
            (usize::MAX - 0xfff) + (slot % 0x100) * 16
        } else {
            slot << 4
        };
        address & !TAG_MASK | tag
    }

    proptest! {
        #[test]
        fn reclaiming_owners_matches_a_full_rescan(
            owners in prop::collection::vec(
                (1usize..0x900, 0u8..3, any::<bool>(), any::<bool>()),
                0..200,
            ),
            first in any::<u8>(),
            second in any::<u8>(),
        ) {
            let dump = dump_window(first);
            let mut envelope = ObservedEnvelope::new();
            envelope.rebuild(dump);
            let mut all = FxHashSet::default();
            let mut dead = FxHashSet::default();
            let mut previous: Option<usize> = None;
            for (index, &(slot, tag, high, dies)) in owners.iter().enumerate() {
                let mut bits = owner_bits(slot, tag, high);
                if index % 3 == 2 {
                    if let Some(alias) = previous {
                        // Same address, a different tag.
                        let tag = TAG_CONS + ((alias & TAG_MASK) - TAG_CONS + 1) % 3;
                        bits = alias & !TAG_MASK | tag;
                    }
                }
                previous = Some(bits);
                envelope.insert(bits);
                all.insert(bits);
                if dies {
                    dead.insert(bits);
                }
            }
            prop_assert_eq!(&envelope.owners, &all);
            prop_assert_eq!(state(&envelope), scanned(&all, dump));

            // Same dump span: the incremental path.
            envelope.retain_owners(dump, |bits| !dead.contains(&bits));
            let survivors: FxHashSet<usize> = all.difference(&dead).copied().collect();
            prop_assert_eq!(&envelope.owners, &survivors);
            prop_assert_eq!(state(&envelope), scanned(&survivors, dump));

            // A changed dump span takes the full rebuild.
            let moved = dump_window(second);
            let doomed: FxHashSet<usize> = survivors
                .iter()
                .copied()
                .filter(|bits| (bits >> 4) % 2 == usize::from(second % 2))
                .collect();
            envelope.retain_owners(moved, |bits| !doomed.contains(&bits));
            let rest: FxHashSet<usize> = survivors.difference(&doomed).copied().collect();
            prop_assert_eq!(&envelope.owners, &rest);
            prop_assert_eq!(state(&envelope), scanned(&rest, moved));
        }
    }
}
