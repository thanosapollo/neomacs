use super::*;
use crate::tagged::value::{TAG_CONS, TAG_STRING, TAG_VECLIKE};

fn reclaim(bits: usize) -> ObservationReclaim {
    let (metadata, mask) = collection_observed_word(bits).unwrap();
    ObservationReclaim { metadata, mask }
}

#[test]
fn refresh_work_is_independent_of_surviving_session_owners() {
    for survivors in [32, 1024, 32768] {
        let mut envelope = ObservedEnvelope::new();
        // One metadata word per owner makes the old all-word scan's growth
        // explicit, without wall-clock thresholds or a synthetic GC sleep.
        for slot in 0..survivors {
            envelope.insert((0x10000000 + slot * 512) | TAG_CONS);
        }
        let dead = 0x20000000 | TAG_CONS;
        envelope.insert(dead);
        envelope.reclaim_words.push(reclaim(dead));
        let mut loads = 0;
        envelope.retain_reclaimed_words(BarrierWindow::NONE, |_| {
            loads += 1;
            0
        });
        assert_eq!(loads, 1, "survivors={survivors}");
        assert_eq!(envelope.owners.len(), survivors);
        assert!(!envelope.owners.contains(&dead));
        assert_eq!(envelope.full, (0x10000000, 0x10000000 + (survivors - 1) * 512 + 1));
        let mut full_scan_loads = 0;
        envelope.retain_words(BarrierWindow::NONE, |_| {
            full_scan_loads += 1;
            u64::MAX
        });
        assert_eq!(full_scan_loads, survivors, "negative control must grow");
    }
}

#[test]
fn cleared_word_delta_matches_full_scan_with_reuse_aliases_and_dump_change() {
    for dump in [BarrierWindow::NONE, BarrierWindow::span(0x300100, 0x300900)] {
        let mut delta = ObservedEnvelope::new();
        let mut scan = ObservedEnvelope::new();
        for slot in 0..512 {
            for tag in [TAG_CONS, TAG_STRING, TAG_VECLIKE] {
                let bits = (0x300000 + slot * 8) | tag;
                delta.insert(bits);
                scan.insert(bits);
                if slot % 3 != 0 {
                    // Some cleared addresses have already been observed again:
                    // their current atomic bits remain set and must survive.
                    delta.reclaim_words.push(reclaim(bits));
                }
            }
        }
        let load = |metadata: CollectionObservedWord| {
            let mut mask = 0u64;
            for bit in 0..64 {
                let slot = (metadata.address_for_bit(bit) - 0x300000) / 8;
                if slot % 3 == 0 || slot % 5 == 0 { mask |= 1 << bit; }
            }
            mask
        };
        delta.retain_reclaimed_words(dump, load);
        scan.retain_words(dump, load);
        assert_eq!(delta.owners, scan.owners);
        assert_eq!(delta.addresses, scan.addresses);
        assert_eq!(delta.full, scan.full);
        assert_eq!(delta.below, scan.below);
        assert_eq!(delta.above, scan.above);
        assert_eq!(delta.words.len(), scan.words.len());
    }
}
