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
