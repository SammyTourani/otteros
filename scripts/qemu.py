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
import fcntl
import json
import os
import re
import socket
import struct
import subprocess
import sys
import threading
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


def build_args(mode, firmware, iso, cpu="max"):
    args = [
        QEMU_BIN,
        "-M", "q35",
        "-m", "512M",
        "-cpu", cpu,
        "-no-reboot",
        "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
        "-cdrom", iso,
    ]

    # Attach the test disk image (brief M3-T1 §6).
    data_img = os.path.join(BUILD_DIR, "data.img")
    if os.path.exists(data_img):
        args += [
            "-drive", f"file={data_img},if=none,id=data,format=raw",
            "-device", "virtio-blk-pci,drive=data,disable-legacy=on",
        ]

    # Attach virtio-net device for network testing (brief M5-T1a).
    args += [
        "-netdev", "user,id=n0",
        "-device", "virtio-net-pci,netdev=n0,disable-legacy=on,mac=52:54:00:4f:54:52",
    ]
    # Debugging aid: OTTEROS_NET_PCAP=path records every frame on the user-mode network.
    if os.environ.get("OTTEROS_NET_PCAP"):
        args += ["-object", f"filter-dump,id=netdump,netdev=n0,file={os.environ['OTTEROS_NET_PCAP']}"]

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
    "enter": "ret", "return": "ret", "tab": "tab", "backspace": "backspace", "space": "spc", "esc": "esc",
    "up": "up", "down": "down", "left": "left", "right": "right", "home": "home", "end": "end", "delete": "delete",
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


# brief M5-T1b step 5: host services the guest reaches through QEMU user networking
# (10.0.2.2:<port> is the Mac's 127.0.0.1:<port>). The DNS port is not 5353: on macOS that is
# mDNS, held by mDNSResponder and apps such as Spotify, so a bind there fails.
ECHO_PORT = 50007
DNS_PORT = 50053
DNS_ZONE = {
    # name: (CNAME target or None, A record of the final name)
    "www.otter.test": ("otter.test", (192, 0, 2, 7)),
    "otter.test": (None, (192, 0, 2, 7)),
}
QEMU_LOCK = os.path.join(BUILD_DIR, "qemu.lock")


def _dns_name_end(msg, i):
    """Offset just past the (uncompressed) name at `i`, and its lowercase text, or None."""
    labels = []
    while i < len(msg):
        n = msg[i]
        i += 1
        if n == 0:
            return i, ".".join(labels)
        if n > 63 or i + n > len(msg):
            return None
        labels.append(msg[i:i + n].decode("ascii", "replace").lower())
        i += n
    return None


def dns_answer(query):
    """A small authoritative answer for DNS_ZONE: the query id and question echoed, a CNAME and
    then the A record for www.otter.test (both names compressed against the question, as real
    servers do), NXDOMAIN (rcode 3) for names outside the zone. None for anything unparseable."""
    if len(query) < 12:
        return None
    qid, flags, qdcount = struct.unpack("!HHH", query[:6])
    if flags & 0x8000 or qdcount != 1:
        return None
    parsed = _dns_name_end(query, 12)
    if parsed is None or parsed[0] + 4 > len(query):
        return None
    end, name = parsed
    qtype, _qclass = struct.unpack("!HH", query[end:end + 4])
    question = query[12:end + 4]
    reply_flags = 0x8000 | 0x0400 | (flags & 0x0100) | 0x0080  # QR, AA, RD echoed, RA
    if name not in DNS_ZONE:
        return struct.pack("!HHHHHH", qid, reply_flags | 3, 1, 0, 0, 0) + question
    target, address = DNS_ZONE[name]
    answers = []
    owner = struct.pack("!H", 0xC00C)  # the question's name
    if target is not None:
        # The target is a suffix of the question name: point into the question for it.
        suffix = 12 + (len(name) - len(target))
        rdata = struct.pack("!H", 0xC000 | suffix)
        answers.append(owner + struct.pack("!HHIH", 5, 1, 3600, len(rdata)) + rdata)
        owner = rdata
    if qtype == 1:
        answers.append(owner + struct.pack("!HHIH", 1, 1, 3600, 4) + bytes(address))
    return struct.pack("!HHHHHH", qid, reply_flags, 1, len(answers), 0, 0) + question + b"".join(answers)


class HostServices:
    """The UDP echo server (ECHO_PORT) and DNS responder (DNS_PORT) on 127.0.0.1, alive from before
    QEMU starts until stop(). A port that cannot be bound raises OSError: a missing service must
    fail the run loudly, never show up as a guest-side timeout."""

    def __init__(self):
        self._stop = threading.Event()
        self._threads = []

    def start(self):
        socks = []
        try:
            for port in (ECHO_PORT, DNS_PORT):
                sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                socks.append(sock)
                sock.bind(("127.0.0.1", port))
                sock.settimeout(0.2)
        except OSError:
            for sock in socks:
                sock.close()
            raise
        for sock, handler in zip(socks, (lambda data: data, dns_answer)):
            thread = threading.Thread(target=self._serve, args=(sock, handler), daemon=True)
            thread.start()
            self._threads.append(thread)
        return self

    def _serve(self, sock, handler):
        with sock:
            while not self._stop.is_set():
                try:
                    data, addr = sock.recvfrom(65535)
                except socket.timeout:
                    continue
                except OSError:
                    return
                reply = handler(data)
                if reply is not None:
                    sock.sendto(reply, addr)

    def stop(self):
        self._stop.set()
        for thread in self._threads:
            thread.join(timeout=2)


