# Brief M5-T1c — a real DNS lookup, and the M5-T1b network fixes

## Goal
M5-T1b's gate fails only at `net_dns_through_the_host` (`Err(Timeout)`): otter-netlogic's
`DnsResolver` builds whole Ethernet frames with zero MACs and a zero source address, always targets
port 53, never sends the CNAME follow-up and never retransmits, and the kernel never feeds it the
replies. Replace it with a sans-I/O lookup defined by an oracle, use it over the kernel's UDP
sockets, and fix the other M5-T1b defects listed below. Acceptance tests (orchestrator, unchanged):
`crates/otter-netlogic/tests/dns_oracle.rs`, `kernel/src/test_cases/net_udp.rs`, `net.rs`.

## Applies
D2, D27 (otter-netlogic stays no_std + alloc, depends only on otter-net-proto). Every `unsafe` block
gets a `// SAFETY:` comment. Never run `git checkout/restore/reset/stash/clean`. Host tests run only
through `scripts/memguard.py` (scripts/verify-crate.sh already does). Do not edit the oracle files.

## Part 1 — `otter_netlogic::dns` (crate)
1. New module `crates/otter-netlogic/src/dns.rs`, exported as `pub mod dns`, implementing exactly
   the API and semantics in the header of `tests/dns_oracle.rs` (`Lookup::start`, `on_response`,
   `poll`, `Step`, `LookupError`). The lookup works on DNS messages (UDP payloads), never frames.
   Parse with bounds checks everywhere: a name decoder that follows compression pointers with a hop
   limit (or only strictly backward pointers) and rejects labels over 63 bytes and names over 255
   bytes; any parse failure means the message is ignored.
2. Delete `DnsResolver`, its `PendingQuery`/cache types and its unit tests from `src/lib.rs`.
   `ArpCache` and `DhcpClient` stay as they are.
3. `scripts/verify-crate.sh otter-netlogic` passes (tests incl. the oracle, clippy native and
   x86_64-apple-darwin, no_std build, no external dependencies).

## Part 2 — kernel (`kernel/src/net/mod.rs`; split into `net/udp.rs`, `net/dns.rs` etc. if it grows past ~600 lines)
1. `resolve_via(name, server_ip, server_port, timeout_ms)`: `Lookup::start` (ids from
   `random::fill`), bind `UdpSocket::bind(0)`, `send_to` the query to `server_ip:server_port`, then
   loop on the step: `Wait { until_ms }` → `recv_from` with the remaining time; a datagram from
   exactly `(server_ip, server_port)` goes to `on_response`, others are ignored; a receive timeout
   goes to `poll(now)`; `Send(q)` → `send_to`; `Done` → map `NotFound` and `CnameLoop` to
   `NetError::NotFound`, `Timeout` to `NetError::Timeout`, `InvalidName` to a new
   `NetError::InvalidName`, `ServerFailure` to a new `NetError::ServerFailure`.
   `resolve(name, timeout_ms)` = `resolve_via` with the lease's DNS server and port 53.
   Remove the `DNS_RESOLVER` static and the `src_port == 53` interception in `handle_udp` (DNS
   replies are ordinary datagrams for the lookup's socket).
2. Configure the interface from the lease: on `Configured`, set the address, netmask, router and
   DNS server (interior-mutable fields, e.g. atomics or a small Mutex); `NetIf::ipv4()`,
   `gateway()`, ARP replies, ICMP and UDP use the current values. Before a lease (and after the
   fallback) they are 10.0.2.15/24 via 10.0.2.2, DNS 10.0.2.3.
3. The 10 s DHCP fallback prints its line once, keeps the DHCP client running, and a later lease
   still configures the interface (log the `[net] dhcp:` line then).
4. Next hop: a destination inside the interface's subnet is ARP-resolved directly; anything else
   goes to the router; 255.255.255.255 uses the broadcast MAC without ARP.
5. Ephemeral ports: 49152..=65535, wrapping within that range; AddressInUse only when all are taken.
6. Received UDP: verify the checksum when it is non-zero (pseudo-header), drop mismatches; drop
   datagrams whose destination IP is neither ours nor broadcast. Count unbound-port and
   bad-checksum drops in `NetStats` (`udp_no_port`, `udp_bad_checksum`).
7. Replace `static mut NETIF_PTR` with a set-once `AtomicPtr<NetIf>` (the `NetIf` leaked from a
   `Box` at init; `interface()` loads it with Acquire). No `static mut` remains in `net/`.

## Acceptance
- `scripts/verify-crate.sh otter-netlogic` → exit 0; `cargo test -p otter-netlogic --test dns_oracle`
  shows 12 passed.
- `source scripts/env.sh && gmake check` → all 13 targets PASS; serial shows the `[net] dhcp:` line
  once per boot and no fallback line.
- `grep -n "static mut" kernel/src/net` → nothing.

## Report
<=15 lines: files, the [check] table, the oracle test count, deviations. Do not commit.
