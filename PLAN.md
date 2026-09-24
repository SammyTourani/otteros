# PLAN — milestones and acceptance criteria

A milestone is DONE only when every box passes via a command or a scripted check, not by inspection.
Tasks (M<n>-T<k>) are what the orchestrator briefs to kernel-dev; each must fit one agent session.

Vision (v2, adopted 2026-09-23, DECISIONS D20): an operating system written entirely by AI, with an AI
living inside it. The finished demo is three moments on a real laptop: cold boot to a designed desktop;
Otter, a local language model on OtterOS's own inference engine, chatting offline; and agent mode, where
Claude, reached through OtterOS's own TCP/IP and TLS 1.3, operates the OS through human-approved tool calls.

## M0 Harness — bootable hello, tools proven
- [x] `gmake iso` produces build/otteros.iso (Limine, hybrid BIOS+UEFI)
- [x] `gmake test` exits 0; serial shows `[ok] boot` and `[ok] tests passed`; < 90 s under TCG
- [x] `gmake bios-test` exits 0
- [x] `gmake panic-test` exits 0 (asserts the kernel reported a failure exit code on a deliberate panic)
- [x] `gmake shot` writes artifacts/shot.png showing text rendered on the framebuffer
- [x] 16550 serial driver, framebuffer text with embedded font, panic handler, qemu exit, test runner
- [x] rust-toolchain.toml pinned; clippy clean; README.md tells the story and how to run

## M1 Kernel core
- [x] GDT + TSS; IDT with every exception handler (page fault prints CR2 + RIP); double fault on an IST stack
- [x] ACPI RSDP/XSDT/MADT parsing; LAPIC + I/O APIC; legacy PIC masked; LAPIC timer at 1 kHz; uptime counter
- [x] PS/2 keyboard IRQ -> scancode set 1 -> key events with modifiers
- [x] Physical memory manager from the Limine memory map (bitmap or buddy); stats at boot
- [x] Virtual memory: own page tables, higher-half kernel, HHDM, map/unmap API, guard pages
- [x] Kernel heap (`GlobalAlloc`, slab or free-list); `Vec`/`String`/`Box` work; heap stress test
- [x] Framebuffer console: scrolling, colours, `kprintln!`, mirrored to serial
- [x] >= 25 in-kernel tests covering the above; `gmake test` green (94 tests)

## M2 Processes and userspace
- [x] Kernel threads; preemptive round-robin on the LAPIC timer; sleep/wake; spinlock, mutex, wait queues
- [x] Ring 3: per-process address space; `syscall`/`sysret`; syscall table (write, read, exit, spawn, yield, sleep, getpid, mmap, ...)
- [x] ELF64 loader; initramfs (ustar) as a Limine module; `init` runs from it
- [x] Userspace runtime crate `user/libotter`: syscalls, allocator, print!, minimal std-like API
- [ ] Userspace shell on the console: help, echo, ps, uptime, run <prog>, exit
- [x] A crashing user program is killed and reported; the kernel survives (tested)
- [x] `gmake test` includes userspace tests (programs in the initramfs report via a syscall)
- [ ] Console font covers printable ASCII incl. lowercase (8x16, permissive licence), so shell output reads normally

## M3 Storage and filesystem
- [ ] PCI enumeration (ACPI MCFG / MMCONFIG, port-I/O fallback); device list at boot
- [ ] virtio-blk driver (modern PCI transport); sector read/write; block cache
- [ ] FAT32 read + write (files, directories, long names); image created by scripts, verified with mtools from macOS
- [ ] VFS: open/read/write/close/readdir/stat/mkdir/unlink; devfs (`/dev/console`, `/dev/fb0`, `/dev/null`); mount table
- [ ] Shell gains ls cat write mkdir rm cp and runs programs from disk
- [ ] Scripted test: write file -> reboot -> read back identical

## M4 Desktop: graphics, windows, apps
- [ ] PS/2 mouse driver; cursor drawn on screen
- [ ] Kernel: shared memory + message-passing IPC syscalls; framebuffer handoff to a userspace display server
- [ ] Display server: compositor with damage rectangles, overlapping windows, decorations, focus, input routing
- [ ] Client library: create window, draw (rects, text, bitmaps), receive events
- [ ] Apps: launcher/taskbar, terminal (runs the shell), text editor (open/edit/save on FAT32), about box
- [ ] Scripted GUI test: screenshot shows >= 3 overlapping windows; injected keystrokes type into the editor and save; the file exists after reboot
- [ ] Visual quality bar (D24): own TrueType parser + anti-aliased rasterizer with an embedded OFL font; alpha compositing, rounded corners, soft shadows; one designed theme; own PNG decoder (inflate) for wallpaper and icons

## M5 Networking
- [ ] virtio-net and e1000 drivers (RX/TX rings, interrupts)
- [ ] Ethernet, ARP, IPv4, ICMP echo, UDP, DHCP client, DNS resolver
- [ ] TCP: handshake, retransmission, receive window, orderly close; stress-tested against QEMU user networking
- [ ] Socket syscalls; ping, nslookup, `fetch <url>` (HTTP/1.1 GET); a GUI page viewer rendering headings/paragraphs/links of simple HTML
- [ ] Test: `fetch http://example.com/` returns the expected title string in QEMU

