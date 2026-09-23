# STATUS — the loop reads this first. Keep it short and current.

## Current milestone: M2 Processes and userspace
## Next task: M2-T1 in flight (scheduler; resumed 2026-09-23 after a usage-limit cutoff, same agent). Then M2-T2 ring 3 + syscalls, M2-T3 ELF/initramfs/libotter/init, M2-T4 shell + 8x16 font (briefs written).

## Done
- 2026-09-23 Vision v2 adopted (D20-D26): M6 multicore, M7 local LLM, M8 crypto + TLS 1.3, M9 agent mode, M10 real hardware. Heartbeat cron + keep-awake set up.
- 2026-09-14 M1-T7 console: shadow-buffered framebuffer console, ANSI subset, early-boot replay, serial ANSI stripping, `gmake check` gate (11 targets). 94 tests. **M1 complete.**
- 2026-09-14 M1-T6 PS/2 keyboard: i8042 init with bounded waits, IRQ 1 via I/O APIC, set-1 decoder with modifiers/caps lock, QMP send-key end-to-end tests (hello + caps lock LED path). 80 tests.
- 2026-09-14 M1-T5 ACPI (RSDP/XSDT/RSDT, MADT, HPET, bounds-checked), LAPIC + I/O APIC + PIC masked, PIT-calibrated 1 kHz LAPIC timer, IrqMutex everywhere, IRQ dispatch table, interrupts enabled. 66 tests.
- 2026-09-14 M1-T4 VMM: own page tables, W^X kernel, HHDM (+first 4 GiB), guard-paged boot stack, stackoverflow-test, lock-free guard registry. 49 tests.
- 2026-09-13 M1-T3 heap: slab classes 16..2048 + large objects via PMM, exact double-free/class-mismatch detection, heap-fault-tests. 37 tests. Tests now live in kernel/src/test_cases/.
- 2026-09-13 M1-T2 PMM: bitmap allocator, ownership-checked free, low-memory pool (<1 MiB reserved, alloc_frame_low), pmm-fault-tests. 25 tests.
- 2026-09-13 M1-T1 GDT/TSS/IDT/exception stubs/double fault on IST + review fixes (emergency serial writer, IST re-entry guards, cli, canary test, gmake lint). 11 tests. fault-test/df-test targets added.
- 2026-09-13 Project scaffolded. Toolchain: qemu 11.1, xorriso, mtools, dosfstools, gmake 4.4, rust nightly-2026-09-12 + x86_64-unknown-none.
- 2026-09-13 M0 complete and independently verified: iso, test (3 in-kernel tests, 2 s), bios-test, panic-test, shot (1280x800 banner), clippy clean.

## Blockers / decisions needed from the human (do not block on these)
- Name confirmed by Sammy 2026-09-13: OtterOS. Public repo: https://github.com/SammyTourani/otteros (push after every commit).
- M10 hardware: Sammy has a laptop; model, CPU, NIC and touchpad type unknown. Smoke test + what to report: docs/HARDWARE.md. Record results here.
- Agent mode (M9) needs Sammy to create /config/anthropic.key on the data partition himself; never handled by agents.

## Notes for the next iteration
- Session heartbeat cron e4bd4a9d ("13,43 * * * *") expires 2026-09-30; re-create per LOOP.md.
- QEMU TCG on this Mac delivers timer interrupts at only ~650 Hz although the LAPIC is programmed for 1000 Hz (host timer slack, not a kernel bug). Never assert tight timing in tests; use wide windows.
- Limine trap: never declare a second static of the same Limine request type (bootloader hangs before _start); reuse mm::memmap_entries() and the existing request statics.
- Linker trap: LLD emits .got even with relocation-model=static; it is folded into the data segment in linker-x86_64.ld. Keep it there when editing the script.
- Intermediate page-table frames are never freed yet (documented TODO in vmm/paging).
- Two kernel ELFs exist: `otteros-kernel` (normal) and `otteros-kernel-test` (custom_test_frameworks harness). New modules go in kernel/src/lib.rs so both binaries share them; new tests are `#[test_case]`s reachable from test_main.rs.
- `.cargo/config.toml` lives at the repo root (kernel rustflags: static reloc, kernel code model, no SSE). When userspace arrives in M2, give `user/` its own cargo workspace and config that overrides these flags, or move the kernel config under kernel/ and build with `cd kernel`.
- `bitflags` was dropped from Cargo.toml as unused; re-add when needed (still allowed by D2).
- QEMU under TCG boots the test kernel in ~2 s; keep test timeouts generous anyway (90 s) for later milestones.