def lock_qemu_runs(timeout):
    """Serialise QEMU runs in this checkout: they share artifacts/serial.log, build/qmp.sock and the
    host service ports, so a second concurrent run (a gate an agent left behind) would inject keys
    into, overwrite the log of, and steal the ports of the first. Waits up to `timeout` seconds;
    the returned file keeps the lock until the process exits."""
    os.makedirs(BUILD_DIR, exist_ok=True)
    lock = open(QEMU_LOCK, "w")
    deadline = time.monotonic() + timeout
    announced = False
    while True:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return lock
        except BlockingIOError:
            if time.monotonic() >= deadline:
                lock.close()
                return None
            if not announced:
                print("[qemu.py] another QEMU run in this checkout holds build/qemu.lock; waiting",
                      file=sys.stderr)
                announced = True
            time.sleep(0.5)


def with_qemu_session(timeout, run):
    """`run()` while holding build/qemu.lock with the host services up; its exit code, or 1 when
    the lock or a service port could not be had."""
    lock = lock_qemu_runs(timeout)
    if lock is None:
        print(f"[qemu.py] gave up after {timeout:.0f}s waiting for build/qemu.lock", file=sys.stderr)
        return 1
    with lock:
        try:
            services = HostServices().start()
        except OSError as exc:
            print(f"[qemu.py] cannot start the host services on 127.0.0.1:{ECHO_PORT} and "
                  f":{DNS_PORT}: {exc}", file=sys.stderr)
            return 1
        try:
            return run()
        finally:
            services.stop()


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


def check_rtc_line(log, launch_utc):
    """Brief M8-T7: the kernel logs `[rtc] YYYY-MM-DD HH:MM:SS UTC` at boot. QEMU's RTC follows
    the host's UTC clock, so the logged time must fall between 5 s before this QEMU was launched
    and 600 s after (boot under TCG and host load). Returns an error message or None; a log
    without the line is not checked here (targets that require it pass --expect-serial)."""
    import calendar
    m = re.search(r"\[rtc\] (\d{4})-(\d\d)-(\d\d) (\d\d):(\d\d):(\d\d) UTC", log)
    if not m:
        return None
    try:
        logged = calendar.timegm(tuple(int(g) for g in m.groups()) + (0, 0, 0))
    except (ValueError, OverflowError):
        return f"unparseable RTC line {m.group(0)!r}"
    if not launch_utc - 5 <= logged <= launch_utc + 600:
        host = time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(launch_utc))
        return f"RTC says {m.group(0)[6:]} but the host's UTC clock read {host} at launch"
    return None


def cmd_test(iso, firmware, timeout, expect_failure, expect_serial, send_keys=None, cpu="max"):
    os.makedirs(ARTIFACTS_DIR, exist_ok=True)
    os.makedirs(BUILD_DIR, exist_ok=True)
    reset_outputs(SERIAL_LOG, QMP_SOCK)
    open(SERIAL_LOG, "wb").close()

    args = build_args("test", firmware, iso, cpu)
    start = time.monotonic()
    launch_utc = time.time()
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

    rtc_error = check_rtc_line(log, launch_utc)
    if rtc_error:
        print(f"[qemu.py] {rtc_error}", file=sys.stderr)
        passed = False

    return finish(0 if passed else 1)


def cmd_shot(iso, firmware, timeout, cpu="qemu64"):
    os.makedirs(ARTIFACTS_DIR, exist_ok=True)
    os.makedirs(BUILD_DIR, exist_ok=True)
    shot_png = os.path.join(ARTIFACTS_DIR, "shot.png")
    reset_outputs(SERIAL_LOG, QMP_SOCK, shot_png)
    open(SERIAL_LOG, "wb").close()

    args = build_args("shot", firmware, iso, cpu)
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


def cmd_run(iso, firmware, cpu="qemu64"):
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
    parser.add_argument(
        "--cpu", default="max", metavar="CPU",
        help="QEMU CPU model (default: qemu64; use 'max' for RDRAND/RDSEED)")
    args = parser.parse_args()

    os.chdir(REPO_ROOT)
    iso = os.path.abspath(args.iso)

    if args.mode == "test":
        return with_qemu_session(args.timeout, lambda: cmd_test(
            iso, args.firmware, args.timeout, args.expect_failure, args.expect_serial, args.send_keys, args.cpu))
    if args.mode == "shot":
        return with_qemu_session(args.timeout, lambda: cmd_shot(iso, args.firmware, args.timeout, args.cpu))
    return cmd_run(iso, args.firmware, args.cpu)


if __name__ == "__main__":
    sys.exit(main())
