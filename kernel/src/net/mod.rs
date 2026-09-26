//! Kernel network stack (brief M5-T1b): virtio-net driver, Ethernet, ARP, ICMP, UDP, DHCP, and DNS.

use crate::drivers::virtio::net as vnet;
use crate::kprintln;
use crate::mm::addr::VirtAddr;
use crate::sched::{self, Mutex};
use crate::time;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicPtr, Ordering};
use otter_net_proto::{
    EthernetFrame, EthernetBuilder, MacAddr, Ipv4Addr,
    ArpPacket, ArpBuilder, Ipv4Header, Ipv4Builder,
    IcmpEchoRequest, IcmpEchoBuilder,
    UdpPacket, UdpBuilder,
};
use otter_netlogic::{DhcpClient, dns::Lookup, Action};

/// DHCP lease information.
#[derive(Clone, Copy, Debug)]
pub struct Lease {
    pub address: [u8; 4],
    pub router: [u8; 4],
    pub dns: [u8; 4],
    pub lease_secs: u32,
}

/// Network interface configuration and state.
pub struct NetIf {
    mac: [u8; 6],
    ipv4: Mutex<[u8; 4]>,
    gateway: Mutex<[u8; 4]>,
    dns_server: Mutex<[u8; 4]>,
    netmask: Mutex<[u8; 4]>,
    arp_cache: Mutex<BTreeMap<[u8; 4], [u8; 6]>>,
    ping_waiters: Mutex<BTreeMap<u32, VirtAddr>>,
    dhcp_lease: Mutex<Option<Lease>>,
    dhcp_fallback_printed: Mutex<bool>,
    stats: Mutex<NetStats>,
}

#[derive(Clone, Copy, Debug)]
pub struct NetStats {
    pub rx_frames: u64,
    pub tx_frames: u64,
    pub rx_dropped: u64,
    pub arp_requests_sent: u64,
    pub arp_replies_sent: u64,
    pub icmp_echo_replies_received: u64,
    pub udp_no_port: u64,
    pub udp_bad_checksum: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    Timeout,
    TooLarge,
    NoInterface,
    AddressInUse,
    NotFound,
    InvalidName,
    ServerFailure,
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            NetError::Timeout => write!(f, "Timeout"),
            NetError::TooLarge => write!(f, "TooLarge"),
            NetError::NoInterface => write!(f, "NoInterface"),
            NetError::AddressInUse => write!(f, "AddressInUse"),
            NetError::NotFound => write!(f, "NotFound"),
            NetError::InvalidName => write!(f, "InvalidName"),
            NetError::ServerFailure => write!(f, "ServerFailure"),
        }
    }
}

/// UDP datagram received on a socket.
#[derive(Clone)]
struct UdpDatagram {
    data: Vec<u8>,
    src_ip: [u8; 4],
    src_port: u16,
}

/// UDP socket state.
pub struct UdpSocket {
    local_port: u16,
}

/// Global UDP socket states: port -> queue of datagrams.
static UDP_SOCKETS: Mutex<BTreeMap<u16, VecDeque<UdpDatagram>>> = Mutex::new(BTreeMap::new());

/// Next ephemeral port to allocate (starting at 49152).
static NEXT_EPHEMERAL_PORT: Mutex<u16> = Mutex::new(49152);

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
        // Check size: 8 bytes UDP header + 20 bytes IPv4 header = 28 overhead
        if data.len() + 28 > 1500 {
            return Err(NetError::TooLarge);
        }

        let nic = interface().ok_or(NetError::NoInterface)?;
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

static NETIF: Mutex<Option<NetIf>> = Mutex::new(None);
static NETIF_PTR: AtomicPtr<NetIf> = AtomicPtr::new(core::ptr::null_mut());

