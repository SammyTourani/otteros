# OtterOS — operating manual for agents

OtterOS is an x86_64 operating system written from scratch in Rust, authored entirely by AI agents.
Goal (vision v2, DECISIONS D20): an OS written entirely by AI with an AI inside it. Boots on a real laptop to a
designed desktop; runs a local language model on its own inference engine; lets Claude operate it over its own TLS 1.3. "OtterOS" is a placeholder codename.

## The one rule that matters
No human writes code in this repo. The public claim is "an AI built a usable OS end to end."
Therefore: no OS-logic crates. See DECISIONS.md D2 for the crate allowlist.

## Roles and models (token policy)
- **Orchestrator** = the interactive Claude Code session (the session's frontier model). Plans, briefs, verifies, decides.
  Does NOT write kernel code and does NOT read source files unless a task has failed twice.
- **kernel-dev** (Sonnet) = writes all code. Gets a self-contained brief with acceptance commands.
  Runs the acceptance commands before returning. Returns a <=15-line report, never code or logs.
- **kernel-review** (Sonnet) = reviews diffs for memory safety and spec conformance when asked
  (memory management, scheduler, drivers, IPC, anything `unsafe`-heavy).
Cheap models do the typing; the expensive model does the judging.

## Repo map
kernel/        kernel crate (no_std, x86_64-unknown-none) — its own cargo project
user/          userspace crates (from M2) — separate cargo workspace
scripts/       qemu runner, tests, screenshots, env.sh
briefs/        task briefs written by the orchestrator
third_party/   fetched at build time (Limine); gitignored
artifacts/     screenshots and logs; gitignored except artifacts/keep/
PLAN.md        milestones + acceptance criteria (the contract)
STATUS.md      done / next / blockers (the loop's memory)
DECISIONS.md   architecture decisions; append-only
LOOP.md        the autonomous loop protocol

## Commands (repo root; GNU make = `gmake`; `source scripts/env.sh` first)
gmake build       build kernel (+ userspace once it exists)
gmake iso         build/otteros.iso (hybrid BIOS+UEFI via Limine)
gmake test        headless UEFI boot, in-kernel tests, exit 0 on pass (isa-debug-exit); must pass under host CPU load
gmake bios-test   same via legacy BIOS boot path
gmake shot        headless boot, QMP screendump -> artifacts/shot.png
gmake lint        cargo clippy --tests with warnings denied
gmake check       the gate: every target as a PASS/FAIL table (run before declaring any task done)
gmake fault-test / df-test / panic-test   negative tests: expect a specific failure on serial
gmake run         visible QEMU window for humans
Serial COM1 is the primary log channel; every run tees it to artifacts/serial.log.

## Conventions
- **Every host test run goes through `scripts/memguard.py -- cargo test ...`** (kills the process group above
  2 GB resident; override with `--cap-mb`). A runaway test once allocated 61.7 GB on this 16 GB Mac and caused
  a kernel watchdog panic and reboot (2026-09-24). Tests must also bound their own loops and buffers.
- Rust nightly pinned in rust-toolchain.toml. `#![no_std]`, `#![no_main]`.
- Every `unsafe` block gets a `// SAFETY:` comment stating the invariant it relies on.
- Zero warnings. `gmake lint` (clippy incl. tests, warnings denied) clean before a task is done.
- Panics print `PANIC at file:line: msg` to serial; in test mode they exit QEMU with the failure code.
- Tests: in-kernel runner (custom_test_frameworks, `#[test_case]`); `gmake test` runs everything.
- The orchestrator commits per completed task: `M<n>-T<k>: <what>`. Never commit build outputs.
- Keep files under ~600 lines; split modules early. Log to serial with a module prefix: `[pmm]`, `[sched]`.

## macOS specifics (Mac Mini M4, Apple Silicon)
- QEMU x86_64 runs under TCG (pure emulation), roughly 10x slower than native: use generous timeouts.
- No `timeout` binary: use `gtimeout` (coreutils) or Python subprocess timeouts.
- BSD sed (`sed -i ''`); prefer Python for text munging in scripts.
- Homebrew prefix /opt/homebrew. OVMF: /opt/homebrew/share/qemu/edk2-x86_64-code.fd (+ edk2-i386-vars.fd, copy to build/ writable).
- rustup at /opt/homebrew/opt/rustup/bin; toolchains in ~/.rustup; proxies in ~/.cargo/bin. scripts/env.sh exports PATH.
- System `make` is GNU make 3.81 (ancient). Always `gmake`.

## How a task is done (kernel-dev)
1. Read the brief, CLAUDE.md, DECISIONS.md, the relevant STATUS.md lines. Only then read the source you need.
2. Implement. `gmake build` often; the brief's acceptance commands before declaring done.
3. If acceptance fails, fix it. If stuck after real effort, return with the exact failure and what you tried. Never fake success.
4. Return <=15 lines: files touched, acceptance commands + exit codes, deviations, decisions needed.
