# Architecture decisions (append-only; reference by ID)

- **D1 Target.** x86_64, UEFI-first, legacy BIOS also working (Limine hybrid ISO). The final demo boots from a USB stick on an arbitrary cheap laptop.
- **D2 Language and crates.** Rust nightly, target `x86_64-unknown-none`. Allowed external crates: `limine` (boot protocol structs only), `bitflags`, `spin` (may later be replaced by our own locks). Forbidden: `x86_64`, `bootloader`, `uefi`, `smoltcp`, `fatfs`, `embedded-graphics`, `acpi`, `linked_list_allocator`, `buddy_system_allocator`, `pc-keyboard`, `virtio-drivers`, or any crate implementing OS logic. Ours: GDT/IDT, paging, allocators, scheduler, drivers, VFS, FS, TCP/IP, window system. Reason: the public claim is that the AI wrote the OS.
- **D3 Bootloader.** Limine (binary release branch), base revision 3. Requests: framebuffer, HHDM, memory map, RSDP, modules, executable address/cmdline; SMP later. Gives a linear framebuffer + memory map on UEFI, BIOS and real hardware.
- **D4 Kernel shape.** Monolithic kernel in the higher half (0xffffffff80000000); userspace processes with their own page tables. Simplest path to a usable system in weeks.
- **D5 Feedback channels.** Serial COM1 for logs/test results; `isa-debug-exit` (port 0xf4) for pass/fail; QMP `screendump` PNG for visual checks. Cheap, machine-readable, no GUI required.
- **D6 Boot modes.** One kernel binary; behaviour selected by the Limine command line (`test`, `test panic`, later `gui`, `headless`). Two generated limine.conf variants.
- **D7 Storage.** virtio-blk in QEMU first; NVMe then AHCI for real hardware. FAT32 on disk (USB sticks are FAT; macOS can read the results). Own VFS above it.
- **D8 Networking.** virtio-net (QEMU) and Intel e1000/e1000e (QEMU + many real laptops). Own stack: Ethernet, ARP, IPv4, ICMP, UDP, DHCP, DNS, TCP. HTTP only, no TLS. QEMU user-mode networking provides DHCP/DNS/NAT for tests.
- **D9 Input.** PS/2 keyboard + mouse via i8042 (QEMU and most laptops' built-in keyboard/touchpad). USB HID is a stretch goal.
- **D10 GUI.** Userspace display server owns the framebuffer; clients draw into shared-memory buffers; message-passing IPC. Kernel provides framebuffer mapping, IPC, shared memory. App crashes do not take the OS down.
- **D11 Assets.** Permissively licensed bitmap fonts embedded as data (public-domain 8x8 / 8x16). Assets are not "code" for the purpose of the claim; record their licence in THIRD_PARTY_NOTICES.md.
- **D12 Out of scope.** Wi-Fi, TLS, sound, 32-bit, SMP scheduling (may come later), full USB stack (stretch).
