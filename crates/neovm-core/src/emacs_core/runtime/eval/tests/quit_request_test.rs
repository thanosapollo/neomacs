//! `QuitRequest`: the cross-thread C-g request the input bridges raise and
//! the evaluator drains (see `attention.rs`).

use super::QuitRequest;
use crate::emacs_core::eval::Context;
use crate::emacs_core::value::Value;

#[test]
fn quit_request_raises_takes_and_clears() {
    let request = QuitRequest::new();
    assert!(!request.is_requested(), "a new request is lowered");
    assert!(!request.take(), "taking a lowered request reports nothing");

    request.request();
    request.request();
    assert!(request.is_requested(), "a raise is visible to the poll");
    assert!(request.take(), "the first take consumes the raise");
    assert!(!request.is_requested());
    assert!(!request.take(), "a raise is consumed exactly once");

    request.request();
    request.clear();
    assert!(!request.is_requested(), "clear lowers a raised request");
}

#[test]
fn quit_request_clones_share_one_flag() {
    let request = QuitRequest::new();
    let bridge = request.clone();
    bridge.request();
    assert!(
        request.is_requested(),
        "the evaluator sees the bridge's raise"
    );
    assert!(request.take());
    assert!(
        !bridge.is_requested(),
        "the evaluator's take lowers the bridge's copy too"
    );
}

/// The process word counts raised requests, not raises: a second raise of a
/// raised request adds nothing, a take removes exactly what the raise added,
/// and a request dropped while raised gives its unit back.
#[test]
fn quit_request_counts_raised_cells_exactly() {
    use crate::emacs_core::eval::ASYNC_ATTENTION;
    let count = || ASYNC_ATTENTION.raised_quit_requests_for_test();
    let base = count();
    let a = QuitRequest::new();
    let b = QuitRequest::new();
    a.request();
    a.request();
    assert_eq!(count(), base + 1, "raising a raised request adds nothing");
    b.request();
    assert_eq!(count(), base + 2, "each raised cell counts once");
    assert!(a.take());
    assert_eq!(count(), base + 1);
    assert!(!a.take(), "a second take finds nothing");
    assert_eq!(count(), base + 1, "and removes nothing");
    let b_clone = b.clone();
    drop(b);
    assert_eq!(count(), base + 1, "a live clone keeps the raise");
    drop(b_clone);
    assert_eq!(
        count(),
        base,
        "the last handle of a raised request releases its unit"
    );
    let c = QuitRequest::new();
    c.request();
    c.clear();
    drop(c);
    assert_eq!(count(), base, "a cleared request owes nothing at drop");
}

/// A raised request is what the safe point's fast test sees: the word goes
/// nonzero, `maybe_quit_hot_ok` refuses, and the drain lowers both.
#[test]
fn a_raised_quit_request_sends_the_safe_point_to_its_slow_path() {
    crate::test_utils::init_test_tracing();
    let mut ctx = Context::new();
    assert!(ctx.maybe_quit_hot_ok(), "nothing pending at start");
    ctx.quit_requested.request();
    assert_ne!(crate::emacs_core::eval::ASYNC_ATTENTION.load(), 0);
    assert!(
        !ctx.maybe_quit_hot_ok(),
        "a raised request is due at the next safe point"
    );
    let err = ctx.maybe_quit().expect_err("the drain signals quit");
    assert!(format!("{err:?}").contains("quit"), "{err:?}");
    assert!(!ctx.quit_requested.is_requested());
    ctx.set_quit_flag_value(Value::NIL);
    assert_eq!(crate::emacs_core::eval::ASYNC_ATTENTION.load(), 0);
    assert!(ctx.maybe_quit_hot_ok());
}

/// A raise from another thread -- the input bridge's shape -- reaches the
/// evaluator's next safe point as a `quit` signal, and the request is
/// drained so the following poll does not fire again.
#[test]
fn quit_request_from_another_thread_interrupts_an_evaluator_loop() {
    crate::test_utils::init_test_tracing();
    let mut ctx = Context::new();
    let bridge = ctx.quit_requested.clone();
    let raiser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        bridge.request();
    });
    let result = ctx.eval_str("(let ((i 0)) (while t (setq i (1+ i))))");
    raiser.join().expect("raiser thread");
    match result {
        Err(e) => {
            let msg = format!("{e}");
            assert!(msg.contains("quit"), "expected quit, got: {msg}");
        }
        Ok(v) => panic!("expected a quit signal, got {v:?}"),
    }
    assert!(
        !ctx.quit_requested.is_requested(),
        "the safe point drained the request"
    );
    assert!(ctx.maybe_quit_hot_ok() || !ctx.quit_flag_value().is_nil());
    ctx.set_quit_flag_value(Value::NIL);
    assert!(
        ctx.maybe_quit_hot_ok(),
        "nothing is pending after the drain"
    );
}

// ---------------------------------------------------------------------------
// GNU force quit (keyboard.c `handle_interrupt'): in a graphical session the
// third C-g that arrives while `quit-flag' is still set clears `inhibit-quit',
// so a loop running with quitting inhibited can still be stopped.
// ---------------------------------------------------------------------------

fn inhibited_context() -> Context {
    crate::test_utils::init_test_tracing();
    let mut ctx = Context::new();
    ctx.assign("inhibit-quit", Value::T);
    ctx
}

fn is_quit(result: &Result<(), crate::emacs_core::error::Flow>) -> bool {
    matches!(result, Err(flow) if format!("{flow:?}").contains("quit"))
}

