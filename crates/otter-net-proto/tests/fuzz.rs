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
