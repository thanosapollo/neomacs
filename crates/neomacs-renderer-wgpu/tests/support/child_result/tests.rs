use super::assert_child_success;
use std::process::{Command, Output};

// module_path! keeps this CPU fixture valid in both importing GPU harnesses.
fn fixture_name() -> String {
    let (_, module) = module_path!().split_once("::").unwrap();
    format!("{module}::child_fixture")
}

fn child(exact: &str) -> Output {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", exact, "--color", "never"])
        .env_remove("RUST_TEST_NOCAPTURE")
        .env_remove("NEOMACS_CHILD_RESULT_FAIL")
        .output()
        .unwrap()
}

fn rejected(output: &Output, exact: &str) {
    assert!(
        std::panic::catch_unwind(|| assert_child_success(output, exact)).is_err(),
        "child result was accepted without exactly one passing intended test"
    );
}

#[test]
fn child_fixture() {
    println!("CPU fixture output must not obscure the libtest identity line");
    assert!(std::env::var_os("NEOMACS_CHILD_RESULT_FAIL").is_none());
}

#[test]
fn accepts_one_real_passing_child() {
    let exact = fixture_name();
    assert_child_success(&child(&exact), &exact);
}

#[test]
fn rejects_zero_test_success_from_wrong_exact_filter() {
    let exact = format!("{}::does_not_exist", module_path!());
    let output = child(&exact);
    assert!(output.status.success(), "libtest zero selection control");
    assert!(String::from_utf8_lossy(&output.stdout).contains("0 passed"));
    rejected(&output, &exact);
}

#[test]
fn rejects_failed_child() {
    let exact = fixture_name();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &exact, "--color", "never"])
        .env_remove("RUST_TEST_NOCAPTURE")
        .env("NEOMACS_CHILD_RESULT_FAIL", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    rejected(&output, &exact);
}

#[test]
fn rejects_non_single_or_ignored_summary_counts() {
    let exact = fixture_name();
    let mut output = child(&exact);
    // Deliberate output fixtures use a real successful process status. In
    // particular, "11 passed" must not satisfy a substring "1 passed" check.
    for counts in [
        "0 passed; 0 failed; 0 ignored; 0 measured",
        "11 passed; 0 failed; 0 ignored; 0 measured",
        "21 passed; 0 failed; 0 ignored; 0 measured",
        "1 passed; 1 failed; 0 ignored; 0 measured",
        "0 passed; 0 failed; 1 ignored; 0 measured",
        "1 passed; 0 failed; 1 ignored; 0 measured",
        "1 passed; 0 failed; 0 ignored; 1 measured",
    ] {
        output.stdout = format!(
            "test {exact} ... ok\ntest result: ok. {counts}; 0 filtered out; finished in 0.00s\n"
        )
        .into_bytes();
        rejected(&output, &exact);
    }
}

#[test]
fn rejects_missing_malformed_or_duplicate_summary() {
    let exact = fixture_name();
    let mut output = child(&exact);
    let valid = String::from_utf8(output.stdout.clone()).unwrap();
    for text in [
        String::new(),
        format!("test {exact} ... ok\n"),
        format!("{valid}{valid}"),
        valid.replace("1 passed", "invalid passed"),
        valid.replace("test result: ok.", "test result: FAILED."),
    ] {
        output.stdout = text.into_bytes();
        rejected(&output, &exact);
    }
}

#[test]
fn rejects_wrong_or_missing_passed_identity() {
    let exact = fixture_name();
    let mut output = child(&exact);
    let valid = String::from_utf8(output.stdout.clone()).unwrap();
    for text in [
        valid.replace(&exact, "another_test"),
        valid.replace(&format!("test {exact} ... ok"), ""),
    ] {
        output.stdout = text.into_bytes();
        rejected(&output, &exact);
    }
}
