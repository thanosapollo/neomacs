//! Affected #68 fixtures linked to the ordinary neovm-core library.
//! Do not run `cargo test --lib`: this target selects fixtures at compile time.

pub use neovm_core::{buffer, heap_types};
pub use neovm_core::emacs_core::{error, eval};

// Original fixture paths are retained, including their crate-relative imports.
mod emacs_core {
    pub use neovm_core::emacs_core::*;
    pub mod builtins {
        pub use neovm_core::emacs_core::builtins::*;
        pub use neovm_core::case68_test_support::{builtin_upcase_in_state, builtin_downcase_in_state, builtin_char_equal};
    }
    pub mod eval {
        pub use neovm_core::emacs_core::eval::*;
        pub use neovm_core::case68_test_support::set_builtin_frontend_for_test;
    }
}

mod test_utils {
    use std::cell::RefCell;
    use neovm_core::tagged::gc::{
        TaggedHeap, set_tagged_heap, tagged_heap_is_installed,
    };
    pub use neovm_core::test_utils::{
        runtime_startup_context, runtime_startup_eval_all, runtime_startup_eval_one,
    };

    thread_local! {
        // Pure fixtures allocate Values without a Context. Install an explicitly
        // owned fixture heap rather than enabling cfg(test)'s implicit fallback.
        // A Box keeps its address stable; it lives until this test thread exits.
        static FIXTURE_HEAP: RefCell<Option<Box<TaggedHeap>>> = const { RefCell::new(None) };
    }

    /// Bind the lazy Unicode fixture to the running test's workspace root,
    /// including nextest archive remaps, and escape its quoted Lisp token.
    pub fn r022_eval_with_support(src: &str) -> String {
        let directory = neovm_core::test_utils::workspace_root()
            .join("crates/neovm-core/src/emacs_core/text/casefiddle/tests/r019_unicode_support");
        let directory = directory.to_str().expect("Unicode fixture path is UTF-8")
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        runtime_startup_eval_one(&src.replace("__CASE68_SUPPORT_DIRECTORY__", &directory))
    }

    pub fn init_test_tracing() {
        neovm_core::test_utils::init_test_tracing();
        if !tagged_heap_is_installed() {
            FIXTURE_HEAP.with(|slot| {
                let mut heap = slot.borrow_mut();
                let heap = heap.get_or_insert_with(|| Box::new(TaggedHeap::new()));
                set_tagged_heap(heap);
            });
        }
    }
}

mod casefiddle {
    pub use neovm_core::case68_test_support::{
        builtin_capitalize, builtin_capitalize_word, builtin_char_resolve_modifiers,
        builtin_upcase_initials, builtin_upcase_region,
    };
    use neovm_core::Value;

    mod tests {
        include!("../src/emacs_core/text/casefiddle/tests/mod.rs");
    }
}

#[path = "../src/emacs_core/text/casefiddle/tests/r014_unibyte.rs"]
mod r014_unibyte_tests;

#[path = "../src/emacs_core/text/casefiddle/tests/r017_special_up.rs"]
mod r017_special_up_tests;

#[path = "../src/emacs_core/text/casefiddle/tests/r018_nil_up.rs"]
mod r018_nil_up_tests;

#[path = "../src/emacs_core/text/casefiddle/tests/r019_lazy_unicode.rs"]
mod r019_lazy_unicode_tests;

#[path = "../src/emacs_core/text/casefiddle/tests/r020_after_load_flow.rs"]
mod r020_after_load_flow_tests;

#[path = "case68_native/casetab.rs"]
mod casetab;
#[path = "case68_native/search.rs"]
mod search;
#[path = "case68_native/frontend.rs"]
mod frontend;

#[path = "case68_native/regex.rs"]
mod regex;
#[path = "case68_native/strings.rs"]
mod strings;
#[path = "case68_native/native_strings.rs"]
mod native_strings;

#[path = "case68_native/r022_text_downcase.rs"]
mod r022_text_downcase;

#[path = "case68_native/r022_review_musts.rs"]
mod r022_review_musts;

#[path = "case68_native/r022_callback_text.rs"]
mod r022_callback_text;

#[path = "case68_native/r022_callback_buffer.rs"]
mod r022_callback_buffer;

#[path = "case68_native/r022_raw_identity.rs"]
mod r022_raw_identity;

pub use neovm_core::case68_test_support::dispatch_builtin_without_eval_state;
