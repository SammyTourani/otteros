//! UDP checksum stats verification (brief M5-T1d): a UDP echo round trip should not
//! increment udp_bad_checksum (QEMU never corrupts a datagram).

use alloc::vec::Vec;
use otteros_kernel::net::{self, UdpSocket};

const HOST: [u8; 4] = [10, 0, 2, 2];
const ECHO_PORT: u16 = 50007;

#[test_case]
fn net_stats_udp_checksum() {
    let nic = net::interface().expect("virtio-net was probed at boot");

    // Do a UDP echo round trip (100 bytes)
    let sock = UdpSocket::bind(0).expect("bind an ephemeral port");
    let payload: Vec<u8> = (0..100).map(|i| (i * 7 + 100) as u8).collect();
    sock.send_to(&payload, HOST, ECHO_PORT).expect("send");
    let mut buf = [0u8; 2048];
    let (n, from, port) = sock.recv_from(&mut buf, 3000).expect("the echo arrives within 3 s");
    assert_eq!((from, port), (HOST, ECHO_PORT), "reply source");
    assert_eq!(&buf[..n], &payload[..], "echo of 100 bytes");

    // Verify that no bad checksums were recorded
    let stats = nic.stats();
    assert_eq!(stats.udp_bad_checksum, 0, "no UDP datagrams had bad checksums");
}
