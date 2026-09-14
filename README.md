# OtterOS

OtterOS is an x86_64 operating system, written from scratch in Rust, boots on
real hardware (UEFI and legacy BIOS), and eventually gets you a desktop with
a mouse and windows, a terminal, a text editor, and its own TCP/IP stack
fetching a web page over Ethernet. "OtterOS" is a placeholder codename.

## The AI-authored claim

No human writes code in this repository. Every kernel and userspace source
file is authored by Claude agents: an orchestrator plans and briefs each
task, `kernel-dev` implements it against a self-contained brief and runs its
own acceptance commands, and `kernel-review` reviews the unsafe-heavy diffs.
Humans run the agent loop and supply the hardware for the final demo.
DECISIONS.md D2 backs this with a hard rule: the only external crates the
kernel may depend on are `limine` (boot protocol structs), `bitflags`, and
`spin` -- no crate that implements OS logic (paging, allocators, a
scheduler, drivers, a filesystem, TCP/IP, ...) on the kernel's behalf. See
CLAUDE.md for the full agent roles/rules and DECISIONS.md for the rest of
the architecture record.

## Building and running

Everything below assumes macOS with Homebrew (`qemu`, `xorriso`, `mtools`)
and `rustup` installed, run from the repository root:

```sh
source scripts/env.sh   # puts rustup/cargo/homebrew on PATH
gmake iso                # -> build/otteros.iso (hybrid UEFI + legacy BIOS)
gmake test                # headless UEFI boot, in-kernel tests, exit 0 on pass
gmake bios-test            # the same tests via the legacy BIOS boot path
gmake panic-test           # boots into a deliberate panic and checks it's reported
gmake fault-test           # boots into a deliberate page fault and checks it's reported
gmake df-test              # boots into a deliberate double fault and checks it's reported
gmake shot                 # headless boot -> artifacts/shot.png (QMP screendump)
gmake run                  # a real QEMU window, for humans
gmake lint                 # cargo clippy, including #[cfg(test)] code, -D warnings
```

`third_party/limine` (the bootloader binaries and host deploy tool) and
`third_party/limine-rust-template` (reference material) are fetched on
demand and gitignored; `gmake deps` fetches them explicitly. Serial COM1 is
the primary log channel -- every run tees it to `artifacts/serial.log`.

Use GNU Make as `gmake`, not `make` (macOS ships GNU Make 3.81; our
Makefile needs a modern one).

## Milestones

The full acceptance criteria for each milestone live in
[PLAN.md](PLAN.md); current progress is tracked in
[STATUS.md](STATUS.md). Summary:

| Milestone | What it proves |
| --- | --- |
| M0 Harness | Boots to a serial + framebuffer "hello", in-kernel test runner, QEMU harness (this repo's current state) |
| M1 Kernel core | GDT/IDT, ACPI + APIC, PS/2 keyboard, physical + virtual memory, a kernel heap, a framebuffer console |
| M2 Processes | Kernel threads, ring 3 with syscalls, an ELF loader, an initramfs, a userspace shell |
| M3 Storage | PCI, virtio-blk, FAT32 read/write, a VFS |
| M4 Graphics | PS/2 mouse, a userspace display server, overlapping windows, a terminal and text editor |
| M5 Networking | virtio-net/e1000, its own Ethernet/ARP/IPv4/TCP/UDP/DHCP/DNS stack, `fetch <url>` |
| M6 Real hardware | Boots from a USB stick on an arbitrary laptop; a recorded demo |

`gmake test` (plus, from M4 on, a scripted GUI test) must stay green through
M0-M5; M6 is demonstrated on real hardware.
