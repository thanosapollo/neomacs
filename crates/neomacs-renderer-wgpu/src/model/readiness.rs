//! The readiness fence of one model (design §6.2), as a pure value.
//!
//! The render thread owns the whole predicate and reports its *value*, not
//! edges, so no transition can be lost:
//!
//! fence = showing set non-empty ∧ every member presented `rev` ∧ ¬interacting
//!
//! After every change to an input the caller runs [`Readiness::evaluate`],
//! which answers the `Readiness` event to emit, if any: one is emitted iff
//! the fence or `settled` differs from the last emission, or the fence holds
//! and `rev` differs. While the fence is down a rev bump alone emits nothing,
//! so an orbit costs one event at its start, not one per step.

use std::collections::BTreeMap;

/// A native window, as the render thread names it.
pub type WindowKey = u64;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Readiness {
    rev: u64,
    settled: u64,
    interacting: bool,
    /// Window -> the rev it last presented.
    showing: BTreeMap<WindowKey, u64>,
    /// (fence, rev, settled) last emitted; `None` before the first.
    last: Option<(bool, u64, u64)>,
}

impl Readiness {
    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn settled(&self) -> u64 {
        self.settled
    }

    pub fn interacting(&self) -> bool {
        self.interacting
    }

    /// A change to the pixels: an applied request, an orbit step, a wheel
    /// notch, an upload, a rescale or reset, a fallback highlight clear.
    pub fn bump(&mut self) -> u64 {
        self.rev += 1;
        self.rev
    }

    /// Lisp request `req` was processed (applied or rejected).
    pub fn settle(&mut self, req: u64) {
        self.settled = self.settled.max(req);
    }

    pub fn set_interacting(&mut self, interacting: bool) {
        self.interacting = interacting;
    }

    /// `window` presented a frame compositing this model at `rev`.
    pub fn presented(&mut self, window: WindowKey, rev: u64) {
        self.showing.insert(window, rev);
    }

    /// `window` no longer shows this model (a presented frame lacks it, or
    /// the window is hidden or gone).
    pub fn left(&mut self, window: WindowKey) {
        self.showing.remove(&window);
    }

    pub fn is_showing_in(&self, window: WindowKey) -> bool {
        self.showing.contains_key(&window)
    }

    pub fn fence(&self) -> bool {
        !self.showing.is_empty()
            && self.showing.values().all(|&rev| rev == self.rev)
            && !self.interacting
    }

    /// The `(current, rev, req)` to emit now, if the value changed.
    pub fn evaluate(&mut self) -> Option<(bool, u64, u64)> {
        let fence = self.fence();
        let now = (fence, self.rev, self.settled);
        let emit = match self.last {
            None => fence || self.settled != 0,
            Some((last_fence, last_rev, last_settled)) => {
                fence != last_fence || self.settled != last_settled || (fence && self.rev != last_rev)
            }
        };
        if emit {
            self.last = Some(now);
            Some(now)
        } else {
            None
        }
    }
}

#[cfg(test)]
#[path = "readiness/tests.rs"]
mod tests;
