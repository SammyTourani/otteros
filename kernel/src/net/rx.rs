//! RX thread: frame reception and dispatch (ARP, ICMP, UDP).

use crate::drivers::virtio::net as vnet;
use crate::mm::addr::VirtAddr;
use crate::time;
use otter_net_proto::{
    EthernetFrame, ArpPacket, ArpBuilder, Ipv4Header, IcmpEchoRequest, IcmpEchoBuilder,
    MacAddr, Ipv4Addr, UdpPacket,
};

use super::NetIf;

/// RX thread: polls the device, handles frames, and drives DHCP/DNS state machines.
pub fn net_rx_thread(_arg: usize) -> i32 {
    // Initialize DHCP client
    {
        let mut dhcp_opt = super::DHCP_CLIENT.lock();
        if let Some(netif) = super::interface() {
            // The transaction id only matches replies to this client; networking must not wait for
            // the CSPRNG, so fall back to the TSC mixed with the MAC when it is not seeded yet.
            let mut xid_buf = [0u8; 4];
            let xid = if crate::random::try_fill(&mut xid_buf) {
                u32::from_be_bytes(xid_buf)
            } else {
                // SAFETY: rdtsc only reads the time-stamp counter; it has no side effects.
                let tsc = unsafe { core::arch::x86_64::_rdtsc() };
                (tsc as u32) ^ (tsc >> 32) as u32 ^ u32::from_be_bytes([netif.mac[2], netif.mac[3], netif.mac[4], netif.mac[5]])
            };
            *dhcp_opt = Some(otter_netlogic::DhcpClient::new(
                MacAddr::from_bytes(&netif.mac).unwrap(),
                xid,
            ));
        }
        drop(dhcp_opt);
    }

    let dhcp_start = time::uptime_ms();

    loop {
        time::sleep_ms(1);

        // Drive DHCP state machine
        {
            let mut dhcp_opt = super::DHCP_CLIENT.lock();
            if let Some(dhcp) = dhcp_opt.as_mut() {
                let now = time::uptime_ms();
                let actions = dhcp.tick(now as u32);
                for action in actions {
                    use otter_netlogic::Action;
                    match action {
                        Action::Send(frame_data) => {
                            if let Some(dev) = vnet::device() {
                                let _ = dev.transmit(&frame_data);
                            }
                        }
                        Action::Configured { ip, router, dns, lease_ms, mask } => {
                            if let Some(netif) = super::interface() {
                                super::apply_lease(netif, ip, router, dns, lease_ms, mask);
                            }
                        }
                        _ => {}
                    }
                }
            }
            drop(dhcp_opt);
        }

        // Check DHCP timeout (10 seconds)
        if time::uptime_ms() - dhcp_start > 10000
            && let Some(netif) = super::interface()
        {
            let lease_guard = netif.dhcp_lease.lock();
            if lease_guard.is_none() {
                drop(lease_guard);
                // Fell back to static IP - print only once
                let mut fallback_guard = netif.dhcp_fallback_printed.lock();
                if !*fallback_guard {
                    *fallback_guard = true;
                    crate::kprintln!("[net] dhcp timeout; using fallback 10.0.2.15/24 via 10.0.2.2");
                }
            }
        }

        // Process RX frames
        if let Some(dev) = vnet::device() {
            while let Some((frame_addr, frame_len)) = dev.receive() {
                if let Some(netif) = super::interface() {
                    process_frame(netif, frame_addr, frame_len, dev);
                }
            }
        }
    }
}

