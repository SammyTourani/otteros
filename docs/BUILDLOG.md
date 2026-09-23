# Build log

Every task is briefed by the orchestrator, implemented by a `kernel-dev` agent, checked by the
orchestrator's own run of the gate, and, for unsafe-heavy code, reviewed by a separate
`kernel-review` agent. The last column is the evidence that the review loop matters.

| Date | Task | Tests | Caught before merge |
| --- | --- | --- | --- |
| 2026-09-13 | M0-T1 boot harness: Limine, serial, framebuffer text, QEMU test/shot | 3 | — |
| 2026-09-13 | M1-T1 GDT, TSS, IDT, exception stubs, double fault on IST | 11 | Implementer: LLVM put the interrupt stacks in read-only memory (silent triple fault). Review: an NMI during a log line could deadlock; IST stacks could be re-entered; no `cli` at entry |
| 2026-09-13 | M1-T2 physical memory manager | 25 | Review REJECT: freeing a kernel-image frame was silently accepted; the allocator bitmap sat at physical address 0 |
| 2026-09-13 | M1-T3 kernel heap | 37 | Review: freeing with the wrong layout could corrupt another size class; stress test could miss aliasing |
| 2026-09-14 | M1-T4 virtual memory, W^X, guard-paged stacks | 49 | Implementer: a duplicate bootloader request hung boot; the linker put `.got` outside the data segment. Review: a lock in the double-fault path could deadlock |
| 2026-09-14 | M1-T5 ACPI, APIC, 1 kHz timer, interrupts on | 66 | BIOS test: SeaBIOS tables are unaligned (panic). Review: unbounded ACPI table lengths from firmware |
| 2026-09-14 | M1-T6 PS/2 keyboard, end-to-end typing via QMP | 80 | Review REJECT: pressing Caps Lock could freeze the machine (lock held across `hlt`) |
| 2026-09-14 | M1-T7 framebuffer console, `gmake check` gate | 94 | Review: interrupts disabled for the whole boot-log replay; colour codes leaking into the serial log |