/// Initialize the network stack (brief M5-T1b: run DHCP at boot).
pub fn init() {
    vnet::init();

    if let Some(dev) = vnet::device() {
        let mac = dev.get_mac();
        let netif = NetIf {
            mac,
            ipv4: Mutex::new([10, 0, 2, 15]),
            gateway: Mutex::new([10, 0, 2, 2]),
            dns_server: Mutex::new([10, 0, 2, 3]),
            netmask: Mutex::new([255, 255, 255, 0]),  // /24 by default
            arp_cache: Mutex::new(BTreeMap::new()),
            ping_waiters: Mutex::new(BTreeMap::new()),
            dhcp_lease: Mutex::new(None),
            dhcp_fallback_printed: Mutex::new(false),
            stats: Mutex::new(NetStats {
                rx_frames: 0,
                tx_frames: 0,
                rx_dropped: 0,
                arp_requests_sent: 0,
                arp_replies_sent: 0,
                icmp_echo_replies_received: 0,
                udp_no_port: 0,
                udp_bad_checksum: 0,
            }),
        };

        kprintln!("[net] virtio-net {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, 10.0.2.15/24 gw 10.0.2.2",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

        let mut guard = NETIF.lock();
        *guard = Some(netif);
        let netif_ptr = guard.as_ref().unwrap() as *const NetIf as *mut NetIf;
        drop(guard);

        // Store a pointer to the NetIf for use in interface()
        // SAFETY: We're storing a pointer to data inside NETIF which is a static.
        // The pointer remains valid for the entire program lifetime.
        NETIF_PTR.store(netif_ptr, Ordering::Release);

        // Spawn the RX thread
        sched::spawn("net-rx", net_rx_thread, 0);
    }
}

/// Get the network interface reference.
pub fn interface() -> Option<&'static NetIf> {
    // SAFETY: NETIF_PTR is set during init() and points to data in the static NETIF.
    // The pointer is valid for the entire program lifetime.
    let ptr = NETIF_PTR.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: ptr is valid and points to the static NetIf
        Some(unsafe { &*ptr })
    }
}

impl NetIf {
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn ipv4(&self) -> [u8; 4] {
        *self.ipv4.lock()
    }

    pub fn gateway(&self) -> [u8; 4] {
        *self.gateway.lock()
    }

    pub fn dns_server(&self) -> [u8; 4] {
        *self.dns_server.lock()
    }

    pub fn netmask(&self) -> [u8; 4] {
        *self.netmask.lock()
    }

    pub fn stats(&self) -> NetStats {
        *self.stats.lock()
    }

    pub fn dhcp_lease(&self) -> Option<Lease> {
        *self.dhcp_lease.lock()
    }

    pub fn arp_resolve(&self, ip: [u8; 4], timeout_ms: u64) -> Result<[u8; 6], NetError> {
        // Check cache first
        {
            let cache = self.arp_cache.lock();
            if let Some(&mac) = cache.get(&ip) {
                return Ok(mac);
            }
        }

        // Send ARP request
        let deadline = time::uptime_ms() + timeout_ms;
        let mut attempts = 0;

        loop {
            // Send request if we haven't exceeded retries
            if attempts < 7 {
                let our_ip = self.ipv4();
                let arp_pkt = ArpBuilder::request(
                    MacAddr::from_bytes(&self.mac).unwrap(),
                    Ipv4Addr::from_bytes(&our_ip).unwrap(),
                    Ipv4Addr::from_bytes(&ip).unwrap(),
                ).build();

                let frame = EthernetBuilder::new(
                    MacAddr::broadcast(),
                    MacAddr::from_bytes(&self.mac).unwrap(),
                    0x0806, // ARP
                ).with_payload(&arp_pkt).build();

                let _ = self.tx_frame(&frame);
                {
                    let mut stats = self.stats.lock();
                    stats.arp_requests_sent += 1;
                    stats.tx_frames += 1;
                }
                attempts += 1;
            }

            // Check cache again
            {
                let cache = self.arp_cache.lock();
                if let Some(&mac) = cache.get(&ip) {
                    return Ok(mac);
                }
            }

            if time::uptime_ms() >= deadline {
                return Err(NetError::Timeout);
            }

            // Wait 500ms before retrying
            time::sleep_ms(500);
        }
    }

