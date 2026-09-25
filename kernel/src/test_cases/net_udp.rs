//! UDP, DHCP and DNS acceptance tests for brief M5-T1b, written by the orchestrator: the
//! implementation must pass these unchanged. QEMU user networking maps 10.0.2.2:<port> to the Mac's
//! 127.0.0.1:<port>, where scripts/qemu.py runs a UDP echo server (port 50007) and a small DNS
//! responder (port 5353: www.otter.test is a CNAME for otter.test, which has A 192.0.2.7; anything
//! else is NXDOMAIN) for the duration of the boot.

use alloc::vec::Vec;
use otteros_kernel::net::{self, NetError, UdpSocket};

const HOST: [u8; 4] = [10, 0, 2, 2];
const ECHO_PORT: u16 = 50007;
const DNS_PORT: u16 = 5353;

#[test_case]
fn net_dhcp_lease_from_qemu() {
    let nic = net::interface().expect("virtio-net was probed at boot");
    let lease = nic.dhcp_lease().expect("a DHCP lease was obtained at boot");
    assert_eq!(lease.address, [10, 0, 2, 15]);
    assert_eq!(lease.router, [10, 0, 2, 2]);
    assert_eq!(lease.dns, [10, 0, 2, 3]);
    assert!(lease.lease_secs > 0);
    assert_eq!(nic.ipv4(), lease.address, "the interface is configured from the lease");
}

#[test_case]
fn net_udp_echo_through_the_host() {
    let sock = UdpSocket::bind(0).expect("bind an ephemeral port");
    assert!(sock.local_port() >= 1024);
    for size in [1usize, 100, 512, 1472] {
        let payload: Vec<u8> = (0..size).map(|i| (i * 7 + size) as u8).collect();
        sock.send_to(&payload, HOST, ECHO_PORT).expect("send");
        let mut buf = [0u8; 2048];
        let (n, from, port) = sock.recv_from(&mut buf, 3000).expect("the echo arrives within 3 s");
        assert_eq!((from, port), (HOST, ECHO_PORT), "reply source");
        assert!(buf[..n] == payload[..], "echo of {size} bytes");
    }
    assert_eq!(sock.send_to(&[0u8; 1473], HOST, ECHO_PORT), Err(NetError::TooLarge), "one datagram per 1500-byte packet");
    let mut buf = [0u8; 16];
    assert_eq!(sock.recv_from(&mut buf, 300), Err(NetError::Timeout), "nothing else arrives");
    let other = UdpSocket::bind(sock.local_port());
    assert!(matches!(other, Err(NetError::AddressInUse)), "a port is bound once");
}

#[test_case]
fn net_dns_through_the_host() {
    assert_eq!(net::resolve_via("www.otter.test", HOST, DNS_PORT, 3000), Ok([192, 0, 2, 7]), "CNAME followed to the A record");
    assert_eq!(net::resolve_via("missing.otter.test", HOST, DNS_PORT, 3000), Err(NetError::NotFound));
}
