use super::*;
use crate::tagged::gc::collection_observed_word;
use crate::tagged::value::TAG_CONS;

fn entry(slot: usize) -> ObservationReclaim {
    let (metadata, mask) = collection_observed_word((0x510000 + slot * 512) | TAG_CONS).unwrap();
    ObservationReclaim { metadata, mask }
}

#[test]
fn bounded_history_detects_overwrite_and_recovers_at_current_cursor() {
    let mut journal = Reclaims::new();
    let mut out = Vec::new();
    for slot in 0..CAPACITY {
        journal.push(entry(slot));
    }
    assert_eq!(journal.since(0, &mut out), (CAPACITY as u64, true));
    assert_eq!(out.len(), CAPACITY);
    journal.push(entry(CAPACITY));
    assert_eq!(journal.since(0, &mut out), (CAPACITY as u64 + 1, false));
    assert!(out.is_empty());
    assert_eq!(journal.since(CAPACITY as u64, &mut out), (CAPACITY as u64 + 1, true));
    assert_eq!(out.len(), 1);
    assert!(std::ptr::eq(out[0].metadata.word, entry(CAPACITY).metadata.word));
    assert_eq!(journal.since(journal.next, &mut out), (journal.next, true));
    assert!(out.is_empty());
}

#[test]
fn exhausted_history_never_accepts_a_cursor() {
    let mut journal = Reclaims::new();
    journal.next = u64::MAX;
    journal.push(entry(0));
    assert_eq!(journal.since(u64::MAX, &mut Vec::new()), (u64::MAX, false));
}
