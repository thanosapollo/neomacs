//! Every trace of design §6.2, step by step.

use super::*;

const A: WindowKey = 1;
const B: WindowKey = 2;

/// req 7 at rev 12 shown in A and B, last emission R{t,12,7}.
fn settled_at_12() -> Readiness {
    let mut r = Readiness::default();
    while r.rev() < 12 {
        r.bump();
    }
    r.settle(7);
    r.presented(A, 12);
    r.presented(B, 12);
    assert_eq!(r.evaluate(), Some((true, 12, 7)));
    r
}

#[test]
fn orbit_released_after_presentation() {
    let mut r = settled_at_12();
    r.set_interacting(true);
    r.bump();
    assert_eq!(r.evaluate(), Some((false, 13, 7)));
    while r.rev() < 20 {
        r.bump();
        assert_eq!(r.evaluate(), None, "no emission per orbit step");
    }
    r.presented(A, 20);
    r.presented(B, 20);
    assert_eq!(r.evaluate(), None, "still interacting");
    r.set_interacting(false);
    assert_eq!(r.evaluate(), Some((true, 20, 7)));
}

#[test]
fn orbit_released_before_presentation() {
    let mut r = settled_at_12();
    r.set_interacting(true);
    r.bump();
    assert_eq!(r.evaluate(), Some((false, 13, 7)));
    while r.rev() < 20 {
        r.bump();
    }
    r.set_interacting(false);
    assert_eq!(r.evaluate(), None, "A and B behind");
    r.presented(A, 20);
    r.presented(B, 18);
    assert_eq!(r.evaluate(), None, "B behind");
    r.presented(B, 20);
    assert_eq!(r.evaluate(), Some((true, 20, 7)));
}

#[test]
fn wheel_settles_on_its_quiet_deadline() {
    let mut r = settled_at_12();
    r.set_interacting(true);
    r.bump();
    assert_eq!(r.evaluate(), Some((false, 13, 7)));
    r.presented(A, 13);
    r.presented(B, 13);
    assert_eq!(r.evaluate(), None, "interacting");
    r.set_interacting(false);
    assert_eq!(r.evaluate(), Some((true, 13, 7)));
}

#[test]
fn all_windows_hidden_then_shown_at_the_same_rev() {
    let mut r = settled_at_12();
    r.left(A);
    assert_eq!(r.evaluate(), None, "B still shows rev 12");
    r.left(B);
    assert_eq!(r.evaluate(), Some((false, 12, 7)));
    r.presented(A, 12);
    assert_eq!(r.evaluate(), Some((true, 12, 7)), "the same pair recurs");
}

#[test]
fn idle_rejected_resize_settles_without_presentation() {
    let mut r = settled_at_12();
    r.settle(8);
    assert_eq!(r.evaluate(), Some((true, 12, 8)));
}

#[test]
fn invalid_highlight_is_an_applied_fallback() {
    let mut r = settled_at_12();
    r.bump();
    r.settle(9);
    assert_eq!(r.evaluate(), Some((false, 13, 9)));
    r.presented(A, 13);
    assert_eq!(r.evaluate(), None);
    r.presented(B, 13);
    assert_eq!(r.evaluate(), Some((true, 13, 9)));
}

#[test]
fn rescale_or_reset_drops_then_restores() {
    let mut r = settled_at_12();
    r.bump();
    assert_eq!(r.evaluate(), Some((false, 13, 7)));
    r.presented(A, 13);
    r.presented(B, 13);
    assert_eq!(r.evaluate(), Some((true, 13, 7)));
}

#[test]
fn nothing_is_emitted_before_anything_happened() {
    let mut r = Readiness::default();
    assert_eq!(r.evaluate(), None);
    r.bump();
    assert_eq!(r.evaluate(), None, "a load's rev bump alone is not news");
    r.presented(A, 1);
    assert_eq!(r.evaluate(), Some((true, 1, 0)));
}
