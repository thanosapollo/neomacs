//! Native Lisp declarations for native model views.

use super::*;
use crate::emacs_core::subr::{NativeFn, SubrArity, SubrSpec};

crate::emacs_core::subr::define_subrs! {
    SubrSpec::new(
        "neomacs-model--available-p",
        NativeFn::ContextVec(available),
        SubrArity::new(0, Some(0)),
    ),
    SubrSpec::new(
        "neomacs-model--create",
        NativeFn::ContextVec(create),
        SubrArity::new(5, Some(5)),
    ),
}
