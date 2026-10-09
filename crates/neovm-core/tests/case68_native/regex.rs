//! Exact original regex case controls.
use neovm_core::case68_test_support::apply_match_case;

#[test]
fn replace_match_case_capitalizes_each_word_like_gnu() {
    crate::test_utils::init_test_tracing();
    assert_eq!(apply_match_case("[alice:5]", "Alice"), "[Alice:5]");
    assert_eq!(
        apply_match_case("h_hello w_world", "Hello World"),
        "H_Hello W_World"
    );
}

#[test]
fn replace_match_case_upcases_all_caps_matches() {
    crate::test_utils::init_test_tracing();
    assert_eq!(apply_match_case("foo-bar", "FOO"), "FOO-BAR");
}
