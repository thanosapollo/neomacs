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
    SubrSpec::new(
        "neomacs-model--id",
        NativeFn::ContextVec(id),
        SubrArity::new(1, Some(1)),
    ),
    SubrSpec::new(
        "neomacs-model--set-view",
        NativeFn::ContextVec(set_view),
        SubrArity::new(5, Some(5)),
    ),
    SubrSpec::new(
        "neomacs-model--set-highlight",
        NativeFn::ContextVec(set_highlight),
        SubrArity::new(4, Some(4)),
    ),
    SubrSpec::new(
        "neomacs-model--resize",
        NativeFn::ContextVec(resize),
        SubrArity::new(4, Some(4)),
    ),
    SubrSpec::new(
        "neomacs-model--freeze",
        NativeFn::ContextVec(freeze),
        SubrArity::new(2, Some(2)),
    ),
}
