//! Byte-flip fuzzing (brief M5-T0c): every parser must return a value or an error on mutated
//! input, never panic or hang. Each fixture is mutated 1000 times with 1-4 random bit flips.

use otter_net_proto::*;
use std::path::Path;

fn fixtures() -> Vec<Vec<u8>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("fixtures directory") {
        let path = entry.expect("fixture entry").path();
        if path.extension().is_some_and(|e| e == "bin") {
            out.push(std::fs::read(&path).expect("read fixture"));
        }
    }
    assert!(out.len() >= 9, "expected the generated fixtures, found {}", out.len());
    out
}

fn parse_all(bytes: &[u8]) {
    let _ = EthernetFrame::new(bytes);
    let _ = ArpPacket::new(bytes);
    let _ = Ipv4Header::new(bytes);
    let _ = IcmpEchoRequest::new(bytes);
    let _ = UdpPacket::new(bytes);
    let _ = DhcpPacket::new(bytes);
    let _ = DnsHeader::new(bytes);
    // DNS names start after the 12-byte header; also try misaligned offsets.
    for offset in [0usize, 12, 13, 20] {
        if let Ok(query) = DnsQuery::new(bytes, offset) {
            let _ = query.parse_name();
        }
    }
}

#[test]
fn byte_flips_never_panic() {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut cases = 0usize;
    for original in fixtures() {
        for _ in 0..1000 {
            let mut data = original.clone();
            let flips = 1 + (next() % 4) as usize;
            for _ in 0..flips {
                let index = (next() as usize) % data.len();
                data[index] ^= 1u8 << (next() % 8);
            }
            parse_all(&data);
            cases += 1;
        }
    }
    assert!(cases >= 9000, "ran only {cases} mutated inputs");
}

#[test]
fn every_prefix_of_every_fixture_is_handled() {
    for original in fixtures() {
        for len in 0..=original.len() {
            parse_all(&original[..len]);
        }
    }
}

/// Regression (M5-T1c): a 1-byte UDP datagram arrives in a 60-byte Ethernet frame; the 17 bytes of
/// padding after it are not payload. Payloads end at the IPv4 Total Length and the UDP Length.
#[test]
fn ethernet_padding_is_not_payload() {
    use otter_net_proto::{Ipv4Header, UdpPacket};
    let mut ip = vec![0x45, 0, 0, 29, 0, 0, 0x40, 0, 64, 17, 0, 0, 10, 0, 2, 2, 10, 0, 2, 15];
    ip.extend_from_slice(&[0xC3, 0x57, 0xC0, 0x00, 0, 9, 0, 0, 0x2A]); // UDP 50007 -> 49152, 1 byte
    ip.extend_from_slice(&[0u8; 17]); // padding up to a 60-byte frame (14 + 46)
    let hdr = Ipv4Header::new(&ip).expect("valid IPv4");
    assert_eq!(hdr.payload().len(), 9, "IPv4 payload ends at Total Length");
    let udp = UdpPacket::new(hdr.payload()).expect("valid UDP");
    assert_eq!(udp.payload(), &[0x2A], "UDP payload ends at Length");
    let mut short_total = ip.clone();
    short_total[3] = 19;
    assert!(Ipv4Header::new(&short_total).is_err(), "Total Length below the header length");
    let mut long_total = ip.clone();
    long_total[2] = 0x05;
    assert!(Ipv4Header::new(&long_total).is_err(), "Total Length beyond the frame");
    assert!(UdpPacket::new(&[0xC3, 0x57, 0xC0, 0x00, 0, 7, 0, 0]).is_err(), "UDP Length below 8");
    assert!(UdpPacket::new(&[0xC3, 0x57, 0xC0, 0x00, 0, 20, 0, 0, 1]).is_err(), "UDP Length beyond the data");
}

/// M5-T1d: UDP checksum verification (RFC 768).
#[test]
fn udp_checksum_verify() {
    use otter_net_proto::{Ipv4Addr, UdpBuilder, UdpPacket};

    let src = Ipv4Addr::new(10, 0, 2, 2);
    let dst = Ipv4Addr::new(10, 0, 2, 15);

    // Build a valid UDP packet with computed checksum
    let payload = b"Hello, UDP checksum!";
    let udp_bytes = UdpBuilder::new(50007, 49152)
        .with_payload(payload)
        .build_with_pseudo_header(src, dst);
    let udp = UdpPacket::new(&udp_bytes).expect("valid UDP");

    // Verify the builder's output always verifies
    assert!(udp.verify_checksum(src, dst), "builder's output verifies");

    // Checksum 0 means "no checksum computed" and always passes
    let mut no_cs = udp_bytes.clone();
    no_cs[6..8].copy_from_slice(&[0, 0]);
    let udp_no_cs = UdpPacket::new(&no_cs).expect("valid UDP with zero checksum");
    assert!(udp_no_cs.verify_checksum(src, dst), "checksum 0 always passes");

    // A flipped bit in the payload should fail
    let mut bad_payload = udp_bytes.clone();
    if bad_payload.len() > 8 {
        bad_payload[8] ^= 1;
        let udp_bad = UdpPacket::new(&bad_payload).expect("valid UDP structure");
        assert!(!udp_bad.verify_checksum(src, dst), "flipped payload bit fails");
    }

    // Test 1,000 random payloads: all should verify after being built
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut rng = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    };

    for _ in 0..1000 {
        let payload_len = (rng() as usize) % 1400;
        let mut payload = vec![0u8; payload_len];
        for p in &mut payload {
            *p = rng();
        }

        let udp_bytes = UdpBuilder::new(50007, 49152)
            .with_payload(&payload)
            .build_with_pseudo_header(src, dst);
        let udp = UdpPacket::new(&udp_bytes).expect("valid UDP");
        assert!(udp.verify_checksum(src, dst), "random payload {} verifies", payload_len);
    }
}

/// Regression (M5-T1d review): when the computed UDP checksum is 0x0000 the sender transmits
/// 0xFFFF (RFC 768); that datagram is valid and must verify.
#[test]
fn udp_checksum_zero_is_sent_as_ffff_and_verifies() {
    use otter_net_proto::{checksum, Ipv4Addr, UdpPacket};
    let (src, dst) = (Ipv4Addr::new(10, 0, 2, 2), Ipv4Addr::new(10, 0, 2, 15));
    let sum_for = |payload: [u8; 2]| {
        let mut d = vec![10, 0, 2, 2, 10, 0, 2, 15, 0, 17, 0, 10];
        d.extend_from_slice(&[0xC3, 0x57, 0xC0, 0x00, 0, 10, 0, 0]);
        d.extend_from_slice(&payload);
        checksum::checksum(&d)
    };
    let payload = (0..=u16::MAX).map(|v| v.to_be_bytes()).find(|p| sum_for(*p) == 0).expect("some payload sums to zero");
    let mut datagram = vec![0xC3, 0x57, 0xC0, 0x00, 0, 10, 0xFF, 0xFF];
    datagram.extend_from_slice(&payload);
    let udp = UdpPacket::new(&datagram).unwrap();
    assert!(udp.verify_checksum(src, dst), "0xFFFF stands for a computed 0");
    datagram[8] ^= 1;
    assert!(!UdpPacket::new(&datagram).unwrap().verify_checksum(src, dst), "and a flipped bit still fails");
}
