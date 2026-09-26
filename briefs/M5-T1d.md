# Brief M5-T1d — network module split, received UDP checksums, per-test durations

## Goal
Three leftovers from M5-T1b/c, all small:
1. `kernel/src/net/mod.rs` is 978 lines; split it so no file under `kernel/src/net/` exceeds 600.
2. Received UDP checksums are not verified and `NetStats::udp_bad_checksum` is never incremented.
3. The kernel suite takes ~130 s under TCG and nobody knows which tests are slow: the in-kernel
   runner prints each test's duration.

## Applies
D2 (otter-net-proto is a path dependency), CLAUDE.md conventions (`// SAFETY:` on every `unsafe`,
zero warnings, module log prefixes). Never run git checkout/restore/reset/stash/clean. Do not edit
anything in `kernel/src/test_cases/` except where item 2 says so.

## Design
1. Split by concern, no behaviour change: `net/mod.rs` (NetIf, init, interface(), NetError,
   NetStats, re-exports), `net/rx.rs` (the net-rx thread, frame dispatch, ARP/ICMP handling),
   `net/udp.rs` (UdpSocket, the socket table, handle_udp), `net/dhcp.rs` (driving the DHCP
   client, applying a lease), `net/dns.rs` (resolve_via, resolve). Public paths stay the same
   (`net::UdpSocket`, `net::resolve_via`, ...).
2. In `otter-net-proto`, add `UdpPacket::verify_checksum(&self, src: Ipv4Addr, dst: Ipv4Addr) ->
   bool` (RFC 768: one's-complement sum over the pseudo-header — src, dst, zero, protocol 17, UDP
   length — and the whole datagram; a transmitted checksum of 0 means "none" and verifies; a
   computed 0 is sent as 0xFFFF) with host tests in `crates/otter-net-proto/tests/fuzz.rs`: a
   captured datagram verifies, one flipped payload bit fails, checksum 0 passes, the builder's
   output always verifies (1,000 random payloads). The kernel's handle_udp drops datagrams that
   fail and increments `udp_bad_checksum`; unbound ports increment `udp_no_port` (already).
   Add one kernel test in a new file `kernel/src/test_cases/net_stats.rs` (registered in
   mod.rs; tests run alphabetically, so it cannot rely on other tests having run): it does its own
   UDP echo round trip (100 bytes to 10.0.2.2:50007, as net_udp.rs does) and then asserts
   `stats().udp_bad_checksum == 0` (QEMU never corrupts a datagram, so a nonzero count means the
   verifier is wrong).
3. `kernel/src/tests.rs`: after each passing test print `ok (N ms)` instead of `ok`, using the
   uptime clock (tests that run before the clock exists print `ok`). The `[ok] tests passed (N)`
   line is unchanged (the harness matches it). Then list the ten slowest tests of one `gmake test`
   run in your report.

## Acceptance
- `scripts/verify-crate.sh otter-net-proto` → exit 0.
- `source scripts/env.sh && gmake check` → 13/13 PASS.
- `wc -l kernel/src/net/*.rs` → every file <= 600.

## Report
<=15 lines: files, the [check] table, the ten slowest tests with durations, deviations. Do not commit.
