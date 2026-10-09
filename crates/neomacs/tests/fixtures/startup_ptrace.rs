//! Standalone disposable inferior importing the exact production module.
#[path = "../../src/startup_ptrace.rs"]
mod startup_ptrace;

fn main() {
    use std::io::{self, Write};
    use std::os::unix::process::CommandExt;
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode != "wait" {
        startup_ptrace::configure().unwrap_or_else(|error| {
            eprintln!("NEOMACS_ALLOW_PTRACE=1: cannot allow ptrace: {error}");
            std::process::exit(1);
        });
    }
    if mode == "exec" {
        // The replacement must NOT reapply the opt-in: prove kernel retention.
        let error = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("wait")
            .env_remove("NEOMACS_ALLOW_PTRACE")
            .exec();
        panic!("exec failed: {error}");
    }
    if mode == "nondumpable" {
        assert_eq!(
            unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong) },
            0
        );
    }
    println!("{}", std::process::id());
    io::stdout().flush().unwrap();
    let mut line = String::new();
    io::stdin().read_line(&mut line).unwrap();
}
