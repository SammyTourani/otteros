# Brief M3-T1c — `otter-virtio`: split virtqueues and the virtio 1.x modern transport as a pure crate

## Goal
The part of every virtio driver that is easy to get subtly wrong — descriptor rings, free lists,
index wraparound, memory barriers, notification suppression and device initialisation — built and
proven on the Mac, so the kernel's virtio-blk (M3-T1b) and virtio-net (M5) drivers only map DMA
memory and PCI registers onto it. The existing kernel/src/drivers/virtio/queue.rs (from M3-T1a) will
be replaced by this crate in M3-T1b; do not touch kernel files in this task.

## Applies
D2, D27 (no_std + alloc, zero external dependencies). `unsafe` is allowed only for volatile/atomic
access to ring memory given as a raw region, each block with a `// SAFETY:` comment.
Spec: OASIS virtio 1.2 (or 1.1) §2.7 split virtqueues, §2.7.7 used-buffer notification suppression
and §2.7.10 available-buffer suppression (EVENT_IDX), §3.1 device initialisation, §4.1.4 PCI
capabilities and common configuration layout, §5.2 block device requests, §5.1.6 net header.

## Design (crates/otter-virtio)
1. `RingMemory`: a caller-provided region (virtual pointer + the device-visible physical address +
   length) holding the descriptor table, available ring and used ring with the spec's alignment
   (16 / 2 / 4) and sizes; `SplitQueue::layout(queue_size)` returns the byte offsets and total size
   for queue sizes 1..=32768 (powers of two).
2. `SplitQueue` (driver side): `add(&[Buffer{addr, len, device_writes}], token) -> Result<head>`
   builds a descriptor chain from a free list (NEXT/WRITE flags); publishes into the available ring
   with a release fence before the index store; `should_notify()` implements both the
   NO_NOTIFY flag and EVENT_IDX (`vring_need_event` with u16 wraparound); `pop_used() ->
   Option<(token, written_len)>` reads the used index with acquire ordering, returns each chain's
   descriptors to the free list and never returns a token twice; `set_used_event()` for interrupt
   suppression; capacity and "queue full" handling; tokens are opaque `u64`s.
3. `Transport` trait for the modern PCI common-configuration registers (device/driver feature
   select + value, num_queues, device_status, queue_select/size/msix_vector/enable/notify_off,
   queue desc/driver/device addresses, config_generation, config_msix_vector) and notify writes.
   `init_device(transport, wanted_features) -> Result<negotiated>` does the §3.1 sequence:
   reset → ACKNOWLEDGE → DRIVER → read features → write the subset (VERSION_1 required) →
   FEATURES_OK → re-read FEATURES_OK (error if the device cleared it) → queue setup (size capped at
   the device maximum, addresses written, MSI-X vector written and read back: 0xFFFF = failure) →
   DRIVER_OK; any error sets FAILED. Device config reads use the config_generation retry loop.
4. Helpers: virtio-blk request header (type IN/OUT/FLUSH/GET_ID, sector), status byte parsing
   (OK/IOERR/UNSUPP); virtio-net header (12 bytes, num_buffers) encode/decode.

## Tests (host; `cargo test -p otter-virtio`; ≥ 30 tests)
- Layout: offsets, alignment and total size for every power-of-two queue size, checked against the
  spec formulas written independently in the test.
- Deterministic device model (same thread): 200,000 seeded random operations (add chains of 1–4
   buffers, device consumes some available entries in order, completes them in random order with
   random written lengths, driver pops): every token comes back exactly once with its length,
   no descriptor is reused while in flight, the free list is whole at the end, indices wrap past
   65535 many times (use queue sizes 1, 2, 8, 256 and 32768).
- Concurrent device model (the important one): the device runs on a second OS thread over the
  same ring memory, using only the spec's ordering (acquire load of avail idx, release store of
  used idx), echoing request bytes into device-writable buffers; the driver thread submits and
  pops 1,000,000 requests and checks every payload checksum. The Mac's ARM64 CPU is weakly
  ordered, so a missing fence shows up here as corrupted data or a lost token; the test must pass
  10 runs in a row.
- EVENT_IDX: `vring_need_event` truth table incl. wraparound; notification suppressed/required
  exactly as specified; used_event honoured by the device model.
- Transport: a mock register file implementing the device side of §3.1 and §4.1.4.3; successful
  negotiation; device rejecting features (clears FEATURES_OK → error and FAILED set); VERSION_1
  missing → error; MSI-X vector readback 0xFFFF → error; queue size larger than the device maximum
  is capped; config_generation change during a read → retried.
- virtio-blk and net header encode/decode round trips against hand-written byte fixtures.

## Acceptance
`scripts/verify-crate.sh otter-virtio` → ≥ 30 tests passed, 0 failed, clippy ok, no_std ok,
external deps 0; the concurrent test passes 10 consecutive runs
(`cargo test -p otter-virtio --release concurrent` in a loop).

## Report
<=10 lines: the verify-crate line, ops and requests exercised, the 10-run result, deviations.
Do not commit.
