//! UDP socket implementation.

use crate::sched::Mutex;
use crate::time;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use otter_net_proto::UdpPacket;

use super::{NetError, NetIf};

/// UDP datagram received on a socket.
#[derive(Clone)]
pub struct UdpDatagram {
    pub data: Vec<u8>,
    pub src_ip: [u8; 4],
    pub src_port: u16,
}

/// UDP socket state.
pub struct UdpSocket {
    local_port: u16,
}

/// Global UDP socket states: port -> queue of datagrams.
pub static UDP_SOCKETS: Mutex<BTreeMap<u16, VecDeque<UdpDatagram>>> = Mutex::new(BTreeMap::new());

/// Next ephemeral port to allocate (starting at 49152).
pub static NEXT_EPHEMERAL_PORT: Mutex<u16> = Mutex::new(49152);

impl UdpSocket {
    /// Bind to a port (0 = ephemeral).
    pub fn bind(port: u16) -> Result<Self, NetError> {
        if port == 0 {
            // Allocate ephemeral port (49152..=65535)
            let mut next_port = NEXT_EPHEMERAL_PORT.lock();
            let mut sockets = UDP_SOCKETS.lock();
            let mut attempt_port = *next_port;
            loop {
                if let alloc::collections::btree_map::Entry::Vacant(e) = sockets.entry(attempt_port) {
                    // Found free port
                    e.insert(VecDeque::new());
                    // Wrap to 49152 if we go past 65535
                    let next = if attempt_port == 65535 { 49152 } else { attempt_port + 1 };
                    *next_port = next;
                    return Ok(UdpSocket {
                        local_port: attempt_port,
                    });
                }
                // Increment with wrapping within the range
                attempt_port = if attempt_port == 65535 { 49152 } else { attempt_port + 1 };
                if attempt_port == *next_port {
                    // Wrapped around, no ports available
                    return Err(NetError::AddressInUse);
                }
            }
        } else {
            // Try to bind to specific port
            let mut sockets = UDP_SOCKETS.lock();
            if let alloc::collections::btree_map::Entry::Vacant(e) = sockets.entry(port) {
                e.insert(VecDeque::new());
                Ok(UdpSocket { local_port: port })
            } else {
                Err(NetError::AddressInUse)
            }
        }
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    pub fn send_to(&self, data: &[u8], dst_ip: [u8; 4], dst_port: u16) -> Result<(), NetError> {
        use otter_net_proto::{EthernetBuilder, Ipv4Builder, Ipv4Addr, MacAddr, UdpBuilder};

        // Check size: 8 bytes UDP header + 20 bytes IPv4 header = 28 overhead
        if data.len() + 28 > 1500 {
            return Err(NetError::TooLarge);
        }

        let nic = super::interface().ok_or(NetError::NoInterface)?;
        let src_ip = nic.ipv4();

        // Build UDP packet
        let udp = UdpBuilder::new(self.local_port, dst_port)
            .with_payload(data)
            .build_with_pseudo_header(
                Ipv4Addr::from_bytes(&src_ip).unwrap(),
                Ipv4Addr::from_bytes(&dst_ip).unwrap(),
            );

        // Build IPv4 packet
        let ipv4 = Ipv4Builder::new(
            Ipv4Addr::from_bytes(&src_ip).unwrap(),
            Ipv4Addr::from_bytes(&dst_ip).unwrap(),
            17, // UDP protocol
        )
        .with_payload(&udp)
        .build();

        // Determine next hop: direct ARP for subnet, gateway for others
        let target_ip = if dst_ip == [255, 255, 255, 255] {
            // Broadcast: no ARP needed, handled separately
            let broadcast_mac = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
            let frame = EthernetBuilder::new(
                MacAddr::from_bytes(&broadcast_mac).unwrap(),
                MacAddr::from_bytes(&nic.mac).unwrap(),
                0x0800, // IPv4
            )
            .with_payload(&ipv4)
            .build();
            return nic.tx_frame(&frame);
        } else {
            // Check if destination is in the same subnet using the netmask
            let our_ip_u32 = u32::from_be_bytes(src_ip);
            let dst_ip_u32 = u32::from_be_bytes(dst_ip);
            let netmask_u32 = u32::from_be_bytes(nic.netmask());

            if (our_ip_u32 & netmask_u32) == (dst_ip_u32 & netmask_u32) {
                // Same subnet: resolve directly
                dst_ip
            } else {
                // Different subnet: use gateway
                nic.gateway()
            }
        };

        // Resolve MAC using ARP
        let target_mac = nic.arp_resolve(target_ip, 3000)?;

        // Build Ethernet frame
        let frame = EthernetBuilder::new(
            MacAddr::from_bytes(&target_mac).unwrap(),
            MacAddr::from_bytes(&nic.mac).unwrap(),
            0x0800, // IPv4
        )
        .with_payload(&ipv4)
        .build();

        nic.tx_frame(&frame)
    }

    pub fn recv_from(&self, buf: &mut [u8], timeout_ms: u64) -> Result<(usize, [u8; 4], u16), NetError> {
        let deadline = time::uptime_ms() + timeout_ms;
        loop {
            let mut sockets = UDP_SOCKETS.lock();
            if let Some(queue) = sockets.get_mut(&self.local_port)
                && let Some(dg) = queue.pop_front()
            {
                let len = core::cmp::min(buf.len(), dg.data.len());
                buf[..len].copy_from_slice(&dg.data[..len]);
                return Ok((len, dg.src_ip, dg.src_port));
            }
            drop(sockets);

            if time::uptime_ms() >= deadline {
                return Err(NetError::Timeout);
            }

            time::sleep_ms(1);
        }
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        let mut sockets = UDP_SOCKETS.lock();
        sockets.remove(&self.local_port);
    }
}

/// Handle a UDP packet.
pub fn handle_udp(netif: &NetIf, udp: &UdpPacket, ipv4: &otter_net_proto::Ipv4Header) {

    let dst_port = udp.dst_port();
    let src_port = udp.src_port();

    // Verify UDP checksum
    if let (Ok(src_ip), Ok(dst_ip)) = (ipv4.src(), ipv4.dst())
        && !udp.verify_checksum(src_ip, dst_ip)
    {
        let mut stats = netif.stats.lock();
        stats.udp_bad_checksum += 1;
        return;
    }

    // Check if this is DHCP (port 68 = client, 67 = server)
    if dst_port == 68 {
        if let Ok(_src_ip) = ipv4.src() {
            let dhcp_payload = udp.payload();
            let mut dhcp_opt = super::DHCP_CLIENT.lock();
            if let Some(dhcp) = dhcp_opt.as_mut() {
                let now = time::uptime_ms();
                let actions = dhcp.handle_packet(dhcp_payload, now as u32);
                for action in actions {
                    use otter_netlogic::Action;
                    match action {
                        Action::Send(frame_data) => {
                            if let Some(dev) = crate::drivers::virtio::net::device() {
                                let _ = dev.transmit(&frame_data);
                            }
                        }
                        Action::Configured { ip, router, dns, lease_ms, mask } => {
                            super::apply_lease(netif, ip, router, dns, lease_ms, mask);
                        }
                        _ => {}
                    }
                }
            }
        }
        return;
    }

    // Regular UDP socket
    if let Ok(src_ip) = ipv4.src()
        && let Ok(dst_ip) = ipv4.dst()
    {
        let our_ip = netif.ipv4();
        let dst_ip_bytes = *dst_ip.as_bytes();

        // Check destination: must be ours or broadcast
        if dst_ip_bytes != our_ip && dst_ip_bytes != [255, 255, 255, 255] {
            // Compute subnet broadcast
            let our_ip_u32 = u32::from_be_bytes(our_ip);
            let netmask_u32 = u32::from_be_bytes(netif.netmask());
            let subnet_bcast_u32 = (our_ip_u32 & netmask_u32) | !netmask_u32;
            let subnet_bcast = subnet_bcast_u32.to_be_bytes();

            if dst_ip_bytes != subnet_bcast {
                // Not for us, drop it
                return;
            }
        }

        let mut sockets = UDP_SOCKETS.lock();
        if let Some(queue) = sockets.get_mut(&dst_port) {
            if queue.len() < 64 {
                let dg = UdpDatagram {
                    data: udp.payload().to_vec(),
                    src_ip: *src_ip.as_bytes(),
                    src_port,
                };
                queue.push_back(dg);
            }
        } else {
            // No socket bound to this port
            let mut stats = netif.stats.lock();
            stats.udp_no_port += 1;
        }
    }
}