/// Process a received Ethernet frame.
fn process_frame(netif: &NetIf, frame_addr: VirtAddr, frame_len: usize, dev: &'static vnet::VirtioNet) {
    // The frame_addr points to the start of the RX buffer, which includes the 12-byte virtio-net header.
    // Skip the header to get the actual Ethernet frame.
    let header_size = 12usize;
    if frame_len < header_size {
        let mut stats = netif.stats.lock();
        stats.rx_dropped += 1;
        return;
    }

    // Find the descriptor index from the RX queue
    let rx_buffer_idx = find_rx_buffer_idx(dev, frame_addr);

    // SAFETY: frame_addr points to the start of an RX buffer; we skip the 12-byte header.
    let frame_data_addr = frame_addr.as_u64() + header_size as u64;
    let frame_data_len = frame_len - header_size;
    let frame_data = unsafe { core::slice::from_raw_parts(frame_data_addr as *const u8, frame_data_len) };

    match EthernetFrame::new(frame_data) {
        Ok(frame) => {
            let mut stats = netif.stats.lock();
            stats.rx_frames += 1;
            drop(stats);

            let ethertype = frame.ethertype();
            match ethertype {
                0x0806 => {
                    // ARP
                    if let Ok(arp) = ArpPacket::new(frame.payload()) {
                        handle_arp(netif, &arp, &frame);
                    }
                }
                0x0800 => {
                    // IPv4: handle ICMP and UDP
                    if let Ok(ipv4) = Ipv4Header::new(frame.payload()) {
                        match ipv4.protocol() {
                            1 => {
                                // ICMP
                                if let Ok(icmp) = IcmpEchoRequest::new(ipv4.payload()) {
                                    handle_icmp_echo(netif, &icmp, &ipv4, &frame);
                                }
                            }
                            17 => {
                                // UDP
                                if let Ok(udp) = UdpPacket::new(ipv4.payload()) {
                                    super::udp::handle_udp(netif, &udp, &ipv4);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {
                    let mut stats = netif.stats.lock();
                    stats.rx_dropped += 1;
                }
            }
        }
        Err(_) => {
            let mut stats = netif.stats.lock();
            stats.rx_dropped += 1;
        }
    }

    // Repost the RX buffer
    if rx_buffer_idx < 64 {
        dev.repost_rx_buffer(rx_buffer_idx);
    }
}

/// Find which RX buffer index corresponds to a frame address.
fn find_rx_buffer_idx(dev: &vnet::VirtioNet, frame_addr: VirtAddr) -> usize {
    for i in 0..64 {
        if let Some(buf_virt) = dev.get_rx_buffer_virt(i)
            && buf_virt.as_u64() == frame_addr.as_u64()
        {
            return i;
        }
    }
    64 // Invalid
}

/// Handle an ARP packet.
fn handle_arp(netif: &NetIf, arp: &ArpPacket, _eth: &EthernetFrame) {
    if arp.oper() != 1 {
        // Not a request
        if arp.oper() == 2 {
            // Reply: add to cache
            if let (Ok(spa), Ok(sha)) = (arp.spa(), arp.sha()) {
                let mut cache = netif.arp_cache.lock();
                cache.insert(*spa.as_bytes(), *sha.as_bytes());
            }
        }
        return;
    }

    // ARP request: check if it's for us
    let our_ip = netif.ipv4();
    if let Ok(tpa) = arp.tpa()
        && tpa.as_bytes() == &our_ip
        && let (Ok(sha), Ok(spa)) = (arp.sha(), arp.spa())
    {
        // Cache the sender
        {
            let mut cache = netif.arp_cache.lock();
            cache.insert(*spa.as_bytes(), *sha.as_bytes());
        }

        // Send ARP reply
        let arp_reply = ArpBuilder::reply(
            MacAddr::from_bytes(&netif.mac).unwrap(),
            Ipv4Addr::from_bytes(&our_ip).unwrap(),
            sha,
            spa,
        ).build();

        let frame = otter_net_proto::EthernetBuilder::new(
            sha,
            MacAddr::from_bytes(&netif.mac).unwrap(),
            0x0806,
        ).with_payload(&arp_reply).build();

        if let Some(dev) = vnet::device() {
            let _ = dev.transmit(&frame);
            let mut stats = netif.stats.lock();
            stats.arp_replies_sent += 1;
            stats.tx_frames += 1;
        }
    }
}

/// Handle an ICMP echo request.
fn handle_icmp_echo(netif: &NetIf, icmp: &IcmpEchoRequest, ipv4: &Ipv4Header, eth: &EthernetFrame) {
    if icmp.icmp_type() != 8 {
        // Not an echo request, check if it's a reply (type 0)
        if icmp.icmp_type() == 0 {
            // Echo reply: wake up ping waiter
            let seq_key = ((icmp.identifier() as u32) << 16) | (icmp.sequence() as u32);
            let mut waiters = netif.ping_waiters.lock();
            if waiters.insert(seq_key, VirtAddr::new(1)).is_some() {
                // Mark as received if this key was already present
                let mut stats = netif.stats.lock();
                stats.icmp_echo_replies_received += 1;
            }
        }
        return;
    }

    // It's an echo request for us
    let our_ip = netif.ipv4();
    if let (Ok(dst), Ok(src)) = (ipv4.dst(), ipv4.src())
        && dst.as_bytes() == &our_ip
    {
        // Build echo reply
        let reply = IcmpEchoBuilder::reply(icmp.identifier(), icmp.sequence())
            .with_payload(icmp.payload())
            .build();

        // Build IPv4 reply
        let ipv4_reply = otter_net_proto::Ipv4Builder::new(
            Ipv4Addr::from_bytes(&our_ip).unwrap(),
            src,
            1, // ICMP
        ).with_payload(&reply).build();

        // Get sender MAC
        let src_mac = match eth.src() {
            Ok(m) => m,
            Err(_) => return,
        };

        // Build Ethernet reply
        let frame = otter_net_proto::EthernetBuilder::new(
            src_mac,
            MacAddr::from_bytes(&netif.mac).unwrap(),
            0x0800,
        ).with_payload(&ipv4_reply).build();

        if let Some(dev) = vnet::device() {
            let _ = dev.transmit(&frame);
            let mut stats = netif.stats.lock();
            stats.tx_frames += 1;
        }
    }
}