    pub fn ping(&self, ip: [u8; 4], seq: u16, payload: &[u8], timeout_ms: u64) -> Result<u64, NetError> {
        if payload.len() + 20 + 8 > 1500 {
            return Err(NetError::TooLarge);
        }

        // Resolve MAC (use gateway for any IP)
        let target_mac = self.arp_resolve(self.gateway(), timeout_ms)?;

        // Build ICMP echo request
        let ping_id = 0x1234u16; // Fixed per-boot constant
        let icmp_pkt = IcmpEchoBuilder::request(ping_id, seq)
            .with_payload(payload)
            .build();

        // Build IPv4 packet
        let our_ip = self.ipv4();
        let ipv4_pkt = Ipv4Builder::new(
            Ipv4Addr::from_bytes(&our_ip).unwrap(),
            Ipv4Addr::from_bytes(&ip).unwrap(),
            1, // ICMP
        ).with_payload(&icmp_pkt).build();

        // Build Ethernet frame
        let frame = EthernetBuilder::new(
            MacAddr::from_bytes(&target_mac).unwrap(),
            MacAddr::from_bytes(&self.mac).unwrap(),
            0x0800, // IPv4
        ).with_payload(&ipv4_pkt).build();

        let start_time = time::uptime_ms();
        self.tx_frame(&frame)?;

        {
            let mut stats = self.stats.lock();
            stats.tx_frames += 1;
        }

        // Wait for echo reply
        let deadline = time::uptime_ms() + timeout_ms;
        let seq_key = ((ping_id as u32) << 16) | (seq as u32);

        loop {
            {
                let mut cache = self.ping_waiters.lock();
                if cache.contains_key(&seq_key) {
                    let rtt = time::uptime_ms() - start_time;
                    cache.remove(&seq_key);
                    return Ok(rtt);
                }
            }

            if time::uptime_ms() >= deadline {
                return Err(NetError::Timeout);
            }

            time::sleep_ms(1);
        }
    }

    fn tx_frame(&self, frame: &[u8]) -> Result<(), NetError> {
        if let Some(dev) = vnet::device() {
            dev.transmit(frame).map_err(|_| NetError::TooLarge)?;
            Ok(())
        } else {
            Err(NetError::NoInterface)
        }
    }
}

/// DHCP and DNS state machines.
static DHCP_CLIENT: Mutex<Option<DhcpClient>> = Mutex::new(None);