## M6 Multicore and SIMD
- [ ] All CPUs online via the Limine MP request; per-CPU data (GS base) and per-CPU LAPIC timers; xAPIC and x2APIC
- [ ] SMP scheduler: per-CPU run queues, load balancing, reschedule and TLB-shootdown IPIs; every lock audited for SMP
- [ ] User threads: thread_spawn / thread_join / futex-style wait+wake syscalls; libotter threads, Mutex, Condvar
- [ ] XSAVE/XRSTOR with AVX/AVX2 state for user threads (CPUID leaf 0xD sizing), SSE-only fallback
- [ ] `gmake check` runs with `-smp 4`; an 8-thread parallel stress test passes 20 runs in a row

## M7 Otter: a language model living inside the OS
- [ ] Host tool `tools/otter-convert` (Rust, std only): safetensors + tokenizer.json -> `.otm` (config, Q8 group-quantized weights, BPE vocab/merges, chat template); emits golden logits for a fixed prompt
- [ ] `user/otter-llm`: from-scratch Llama-family runtime (RMSNorm, RoPE, GQA attention with KV cache, SwiGLU), byte-level BPE tokenizer with the model's chat template, seeded sampling (greedy, temperature, top-p)
- [ ] Q8 matmul kernels: scalar reference, SSE2, AVX2 with runtime CPUID dispatch; multi-threaded across all cores
- [ ] Golden tests: a deterministic tiny model from the host tool matches host logits (max abs err < 1e-3) for 32 steps in `gmake test`; the real model's first 16 greedy tokens match the host reference in `gmake model-test`
- [ ] `otter "prompt"` in the shell and an Otter chat app in the desktop: streaming tokens, tokens/s readout, stop button
- [ ] Default model: the strongest Apache-2.0/MIT instruct model that runs >= 8 tokens/s on the test laptop's CPU; weights on the data partition via `gmake models`, never committed

## M8 Trust: cryptography and TLS 1.3 from scratch
- [ ] `crates/otter-crypto` (no_std, no deps): SHA-256/384/512, HMAC, HKDF, ChaCha20-Poly1305, AES-128/256-GCM (constant-time software path, AES-NI + PCLMULQDQ when present), X25519, P-256/P-384 ECDSA verify, RSA PKCS#1 v1.5 + PSS verify; RFC and Project Wycheproof vectors pass
- [ ] Entropy: RDSEED/RDRAND plus TSC-jitter pool feeding a ChaCha20 DRBG; `getrandom` syscall
- [ ] CMOS RTC wall clock; X.509/DER parser, chain building to an embedded root store, SAN hostname and validity checks
- [ ] TLS 1.3 client (RFC 8446): x25519/secp256r1 key shares, HelloRetryRequest, TLS_AES_128_GCM_SHA256 / TLS_AES_256_GCM_SHA384 / TLS_CHACHA20_POLY1305_SHA256, certificate verification, KeyUpdate, close_notify; any verification failure aborts
- [ ] Interop: a host Python TLS 1.3 server with a test CA, reached from QEMU, passes for every suite; live `fetch https://example.com` passes when online (skipped offline)
- [ ] `fetch` and the page viewer speak HTTPS

## M9 Agent mode: the AI operates its own OS
- [ ] Tool protocol: list_dir, read_file, write_file, run_command (captured output), open_app(app, path), fetch_url, screenshot (own PNG encoder)
- [ ] Every state-changing tool call raises an on-screen Allow/Deny dialog (D23); an action-log window shows everything the agent did
- [ ] Claude backend: Messages API over HTTPS with streaming and tool use; the key is read from `/config/anthropic.key` on the data partition, created by the human, never committed or logged
- [ ] Local backend: Otter with grammar-constrained decoding so its tool calls are always valid for a small tool set
- [ ] Scripted QEMU test: a host mock Claude server (TLS, test CA) replays a tool-use conversation; the test approves via injected input and asserts the file was written and the editor window shows it

## M10 Real hardware and the demo
- [ ] `gmake usb-image` -> dd-able hybrid image with a FAT32 data partition; docs/HARDWARE.md covers Secure Boot and the boot menu
- [ ] ACPI/APIC/HPET timing and i8042 quirks robust on the test laptop
- [ ] xHCI host controller, USB HID keyboard and mouse, USB mass storage (the data partition on the boot stick, persistent)
- [ ] Ethernet driver for the test laptop's NIC (e1000e, RTL8111/8168, or a USB-Ethernet adapter over xHCI, chosen when the model is known)
- [ ] On the laptop: boots to the desktop; Otter chats offline; with Ethernet, agent mode with Claude writes a file and opens it in the editor
- [ ] Internal disks are never written (no NVMe/AHCI write path unless the human opts in)
- [ ] Human records the demo video

## Execution order
M2 -> M3 -> M4 -> M5 -> M6 -> M7 -> M8 -> M9 -> M10. Hardware tasks from M10 may be pulled forward as soon as the
human reports the laptop model and smoke-test result (docs/HARDWARE.md, STATUS.md).

## Definition of done
Every box above is ticked: M0-M9 green in QEMU under `gmake check` (including the GUI, network, TLS interop, LLM golden
and agent-mode scripted tests) and M10 demonstrated on the real laptop and confirmed by the human.
