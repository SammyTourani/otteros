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
| 2026-09-23 | M2-T1 preemptive scheduler, wait queues, mutex, semaphore, join | 110 | Implementer: new threads started with a misaligned stack. Review REJECT: stacks of live threads could be freed through a public API; a wait-queue lock was held while taking the scheduler lock; idle CPU ignored wake-ups until the next timeslice; waiters could be barged past |
| 2026-09-23 | M4-T2 otter-gfx: 2D graphics, PNG codec, TrueType text with GPOS kerning (host-tested, in parallel with M2-T2) | 142 host | Implementer: Inter wraps its kerning in GPOS extension lookups, so kerning silently did nothing until type 9 was unwrapped |
| 2026-09-23 | M8-T1 otter-crypto part 1: SHA-2, HMAC, HKDF, ChaCha20-Poly1305, safe Rust only | 52 + 1090 Wycheproof | Implementer: caught its own transcription errors in three RFC test vectors by re-deriving them independently |
| 2026-09-23 | M2-T2 ring 3, syscalls, user memory checks, FPU state, process lifecycle | 133 | Implementer: page-table entries above the leaf never allowed user access, so no user program could have run. Review (3 rounds): several single system calls could crash the kernel; fixed with range and permission checks, an exception-fixup table and RFLAGS sanitising, then wait/kill lifecycle races |
| 2026-09-23 | M8-T2 otter-crypto part 2: constant-time AES-GCM with AES-NI/PCLMUL path, X25519 | 84 + 651 Wycheproof | Differential test: hardware and software AES-GCM agree on 20,000 random cases under Rosetta |
| 2026-09-23 | M2-T2b/c reusable kernel-stack slots; load-robust test suite | 136 | Orchestrator's gate run under host CPU load exposed timing-fragile tests; rewritten to count scheduler events and verified with 8 CPU burners running |
| 2026-09-24 | M8-T3 otter-crypto part 3: big integers, RSA and ECDSA signature verification | 113 + 2051 Wycheproof | Implementer cross-checked every curve constant in Python (on-curve, n*G = O, primality) before use |
| 2026-09-24 | M8-T4 otter-x509: DER, X.509 path validation, host names, Mozilla root store | 109 + 154 limbo | Implementer: the signed-bytes capture dropped the TBSCertificate header, which would have failed every signature check; caught by real-chain tests |
| 2026-09-24 | M2-T3 ELF loader, initramfs, spawn/wait, libotter runtime, first userspace programs | 157 + 12 user | Review REJECT: several malformed spawn arguments or ELF headers could panic the kernel instead of returning an error; all now rejected with errno and covered by tests |
| 2026-09-24 | M8-T5 otter-tls: from-scratch TLS 1.3 client, live handshakes with 5 production servers | 35 + 5 live | Implementer: the record header was used as AEAD associated data but never written to the wire; caught by local interop against openssl |
| 2026-09-24 | M2-T3b flat memory across thousands of processes | 157 + 12 user | Implementer: functions that never return kept reference-counted handles alive on the abandoned stack, so every exited thread leaked; found by a two-round leak test that now measures 0 frames |
| 2026-09-24 | M7-T1 JSON library, model converter (.otm f32/Q8), Hugging Face reference goldens | 9 + 36 | Orchestrator checks caught what the implementer reported as done: empty tensor tables, a golden file of all-NaN logits from a buggy RNG, and a test comparing against a stale model copy |
| 2026-09-24 | M2-T4a console font, new syscalls, basic shell | 158 + 12 user | Orchestrator zoomed into the screenshot: every Spleen glyph was mirrored (BDF bit order); a host-load gate run exposed a race in the spawn-layout test |
| 2026-09-24 | M7-T2a LLM runtime part 1: loader, tokenizer (40/40 golden prompts), no_std math | 21 | Orchestrator found the converter wrote no BPE merges, the math tests only printed errors (log off by ln 2, sin/cos truncated series), and a deleted failing test; the orchestrator fixed the math itself after two failed rounds |
