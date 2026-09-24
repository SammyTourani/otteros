# Brief M5-T0c — `otter-net-proto`: packet formats for Ethernet, ARP, IPv4, ICMP, UDP, DHCP, DNS (pure crate)

## Goal
Byte-exact encoders and decoders for every packet the OtterOS network stack (M5) sends or receives,
proven on the Mac, so the kernel stack later only has to move bytes and run state machines.

## Applies
D2, D8, D27 (pure no_std + alloc, zero external dependencies).

## Scope and boundaries
New crate `crates/otter-net-proto/`, `scripts/gen-net-fixtures.py`, fixtures under its tests/.
Touch nothing else. Do not run gmake or QEMU.

## Design
Zero-copy views over `&[u8]` with checked accessors, plus builders writing into `&mut [u8]` or `Vec`:
- Ethernet II (dst, src, ethertype; 802.1Q tag skipped if present), MAC address type and formatting.
- ARP for IPv4 over Ethernet (request/reply builders).
- IPv4: header parsing with IHL/options, total length, fragmentation fields (flag unsupported
  fragments), TTL, protocol, the ones'-complement header checksum (verify and compute).
- ICMP echo request/reply with checksum; destination-unreachable parsing.
- UDP with the IPv4 pseudo-header checksum (compute and verify; zero means "no checksum").
- DHCP (RFC 2131/2132): BOOTP fixed header, magic cookie, options (message type, requested IP,
  server id, lease time, subnet mask, router, DNS servers, end/pad), DISCOVER/REQUEST builders,
  OFFER/ACK parsing.
- DNS (RFC 1035): header, question and answer sections, A/AAAA/CNAME records, name compression on
  parse (pointer loops rejected, label and name length limits), query builder with random id input.
Malformed input always returns an error value; nothing panics.

## Tests (host)
`scripts/gen-net-fixtures.py` (Python stdlib: struct, socket.inet_aton) writes byte fixtures with
known field values, including checksums computed by Python. Tests: round trips for every builder;
parse every fixture and check fields; checksum verification including odd-length payloads; a real
DNS response captured from this Mac (`dig example.com A +noedns` bytes via Python's socket to
1.1.1.1 or the system resolver, saved once as a fixture) parsed correctly with compression; RFC 1071
checksum examples; fuzz: truncating each fixture at every offset and flipping 1000 random bytes never
panics.

## Acceptance
`scripts/verify-crate.sh otter-net-proto` shows 0 failed, 0 ignored, clippy ok, no_std ok, 0 external deps; at least 40 tests.

## Report
<=8 lines. Do not commit.
