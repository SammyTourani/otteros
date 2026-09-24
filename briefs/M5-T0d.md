# Brief M5-T0d — `otter-netlogic`: ARP cache, DHCP client and DNS resolver state machines (pure crate)

## Goal
The protocol logic above the packet formats, written sans-I/O with an explicit clock so it is
tested deterministically on the Mac and later driven by the kernel's network stack unchanged.

## Applies
D2, D8, D27. Uses `otter-net-proto` (path dependency) for all packet encoding and decoding.

## Scope and boundaries
New crate `crates/otter-netlogic/`. Touch nothing else. Do not run gmake or QEMU.

## Design (every component: `handle_packet(now_ms, bytes) -> Vec<Action>`, `tick(now_ms) -> Vec<Action>`,
where Action is Send(frame bytes) / Configured / Resolved / Failed; no clocks, sockets or threads inside)
1. ARP cache: entries with 60 s expiry, pending requests queued per IP (max 8 packets), request
   retransmit every 1 s up to 3 times then Failed; answers requests for our own IP.
2. DHCP client (RFC 2131): INIT → SELECTING (DISCOVER, retransmit with 4/8/16 s backoff) → REQUESTING
   (REQUEST for the first OFFER, retransmit) → BOUND (Configured with ip, mask, router, dns, lease)
   → RENEWING at T1 = lease/2 → REBINDING at T2 = 7/8 lease → INIT on expiry; NAK → INIT; ignores
   packets with the wrong xid or client MAC.
3. DNS resolver (RFC 1035 subset): A queries over UDP to the configured server, random query id
   from a caller-supplied RNG, retransmit after 2 s up to 3 times, CNAME chains followed (max 8),
   positive cache honouring the minimum TTL (capped at 1 h), NXDOMAIN reported as Failed, responses
   with a mismatched id or question ignored.

## Tests (host, simulated time)
Each state machine driven through scripted exchanges built with otter-net-proto: happy paths,
every retransmit and timeout path, lease renewal and rebinding, NAK, wrong-xid/wrong-id packets
ignored, CNAME chain, cache hits and expiry, ARP queue overflow. At least 40 tests.

## Acceptance
`scripts/verify-crate.sh otter-netlogic` shows 0 failed, 0 ignored, clippy ok, no_std ok, 0 external deps.

## Report
<=6 lines including that line. Do not commit.
