//! Native Lisp declarations for compositor-owned terminals.

use super::{
    create, cwd_pinning_p, destroy, get_text, resize, set_float, set_palette, spawn, write,
};
use crate::emacs_core::subr::{NativeFn, SubrArity, SubrSpec};

crate::emacs_core::subr::define_subrs! {
    SubrSpec::new(
        "neomacs-terminal-set-palette",
        NativeFn::ContextVec(set_palette),
        SubrArity::new(2, Some(2)),
    ),
    SubrSpec::new(
        "neomacs-terminal-cwd-pinning-p",
        NativeFn::ContextVec(cwd_pinning_p),
        SubrArity::new(1, Some(1)),
    ),
    SubrSpec::new(
        "neomacs-terminal-spawn",
        NativeFn::ContextVec(spawn),
        SubrArity::new(6, Some(6)),
    ),
    SubrSpec::new(
        "neomacs-terminal-create",
        NativeFn::ContextVec(create),
        SubrArity::new(3, Some(4)),
    ),
    SubrSpec::new(
        "neomacs-terminal-write",
        NativeFn::ContextVec(write),
        SubrArity::new(2, Some(2)),
    ),
    SubrSpec::new(
        "neomacs-terminal-resize",
        NativeFn::ContextVec(resize),
        SubrArity::new(3, Some(3)),
    ),
    SubrSpec::new(
        "neomacs-terminal-destroy",
        NativeFn::ContextVec(destroy),
        SubrArity::new(1, Some(1)),
    ),
    SubrSpec::new(
        "neomacs-terminal-set-float",
        NativeFn::ContextVec(set_float),
        SubrArity::new(4, Some(4)),
    ),
    SubrSpec::new(
        "neomacs-terminal-get-text",
        NativeFn::ContextVec(get_text),
        SubrArity::new(1, Some(1)),
    ),
}
