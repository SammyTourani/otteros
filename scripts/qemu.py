#!/usr/bin/env python3
"""OtterOS QEMU runner: run/test/shot modes for the M0 harness.

Usage:
    qemu.py --mode {run,test,shot} --firmware {uefi,bios} --iso PATH
            [--timeout N] [--expect-failure]

Implements the protocol from DECISIONS.md D5 and briefs/M0-T1.md: serial
COM1 is teed to artifacts/serial.log, isa-debug-exit (port 0xf4) reports
pass/fail as the QEMU process exit code ((value << 1) | 1), and QMP
`screendump` produces a PNG of the framebuffer for `gmake shot`.

Stdlib only (no pip installs) so it runs anywhere Python 3 does.
"""
import argparse
import json
import os
import socket
import subprocess
import sys
import time

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
QEMU_BIN = "/opt/homebrew/bin/qemu-system-x86_64"
OVMF_CODE = "/opt/homebrew/share/qemu/edk2-x86_64-code.fd"
OVMF_VARS_SRC = "/opt/homebrew/share/qemu/edk2-i386-vars.fd"

# isa-debug-exit makes the QEMU *process* exit with (value << 1) | 1.
EXIT_SUCCESS = 33  # value 0x10
EXIT_FAILURE = 35  # value 0x11

ARTIFACTS_DIR = os.path.join(REPO_ROOT, "artifacts")
BUILD_DIR = os.path.join(REPO_ROOT, "build")
SERIAL_LOG = os.path.join(ARTIFACTS_DIR, "serial.log")
QMP_SOCK = os.path.join(BUILD_DIR, "qmp.sock")
VARS_FD = os.path.join(BUILD_DIR, "edk2-i386-vars.fd")


def ensure_vars_fd():
    """OVMF's code volume is read-only and shared; its vars volume must be
    writable and per-VM, so we keep a scratch copy under build/."""
    if not os.path.exists(VARS_FD):
        os.makedirs(BUILD_DIR, exist_ok=True)
        with open(OVMF_VARS_SRC, "rb") as src, open(VARS_FD, "wb") as dst:
            dst.write(src.read())


def build_args(mode, firmware, iso):
    args = [
        QEMU_BIN,
        "-M", "q35",
        "-m", "512M",
        "-cpu", "qemu64",
        "-no-reboot",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
        "-cdrom", iso,
    ]

    if firmware == "uefi":
        ensure_vars_fd()
        args += [
            "-drive", f"if=pflash,unit=0,format=raw,readonly=on,file={OVMF_CODE}",
            "-drive", f"if=pflash,unit=1,format=raw,file={VARS_FD}",
        ]
    else:
        args += ["-boot", "d"]

    if mode == "run":
        args += ["-display", "cocoa", "-serial", "stdio"]
    else:
        args += [
            "-display", "none",
            "-serial", f"file:{SERIAL_LOG}",
            "-qmp", f"unix:{QMP_SOCK},server,nowait",
        ]
    return args


class Qmp:
    """Minimal QMP client over a unix socket (handshake + one command at a
    time). Stdlib-only stand-in for a real QMP library."""

    def __init__(self, path, connect_timeout=5.0):
        deadline = time.monotonic() + connect_timeout
        last_err = None
        self.sock = None
        while time.monotonic() < deadline:
            try:
                sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                sock.settimeout(10)
                sock.connect(path)
                self.sock = sock
                break
            except OSError as exc:
                last_err = exc
                time.sleep(0.1)
        if self.sock is None:
            raise ConnectionError(f"could not connect to QMP at {path}: {last_err}")
        self.buf = b""
        self._readline()  # greeting
        self._command("qmp_capabilities")

    def _readline(self):
        while b"\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise ConnectionError("QMP socket closed unexpectedly")
            self.buf += chunk
        line, _, self.buf = self.buf.partition(b"\n")
        return json.loads(line.decode())

    def _command(self, execute, **arguments):
        payload = {"execute": execute}
        if arguments:
            payload["arguments"] = arguments
        self.sock.sendall((json.dumps(payload) + "\n").encode())
        while True:
            msg = self._readline()
            if "return" in msg or "error" in msg:
                return msg
            # Anything else is an async event; keep waiting for our reply.

    def screendump(self, filename):
        return self._command("screendump", filename=filename, format="png")

    def close(self):
        self.sock.close()


