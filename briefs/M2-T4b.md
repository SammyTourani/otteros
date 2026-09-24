# Brief M2-T4b — keys as terminal bytes, the otsh line editor and built-ins, the scripted shell test

## Goal
Finish the shell a person uses at the keyboard, and prove it with a scripted typing test. Closes M2.

## Applies
D2, D6, D16, D17.

## Steps (in order; run the named check after each)
1. Kernel, fd 0: special keys arrive as terminal byte sequences: Up ESC[A, Down ESC[B, Right ESC[C,
   Left ESC[D, Home ESC[H, End ESC[F, Delete ESC[3~, Backspace 0x7F, Enter '\n', Ctrl+letter as
   0x01..0x1A. Add a kernel #[test_case] that feeds scancodes through the decoder into the fd-0
   buffer and checks the exact bytes. Check: `gmake test`.
2. otsh line editor: prompt `otter> ` in bold, cursor movement (Left/Right/Home/End, Ctrl-A/E),
   Backspace and Delete mid-line, Ctrl-U clears the line, Ctrl-L clears the screen, Up/Down walk a
   64-entry history. Redraw the line with ANSI sequences the kernel console understands. Check:
   `gmake build`, and a host unit test of the pure line-editing state machine (factor it into a
   module with no OS calls; `cd user && cargo test` for that module, or a small host crate).
3. Built-ins with real data: help, echo, clear, ps (proc_list: pid, ppid, state, name), mem and
   uptime (sysinfo), kill <pid>, run <prog> [args], exit [code], reboot, history. A bare word runs
   /bin/<word> if it exists, else prints `otsh: <word>: command not found`. After a child ends,
   show its non-zero exit code or kill reason. init runs /bin/sh in normal mode and restarts it if
   it exits. Check: `gmake build` and `gmake shot` (prompt visible).
4. `gmake shell-test` exactly as briefs/M2-T4.md §4 (injected keys, all expected serial patterns,
   `--test` mode ends with test_exit), re-added to scripts/check.sh. Check: `gmake shell-test`
   three times, then `gmake check` all PASS.

## Report
<=10 lines with the observed evidence for each step. Do not commit.
