//! Network acceptance tests for brief M5-T1a, written by the orchestrator: the implementation must
//! pass these unchanged. QEMU user networking (slirp) puts the guest at 10.0.2.15 with the gateway
//! (and the host) at 10.0.2.2; scripts/qemu.py gives the virtio-net card the MAC below, so a driver
//! that does not read the MAC from the device cannot pass.

use otteros_kernel::net::{self, NetError};
use otteros_kernel::kprintln;

const OUR_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x4f, 0x54, 0x52];
const OUR_IP: [u8; 4] = [10, 0, 2, 15];
const GATEWAY: [u8; 4] = [10, 0, 2, 2];

fn nic() -> &'static net::NetIf {
    net::interface().expect("virtio-net was probed at boot")
}

#[test_case]
fn net_mac_comes_from_the_device() {
    assert_eq!(nic().mac(), OUR_MAC, "MAC read from the virtio-net device configuration");
    assert_eq!(nic().ipv4(), OUR_IP);
    assert_eq!(nic().gateway(), GATEWAY);
}

#[test_case]
fn net_arp_resolves_the_gateway_and_caches_it() {
    let first = nic().arp_resolve(GATEWAY, 3000).expect("ARP reply from the QEMU gateway");
    assert!(first != [0; 6] && first[0] & 1 == 0, "a unicast MAC: {first:02x?}");
    let sent = nic().stats().arp_requests_sent;
    assert!(sent >= 1);
    let again = nic().arp_resolve(GATEWAY, 3000).expect("cached");
    assert_eq!(first, again);
    assert_eq!(nic().stats().arp_requests_sent, sent, "the second lookup is answered from the cache");
}

#[test_case]
fn net_ping_the_gateway() {
    for seq in 1..=5u16 {
        let payload = [seq as u8; 56];
        let rtt = nic().ping(GATEWAY, seq, &payload, 5000).expect("ICMP echo reply from 10.0.2.2");
        kprintln!("[net] ping 10.0.2.2 seq {} rtt {} ms", seq, rtt);
    }
    let s = nic().stats();
    assert!(s.tx_frames >= 6 && s.rx_frames >= 6, "frames really went through the card: {} tx, {} rx", s.tx_frames, s.rx_frames);
    assert_eq!(nic().ping(GATEWAY, 99, &[0u8; 2000], 1000), Err(NetError::TooLarge), "payloads beyond one frame are refused");
}
