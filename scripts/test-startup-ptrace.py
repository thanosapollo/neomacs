#!/usr/bin/env python3
"""Real sibling ptrace regression; run ONLY inside a private PID sandbox.

Accepts a rustc-built tests/fixtures/startup_ptrace.rs executable. No editor,
root privilege or host sysctl change is used. Refuses non-Yama-1 environments.
"""
import ctypes
import errno
import os
from pathlib import Path
import select
import subprocess
import sys


def drop_capabilities():
    class Header(ctypes.Structure):
        _fields_ = [("version", ctypes.c_uint32), ("pid", ctypes.c_int)]
    class Data(ctypes.Structure):
        _fields_ = [("effective", ctypes.c_uint32), ("permitted", ctypes.c_uint32),
                    ("inheritable", ctypes.c_uint32)]
    libc = ctypes.CDLL(None, use_errno=True)
    assert libc.capset(ctypes.byref(Header(0x20080522, 0)), (Data * 2)()) == 0
    status = Path("/proc/self/status").read_text()
    for field in ("CapEff", "CapPrm", "CapInh"):
        assert int(status.split(field + ":")[1].splitlines()[0].strip(), 16) == 0


def trace(pid):
    libc = ctypes.CDLL(None, use_errno=True)
    libc.ptrace.restype = ctypes.c_long
    result = libc.ptrace(16, pid, None, None)  # PTRACE_ATTACH
    if result == -1:
        return ctypes.get_errno()
    waited, status = os.waitpid(pid, 0)
    assert waited == pid and os.WIFSTOPPED(status)
    assert libc.ptrace(17, pid, None, None) == 0  # PTRACE_DETACH
    return 0


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "trace":
        drop_capabilities()
        print(trace(int(sys.argv[2])))
        return
    # Sandbox wrapper mounts a fresh /proc; namespace init must be PID 1 and
    # there must not be a host-size process table. Fail closed, not a sandbox.
    assert os.getuid() != 0, "non-root user required"
    assert Path("/proc/1/comm").read_text().strip() != "systemd", "private PID sandbox required"
    assert len([p for p in Path("/proc").iterdir() if p.name.isdigit()]) < 64
    assert Path("/proc/sys/kernel/yama/ptrace_scope").read_text().strip() == "1"
    drop_capabilities()  # user namespace grants caps: remove the Yama bypass
    binary = str(Path(sys.argv[1]).resolve())
    for value, mode, expected in [
        (None, "normal", errno.EPERM), ("0", "normal", errno.EPERM),
        ("true", "normal", errno.EPERM), ("1 ", "normal", errno.EPERM),
        ("1", "normal", 0), ("1", "exec", 0),
        (None, "exec", errno.EPERM), ("1", "nondumpable", errno.EPERM),
    ]:
        env = dict(os.environ)
        env.pop("NEOMACS_ALLOW_PTRACE", None)
        if value is not None:
            env["NEOMACS_ALLOW_PTRACE"] = value
        with subprocess.Popen([binary, mode], env=env, stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as inferior:
            try:
                assert inferior.stdout is not None
                assert select.select([inferior.stdout], [], [], 5)[0], "inferior readiness timeout"
                pid = int(inferior.stdout.readline())
                assert pid == inferior.pid
                # Both children have this harness as parent: tracer is not an ancestor.
                result = subprocess.run([sys.executable, __file__, "trace", str(pid)],
                                        capture_output=True, text=True, timeout=5, check=True)
                actual = int(result.stdout.strip())
                assert actual == expected, (value, mode, actual, expected, result.stderr)
                print(f"PASS value={value!r} mode={mode} errno={actual}", flush=True)
            finally:
                inferior.communicate("done\n", timeout=5)
                assert inferior.returncode == 0, inferior.returncode


if __name__ == "__main__":
    main()
