# Brief M3-T1b — virtio-blk in the kernel on top of `otter-virtio` (polled I/O)

## Goal
The kernel reads and writes the QEMU virtio disk for real: device capacity from the device, sector
reads and writes through a split virtqueue, verified by the orchestrator's acceptance tests in
kernel/src/test_cases/virtio_blk.rs (must pass unchanged). Completion is polled in this task;
MSI-X interrupts and blocking waits are M3-T1c-irq, later.

## Applies
D2 (the kernel may depend on the repo's own pure crates by path: this is the first, `otter-virtio`),
D14, D15 (MMIO windows live in the kernel dynamic region), D27. Every `unsafe` block gets a
`// SAFETY:` comment. The old kernel/src/drivers/virtio/{queue.rs,pci_transport.rs} and the stub in
blk.rs (which hard-codes the capacity) are replaced.

## Design
1. `kernel/Cargo.toml`: `otter-virtio = { path = "../crates/otter-virtio" }`. Use its `SplitQueue`,
   `RingMemory`, `Transport`, `negotiate`, `queue_size_for`, `setup_queue`, `finish_init`,
   `read_config_consistent`, `BlockRequest` and `BLOCK_*` constants; do not re-implement them.
2. `kernel/src/mm/mmio.rs`: `pub fn map(phys: PhysAddr, len: usize) -> Option<VirtAddr>` maps the
   range into a dedicated window of the kernel dynamic region, 0xffff_c800_0000_0000 up to
   0xffff_c900_0000_0000 (kernel stacks use the start of the region; check they cannot reach it),
   page-granular, flags PRESENT | WRITABLE | NO_CACHE | WRITE_THROUGH | NO_EXECUTE | GLOBAL,
   returning the virtual address of `phys` (offset within the page preserved). A bump allocator under
   a lock; mapping the same physical page twice may reuse the earlier mapping. Never use the HHDM for
   device registers: it is cacheable and may not cover 64-bit BARs.
3. PCI: enable Memory Space and Bus Master in the command register. Walk the vendor-specific
   capabilities (cap id 0x09): cfg_type at +3 (1 common, 2 notify with notify_off_multiplier at +16,
   3 ISR, 4 device-specific), BAR index at +4, offset at +8 (u32), length at +12 (u32). Map each
   structure with `mmio::map(bar_address + offset, length)`.
4. A `Transport` implementation over the common-configuration structure (virtio 1.2 §4.1.4.3
   offsets: device_feature_select 0x00, device_feature 0x04, driver_feature_select 0x08,
   driver_feature 0x0C, config_msix_vector 0x10, num_queues 0x12, device_status 0x14,
   config_generation 0x15, queue_select 0x16, queue_size 0x18, queue_msix_vector 0x1A,
   queue_enable 0x1C, queue_notify_off 0x1E, queue_desc 0x20, queue_driver 0x28, queue_device 0x30)
   with volatile accesses; `notify(queue, notify_off)` writes the queue index at
   notify_base + notify_off * notify_off_multiplier; `read_config` reads the device-specific
   structure (capacity is the u64 at offset 0, in 512-byte sectors).
5. DMA memory from `pmm::alloc_contiguous`, used through its HHDM alias: the ring (size from
   `Layout::for_size`) and a bounce buffer of 256 KiB plus a request header and a status byte. Caller
   buffers are heap memory and not physically contiguous, so data is copied through the bounce
   buffer; a request moves at most 256 KiB (512 sectors) and larger reads/writes are split.
6. `drivers::virtio::blk`: `pub fn init()` (called at boot after PCI, logs
   `[virtio-blk] vda: <sectors> sectors (<MiB> MiB), features 0x<negotiated>`), `pub fn vda() ->
   Option<&'static VirtioBlk>`, and on `VirtioBlk` (Sync, internally locked with an IrqMutex):
   `capacity()`, `read(lba, &mut [u8])`, `write(lba, &[u8])`, `flush()`, `requests()` (device
   requests submitted so far). `BlkError { InvalidLength, OutOfRange, Io(u8), Timeout }` with Debug,
   PartialEq. Lengths must be a non-zero multiple of 512; any sector past the end is OutOfRange
   (checked before anything is submitted). Negotiate VERSION_1 plus FLUSH if offered. Completion:
   after `should_notify()` → `notify`, poll `pop_used()` until the token returns (timeout 5 s via
   `time::uptime_ms()` → Timeout); check the status byte (0 OK, else Io(status)).
7. `scripts/mkdisk.py` writes 131,079 sectors (64 MiB + 7, same pattern), and the GNUmakefile rule
   `build/data.img` depends on scripts/mkdisk.py so the image is rebuilt when the script changes.
8. Add `mod virtio_blk;` to kernel/src/test_cases/mod.rs. The existing pci tests keep passing.

## Acceptance
- `source scripts/env.sh && gmake check` → every target PASS, including the six virtio_blk tests,
  and serial shows the `[virtio-blk] vda:` line and the `seq read ... MiB/s` line.
- `gmake lint` clean; every `unsafe` block has a `// SAFETY:` comment.

## Report
<=15 lines: files, acceptance results with exit codes, the MiB/s line, deviations. Do not commit.
