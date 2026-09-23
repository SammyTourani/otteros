#!/usr/bin/env python3
"""OtterOS QEMU runner: run/test/shot modes for the M0/M1 harness.

Usage:
    qemu.py --mode {run,test,shot} --firmware {uefi,bios} --iso PATH
            [--timeout N] [--expect-failure] [--expect-serial REGEX ...]
            [--send-keys SPEC]

Implements the protocol from DECISIONS.md D5 and briefs/M0-T1.md: serial
COM1 is teed to artifacts/serial.log, isa-debug-exit (port 0xf4) reports
pass/fail as the QEMU process exit code ((value << 1) | 1), and QMP
`screendump` produces a PNG of the framebuffer for `gmake shot`. `--mode
test` additionally fails if any `--expect-serial` regex (brief M1-T1) isn't
found in the captured serial log via `re.search`, even if the exit code
otherwise matched.

`--mode test --send-keys SPEC` (brief M1-T6) waits for `[kbd] ready` on
serial (printed by the `keyboard_e2e` test case once it's actually
consuming keyboard events), then sends SPEC via QMP `send-key`, 30ms
between every individual key press. SPEC is a comma-separated sequence of
tokens (kernel-review fix 6): a token naming a special key (`caps_lock`,
`enter`, `tab`, ... -- see `_SPECIAL_KEY_QCODE`) presses just that key;
any other token is literal text, typed one character at a time (qcode
names, holding shift for capitals/shifted symbols). `--send-keys hello`
is just the one-token case. Failing to see `[kbd] ready`, or any `OSError`
talking to QMP (a closed/timed-out socket included), within the overall
`--timeout` fails the run the same way a timed-out boot does.

Stdlib only (no pip installs) so it runs anywhere Python 3 does.
"""
import argparse
import json
import os
import re
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

    def send_key(self, qcodes, hold_time=None):
        """One QMP `send-key`: presses (and releases) every qcode in
        `qcodes` together, e.g. `["shift", "h"]` for a capital H."""
        keys = [{"type": "qcode", "data": qcode} for qcode in qcodes]
        kwargs = {"keys": keys}
        if hold_time is not None:
            kwargs["hold-time"] = hold_time
        return self._command("send-key", **kwargs)

    def close(self):
        self.sock.close()


# US QWERTY: character -> (qcode, needs_shift). Covers letters, digits and
# common punctuation -- brief M1-T6 step 6 only actually exercises the
# lowercase-letter rows (`--send-keys hello`), but "handle shift for
# capitals" is part of the design, so shifted symbols/uppercase letters are
# supported too rather than only the one string the acceptance test sends.
_SHIFTED_DIGIT_QCODE = {
    "!": "1", "@": "2", "#": "3", "$": "4", "%": "5",
    "^": "6", "&": "7", "*": "8", "(": "9", ")": "0",
}
_SHIFTED_PUNCT_QCODE = {
    "_": "minus", "+": "equal", "{": "bracket_left", "}": "bracket_right",
    ":": "semicolon", '"': "apostrophe", "~": "grave_accent", "|": "backslash",
    "<": "comma", ">": "dot", "?": "slash",
}
_PLAIN_PUNCT_QCODE = {
    "-": "minus", "=": "equal", "[": "bracket_left", "]": "bracket_right",
    ";": "semicolon", "'": "apostrophe", "`": "grave_accent", "\\": "backslash",
    ",": "comma", ".": "dot", "/": "slash", " ": "spc", "\n": "ret", "\t": "tab",
}


def char_to_qcodes(ch):
    """One character -> the list of qcodes a single QMP `send-key` should
    press together to type it (e.g. `["shift", "h"]` for `'H'`)."""
    if ch.isalpha() and ch.isascii():
        return (["shift", ch.lower()] if ch.isupper() else [ch.lower()])
    if ch.isdigit():
        return [ch]
    if ch in _SHIFTED_DIGIT_QCODE:
        return ["shift", _SHIFTED_DIGIT_QCODE[ch]]
    if ch in _SHIFTED_PUNCT_QCODE:
        return ["shift", _SHIFTED_PUNCT_QCODE[ch]]
    if ch in _PLAIN_PUNCT_QCODE:
        return [_PLAIN_PUNCT_QCODE[ch]]
    raise ValueError(f"qemu.py: no qcode mapping for {ch!r}")


# kernel-review, M1-T6 fix 6: named, non-printing keys `--send-keys`'s
# comma-separated sequence syntax can address by name (each pressed and
# released as its own qcode, not typed as literal text) -- e.g.
# `hello,caps_lock,h,caps_lock` toggles Caps Lock on, types `h` (decoded
# as `H` with the lock latched), then toggles it back off. This is how
# the `keyboard_e2e_caps_lock_led` kernel test exercises the LED path
# with IRQ1 already live. A comma can therefore no longer be typed as a
# literal character through `--send-keys` -- an acceptable trade, nothing
# this suite needs to type contains one.
_SPECIAL_KEY_QCODE = {
    "caps_lock": "caps_lock", "num_lock": "num_lock", "scroll_lock": "scroll_lock",
    "shift": "shift", "ctrl": "ctrl", "alt": "alt",
    "enter": "ret", "tab": "tab", "backspace": "backspace", "space": "spc", "esc": "esc",
}


