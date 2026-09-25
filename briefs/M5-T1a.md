# Brief M5-T1a — virtio-net, Ethernet, ARP and ICMP in the kernel

## Goal
OtterOS puts real frames on a network: a virtio-net driver built on `otter-virtio` (like the
virtio-blk driver of M3-T1b), a receive path that answers ARP for our address, an ARP cache, and
ICMP echo so `ping` of the QEMU gateway works. The orchestrator's acceptance tests are
kernel/src/test_cases/net.rs (must pass unchanged). UDP, DHCP and DNS are M5-T1b; TCP comes with
otter-tcp later.

## Applies
D2 (path dependencies on the repo's own `otter-virtio` and `otter-net-proto`), D15, D27. Every
`unsafe` block gets a `// SAFETY:` comment. Reuse the virtio-blk plumbing (mm::mmio, the PCI
vendor-capability walk and the Transport over the common configuration): move the shared parts into
`kernel/src/drivers/virtio/pci.rs` (or similar) so both drivers use one implementation.

## Design
1. QEMU (scripts/qemu.py, every boot target): `-netdev user,id=n0 -device
   virtio-net-pci,netdev=n0,disable-legacy=on,mac=52:54:00:4f:54:52`.
2. `kernel/src/drivers/virtio/net.rs`: device 1af4:1041. Negotiate VERSION_1 + VIRTIO_NET_F_MAC (bit
   5) only (no offloads, no MRG_RXBUF: the virtio-net header is 12 bytes and every RX buffer must hold
   a full frame, so use 2 KiB buffers). Queue 0 = receive, queue 1 = transmit. Read the MAC from the
   device configuration (offset 0, 6 bytes). Post 64 RX buffers (physically contiguous DMA memory
   from the PMM, HHDM alias); repost each buffer after its frame is consumed. TX through a DMA bounce
   area (header zeroed + frame), serialized by a lock; reclaim TX descriptors as they complete.
3. `kernel/src/net/mod.rs`: `pub fn init()` (after PCI; log `[net] virtio-net <mac>, 10.0.2.15/24 gw
   10.0.2.2`), `pub fn interface() -> Option<&'static NetIf>`, a kernel thread `net-rx` that polls the
   receive queue every millisecond (sleep between polls) and dispatches frames with `otter-net-proto`:
   ARP requests for 10.0.2.15 get a reply; ARP replies fill the cache; ICMP echo requests to us get a
   reply; ICMP echo replies wake the matching `ping` waiter (a small table keyed by id/seq); anything
   else (IPv6, other ethertypes) is counted and dropped. Static configuration in this task: address
   10.0.2.15/24, gateway 10.0.2.2.
4. `NetIf` (Sync): `mac() -> [u8; 6]`, `ipv4() -> [u8; 4]`, `gateway() -> [u8; 4]`,
   `arp_resolve(ip, timeout_ms) -> Result<[u8; 6], NetError>` (cache first; otherwise broadcast a
   request, retry every 500 ms, wait for the reply), `ping(ip, seq, payload, timeout_ms) ->
   Result<u64 /*rtt ms*/, NetError>` (resolves the gateway's MAC for off-link or on-link targets,
   sends an echo request with id = a per-boot constant, waits for the matching reply), `stats() ->
   NetStats { rx_frames, tx_frames, rx_dropped, arp_requests_sent, arp_replies_sent,
   icmp_echo_replies_received }`. `NetError { Timeout, TooLarge, NoInterface }` with Debug, PartialEq;
   a payload that cannot fit one 1500-byte IP packet is TooLarge.
5. Add `mod net;` to kernel/src/test_cases/mod.rs. The virtio_blk tests keep passing.

## Acceptance
- `source scripts/env.sh && gmake check` → every target PASS (including test-max), with the three net
  tests; serial shows the `[net] virtio-net` line and five `[net] ping 10.0.2.2 seq n rtt x ms` lines.
- `gmake lint` clean.

## Report
<=15 lines: files, the [check] table, the [net] lines, deviations. Do not commit.
