//! Shared libtest child-result guard for budget-isolated GPU tests.

use std::process::Output;

// Use libtest's normal capture and --color never in the child so test-body
// output cannot interleave with the passing test's identity line.
pub(crate) fn assert_child_success(output: &Output, exact: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut summaries = stdout
        .lines()
        .filter(|line| line.starts_with("test result: "));
    let counts = summaries
        .next()
        .and_then(|line| line.strip_prefix("test result: ok. "))
        .and_then(|summary| {
            let mut fields = summary.split("; ");
            [" passed", " failed", " ignored", " measured"]
                .into_iter()
                .map(|suffix| fields.next()?.strip_suffix(suffix)?.parse::<usize>().ok())
                .collect::<Option<Vec<_>>>()
        });
    let passed_identity = format!("test {exact} ... ok");
    assert!(
        output.status.success()
            && counts.as_deref() == Some(&[1, 0, 0, 0])
            && summaries.next().is_none()
            && stdout
                .lines()
                .filter(|line| *line == passed_identity)
                .count()
                == 1,
        "child must run exactly one passing test ({exact}); status={}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
}

#[cfg(test)]
#[path = "child_result/tests.rs"]
mod tests;