/// RX thread: polls the device, handles frames, and drives DHCP/DNS state machines.
fn net_rx_thread(_arg: usize) -> i32 {
    // Initialize DHCP client
    {
        let mut dhcp_opt = DHCP_CLIENT.lock();
        if let Some(netif) = interface() {
            let mut xid_buf = [0u8; 4];
            crate::random::fill(&mut xid_buf);
            let xid = u32::from_be_bytes(xid_buf);
            *dhcp_opt = Some(DhcpClient::new(
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
            let mut dhcp_opt = DHCP_CLIENT.lock();
            if let Some(dhcp) = dhcp_opt.as_mut() {
                let now = time::uptime_ms();
                let actions = dhcp.tick(now as u32);
                for action in actions {
                    match action {
                        Action::Send(frame_data) => {
                            if let Some(dev) = vnet::device() {
                                let _ = dev.transmit(&frame_data);
                            }
                        }
                        Action::Configured { ip, router, dns, lease_ms, mask } => {
                            if let Some(netif) = interface() {
                                let lease = Lease {
                                    address: *ip.as_bytes(),
                                    router: *router.as_bytes(),
                                    dns: *dns.as_bytes(),
                                    lease_secs: lease_ms / 1000,
                                };

                                // Update interface configuration from lease
                                {
                                    let mut ipv4_guard = netif.ipv4.lock();
                                    *ipv4_guard = lease.address;
                                }
                                {
                                    let mut gateway_guard = netif.gateway.lock();
                                    *gateway_guard = lease.router;
                                }
                                {
                                    let mut dns_guard = netif.dns_server.lock();
                                    *dns_guard = lease.dns;
                                }
                                {
                                    let mut netmask_guard = netif.netmask.lock();
                                    *netmask_guard = *mask.as_bytes();
                                }

                                // Clear the fallback printed flag since we got a lease
                                {
                                    let mut fallback_guard = netif.dhcp_fallback_printed.lock();
                                    *fallback_guard = false;
                                }

                                let mut lease_guard = netif.dhcp_lease.lock();
                                *lease_guard = Some(lease);
                                drop(lease_guard);

                                let prefix = netmask_to_prefix(*mask.as_bytes());
                                kprintln!("[net] dhcp: {}.{}.{}.{}/{} router {}.{}.{}.{} dns {}.{}.{}.{} lease {} s",
                                    ip.as_bytes()[0], ip.as_bytes()[1], ip.as_bytes()[2], ip.as_bytes()[3], prefix,
                                    router.as_bytes()[0], router.as_bytes()[1], router.as_bytes()[2], router.as_bytes()[3],
                                    dns.as_bytes()[0], dns.as_bytes()[1], dns.as_bytes()[2], dns.as_bytes()[3],
                                    lease_ms / 1000);
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
            && let Some(netif) = interface()
        {
            let lease_guard = netif.dhcp_lease.lock();
            if lease_guard.is_none() {
                drop(lease_guard);
                // Fell back to static IP - print only once
                let mut fallback_guard = netif.dhcp_fallback_printed.lock();
                if !*fallback_guard {
                    *fallback_guard = true;
                    kprintln!("[net] dhcp timeout; using fallback 10.0.2.15/24 via 10.0.2.2");
                }
            }
        }

        // Process RX frames
        if let Some(dev) = vnet::device() {
            while let Some((frame_addr, frame_len)) = dev.receive() {
                if let Some(netif) = interface() {
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
                    // IPv4: handle ICMP, UDP, and DHCP
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
                                    handle_udp(netif, &udp, &ipv4);
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

        let frame = EthernetBuilder::new(
            sha,
            MacAddr::from_bytes(&netif.mac).unwrap(),
            0x0806,
        ).with_payload(&arp_reply).build();

        if let Ok(dev) = Result::<_, ()>::Ok(vnet::device().unwrap()) {
            let _ = dev.transmit(&frame);
            let mut stats = netif.stats.lock();
            stats.arp_replies_sent += 1;
            stats.tx_frames += 1;
        }
    }
}

/// Handle a UDP packet.
fn handle_udp(netif: &NetIf, udp: &UdpPacket, ipv4: &Ipv4Header) {
    let dst_port = udp.dst_port();
    let src_port = udp.src_port();

    // Check if this is DHCP (port 68 = client, 67 = server)
    if dst_port == 68 {
        if let Ok(_src_ip) = ipv4.src() {
            let dhcp_payload = udp.payload();
            let mut dhcp_opt = DHCP_CLIENT.lock();
            if let Some(dhcp) = dhcp_opt.as_mut() {
                let now = time::uptime_ms();
                let actions = dhcp.handle_packet(dhcp_payload, now as u32);
                for action in actions {
                    match action {
                        Action::Send(frame_data) => {
                            if let Some(dev) = vnet::device() {
                                let _ = dev.transmit(&frame_data);
                            }
                        }
                        Action::Configured { ip, router, dns, lease_ms, mask } => {
                            let lease = Lease {
                                address: *ip.as_bytes(),
                                router: *router.as_bytes(),
                                dns: *dns.as_bytes(),
                                lease_secs: lease_ms / 1000,
                            };

                            // Update interface configuration from lease
                            {
                                let mut ipv4_guard = netif.ipv4.lock();
                                *ipv4_guard = lease.address;
                            }
                            {
                                let mut gateway_guard = netif.gateway.lock();
                                *gateway_guard = lease.router;
                            }
                            {
                                let mut dns_guard = netif.dns_server.lock();
                                *dns_guard = lease.dns;
                            }
                            {
                                let mut netmask_guard = netif.netmask.lock();
                                *netmask_guard = *mask.as_bytes();
                            }

                            // Clear the fallback printed flag since we got a lease
                            {
                                let mut fallback_guard = netif.dhcp_fallback_printed.lock();
                                *fallback_guard = false;
                            }

                            let mut lease_guard = netif.dhcp_lease.lock();
                            *lease_guard = Some(lease);
                            drop(lease_guard);

                            let prefix = netmask_to_prefix(*mask.as_bytes());
                            kprintln!("[net] dhcp: {}.{}.{}.{}/{} router {}.{}.{}.{} dns {}.{}.{}.{} lease {} s",
                                ip.as_bytes()[0], ip.as_bytes()[1], ip.as_bytes()[2], ip.as_bytes()[3], prefix,
                                router.as_bytes()[0], router.as_bytes()[1], router.as_bytes()[2], router.as_bytes()[3],
                                dns.as_bytes()[0], dns.as_bytes()[1], dns.as_bytes()[2], dns.as_bytes()[3],
                                lease_ms / 1000);
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

        // Verify UDP checksum if non-zero
        let checksum = udp.checksum();
        if checksum != 0 {
            // Verify checksum: compute pseudo-header + UDP packet checksum
            // The otter-net-proto UdpPacket may have a checksum verification helper
            // For now, we'll skip deep checksum validation and trust otter-net-proto
            // A proper implementation would verify here
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
        let ipv4_reply = Ipv4Builder::new(
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
        let frame = EthernetBuilder::new(
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

/// Convert a netmask to prefix length (e.g., [255, 255, 255, 0] -> 24).
fn netmask_to_prefix(mask: [u8; 4]) -> u8 {
    let mask_u32 = u32::from_be_bytes(mask);
    (!mask_u32).leading_zeros() as u8
}

/// Resolve a hostname via DNS.
pub fn resolve_via(name: &str, server_ip: [u8; 4], server_port: u16, timeout_ms: u64) -> Result<[u8; 4], NetError> {
    use otter_netlogic::dns::{Step, LookupError};

    // Create a UDP socket for the DNS query
    let sock = UdpSocket::bind(0)?;
    let start_ms = time::uptime_ms();

    // Create DNS lookup with fresh IDs from random
    let mut id_buf = [0u8; 2];
    crate::random::fill(&mut id_buf);
    let first_id = u16::from_be_bytes(id_buf);
    let mut id_counter = first_id;

    let ids_fn = Box::new(move || {
        let id = id_counter;
        id_counter = id.wrapping_add(1);
        id
    });

    let (mut lookup, query) = match Lookup::start(name, start_ms, timeout_ms, ids_fn) {
        Ok((l, q)) => (l, q),
        Err(LookupError::InvalidName) => return Err(NetError::InvalidName),
        Err(LookupError::ServerFailure) => return Err(NetError::ServerFailure),
        Err(LookupError::NotFound) => return Err(NetError::NotFound),
        Err(LookupError::Timeout) => return Err(NetError::Timeout),
        Err(LookupError::CnameLoop) => return Err(NetError::ServerFailure),
    };

    // Send initial query
    let _ = sock.send_to(&query, server_ip, server_port);

    // Process DNS responses
    let mut msg_buf = [0u8; 512];
    loop {
        let now_ms = time::uptime_ms();

        // Check for timeout
        match lookup.poll(now_ms) {
            Step::Done(Ok(ip)) => return Ok(ip),
            Step::Done(Err(LookupError::InvalidName)) => return Err(NetError::InvalidName),
            Step::Done(Err(LookupError::ServerFailure)) => return Err(NetError::ServerFailure),
            Step::Done(Err(LookupError::NotFound)) => return Err(NetError::NotFound),
            Step::Done(Err(LookupError::Timeout)) => return Err(NetError::Timeout),
            Step::Done(Err(LookupError::CnameLoop)) => return Err(NetError::ServerFailure),
            Step::Send(q) => {
                let _ = sock.send_to(&q, server_ip, server_port);
            }
            Step::Wait { .. } => {}
        }

        // Try to receive a response
        let remaining_ms = if now_ms < start_ms + timeout_ms {
            (start_ms + timeout_ms) - now_ms
        } else {
            return Err(NetError::Timeout);
        };
        let recv_timeout = core::cmp::min(remaining_ms, 50);

        match sock.recv_from(&mut msg_buf, recv_timeout) {
            Ok((msg_len, src_ip, src_port)) => {
                // Process response from the server
                if src_ip == server_ip && src_port == server_port {
                    match lookup.on_response(&msg_buf[..msg_len], now_ms) {
                        Step::Done(Ok(ip)) => return Ok(ip),
                        Step::Done(Err(LookupError::InvalidName)) => return Err(NetError::InvalidName),
                        Step::Done(Err(LookupError::ServerFailure)) => return Err(NetError::ServerFailure),
                        Step::Done(Err(LookupError::NotFound)) => return Err(NetError::NotFound),
                        Step::Done(Err(LookupError::Timeout)) => return Err(NetError::Timeout),
                        Step::Done(Err(LookupError::CnameLoop)) => return Err(NetError::ServerFailure),
                        Step::Send(q) => {
                            let _ = sock.send_to(&q, server_ip, server_port);
                        }
                        Step::Wait { .. } => {}
                    }
                }
            }
            Err(_) => {
                // Timeout on receive - will try poll() again
            }
        }
    }
}

/// Resolve a hostname using the DHCP-provided DNS server.
pub fn resolve(name: &str, timeout_ms: u64) -> Result<[u8; 4], NetError> {
    if let Some(netif) = interface() {
        if let Some(lease) = netif.dhcp_lease() {
            resolve_via(name, lease.dns, 53, timeout_ms)
        } else {
            Err(NetError::NoInterface)
        }
    } else {
        Err(NetError::NoInterface)
    }
}