def send_text(qmp, spec, delay=0.03):
    """Sends `spec` over `qmp` (brief M1-T6 step 6 / kernel-review fix 6),
    `delay` seconds between every individual key press. `spec` is a
    comma-separated sequence of tokens: a token naming a key in
    `_SPECIAL_KEY_QCODE` (case-insensitive) presses just that key (e.g.
    `caps_lock`); any other token is literal text, sent one character at
    a time (`char_to_qcodes`). A plain `--send-keys hello` (no commas) is
    just the one-token case, unchanged from before this syntax existed."""
    for token in spec.split(","):
        special = _SPECIAL_KEY_QCODE.get(token.lower())
        if special is not None:
            qmp.send_key([special])
            time.sleep(delay)
            continue
        for ch in token:
            qmp.send_key(char_to_qcodes(ch))
            time.sleep(delay)


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


def _dump_serial_log():
    with open(SERIAL_LOG, "r", errors="replace") as f:
        log = f.read()
    sys.stdout.write(log)
    sys.stdout.flush()
    return log


def cmd_test(iso, firmware, timeout, expect_failure, expect_serial, send_keys=None):
    os.makedirs(ARTIFACTS_DIR, exist_ok=True)
    os.makedirs(BUILD_DIR, exist_ok=True)
    reset_outputs(SERIAL_LOG, QMP_SOCK)
    open(SERIAL_LOG, "wb").close()

    args = build_args("test", firmware, iso)
    start = time.monotonic()
    proc = subprocess.Popen(args)

    def finish(result):
        # brief M2-T2b: printed on *every* return path (a timed-out
        # keyboard wait or QMP failure included, not just the ordinary
        # pass/fail one below) -- under host CPU contention this is the
        # first thing worth looking at: a suite that now takes much
        # longer than usual is the leading indicator of exactly the kind
        # of slowdown that made a fixed 90s budget too tight.
        print(f"[qemu.py] suite took {time.monotonic() - start:.1f}s", file=sys.stderr)
        return result

    if send_keys is not None:
        # brief M1-T6 step 6: never inject before the kernel says it's
        # actually reading the ring (`[kbd] ready`, printed by the
        # `keyboard_e2e` test case only once it's ready to consume
        # events) -- and never wait for that line forever either: a
        # kernel that never prints it (a regression, or a machine with no
        # i8042) must fail this run within the same overall `timeout`,
        # not hang it.
        remaining = max(timeout - (time.monotonic() - start), 1.0)
        if not wait_for_line(SERIAL_LOG, "[kbd] ready", remaining):
            print(f"[qemu.py] timed out after {remaining:.0f}s waiting for '[kbd] ready'", file=sys.stderr)
            terminate(proc)
            _dump_serial_log()
            return finish(1)

        # kernel-review, M1-T6 fix 5: one `try` around the whole
        # connect-greeting-send-close sequence, catching `OSError` (not
        # just `ConnectionError`): `socket.timeout`/`TimeoutError` -- e.g.
        # QEMU dying mid-handshake, or the socket read in `Qmp.__init__`'s
        # greeting/`qmp_capabilities` round trip or in `send_text`'s own
        # `send-key` commands blocking past the socket's 10s timeout --
        # is an `OSError` but *not* a `ConnectionError`, so it would
        # otherwise propagate out as an uncaught exception instead of the
        # clean `[qemu.py] ...` failure message/exit code every other
        # error path here produces.
        try:
            qmp = Qmp(QMP_SOCK)
            send_text(qmp, send_keys)
            qmp.close()
        except OSError as exc:
            print(f"[qemu.py] keyboard injection over QMP failed: {exc}", file=sys.stderr)
            terminate(proc)
            _dump_serial_log()
            return finish(1)

    try:
        remaining = max(timeout - (time.monotonic() - start), 1.0)
        code = proc.wait(timeout=remaining)
        timed_out = False
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        code = None
        timed_out = True
    elapsed = time.monotonic() - start

    log = _dump_serial_log()

    if timed_out:
        print(f"[qemu.py] TIMEOUT after {timeout:.0f}s", file=sys.stderr)
        return finish(1)

    print(f"[qemu.py] qemu exited with code {code} after {elapsed:.1f}s", file=sys.stderr)

    passed = code == EXIT_SUCCESS
    if expect_failure:
        passed = code == EXIT_FAILURE
    if not passed:
        print(f"[qemu.py] expected exit {'failure' if expect_failure else 'success'} "
              f"code, got {code}", file=sys.stderr)

    for pattern in expect_serial or []:
        if not re.search(pattern, log):
            print(f"[qemu.py] expected serial log to match {pattern!r}, but it didn't",
                  file=sys.stderr)
            passed = False

    return finish(0 if passed else 1)


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
    parser.add_argument(
        "--expect-serial", action="append", metavar="REGEX",
        help="require artifacts/serial.log to match REGEX (re.search); repeatable")
    parser.add_argument(
        "--send-keys", metavar="SPEC",
        help="brief M1-T6: wait for '[kbd] ready' on serial, then send SPEC over QMP send-key "
             "(comma-separated tokens; a special-key name like caps_lock, or literal text)")
    args = parser.parse_args()

    os.chdir(REPO_ROOT)
    iso = os.path.abspath(args.iso)

    if args.mode == "test":
        return cmd_test(iso, args.firmware, args.timeout, args.expect_failure, args.expect_serial, args.send_keys)
    if args.mode == "shot":
        return cmd_shot(iso, args.firmware, args.timeout)
    return cmd_run(iso, args.firmware)


if __name__ == "__main__":
    sys.exit(main())
