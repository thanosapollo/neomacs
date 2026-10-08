//! Minimal startup seam for the opt-in normal-library retention harness.
//! The legacy cfg(test) test_utils module is unchanged.

use crate::emacs_core::eval::Context;
use crate::emacs_core::load::create_runtime_startup_evaluator_cached;

pub fn runtime_startup_context() -> Context {
    create_runtime_startup_evaluator_cached().expect("bootstrap")
}
