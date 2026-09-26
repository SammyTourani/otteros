#!/usr/bin/env python3
"""Run a command under a resident-memory cap (macOS has no enforced `ulimit -v`).

The command runs in its own process group; every 0.25 s the summed memory footprint of that group
(cargo, rustc, test binaries...) is checked, and the whole group is killed with SIGKILL above the
cap. A runaway test once reached 61.7 GB on this 16 GB Mac and caused a kernel watchdog panic
(2026-09-24), so every test run by the loop or its agents goes through this guard, and
crates/.cargo/config.toml makes cargo launch every host test binary through it as well.

The footprint is `ri_phys_footprint` from proc_pid_rusage (what jetsam and Activity Monitor count),
not RSS: under memory pressure macOS compresses and swaps a leaking process's cold pages, so its RSS
stays small while its real footprint keeps climbing (a VM measured 286 MB RSS against 6.2 GB).

Usage: scripts/memguard.py [--cap-mb N] [--timeout S] -- command args...
Defaults: cap from OTTEROS_MEM_CAP_MB or 2048 MB; no timeout. Exit code: the command's, or 137
when the guard killed it (with a "[memguard]" line on stderr).
"""
import ctypes
import os
import signal
import subprocess
import sys
import time


_LIBC = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
_RUSAGE_INFO_V2 = 2


class _RUsageInfoV2(ctypes.Structure):
    # struct rusage_info_v2, <sys/resource.h>
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [(name, ctypes.c_uint64) for name in (
        "user_time", "system_time", "pkg_idle_wkups", "interrupt_wkups", "pageins",
        "wired_size", "resident_size", "phys_footprint", "proc_start_abstime", "proc_exit_abstime",
        "child_user_time", "child_system_time", "child_pkg_idle_wkups", "child_interrupt_wkups",
        "child_pageins", "child_elapsed_abstime", "diskio_bytesread", "diskio_byteswritten")]


def _group_members(pgid):
    """(pid, rss in KiB) of every process in the group."""
    out = subprocess.run(["ps", "-A", "-o", "pgid=,pid=,rss="], capture_output=True, text=True).stdout
    members = []
    for line in out.splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[0] == str(pgid):
            members.append((int(parts[1]), int(parts[2])))
    return members


def group_footprint_mb(pgid):
    """Summed phys_footprint of the group in MiB, and the summed RSS for comparison. A process
    that proc_pid_rusage cannot read (it just exited) counts with its RSS instead."""
    footprint = rss = 0
    for pid, rss_kb in _group_members(pgid):
        info = _RUsageInfoV2()
        rss += rss_kb * 1024
        if _LIBC.proc_pid_rusage(pid, _RUSAGE_INFO_V2, ctypes.byref(info)) == 0:
            footprint += info.phys_footprint
        else:
            footprint += rss_kb * 1024
    return footprint / 2**20, rss / 2**20


def main():
    args = sys.argv[1:]
    cap = int(os.environ.get("OTTEROS_MEM_CAP_MB", "2048"))
    timeout = None
    while args and args[0] != "--":
        if args[0] == "--cap-mb":
            cap = int(args[1])
            args = args[2:]
        elif args[0] == "--timeout":
            timeout = float(args[1])
            args = args[2:]
        else:
            sys.exit(f"memguard: unknown option {args[0]}")
    if not args or args[0] != "--" or len(args) < 2:
        sys.exit("usage: memguard.py [--cap-mb N] [--timeout S] -- command args...")
    proc = subprocess.Popen(args[1:], start_new_session=True)
    start = time.time()
    peak = 0.0
    while proc.poll() is None:
        footprint, rss = group_footprint_mb(proc.pid)
        peak = max(peak, footprint)
        reason = None
        if footprint > cap:
            reason = f"memory footprint {footprint:.0f} MB (RSS {rss:.0f} MB) exceeded the {cap} MB cap"
        elif timeout is not None and time.time() - start > timeout:
            reason = f"ran longer than {timeout:.0f} s"
        if reason:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            proc.wait()
            print(f"[memguard] killed {' '.join(args[1:])[:120]}: {reason}", file=sys.stderr)
            sys.exit(137)
        time.sleep(0.25)
    if os.environ.get("OTTEROS_MEMGUARD_VERBOSE"):
        print(f"[memguard] peak {peak:.0f} MB", file=sys.stderr)
    sys.exit(proc.returncode)


if __name__ == "__main__":
    main()
