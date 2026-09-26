//! Kernel network stack (brief M5-T1a): virtio-net driver, Ethernet, ARP, and ICMP echo.

use crate::drivers::virtio::net as vnet;
use crate::kprintln;
use crate::mm::addr::VirtAddr;
use crate::sched::{self, Mutex};
use crate::time;
use alloc::collections::BTreeMap;
use core::fmt;
use otter_net_proto::{
    EthernetFrame, EthernetBuilder, MacAddr, Ipv4Addr,
    ArpPacket, ArpBuilder, Ipv4Header, Ipv4Builder,
    IcmpEchoRequest, IcmpEchoBuilder,
};

/// Network interface configuration and state.
pub struct NetIf {
    mac: [u8; 6],
    ipv4: [u8; 4],
    gateway: [u8; 4],
    arp_cache: Mutex<BTreeMap<[u8; 4], [u8; 6]>>,
    ping_waiters: Mutex<BTreeMap<u32, VirtAddr>>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    Timeout,
    TooLarge,
    NoInterface,
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            NetError::Timeout => write!(f, "Timeout"),
            NetError::TooLarge => write!(f, "TooLarge"),
            NetError::NoInterface => write!(f, "NoInterface"),
        }
    }
}

static NETIF: Mutex<Option<NetIf>> = Mutex::new(None);
static mut NETIF_PTR: *const NetIf = core::ptr::null();

/// Initialize the network stack.
pub fn init() {
    vnet::init();

    if let Some(dev) = vnet::device() {
        let mac = dev.get_mac();
        let netif = NetIf {
            mac,
            ipv4: [10, 0, 2, 15],
            gateway: [10, 0, 2, 2],
            arp_cache: Mutex::new(BTreeMap::new()),
            ping_waiters: Mutex::new(BTreeMap::new()),
            stats: Mutex::new(NetStats {
                rx_frames: 0,
                tx_frames: 0,
                rx_dropped: 0,
                arp_requests_sent: 0,
                arp_replies_sent: 0,
                icmp_echo_replies_received: 0,
            }),
        };

        kprintln!("[net] virtio-net {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, 10.0.2.15/24 gw 10.0.2.2",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

        let mut guard = NETIF.lock();
        *guard = Some(netif);
        drop(guard);

        // Store a static pointer to the NetIf for use in interface()
        // SAFETY: We're storing a pointer to data inside NETIF which is a static.
        // The pointer remains valid for the entire program lifetime.
        unsafe {
            let guard = NETIF.lock();
            NETIF_PTR = guard.as_ref().unwrap() as *const NetIf;
        }

        // Spawn the RX thread
        sched::spawn("net-rx", net_rx_thread, 0);
    }
}

/// Get the network interface reference.
pub fn interface() -> Option<&'static NetIf> {
    // SAFETY: NETIF_PTR is set during init() and points to data in the static NETIF.
    // The pointer is valid for the entire program lifetime.
    unsafe {
        if NETIF_PTR.is_null() {
            None
        } else {
            Some(&*NETIF_PTR)
        }
    }
}

impl NetIf {
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn ipv4(&self) -> [u8; 4] {
        self.ipv4
    }

    pub fn gateway(&self) -> [u8; 4] {
        self.gateway
    }

    pub fn stats(&self) -> NetStats {
        *self.stats.lock()
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
                let arp_pkt = ArpBuilder::request(
                    MacAddr::from_bytes(&self.mac).unwrap(),
                    Ipv4Addr::from_bytes(&self.ipv4).unwrap(),
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
        let target_mac = self.arp_resolve(self.gateway, timeout_ms)?;

        // Build ICMP echo request
        let ping_id = 0x1234u16; // Fixed per-boot constant
        let icmp_pkt = IcmpEchoBuilder::request(ping_id, seq)
            .with_payload(payload)
            .build();

        // Build IPv4 packet
        let ipv4_pkt = Ipv4Builder::new(
            Ipv4Addr::from_bytes(&self.ipv4).unwrap(),
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

/// RX thread: polls the device every millisecond and handles frames.
fn net_rx_thread(_arg: usize) -> i32 {
    loop {
        time::sleep_ms(1);

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
                    // IPv4: handle ICMP only
                    if let Ok(ipv4) = Ipv4Header::new(frame.payload())
                        && ipv4.protocol() == 1
                        && let Ok(icmp) = IcmpEchoRequest::new(ipv4.payload())
                    {
                        handle_icmp_echo(netif, &icmp, &ipv4, &frame);
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
    if let Ok(tpa) = arp.tpa()
        && tpa.as_bytes() == &netif.ipv4
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
            Ipv4Addr::from_bytes(&netif.ipv4).unwrap(),
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
    if let (Ok(dst), Ok(src)) = (ipv4.dst(), ipv4.src())
        && dst.as_bytes() == &netif.ipv4
    {
        // Build echo reply
        let reply = IcmpEchoBuilder::reply(icmp.identifier(), icmp.sequence())
            .with_payload(icmp.payload())
            .build();

        // Build IPv4 reply
        let ipv4_reply = Ipv4Builder::new(
            Ipv4Addr::from_bytes(&netif.ipv4).unwrap(),
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
