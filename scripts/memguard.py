#!/usr/bin/env python3
"""Run a command under a resident-memory cap (macOS has no enforced `ulimit -v`).

The command runs in its own process group; every 0.25 s the summed RSS of that group (cargo, rustc,
test binaries...) is checked, and the whole group is killed with SIGKILL above the cap. A runaway
test once reached 61.7 GB on this 16 GB Mac and caused a kernel watchdog panic (2026-09-24), so
every test run by the loop or its agents goes through this guard.

Usage: scripts/memguard.py [--cap-mb N] [--timeout S] -- command args...
Defaults: cap from OTTEROS_MEM_CAP_MB or 3072 MB; no timeout. Exit code: the command's, or 137
when the guard killed it (with a "[memguard]" line on stderr).
"""
import os
import signal
import subprocess
import sys
import time


def group_rss_mb(pgid):
    out = subprocess.run(["ps", "-A", "-o", "pgid=,rss="], capture_output=True, text=True).stdout
    total_kb = 0
    for line in out.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[0] == str(pgid):
            total_kb += int(parts[1])
    return total_kb / 1024


def main():
    args = sys.argv[1:]
    cap = int(os.environ.get("OTTEROS_MEM_CAP_MB", "3072"))
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
        rss = group_rss_mb(proc.pid)
        peak = max(peak, rss)
        reason = None
        if rss > cap:
            reason = f"resident memory {rss:.0f} MB exceeded the {cap} MB cap"
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
