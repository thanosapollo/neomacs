//! Narrow forwarding seam for the #68 native integration fixtures.
//!
//! This module is absent unless `case68-test-support` is explicitly enabled.
//! Conversion stays in the production modules; no implementation is copied.

use crate::emacs_core::error::EvalResult;
use crate::emacs_core::{Context, Value, casefiddle, casetab};

pub fn builtin_capitalize(args: Vec<Value>) -> EvalResult {
    casefiddle::builtin_capitalize(args)
}

pub fn builtin_upcase_initials(args: Vec<Value>) -> EvalResult {
    casefiddle::builtin_upcase_initials(args)
}

pub fn builtin_char_resolve_modifiers(args: Vec<Value>) -> EvalResult {
    casefiddle::builtin_char_resolve_modifiers(args)
}

pub fn builtin_upcase_region(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    casefiddle::builtin_upcase_region(ctx, args)
}

pub fn builtin_capitalize_word(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    casefiddle::builtin_capitalize_word(ctx, args)
}

pub fn make_case_table_with_pair(uc: i64, lc: i64) -> Value {
    casetab::make_case_table_with_pair(uc, lc)
}

pub fn builtin_set_case_table(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    casetab::builtin_set_case_table(ctx, args)
}

pub fn set_builtin_frontend_for_test(on: Option<bool>) {
    crate::emacs_core::eval::set_builtin_frontend_for_test(on);
}

/// Retain the existing fixtures' `.with` access to the actual core counter.
/// No duplicate counter, translated assertion, or production branch substitute.
pub struct FrontendFastCalls;

pub const FRONTEND_FAST_CALLS: FrontendFastCalls = FrontendFastCalls;

impl FrontendFastCalls {
    pub fn with<R>(&self, f: impl FnOnce(&std::cell::Cell<usize>) -> R) -> R {
        crate::emacs_core::builtins::search::FRONTEND_FAST_CALLS.with(f)
    }
}

pub fn builtin_upcase_in_state(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    crate::emacs_core::builtins::builtin_upcase_in_state(ctx, args)
}

pub fn builtin_downcase_in_state(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    crate::emacs_core::builtins::builtin_downcase_in_state(ctx, args)
}

pub fn builtin_char_equal(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    crate::emacs_core::builtins::builtin_char_equal(ctx, args)
}

pub fn builtin_string_match(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    crate::emacs_core::builtins::builtin_string_match(ctx, args)
}

pub fn builtin_replace_match(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    crate::emacs_core::builtins::builtin_replace_match(ctx, args)
}

pub fn builtin_re_search_forward(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    crate::emacs_core::builtins::builtin_re_search_forward(ctx, args)
}

pub fn builtin_concat(args: Vec<Value>) -> EvalResult {
    crate::emacs_core::builtins::builtin_concat(args)
}

pub fn builtin_format_wrapper_strict_slice(ctx: &mut Context, args: &[Value]) -> EvalResult {
    crate::emacs_core::builtins::builtin_format_wrapper_strict_slice(ctx, args)
}

pub fn dispatch_builtin_without_eval_state(name: &str, args: Vec<Value>) -> Option<EvalResult> {
    crate::emacs_core::builtins::dispatch_builtin_without_eval_state(name, args)
}

pub fn apply_match_case(replacement: &str, matched: &str) -> String {
    crate::emacs_core::regex::apply_match_case(replacement, matched)
}

pub fn builtin_match_beginning(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    crate::emacs_core::builtins::builtin_match_beginning(ctx, args)
}

pub fn builtin_match_end(ctx: &mut Context, args: Vec<Value>) -> EvalResult {
    crate::emacs_core::builtins::builtin_match_end(ctx, args)
}

pub fn builtin_format_message_slice(ctx: &mut Context, args: &[Value]) -> EvalResult {
    crate::emacs_core::builtins::builtin_format_message_slice(ctx, args)
}
