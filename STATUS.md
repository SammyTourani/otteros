# STATUS — the loop reads this first. Keep it short and current.

## Current milestone: M1 Kernel core
## Next task: M1-T1 — GDT + TSS, IDT with all exception handlers (page fault prints CR2 + RIP), double fault on an IST stack, tests that trigger and recover from a breakpoint exception. Brief to write: briefs/M1-T1.md

## Done
- 2026-09-13 Project scaffolded. Toolchain: qemu 11.1, xorriso, mtools, dosfstools, gmake 4.4, rust nightly-2026-09-12 + x86_64-unknown-none.
- 2026-09-13 M0 complete and independently verified: iso, test (3 in-kernel tests, 2 s), bios-test, panic-test, shot (1280x800 banner), clippy clean.

## Blockers / decisions needed from the human (do not block on these)
- Name confirmed by Sammy 2026-09-13: OtterOS. Public repo: https://github.com/SammyTourani/otteros (push after every commit).
- Sammy has a laptop for M6. Model, NIC and whether it has Ethernet are unknown: ask when M5 starts (driver choice) and note the answer here.

## Notes for the next iteration
- Two kernel ELFs exist: `otteros-kernel` (normal) and `otteros-kernel-test` (custom_test_frameworks harness). New modules go in kernel/src/lib.rs so both binaries share them; new tests are `#[test_case]`s reachable from test_main.rs.
- `.cargo/config.toml` lives at the repo root (kernel rustflags: static reloc, kernel code model, no SSE). When userspace arrives in M2, give `user/` its own cargo workspace and config that overrides these flags, or move the kernel config under kernel/ and build with `cd kernel`.
- `bitflags` was dropped from Cargo.toml as unused; re-add when needed (still allowed by D2).
- QEMU under TCG boots the test kernel in ~2 s; keep test timeouts generous anyway (90 s) for later milestones.
