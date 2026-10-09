//! Explicit Linux-only exception to Yama's ancestor-only ptrace policy.

use std::io;

/// Leave the process policy untouched unless the value is exactly `1`.
/// Call in the final editor process, after daemon forking and before workers.
pub(super) fn configure() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        configure_with(std::env::var_os("NEOMACS_ALLOW_PTRACE").as_deref(), || {
            // SAFETY: prctl takes scalar arguments; use unsigned-long width for
            // every variadic argument. This changes only this process's Yama
            // exception, not UID, dumpability, capabilities or the host sysctl.
            let result = unsafe {
                libc::prctl(
                    libc::PR_SET_PTRACER,
                    libc::PR_SET_PTRACER_ANY as libc::c_ulong,
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                )
            };
            if result == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        })?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn configure_with(
    value: Option<&std::ffi::OsStr>,
    enable: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    if value == Some(std::ffi::OsStr::new("1")) {
        enable()?;
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn only_exact_one_enables() {
        for value in [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new("0")),
            Some(OsStr::new("true")),
            Some(OsStr::new("01")),
            Some(OsStr::new("1 ")),
            Some(OsStr::from_bytes(b"\xff")),
        ] {
            configure_with(value, || panic!("disabled value called prctl")).unwrap();
        }
        let mut called = false;
        configure_with(Some(OsStr::new("1")), || {
            called = true;
            Ok(())
        })
        .unwrap();
        assert!(called);
    }

    #[test]
    fn opted_in_failure_is_not_silently_ignored() {
        let error = configure_with(Some(OsStr::new("1")), || {
            Err(io::Error::from_raw_os_error(libc::EPERM))
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    }
}
