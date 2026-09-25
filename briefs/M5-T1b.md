# Brief M5-T1b — UDP sockets, DHCP and DNS in the kernel

## Goal
OtterOS configures itself on a network and talks to it: DHCP (the host-tested state machine in
`otter-netlogic`) obtains the address at boot, kernel UDP sockets send and receive datagrams, and
the DNS resolver (also `otter-netlogic`) turns names into addresses. Acceptance tests:
kernel/src/test_cases/net_udp.rs (orchestrator, unchanged) plus the M5-T1a tests in net.rs.

## Applies
D2 (path dependencies on the repo's `otter-net-proto` and `otter-netlogic`), D27. Every `unsafe`
block gets a `// SAFETY:` comment. Build on kernel/src/net and kernel/src/drivers/virtio/net.rs
(M5-T1a). Use `otter-netlogic`'s DHCP client and DNS resolver state machines; do not re-implement
them (if their API is insufficient, report what is missing instead of copying logic).

## Design
1. DHCP at boot: `net::init()` runs the DHCP client (DISCOVER → OFFER → REQUEST → ACK) over UDP
   68/67 broadcast, with the client's own retransmission timers driven by the `net-rx` thread's
   clock, and configures the interface from the ACK (address, router, DNS, lease time). Log
   `[net] dhcp: 10.0.2.15/24 router 10.0.2.2 dns 10.0.2.3 lease <n> s`. Renewal timers keep running
   (renew at T1). If no lease arrives within 10 s, fall back to 10.0.2.15/24 via 10.0.2.2 and log it.
   `NetIf::dhcp_lease() -> Option<Lease { address, router, dns, lease_secs }>`.
2. UDP: `UdpSocket::bind(port)` (0 = an ephemeral port from 49152 up; AddressInUse when taken),
   `local_port()`, `send_to(data, ip, port)` (ARP-resolves the next hop; TooLarge above 1472 bytes;
   checksum over the pseudo-header), `recv_from(buf, timeout_ms) -> (len, ip, port)` (a bounded queue
   of 64 datagrams per socket; Timeout when nothing arrives), unbinding on drop. The `net-rx` thread
   demultiplexes IPv4/UDP by destination port; datagrams for unbound ports are counted and dropped.
3. DNS: `net::resolve_via(name, server_ip, server_port, timeout_ms) -> Result<[u8; 4], NetError>`
   with otter-netlogic's resolver (query id from `random::fill`, retransmission, CNAME following,
   NXDOMAIN → NotFound), and `net::resolve(name, timeout_ms)` using the lease's DNS server.
4. `NetError` gains `AddressInUse` and `NotFound` (Debug, PartialEq as before).
5. scripts/qemu.py: for every boot target, before starting QEMU, start two host services on
   127.0.0.1 in background threads and stop them afterwards: a UDP echo server on port 50007 (echo
   each datagram to its sender) and a DNS responder on port 5353 answering `www.otter.test` A with a
   CNAME to `otter.test` plus `otter.test` A 192.0.2.7, `otter.test` A 192.0.2.7, and NXDOMAIN (rcode 3)
   for any other name, all with the query id and question echoed (hand-written with `struct`; no
   third-party Python packages).
6. Add `mod net_udp;` to kernel/src/test_cases/mod.rs.

## Acceptance
- `source scripts/env.sh && gmake check` → every target PASS, including the three net_udp tests and
  the three net tests; serial shows the `[net] dhcp:` line.
- `gmake lint` clean.

## Report
<=15 lines: files, the [check] table, the [net] lines, deviations. Do not commit.
