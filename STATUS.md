# STATUS — the loop reads this first. Keep it short and current.

## Current milestone: M1 Kernel core
## Next task: M1-T5 — ACPI, LAPIC/IOAPIC, 1 kHz timer, interrupts on (briefs/M1-T5.md). Then T6 keyboard, T7 console (briefs written).

## Done
- 2026-09-14 M1-T4 VMM: own page tables, W^X kernel, HHDM (+first 4 GiB), guard-paged boot stack, stackoverflow-test, lock-free guard registry. 49 tests.
- 2026-09-13 M1-T3 heap: slab classes 16..2048 + large objects via PMM, exact double-free/class-mismatch detection, heap-fault-tests. 37 tests. Tests now live in kernel/src/test_cases/.
- 2026-09-13 M1-T2 PMM: bitmap allocator, ownership-checked free, low-memory pool (<1 MiB reserved, alloc_frame_low), pmm-fault-tests. 25 tests.
- 2026-09-13 M1-T1 GDT/TSS/IDT/exception stubs/double fault on IST + review fixes (emergency serial writer, IST re-entry guards, cli, canary test, gmake lint). 11 tests. fault-test/df-test targets added.
- 2026-09-13 Project scaffolded. Toolchain: qemu 11.1, xorriso, mtools, dosfstools, gmake 4.4, rust nightly-2026-09-12 + x86_64-unknown-none.
- 2026-09-13 M0 complete and independently verified: iso, test (3 in-kernel tests, 2 s), bios-test, panic-test, shot (1280x800 banner), clippy clean.

## Blockers / decisions needed from the human (do not block on these)
- Name confirmed by Sammy 2026-09-13: OtterOS. Public repo: https://github.com/SammyTourani/otteros (push after every commit).
- Sammy has a laptop for M6. Model, NIC and whether it has Ethernet are unknown: ask when M5 starts (driver choice) and note the answer here.

## Notes for the next iteration
- Limine trap: never declare a second static of the same Limine request type (bootloader hangs before _start); reuse mm::memmap_entries() and the existing request statics.
- Linker trap: LLD emits .got even with relocation-model=static; it is folded into the data segment in linker-x86_64.ld. Keep it there when editing the script.
- Intermediate page-table frames are never freed yet (documented TODO in vmm/paging).
- Two kernel ELFs exist: `otteros-kernel` (normal) and `otteros-kernel-test` (custom_test_frameworks harness). New modules go in kernel/src/lib.rs so both binaries share them; new tests are `#[test_case]`s reachable from test_main.rs.
- `.cargo/config.toml` lives at the repo root (kernel rustflags: static reloc, kernel code model, no SSE). When userspace arrives in M2, give `user/` its own cargo workspace and config that overrides these flags, or move the kernel config under kernel/ and build with `cd kernel`.
- `bitflags` was dropped from Cargo.toml as unused; re-add when needed (still allowed by D2).
- QEMU under TCG boots the test kernel in ~2 s; keep test timeouts generous anyway (90 s) for later milestones.