#[test]
fn third_c_g_while_a_quit_is_pending_forces_the_quit() {
    let mut ctx = inhibited_context();
    for press in 1..=2 {
        ctx.quit_requested.request();
        assert!(ctx.maybe_quit().is_ok(), "press {press} stays inhibited");
        assert!(
            ctx.quit_flag_value().is_truthy(),
            "press {press} leaves the quit pending"
        );
        assert!(ctx.eval_str("inhibit-quit").unwrap().is_truthy());
    }
    ctx.quit_requested.request();
    let third = ctx.maybe_quit();
    assert!(is_quit(&third), "the third press quits: {third:?}");
    assert!(ctx.eval_str("inhibit-quit").unwrap().is_nil());
}

#[test]
fn presses_that_arrive_between_two_safe_points_each_count() {
    let mut ctx = inhibited_context();
    ctx.quit_requested.request();
    ctx.quit_requested.request();
    assert!(ctx.maybe_quit().is_ok(), "two presses are not a force quit");
    ctx.quit_requested.request();
    assert!(is_quit(&ctx.maybe_quit()));
}

#[test]
fn a_press_with_no_pending_quit_restarts_the_count() {
    let mut ctx = inhibited_context();
    for _ in 0..2 {
        ctx.quit_requested.request();
        assert!(ctx.maybe_quit().is_ok());
    }
    // The pending quit is consumed (as a key, say): GNU counts from one again.
    ctx.set_quit_flag_value(Value::NIL);
    for _ in 0..2 {
        ctx.quit_requested.request();
        assert!(ctx.maybe_quit().is_ok(), "the count restarted");
    }
    assert!(ctx.eval_str("inhibit-quit").unwrap().is_truthy());
}

/// The reported wedge: a loop run with `inhibit-quit' bound to t, stopped
/// from the input bridge's thread by three C-g presses.
#[test]
fn three_c_g_from_the_input_bridge_stop_an_inhibited_loop() {
    crate::test_utils::init_test_tracing();
    let mut ctx = Context::new();
    let bridge = ctx.quit_requested.clone();
    let raiser = std::thread::spawn(move || {
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            bridge.request();
        }
    });
    let result =
        ctx.eval_str("(condition-case nil (let ((inhibit-quit t)) (while t)) (quit 'escaped))");
    raiser.join().expect("raiser thread");
    assert_eq!(result.unwrap(), Value::symbol("escaped"));
}

/// Lowering a request and dropping its press count are one step: a press
/// that lands while `clear` runs either is cleared with it or survives with
/// its count, so a raised request always has a press to report.
#[test]
fn a_press_racing_clear_is_never_raised_without_its_count() {
    use std::sync::{Arc, Barrier};
    const ROUNDS: usize = 100_000;
    let request = QuitRequest::new();
    let bridge = request.clone();
    let start = Arc::new(Barrier::new(2));
    let done = Arc::new(Barrier::new(2));
    let (bridge_start, bridge_done) = (Arc::clone(&start), Arc::clone(&done));
    let raiser = std::thread::spawn(move || {
        for _ in 0..ROUNDS {
            bridge_start.wait();
            bridge.request();
            bridge_done.wait();
            bridge_done.wait();
        }
    });
    let mut lost = 0;
    for _ in 0..ROUNDS {
        request.request();
        start.wait();
        request.clear();
        done.wait();
        let raised = request.is_requested();
        let presses = request.take_presses();
        if raised && presses == 0 {
            lost += 1;
        }
        request.clear();
        done.wait();
    }
    raiser.join().expect("raiser thread");
    assert_eq!(lost, 0, "rounds where a raised request reported no press");
}

/// GNU's `handle_interrupt` sets `quit-flag` to t on every C-g, replacing
/// a `throw-on-input` tag: a real C-g quits rather than throwing to
/// `while-no-input`'s catch.
#[test]
fn a_c_g_replaces_a_throw_on_input_tag_with_t() {
    let mut ctx = inhibited_context();
    ctx.assign("throw-on-input", Value::symbol("tag"));
    ctx.set_quit_flag_value(Value::symbol("tag"));
    ctx.quit_requested.request();
    assert!(ctx.maybe_quit().is_ok(), "one press stays inhibited");
    assert_eq!(ctx.quit_flag_value(), Value::T);
    for _ in 0..2 {
        ctx.quit_requested.request();
    }
    let third = ctx.maybe_quit();
    assert!(
        is_quit(&third),
        "the third press quits, not throws: {third:?}"
    );
}

/// A C-g read as an event (`read-event' under `inhibit-quit') is counted
/// when it arrives, against the flag it found, and does not stay a pending
/// quit (GNU read_char): it neither completes an earlier force-quit count
/// nor quits later.
#[test]
fn a_c_g_read_as_an_event_does_not_complete_an_old_force_quit() {
    let mut ctx = inhibited_context();
    for _ in 0..2 {
        ctx.quit_requested.request();
        assert!(ctx.maybe_quit().is_ok());
    }
    // Lisp handles the pending quit, then reads a fresh C-g as an event.
    ctx.set_quit_flag_value(Value::NIL);
    ctx.quit_requested.request();
    ctx.request_quit_from_keyboard_input();
    assert!(
        ctx.quit_flag_value().is_nil(),
        "the event is not also a quit"
    );
    assert!(ctx.maybe_quit().is_ok(), "one fresh press is no force quit");
    assert!(ctx.eval_str("inhibit-quit").unwrap().is_truthy());
    // Two more presses while it waits are presses one and two again.
    ctx.quit_requested.request();
    ctx.quit_requested.request();
    assert!(ctx.maybe_quit().is_ok());
    assert!(ctx.eval_str("inhibit-quit").unwrap().is_truthy());
}
