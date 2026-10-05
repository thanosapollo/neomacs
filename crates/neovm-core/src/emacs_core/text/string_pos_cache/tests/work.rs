//! Deterministic accounting at the actual positional traversal branches.
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Work {
    pub conversions: usize,
    pub cache_hits: usize,
    pub walked_bytes: usize,
}

thread_local! {
    static WORK: Cell<Work> = Cell::new(Work::default());
}

pub(crate) fn reset() {
    WORK.with(|work| work.set(Work::default()));
}

pub(crate) fn snapshot() -> Work {
    WORK.with(Cell::get)
}

pub(super) fn conversion() {
    WORK.with(|work| {
        let mut next = work.get();
        next.conversions += 1;
        work.set(next);
    });
}

pub(super) fn cache_hit() {
    WORK.with(|work| {
        let mut next = work.get();
        next.cache_hits += 1;
        work.set(next);
    });
}

pub(super) fn walk(bytes: usize) {
    WORK.with(|work| {
        let mut next = work.get();
        next.walked_bytes += bytes;
        work.set(next);
    });
}

#[test]
fn endpoints_preserve_the_interior_pair_without_walking() {
    use super::*;
    let mut ctx = crate::emacs_core::eval::Context::new();
    ctx.setup_thread_locals();
    reset_string_pos_cache();
    let source = Value::string("aαβz");
    let s = source.as_lisp_string().unwrap();
    assert_eq!(string_char_to_byte(source, s, 2), 3);
    let before = CACHE.with(Cell::get).unwrap();
    reset();
    assert_eq!(string_char_to_byte(source, s, 0), 0);
    assert_eq!(string_char_to_byte(source, s, s.schars()), s.sbytes());
    assert_eq!(string_byte_to_char(source, s, 0), 0);
    assert_eq!(string_byte_to_char(source, s, s.sbytes()), s.schars());
    let after = CACHE.with(Cell::get).unwrap();
    assert_eq!(
        (after.char_pos, after.byte_pos),
        (before.char_pos, before.byte_pos)
    );
    assert_eq!(snapshot().conversions, 4);
    assert_eq!(snapshot().walked_bytes, 0);
}
