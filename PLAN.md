# PLAN — milestones and acceptance criteria

A milestone is DONE only when every box passes via a command or a scripted check, not by inspection.
Tasks (M<n>-T<k>) are what the orchestrator briefs to kernel-dev; each must fit one agent session.

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
- [ ] ACPI RSDP/XSDT/MADT parsing; LAPIC + I/O APIC; legacy PIC masked; LAPIC timer at 1 kHz; uptime counter
- [ ] PS/2 keyboard IRQ -> scancode set 1 -> key events with modifiers
- [x] Physical memory manager from the Limine memory map (bitmap or buddy); stats at boot
- [ ] Virtual memory: own page tables, higher-half kernel, HHDM, map/unmap API, guard pages
- [x] Kernel heap (`GlobalAlloc`, slab or free-list); `Vec`/`String`/`Box` work; heap stress test
- [ ] Framebuffer console: scrolling, colours, `kprintln!`, mirrored to serial
- [ ] >= 25 in-kernel tests covering the above; `gmake test` green

## M2 Processes and userspace
- [ ] Kernel threads; preemptive round-robin on the LAPIC timer; sleep/wake; spinlock, mutex, wait queues
- [ ] Ring 3: per-process address space; `syscall`/`sysret`; syscall table (write, read, exit, spawn, yield, sleep, getpid, mmap, ...)
- [ ] ELF64 loader; initramfs (ustar) as a Limine module; `init` runs from it
- [ ] Userspace runtime crate `user/libotter`: syscalls, allocator, print!, minimal std-like API
- [ ] Userspace shell on the console: help, echo, ps, uptime, run <prog>, exit
- [ ] A crashing user program is killed and reported; the kernel survives (tested)
- [ ] `gmake test` includes userspace tests (programs in the initramfs report via a syscall)

## M3 Storage and filesystem
- [ ] PCI enumeration (ACPI MCFG / MMCONFIG, port-I/O fallback); device list at boot
- [ ] virtio-blk driver (modern PCI transport); sector read/write; block cache
- [ ] FAT32 read + write (files, directories, long names); image created by scripts, verified with mtools from macOS
- [ ] VFS: open/read/write/close/readdir/stat/mkdir/unlink; devfs (`/dev/console`, `/dev/fb0`, `/dev/null`); mount table
- [ ] Shell gains ls cat write mkdir rm cp and runs programs from disk
- [ ] Scripted test: write file -> reboot -> read back identical

## M4 Graphics, windows, apps
- [ ] PS/2 mouse driver; cursor drawn on screen
- [ ] Kernel: shared memory + message-passing IPC syscalls; framebuffer handoff to a userspace display server
- [ ] Display server: compositor with damage rectangles, overlapping windows, decorations, focus, input routing
- [ ] Client library: create window, draw (rects, text, bitmaps), receive events
- [ ] Apps: launcher/taskbar, terminal (runs the shell), text editor (open/edit/save on FAT32), about box
- [ ] Scripted GUI test: screenshot shows >= 3 overlapping windows; injected keystrokes type into the editor and save; the file exists after reboot

## M5 Networking
- [ ] virtio-net and e1000 drivers (RX/TX rings, interrupts)
- [ ] Ethernet, ARP, IPv4, ICMP echo, UDP, DHCP client, DNS resolver
- [ ] TCP: handshake, retransmission, receive window, orderly close; stress-tested against QEMU user networking
- [ ] Socket syscalls; ping, nslookup, `fetch <url>` (HTTP/1.1 GET); a GUI page viewer rendering headings/paragraphs/links of simple HTML
- [ ] Test: `fetch http://example.com/` returns the expected title string in QEMU

## M6 Real hardware
- [ ] `gmake usb-image` -> bootable hybrid image written with `dd`
- [ ] ACPI, APIC/HPET timing and i8042 quirks robust on real hardware
- [ ] NVMe and AHCI drivers; a FAT32 partition on the USB stick is the data disk
- [ ] Ethernet driver for the test laptop's NIC (decided when the laptop is known: e1000e or RTL8168/8169)
- [ ] Boots to the desktop on the test laptop; editor saves a file; `fetch` loads a page over Ethernet
- [ ] Human records the real-hardware demo video

## Definition of done
M0–M5 green in QEMU (`gmake test` + scripted GUI test) and M6 demonstrated on a real laptop.
