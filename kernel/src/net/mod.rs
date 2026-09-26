//! Kernel network stack (brief M5-T1b): virtio-net driver, Ethernet, ARP, ICMP, UDP, DHCP, and DNS.

mod dhcp;
mod dns;
mod rx;
pub mod udp;

use crate::drivers::virtio::net as vnet;
use crate::kprintln;
use crate::sched::Mutex;
use alloc::collections::BTreeMap;
use core::fmt;
use core::sync::atomic::{AtomicPtr, Ordering};
use otter_net_proto::{
    EthernetBuilder, MacAddr, Ipv4Addr, Ipv4Builder, IcmpEchoBuilder,
};
use otter_netlogic::DhcpClient;

// Re-export public types from submodules
pub use dhcp::Lease;
pub use dns::{resolve, resolve_via};
pub use udp::UdpSocket;

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

/// Network interface configuration and state.
pub struct NetIf {
    mac: [u8; 6],
    pub ipv4: Mutex<[u8; 4]>,
    pub gateway: Mutex<[u8; 4]>,
    pub dns_server: Mutex<[u8; 4]>,
    pub netmask: Mutex<[u8; 4]>,
    pub arp_cache: Mutex<BTreeMap<[u8; 4], [u8; 6]>>,
    pub ping_waiters: Mutex<BTreeMap<u32, crate::mm::addr::VirtAddr>>,
    pub dhcp_lease: Mutex<Option<Lease>>,
    pub dhcp_fallback_printed: Mutex<bool>,
    pub stats: Mutex<NetStats>,
}

/// DHCP and DNS state machines.
pub static DHCP_CLIENT: Mutex<Option<DhcpClient>> = Mutex::new(None);

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
        crate::sched::spawn("net-rx", rx::net_rx_thread, 0);
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
        use crate::time;

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
                let arp_pkt = otter_net_proto::ArpBuilder::request(
                    MacAddr::from_bytes(&self.mac).unwrap(),
                    Ipv4Addr::from_bytes(&our_ip).unwrap(),
                    Ipv4Addr::from_bytes(&ip).unwrap(),
                ).build();

                let frame = otter_net_proto::EthernetBuilder::new(
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
        use crate::time;

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

    pub fn tx_frame(&self, frame: &[u8]) -> Result<(), NetError> {
        if let Some(dev) = vnet::device() {
            dev.transmit(frame).map_err(|_| NetError::TooLarge)?;
            Ok(())
        } else {
            Err(NetError::NoInterface)
        }
    }
}

// Re-export apply_lease for use in submodules
pub(crate) use dhcp::apply_lease;