def wait_for_line(path, needle, timeout):
    needle_bytes = needle.encode()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if os.path.exists(path):
            with open(path, "rb") as f:
                if needle_bytes in f.read():
                    return True
        time.sleep(0.1)
    return False


def terminate(proc, grace=5):
    if proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=grace)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def reset_outputs(*paths):
    for p in paths:
        if os.path.exists(p):
            os.remove(p)


def cmd_test(iso, firmware, timeout, expect_failure):
    os.makedirs(ARTIFACTS_DIR, exist_ok=True)
    os.makedirs(BUILD_DIR, exist_ok=True)
    reset_outputs(SERIAL_LOG, QMP_SOCK)
    open(SERIAL_LOG, "wb").close()

    args = build_args("test", firmware, iso)
    start = time.monotonic()
    proc = subprocess.Popen(args)
    try:
        code = proc.wait(timeout=timeout)
        timed_out = False
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        code = None
        timed_out = True
    elapsed = time.monotonic() - start

    with open(SERIAL_LOG, "r", errors="replace") as f:
        sys.stdout.write(f.read())
    sys.stdout.flush()

    if timed_out:
        print(f"[qemu.py] TIMEOUT after {timeout:.0f}s", file=sys.stderr)
        return 1

    print(f"[qemu.py] qemu exited with code {code} after {elapsed:.1f}s", file=sys.stderr)

    passed = code == EXIT_SUCCESS
    if expect_failure:
        passed = code == EXIT_FAILURE
    if not passed:
        print(f"[qemu.py] expected exit {'failure' if expect_failure else 'success'} "
              f"code, got {code}", file=sys.stderr)
    return 0 if passed else 1


def cmd_shot(iso, firmware, timeout):
    os.makedirs(ARTIFACTS_DIR, exist_ok=True)
    os.makedirs(BUILD_DIR, exist_ok=True)
    shot_png = os.path.join(ARTIFACTS_DIR, "shot.png")
    reset_outputs(SERIAL_LOG, QMP_SOCK, shot_png)
    open(SERIAL_LOG, "wb").close()

    args = build_args("shot", firmware, iso)
    proc = subprocess.Popen(args)
    try:
        if not wait_for_line(SERIAL_LOG, "[ok] fb banner", timeout):
            print(f"[qemu.py] timed out after {timeout:.0f}s waiting for "
                  "'[ok] fb banner'", file=sys.stderr)
            return 1

        try:
            qmp = Qmp(QMP_SOCK)
        except ConnectionError as exc:
            print(f"[qemu.py] {exc}", file=sys.stderr)
            return 1

        reply = qmp.screendump(shot_png)
        qmp.close()
        if "error" in reply:
            print(f"[qemu.py] screendump failed: {reply['error']}", file=sys.stderr)
            return 1

        deadline = time.monotonic() + 10
        while not os.path.exists(shot_png) and time.monotonic() < deadline:
            time.sleep(0.1)
        if not os.path.exists(shot_png):
            print("[qemu.py] screendump did not produce a file", file=sys.stderr)
            return 1

        print(f"[qemu.py] wrote {shot_png}", file=sys.stderr)
        return 0
    finally:
        terminate(proc)


def cmd_run(iso, firmware):
    args = build_args("run", firmware, iso)
    return subprocess.call(args)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", required=True, choices=["run", "test", "shot"])
    parser.add_argument("--firmware", default="uefi", choices=["uefi", "bios"])
    parser.add_argument("--iso", required=True)
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--expect-failure", action="store_true")
    args = parser.parse_args()

    os.chdir(REPO_ROOT)
    iso = os.path.abspath(args.iso)

    if args.mode == "test":
        return cmd_test(iso, args.firmware, args.timeout, args.expect_failure)
    if args.mode == "shot":
        return cmd_shot(iso, args.firmware, args.timeout)
    return cmd_run(iso, args.firmware)


if __name__ == "__main__":
    sys.exit(main())
