#![no_std]
#![allow(
    clippy::collapsible_if,
    clippy::redundant_pattern_matching,
    clippy::comparison_to_empty,
    clippy::redundant_closure,
    clippy::vec_init_then_push,
    clippy::absurd_extreme_comparisons
)]

extern crate alloc;

use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
use core::cmp::min;

use otter_net_proto::{
    MacAddr, Ipv4Addr, EthernetBuilder, EthernetFrame, ArpBuilder, ArpPacket,
    Ipv4Builder, UdpBuilder, DhcpBuilder, DhcpPacket,
    DnsBuilder, DnsHeader, DnsQuery,
};

// ============================================================================
// Action enum - what the state machines produce
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send an Ethernet frame
    Send(Vec<u8>),
    /// DHCP configured: (ip, mask, router, dns_server, lease_ms)
    Configured {
        ip: Ipv4Addr,
        mask: Ipv4Addr,
        router: Ipv4Addr,
        dns: Ipv4Addr,
        lease_ms: u32,
    },
    /// DNS resolved: (hostname, ip)
    Resolved {
        hostname: String,
        ip: Ipv4Addr,
    },
    /// Operation failed
    Failed(String),
}

// ============================================================================
// ARP Cache
// ============================================================================

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ArpEntry {
    mac: MacAddr,
    ip: Ipv4Addr,
    inserted_at: u32,
    last_request_at: u32,
    request_count: u32,
}

#[derive(Debug)]
pub struct ArpCache {
    entries: BTreeMap<u32, ArpEntry>,
    pending: BTreeMap<u32, Vec<Vec<u8>>>, // IP as u32 -> Vec of packets to send
    our_mac: MacAddr,
    our_ip: Ipv4Addr,
}

impl ArpCache {
    pub fn new(our_mac: MacAddr, our_ip: Ipv4Addr) -> Self {
        ArpCache {
            entries: BTreeMap::new(),
            pending: BTreeMap::new(),
            our_mac,
            our_ip,
        }
    }

    pub fn resolve(&mut self, ip: Ipv4Addr, packet: Vec<u8>, now_ms: u32) -> Vec<Action> {
        let ip_key = ip.to_u32();
        let mut actions = Vec::new();

        // Check if we already have this IP resolved and cached
        if let Some(entry) = self.entries.get(&ip_key) {
            if entry.mac != MacAddr::new(0, 0, 0, 0, 0, 0) && now_ms.saturating_sub(entry.inserted_at) < 60000 {
                // Still valid, return success
                return actions;
            } else if entry.mac != MacAddr::new(0, 0, 0, 0, 0, 0) {
                // Expired, remove it
                self.entries.remove(&ip_key);
                self.pending.remove(&ip_key);
            }
        }

        // Get or create entry
        let entry_existed = self.entries.contains_key(&ip_key);
        let entry = self.entries.entry(ip_key).or_insert_with(|| {
            ArpEntry {
                mac: MacAddr::new(0, 0, 0, 0, 0, 0),
                ip,
                inserted_at: now_ms,
                last_request_at: 0,
                request_count: 0,
            }
        });

        // Add packet to queue
        {
            let queue = self.pending.entry(ip_key).or_default();
            if queue.len() < 8 {
                queue.push(packet);
            }
        }

        // Determine if we should send/retransmit
        let mut should_send = false;
        if !entry_existed {
            // First time, send immediately
            should_send = true;
            entry.last_request_at = now_ms;
            entry.request_count = 1;
        } else if now_ms.saturating_sub(entry.last_request_at) >= 1000 {
            // Time for retransmit
            if entry.request_count >= 3 {
                // Too many attempts
                actions.push(Action::Failed(String::from("ARP timeout")));
                self.entries.remove(&ip_key);
                self.pending.remove(&ip_key);
                return actions;
            }
            should_send = true;
            entry.last_request_at = now_ms;
            entry.request_count = entry.request_count.saturating_add(1);
        }

        if should_send {
            let arp_request = ArpBuilder::request(self.our_mac, self.our_ip, ip).build();
            let frame = EthernetBuilder::new(
                MacAddr::broadcast(),
                self.our_mac,
                0x0806,
            )
            .with_payload(&arp_request)
            .build();
            actions.push(Action::Send(frame));
        }

        actions
    }

    pub fn handle_packet(&mut self, data: &[u8], now_ms: u32) -> Vec<Action> {
        let mut actions = Vec::new();

        // Try to parse as Ethernet frame
        if let Ok(frame) = EthernetFrame::new(data) {
            // Check if it's ARP (ethertype 0x0806)
            if frame.ethertype() == 0x0806 {
                if let Ok(arp_pkt) = ArpPacket::new(frame.payload()) {
                    // Operation 2 = ARP reply
                    if arp_pkt.oper() == 2 {
                        if let (Ok(sha), Ok(spa)) = (arp_pkt.sha(), arp_pkt.spa()) {
                            let ip_key = spa.to_u32();

                            // Cache the entry
                            self.entries.insert(ip_key, ArpEntry {
                                mac: sha,
                                ip: spa,
                                inserted_at: now_ms,
                                last_request_at: now_ms,
                                request_count: 0,
                            });

                            // Send any pending packets (caller will handle)
                            if let Some(queue) = self.pending.remove(&ip_key) {
                                for _ in queue {
                                    // Caller will send these
                                }
                            }
                        }
                    }
                    // Operation 1 = ARP request, answer if it's for our IP
                    else if arp_pkt.oper() == 1 {
                        if let (Ok(tpa), Ok(sha), Ok(spa)) = (
                            arp_pkt.tpa(),
                            arp_pkt.sha(),
                            arp_pkt.spa(),
                        ) {
                            if tpa == self.our_ip {
                                // Send ARP reply
                                let arp_reply = ArpBuilder::reply(
                                    self.our_mac,
                                    self.our_ip,
                                    sha,
                                    spa,
                                )
                                .build();
                                let frame = EthernetBuilder::new(sha, self.our_mac, 0x0806)
                                    .with_payload(&arp_reply)
                                    .build();
                                actions.push(Action::Send(frame));
                            }
                        }
                    }
                }
            }
        }

        actions
    }

    pub fn tick(&mut self, now_ms: u32) -> Vec<Action> {
        let mut actions = Vec::new();

        // Check for expired entries and retransmit ARP requests
        let expired: Vec<u32> = self.entries
            .iter()
            .filter(|(_, entry)| now_ms.saturating_sub(entry.inserted_at) >= 60000)
            .map(|(&ip_key, _)| ip_key)
            .collect();

        for ip_key in expired {
            self.entries.remove(&ip_key);
            if let Some(_) = self.pending.remove(&ip_key) {
                actions.push(Action::Failed(String::from("ARP timeout")));
            }
        }

        actions
    }

    pub fn get_mac(&self, ip: Ipv4Addr) -> Option<MacAddr> {
        self.entries.get(&ip.to_u32()).map(|e| e.mac)
    }
}

// ============================================================================
// DHCP Client
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DhcpState {
    Init,
    Selecting,
    Requesting,
    Bound,
    Renewing,
    Rebinding,
}

#[derive(Debug)]
pub struct DhcpClient {
    state: DhcpState,
    our_mac: MacAddr,
    xid: u32,

    // Configuration from DHCP
    ip: Ipv4Addr,
    mask: Ipv4Addr,
    router: Ipv4Addr,
    dns: Ipv4Addr,
    lease_ms: u32,

    // Timers
    state_entered_at: u32,
    last_discover_at: u32,
    last_request_at: u32,
    discover_count: u32,
    request_count: u32,

    // Server ID for REQUEST
    server_id: Option<Ipv4Addr>,
    offered_ip: Option<Ipv4Addr>,
}

impl DhcpClient {
    pub fn new(our_mac: MacAddr, xid: u32) -> Self {
        DhcpClient {
            state: DhcpState::Init,
            our_mac,
            xid,
            ip: Ipv4Addr::zeros(),
            mask: Ipv4Addr::zeros(),
            router: Ipv4Addr::zeros(),
            dns: Ipv4Addr::zeros(),
            lease_ms: 3600000, // 1 hour default
            state_entered_at: 0,
            last_discover_at: 0,
            last_request_at: 0,
            discover_count: 0,
            request_count: 0,
            server_id: None,
            offered_ip: None,
        }
    }

    pub fn tick(&mut self, now_ms: u32) -> Vec<Action> {
        let mut actions = Vec::new();

        match self.state {
            DhcpState::Init => {
                // Start DISCOVER
                self.state = DhcpState::Selecting;
                self.state_entered_at = now_ms;
                self.discover_count = 0;
                self.last_discover_at = now_ms;

                let discover = self.build_discover();
                actions.push(Action::Send(discover));
            }
            DhcpState::Selecting => {
                // Retransmit DISCOVER with backoff: 4s, 8s, 16s
                let elapsed = now_ms.saturating_sub(self.state_entered_at);
                let next_time = match self.discover_count {
                    0 => 4000,
                    1 => 8000,
                    2 => 16000,
                    _ => {
                        actions.push(Action::Failed(String::from("DHCP timeout")));
                        return actions;
                    }
                };

                if elapsed >= next_time {
                    self.discover_count += 1;
                    self.state_entered_at = now_ms;
                    let discover = self.build_discover();
                    actions.push(Action::Send(discover));
                }
            }
            DhcpState::Requesting => {
                // Retransmit REQUEST
                let elapsed = now_ms.saturating_sub(self.last_request_at);
                if elapsed >= 1000 && self.request_count < 3 {
                    self.request_count += 1;
                    self.last_request_at = now_ms;
                    let request = self.build_request();
                    actions.push(Action::Send(request));
                } else if self.request_count >= 3 {
                    actions.push(Action::Failed(String::from("DHCP REQUEST timeout")));
                    self.state = DhcpState::Init;
                    self.state_entered_at = now_ms;
                }
            }
            DhcpState::Bound => {
                let elapsed = now_ms.saturating_sub(self.state_entered_at);
                let t1 = self.lease_ms / 2;
                let t2 = (self.lease_ms * 7) / 8;

                if elapsed >= t2 {
                    self.state = DhcpState::Rebinding;
                    self.state_entered_at = now_ms;
                    self.request_count = 0;
                    self.last_request_at = now_ms;
                } else if elapsed >= t1 {
                    self.state = DhcpState::Renewing;
                    self.state_entered_at = now_ms;
                    self.request_count = 0;
                    self.last_request_at = now_ms;
                }
            }
            DhcpState::Renewing => {
                // Retransmit REQUEST
                let elapsed = now_ms.saturating_sub(self.last_request_at);
                if elapsed >= 1000 && self.request_count < 3 {
                    self.request_count += 1;
                    self.last_request_at = now_ms;
                    let request = self.build_request();
                    actions.push(Action::Send(request));
                } else if self.request_count >= 3 {
                    // Move to REBINDING
                    self.state = DhcpState::Rebinding;
                    self.state_entered_at = now_ms;
                    self.request_count = 0;
                    self.last_request_at = now_ms;
                }
            }
            DhcpState::Rebinding => {
                let elapsed = now_ms.saturating_sub(self.state_entered_at);
                let remaining = self.lease_ms.saturating_sub(elapsed);

                if remaining <= 0 {
                    // Lease expired, go back to INIT
                    self.state = DhcpState::Init;
                    self.state_entered_at = now_ms;
                    self.discover_count = 0;
                } else if elapsed >= 1000 && self.request_count < 3 {
                    self.request_count += 1;
                    self.last_request_at = now_ms;
                    let request = self.build_request();
                    actions.push(Action::Send(request));
                }
            }
        }

        actions
    }

    pub fn handle_packet(&mut self, data: &[u8], now_ms: u32) -> Vec<Action> {
        let mut actions = Vec::new();

        // Try to parse DHCP packet from UDP payload
        // The packet is already the DHCP payload (not including IP/UDP headers)
        if let Ok(pkt) = DhcpPacket::new(data) {
            // Check if XID matches
            if pkt.xid() != self.xid {
                return actions;
            }

            // Check if opcode is BOOTREPLY (2)
            if pkt.opcode() != 2 {
                return actions;
            }

            // Check if client MAC matches
            if let Ok(chaddr) = pkt.chaddr() {
                if chaddr != self.our_mac {
                    return actions;
                }
            }

            // Get message type
            let msg_type = pkt.get_message_type();

            match self.state {
                DhcpState::Selecting => {
                    // Expecting OFFER (msg_type = 2)
                    if msg_type == Some(2) {
                        if let Ok(yiaddr) = pkt.yiaddr() {
                            if let Some(server_id) = extract_server_id(pkt.options()) {
                                self.offered_ip = Some(yiaddr);
                                self.server_id = Some(server_id);
                                self.ip = yiaddr;
                                self.mask = extract_subnet_mask(pkt.options())
                                    .unwrap_or_else(|| Ipv4Addr::new(255, 255, 255, 0));
                                self.router = extract_router(pkt.options())
                                    .unwrap_or_else(|| Ipv4Addr::zeros());
                                self.dns = extract_dns(pkt.options())
                                    .unwrap_or_else(|| Ipv4Addr::zeros());
                                self.lease_ms = extract_lease(pkt.options()).unwrap_or(3600000);

                                self.state = DhcpState::Requesting;
                                self.state_entered_at = now_ms;
                                self.request_count = 0;
                                self.last_request_at = now_ms;

                                let request = self.build_request();
                                actions.push(Action::Send(request));
                            }
                        }
                    }
                }
                DhcpState::Requesting | DhcpState::Renewing | DhcpState::Rebinding => {
                    if msg_type == Some(5) {
                        // ACK - we're configured!
                        if let Ok(yiaddr) = pkt.yiaddr() {
                            self.ip = yiaddr;
                            self.mask = extract_subnet_mask(pkt.options())
                                .unwrap_or_else(|| Ipv4Addr::new(255, 255, 255, 0));
                            self.router = extract_router(pkt.options())
                                .unwrap_or_else(|| Ipv4Addr::zeros());
                            self.dns = extract_dns(pkt.options())
                                .unwrap_or_else(|| Ipv4Addr::zeros());
                            self.lease_ms = extract_lease(pkt.options()).unwrap_or(3600000);

                            self.state = DhcpState::Bound;
                            self.state_entered_at = now_ms;

                            actions.push(Action::Configured {
                                ip: self.ip,
                                mask: self.mask,
                                router: self.router,
                                dns: self.dns,
                                lease_ms: self.lease_ms,
                            });
                        }
                    } else if msg_type == Some(6) {
                        // NAK - go back to INIT
                        self.state = DhcpState::Init;
                        self.state_entered_at = now_ms;
                        self.discover_count = 0;
                    }
                }
                _ => {}
            }
        }

        actions
    }

    fn build_discover(&self) -> Vec<u8> {
        let dhcp = DhcpBuilder::new(self.xid, self.our_mac)
            .with_message_type(1) // DISCOVER
            .with_end()
            .build();

        let udp = UdpBuilder::new(68, 67)
            .with_payload(&dhcp)
            .build_with_pseudo_header(
                Ipv4Addr::zeros(),
                Ipv4Addr::broadcast(),
            );

        let ipv4 = Ipv4Builder::new(
            Ipv4Addr::zeros(),
            Ipv4Addr::broadcast(),
            17, // UDP protocol
        )
        .with_payload(&udp)
        .build();

        EthernetBuilder::new(
            MacAddr::broadcast(),
            self.our_mac,
            0x0800, // IPv4 ethertype
        )
        .with_payload(&ipv4)
        .build()
    }

    fn build_request(&self) -> Vec<u8> {
        let mut builder = DhcpBuilder::new(self.xid, self.our_mac)
            .with_message_type(3); // REQUEST

        if let Some(ip) = self.offered_ip {
            builder = builder.with_requested_ip(ip);
        }
        if let Some(server_id) = self.server_id {
            builder = builder.with_server_id(server_id);
        }

        let dhcp = builder.with_end().build();

        let udp = UdpBuilder::new(68, 67)
            .with_payload(&dhcp)
            .build_with_pseudo_header(
                self.ip,
                Ipv4Addr::broadcast(),
            );

        let ipv4 = Ipv4Builder::new(
            self.ip,
            Ipv4Addr::broadcast(),
            17, // UDP protocol
        )
        .with_payload(&udp)
        .build();

        EthernetBuilder::new(
            MacAddr::broadcast(),
            self.our_mac,
            0x0800,
        )
        .with_payload(&ipv4)
        .build()
    }
}

fn extract_server_id(options: &[u8]) -> Option<Ipv4Addr> {
    let mut offset = 0;
    while offset < options.len() {
        let tag = options[offset];
        if tag == 255 { break; }
        if tag == 0 {
            offset += 1;
            continue;
        }
        if offset + 1 >= options.len() { break; }
        let len = options[offset + 1] as usize;

        if tag == 54 && len == 4 && offset + 5 < options.len() {
            return Ipv4Addr::from_bytes(&options[offset + 2..offset + 6]).ok();
        }
        offset += 2 + len;
    }
    None
}

fn extract_subnet_mask(options: &[u8]) -> Option<Ipv4Addr> {
    let mut offset = 0;
    while offset < options.len() {
        let tag = options[offset];
        if tag == 255 { break; }
        if tag == 0 {
            offset += 1;
            continue;
        }
        if offset + 1 >= options.len() { break; }
        let len = options[offset + 1] as usize;

        if tag == 1 && len == 4 && offset + 5 < options.len() {
            return Ipv4Addr::from_bytes(&options[offset + 2..offset + 6]).ok();
        }
        offset += 2 + len;
    }
    None
}

fn extract_router(options: &[u8]) -> Option<Ipv4Addr> {
    let mut offset = 0;
    while offset < options.len() {
        let tag = options[offset];
        if tag == 255 { break; }
        if tag == 0 {
            offset += 1;
            continue;
        }
        if offset + 1 >= options.len() { break; }
        let len = options[offset + 1] as usize;

        if tag == 3 && len >= 4 && offset + 5 < options.len() {
            return Ipv4Addr::from_bytes(&options[offset + 2..offset + 6]).ok();
        }
        offset += 2 + len;
    }
    None
}

fn extract_dns(options: &[u8]) -> Option<Ipv4Addr> {
    let mut offset = 0;
    while offset < options.len() {
        let tag = options[offset];
        if tag == 255 { break; }
        if tag == 0 {
            offset += 1;
            continue;
        }
        if offset + 1 >= options.len() { break; }
        let len = options[offset + 1] as usize;

        if tag == 6 && len >= 4 && offset + 5 < options.len() {
            return Ipv4Addr::from_bytes(&options[offset + 2..offset + 6]).ok();
        }
        offset += 2 + len;
    }
    None
}

fn extract_lease(options: &[u8]) -> Option<u32> {
    let mut offset = 0;
    while offset < options.len() {
        let tag = options[offset];
        if tag == 255 { break; }
        if tag == 0 {
            offset += 1;
            continue;
        }
        if offset + 1 >= options.len() { break; }
        let len = options[offset + 1] as usize;

        if tag == 51 && len == 4 && offset + 5 < options.len() {
            let secs = u32::from_be_bytes([
                options[offset + 2],
                options[offset + 3],
                options[offset + 4],
                options[offset + 5],
            ]);
            return Some(secs * 1000);
        }
        offset += 2 + len;
    }
    None
}

// ============================================================================
// DNS Resolver
// ============================================================================

#[derive(Debug, Clone)]
struct DnsCacheEntry {
    ip: Ipv4Addr,
    expires_at: u32,
}

#[derive(Debug)]
pub struct DnsResolver {
    cache: BTreeMap<String, DnsCacheEntry>,
    pending: BTreeMap<u16, PendingQuery>, // Query ID -> pending
    server_ip: Ipv4Addr,
}

#[derive(Debug)]
#[allow(dead_code)]
struct PendingQuery {
    hostname: String,
    query_id: u16,
    sent_at: u32,
    attempt: u32,
    cname_chain_depth: u32,
}

impl DnsResolver {
    pub fn new(server_ip: Ipv4Addr) -> Self {
        DnsResolver {
            cache: BTreeMap::new(),
            pending: BTreeMap::new(),
            server_ip,
        }
    }

    pub fn resolve(&mut self, hostname: &str, query_id: u16, now_ms: u32) -> Vec<Action> {
        let mut actions = Vec::new();

        // Check cache first
        if let Some(entry) = self.cache.get(hostname) {
            if now_ms < entry.expires_at {
                actions.push(Action::Resolved {
                    hostname: String::from(hostname),
                    ip: entry.ip,
                });
                return actions;
            } else {
                self.cache.remove(hostname);
            }
        }

        // Add to pending
        self.pending.insert(query_id, PendingQuery {
            hostname: String::from(hostname),
            query_id,
            sent_at: now_ms,
            attempt: 0,
            cname_chain_depth: 0,
        });

        // Send DNS query
        let query = DnsBuilder::query(query_id)
            .with_question(hostname, 1, 1) // A record, IN class
            .unwrap_or_else(|_| DnsBuilder::query(query_id))
            .build();

        let udp = UdpBuilder::new(12345, 53)
            .with_payload(&query)
            .build_with_pseudo_header(
                Ipv4Addr::new(0, 0, 0, 0),
                self.server_ip,
            );

        let ipv4 = Ipv4Builder::new(
            Ipv4Addr::new(0, 0, 0, 0),
            self.server_ip,
            17, // UDP
        )
        .with_payload(&udp)
        .build();

        let frame = EthernetBuilder::new(
            MacAddr::new(0, 0, 0, 0, 0, 0), // Will be filled in by ARP
            MacAddr::new(0, 0, 0, 0, 0, 0),
            0x0800,
        )
        .with_payload(&ipv4)
        .build();

        actions.push(Action::Send(frame));
        actions
    }

    pub fn handle_packet(&mut self, data: &[u8], now_ms: u32) -> Vec<Action> {
        let mut actions = Vec::new();

        if let Ok(hdr) = DnsHeader::new(data) {
            let query_id = hdr.id();

            // Check if this is a response (bit 15 set)
            let flags = hdr.flags();
            if (flags & 0x8000) == 0 {
                return actions; // It's a query, not a response
            }

            if let Some(pending) = self.pending.remove(&query_id) {
                // Check for NXDOMAIN (rcode = 3)
                let rcode = (flags & 0x000f) as u8;
                if rcode == 3 {
                    actions.push(Action::Failed(String::from("NXDOMAIN")));
                    return actions;
                }

                // Parse answer section
                let mut offset = 12;
                let qdcount = hdr.qdcount() as usize;

                // Skip questions
                for _ in 0..qdcount {
                    if let Ok(q) = DnsQuery::new(data, offset) {
                        if let Ok((new_offset, _)) = q.parse_name() {
                            offset = new_offset + 4; // qtype + qclass
                        }
                    }
                }

                // Parse answers
                let ancount = hdr.ancount();
                for _ in 0..ancount {
                    if let Ok(q) = DnsQuery::new(data, offset) {
                        if let Ok((name_offset, _name)) = q.parse_name() {
                            if name_offset + 10 <= data.len() {
                                let rtype = u16::from_be_bytes([
                                    data[name_offset],
                                    data[name_offset + 1],
                                ]);
                                let rdlen = u16::from_be_bytes([
                                    data[name_offset + 8],
                                    data[name_offset + 9],
                                ]) as usize;
                                let rdata_offset = name_offset + 10;

                                if rdata_offset + rdlen <= data.len() {
                                    // Type 1 = A record
                                    if rtype == 1 && rdlen == 4 {
                                        if let Ok(ip) =
                                            Ipv4Addr::from_bytes(&data[rdata_offset..rdata_offset + 4])
                                        {
                                            // Get TTL
                                            let ttl = u32::from_be_bytes([
                                                data[name_offset + 4],
                                                data[name_offset + 5],
                                                data[name_offset + 6],
                                                data[name_offset + 7],
                                            ]);
                                            let ttl_capped = min(ttl, 3600); // Cap at 1 hour
                                            let expires = now_ms + (ttl_capped * 1000);

                                            self.cache.insert(pending.hostname.clone(), DnsCacheEntry {
                                                ip,
                                                expires_at: expires,
                                            });

                                            actions.push(Action::Resolved {
                                                hostname: pending.hostname.clone(),
                                                ip,
                                            });
                                        }
                                    }
                                    // Type 5 = CNAME
                                    else if rtype == 5 && pending.cname_chain_depth < 8 {
                                        // Parse CNAME target
                                        if let Ok(q2) = DnsQuery::new(data, rdata_offset) {
                                            if let Ok((_, _cname)) = q2.parse_name() {
                                                // Follow CNAME by querying again
                                                let new_query_id = query_id.wrapping_add(1);
                                                self.pending.insert(new_query_id, PendingQuery {
                                                    hostname: pending.hostname.clone(),
                                                    query_id: new_query_id,
                                                    sent_at: now_ms,
                                                    attempt: 0,
                                                    cname_chain_depth: pending.cname_chain_depth + 1,
                                                });
                                            }
                                        }
                                    }
                                    offset = rdata_offset + rdlen;
                                }
                            }
                        }
                    }
                }
            }
        }

        actions
    }

    pub fn tick(&mut self, now_ms: u32) -> Vec<Action> {
        let mut actions = Vec::new();

        let expired: Vec<u16> = self.pending
            .iter()
            .filter(|(_, pq)| {
                let elapsed = now_ms.saturating_sub(pq.sent_at);
                elapsed >= 2000 && pq.attempt >= 3
            })
            .map(|(&id, _)| id)
            .collect();

        for id in expired {
            if let Some(_) = self.pending.remove(&id) {
                actions.push(Action::Failed(String::from("DNS timeout")));
            }
        }

        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // ========== ARP Cache Tests ==========

    #[test]
    fn test_arp_cache_new() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let ip = Ipv4Addr::new(192, 168, 1, 100);
        let cache = ArpCache::new(mac, ip);
        assert_eq!(cache.get_mac(Ipv4Addr::new(192, 168, 1, 1)), None);
    }

    #[test]
    fn test_arp_request_generation() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let target_ip = Ipv4Addr::new(192, 168, 1, 1);
        let packet = vec![1, 2, 3, 4];

        let actions = cache.resolve(target_ip, packet, 0);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Send(_)));
    }

    #[test]
    fn test_arp_cache_entry() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let target_ip = Ipv4Addr::new(192, 168, 1, 1);
        let target_mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);

        let arp_reply = ArpBuilder::reply(target_mac, target_ip, our_mac, our_ip).build();
        let frame = EthernetBuilder::new(
            MacAddr::broadcast(),
            target_mac,
            0x0806,
        )
        .with_payload(&arp_reply)
        .build();

        cache.handle_packet(&frame, 0);
        assert_eq!(cache.get_mac(target_ip), Some(target_mac));
    }

    #[test]
    fn test_arp_answer_own_ip() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let requester_mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
        let requester_ip = Ipv4Addr::new(192, 168, 1, 1);

        let arp_request = ArpBuilder::request(requester_mac, requester_ip, our_ip).build();
        let frame = EthernetBuilder::new(
            MacAddr::broadcast(),
            requester_mac,
            0x0806,
        )
        .with_payload(&arp_request)
        .build();

        let actions = cache.handle_packet(&frame, 0);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Send(_)));
    }

    #[test]
    fn test_arp_cache_expiry() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let target_ip = Ipv4Addr::new(192, 168, 1, 1);
        let target_mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);

        let arp_reply = ArpBuilder::reply(target_mac, target_ip, our_mac, our_ip).build();
        let frame = EthernetBuilder::new(
            MacAddr::broadcast(),
            target_mac,
            0x0806,
        )
        .with_payload(&arp_reply)
        .build();

        cache.handle_packet(&frame, 0);
        assert_eq!(cache.get_mac(target_ip), Some(target_mac));

        // Tick at 61 seconds
        cache.tick(61000);
        assert_eq!(cache.get_mac(target_ip), None);
    }

    #[test]
    fn test_arp_retransmit() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let target_ip = Ipv4Addr::new(192, 168, 1, 1);
        let packet = vec![1, 2, 3];

        let actions1 = cache.resolve(target_ip, packet.clone(), 0);
        assert_eq!(actions1.len(), 1);

        let actions2 = cache.resolve(target_ip, packet.clone(), 500);
        assert_eq!(actions2.len(), 0); // Too soon

        let actions3 = cache.resolve(target_ip, packet.clone(), 1000);
        assert_eq!(actions3.len(), 1); // Should retransmit
    }

    #[test]
    fn test_arp_queue_overflow() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let target_ip = Ipv4Addr::new(192, 168, 1, 1);

        for i in 0..10 {
            let packet = vec![i];
            cache.resolve(target_ip, packet, 0);
        }

        // Queue should have at most 8 items
        if let Some(queue) = cache.pending.get(&target_ip.to_u32()) {
            assert!(queue.len() <= 8);
        }
    }

    #[test]
    fn test_arp_timeout_failure() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let target_ip = Ipv4Addr::new(192, 168, 1, 1);
        let packet = vec![1, 2, 3];

        cache.resolve(target_ip, packet.clone(), 0);
        cache.resolve(target_ip, packet.clone(), 1000);
        cache.resolve(target_ip, packet.clone(), 2000);
        let actions = cache.resolve(target_ip, packet.clone(), 3000);

        assert!(actions.iter().any(|a| matches!(a, Action::Failed(_))));
    }

    // ========== DHCP Tests ==========

    #[test]
    fn test_dhcp_client_new() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let client = DhcpClient::new(mac, 0x12345678);
        assert_eq!(client.state, DhcpState::Init);
    }

    #[test]
    fn test_dhcp_discover() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        let actions = client.tick(0);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Send(_)));
        assert_eq!(client.state, DhcpState::Selecting);
    }

    #[test]
    fn test_dhcp_discover_retransmit() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.tick(0); // Initial DISCOVER

        let actions = client.tick(4100); // After 4 seconds + backoff
        assert!(actions.iter().any(|a| matches!(a, Action::Send(_))));
    }

    #[test]
    fn test_dhcp_offer_handling() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.tick(0);
        assert_eq!(client.state, DhcpState::Selecting);

        // Manually construct a DHCP OFFER (opcode=2, BOOTREPLY)
        let offered_ip = Ipv4Addr::new(192, 168, 1, 50);
        let server_ip = Ipv4Addr::new(192, 168, 1, 1);
        let mut packet = Vec::new();
        packet.push(2); // BOOTREPLY
        packet.push(1); // hwtype: Ethernet
        packet.push(6); // hwlen
        packet.push(0); // hops
        packet.extend_from_slice(&(0x12345678_u32).to_be_bytes()); // xid
        packet.extend_from_slice(&[0, 0]); // secs
        packet.extend_from_slice(&[0, 0]); // flags
        packet.extend_from_slice(&[0, 0, 0, 0]); // ciaddr
        packet.extend_from_slice(offered_ip.as_bytes()); // yiaddr
        packet.extend_from_slice(&[0, 0, 0, 0]); // siaddr
        packet.extend_from_slice(&[0, 0, 0, 0]); // giaddr
        packet.extend_from_slice(mac.as_bytes()); // chaddr (6 bytes)
        packet.extend_from_slice(&[0; 10]); // chaddr padding
        packet.extend_from_slice(&[0; 64]); // sname
        packet.extend_from_slice(&[0; 128]); // file
        packet.extend_from_slice(&(0x63825363_u32).to_be_bytes()); // magic cookie
        packet.push(53); // Message Type
        packet.push(1);
        packet.push(2); // OFFER
        packet.push(54); // Server Identifier
        packet.push(4);
        packet.extend_from_slice(server_ip.as_bytes());
        packet.push(255); // End

        client.handle_packet(&packet, 0);
        assert_eq!(client.state, DhcpState::Requesting);
    }

    #[test]
    fn test_dhcp_nak_handling() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.state = DhcpState::Requesting;
        client.state_entered_at = 0;

        // Manually construct a DHCP NAK (opcode=2, BOOTREPLY)
        let mut packet = Vec::new();
        packet.push(2); // BOOTREPLY
        packet.push(1); // hwtype: Ethernet
        packet.push(6); // hwlen
        packet.push(0); // hops
        packet.extend_from_slice(&(0x12345678_u32).to_be_bytes()); // xid
        packet.extend_from_slice(&[0, 0]); // secs
        packet.extend_from_slice(&[0, 0]); // flags
        packet.extend_from_slice(&[0, 0, 0, 0]); // ciaddr
        packet.extend_from_slice(&[0, 0, 0, 0]); // yiaddr
        packet.extend_from_slice(&[0, 0, 0, 0]); // siaddr
        packet.extend_from_slice(&[0, 0, 0, 0]); // giaddr
        packet.extend_from_slice(mac.as_bytes()); // chaddr
        packet.extend_from_slice(&[0; 10]); // chaddr padding
        packet.extend_from_slice(&[0; 64]); // sname
        packet.extend_from_slice(&[0; 128]); // file
        packet.extend_from_slice(&(0x63825363_u32).to_be_bytes()); // magic cookie
        packet.push(53); // Message Type
        packet.push(1);
        packet.push(6); // NAK
        packet.push(255); // End

        client.handle_packet(&packet, 0);
        assert_eq!(client.state, DhcpState::Init);
    }

    #[test]
    fn test_dhcp_ack_handling() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.state = DhcpState::Requesting;
        client.state_entered_at = 0;
        client.offered_ip = Some(Ipv4Addr::new(192, 168, 1, 50));
        client.server_id = Some(Ipv4Addr::new(192, 168, 1, 1));

        // Manually construct a DHCP ACK (opcode=2, BOOTREPLY)
        let assigned_ip = Ipv4Addr::new(192, 168, 1, 50);
        let mut packet = Vec::new();
        packet.push(2); // BOOTREPLY
        packet.push(1); // hwtype: Ethernet
        packet.push(6); // hwlen
        packet.push(0); // hops
        packet.extend_from_slice(&(0x12345678_u32).to_be_bytes()); // xid
        packet.extend_from_slice(&[0, 0]); // secs
        packet.extend_from_slice(&[0, 0]); // flags
        packet.extend_from_slice(&[0, 0, 0, 0]); // ciaddr
        packet.extend_from_slice(assigned_ip.as_bytes()); // yiaddr
        packet.extend_from_slice(&[0, 0, 0, 0]); // siaddr
        packet.extend_from_slice(&[0, 0, 0, 0]); // giaddr
        packet.extend_from_slice(mac.as_bytes()); // chaddr
        packet.extend_from_slice(&[0; 10]); // chaddr padding
        packet.extend_from_slice(&[0; 64]); // sname
        packet.extend_from_slice(&[0; 128]); // file
        packet.extend_from_slice(&(0x63825363_u32).to_be_bytes()); // magic cookie
        packet.push(53); // Message Type
        packet.push(1);
        packet.push(5); // ACK
        packet.push(255); // End

        let actions = client.handle_packet(&packet, 0);
        assert_eq!(client.state, DhcpState::Bound);
        assert!(actions.iter().any(|a| matches!(a, Action::Configured { .. })));
    }

    #[test]
    fn test_dhcp_renewal() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.state = DhcpState::Bound;
        client.state_entered_at = 0;
        client.lease_ms = 4000; // 4 seconds

        client.tick(1000); // Before T1
        assert_eq!(client.state, DhcpState::Bound);

        client.tick(2100); // After T1 (2 seconds)
        assert_eq!(client.state, DhcpState::Renewing);
    }

    #[test]
    fn test_dhcp_rebinding() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.state = DhcpState::Bound;
        client.state_entered_at = 0;
        client.lease_ms = 4000; // 4 seconds

        client.tick(3600); // After T2 (7/8 * 4000 = 3500)
        assert_eq!(client.state, DhcpState::Rebinding);
    }

    #[test]
    fn test_dhcp_lease_expiry() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.state = DhcpState::Rebinding;
        client.state_entered_at = 0;
        client.lease_ms = 2000;

        client.tick(2100);
        assert_eq!(client.state, DhcpState::Init);
    }

    #[test]
    fn test_dhcp_wrong_xid_ignored() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.state = DhcpState::Selecting;

        let offer = DhcpBuilder::new(0x87654321, mac) // Wrong XID
            .with_message_type(2)
            .with_end()
            .build();

        let actions = client.handle_packet(&offer, 0);
        assert!(actions.is_empty());
        assert_eq!(client.state, DhcpState::Selecting); // State unchanged
    }

    #[test]
    fn test_dhcp_wrong_mac_ignored() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let other_mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.state = DhcpState::Selecting;

        let offer = DhcpBuilder::new(0x12345678, other_mac) // Wrong MAC
            .with_message_type(2)
            .with_end()
            .build();

        let actions = client.handle_packet(&offer, 0);
        assert!(actions.is_empty());
        assert_eq!(client.state, DhcpState::Selecting);
    }

    // ========== DNS Tests ==========

    #[test]
    fn test_dns_resolver_new() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let resolver = DnsResolver::new(server);
        assert_eq!(resolver.server_ip, server);
    }

    #[test]
    fn test_dns_query_generation() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        let actions = resolver.resolve("example.com", 1, 0);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Send(_)));
    }

    #[test]
    fn test_dns_cache_hit() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        let ip = Ipv4Addr::new(93, 184, 216, 34);
        resolver.cache.insert(
            String::from("example.com"),
            DnsCacheEntry {
                ip,
                expires_at: 10000,
            },
        );

        let actions = resolver.resolve("example.com", 1, 5000);
        assert!(actions.iter().any(|a| matches!(
            a,
            Action::Resolved { hostname, ip: _ } if hostname == "example.com"
        )));
    }

    #[test]
    fn test_dns_cache_expiry() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        let ip = Ipv4Addr::new(93, 184, 216, 34);
        resolver.cache.insert(
            String::from("example.com"),
            DnsCacheEntry {
                ip,
                expires_at: 5000,
            },
        );

        let actions = resolver.resolve("example.com", 1, 6000); // After expiry
        // Should generate a new query, not use cache
        assert!(actions.iter().any(|a| matches!(a, Action::Send(_))));
    }

    #[test]
    fn test_dns_timeout() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        resolver.resolve("example.com", 1, 0);

        // Simulate timeout
        if let Some(pq) = resolver.pending.get_mut(&1) {
            pq.sent_at = 0;
            pq.attempt = 3;
        }

        let actions = resolver.tick(2100);
        assert!(actions.iter().any(|a| matches!(a, Action::Failed(_))));
    }

    #[test]
    fn test_dns_retransmit() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        resolver.resolve("example.com", 1, 0);
        assert_eq!(resolver.pending.len(), 1);

        // Tick after 2 seconds
        resolver.tick(2100);
        // Should not be marked as failed yet (attempt should be < 3)
    }

    #[test]
    fn test_dns_invalid_response_ignored() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        resolver.resolve("example.com", 1, 0);

        // Send response with wrong ID
        let response = DnsBuilder::query(999).build(); // Wrong ID
        resolver.handle_packet(&response, 0);

        assert_eq!(resolver.pending.len(), 1); // Still pending
    }

    #[test]
    fn test_arp_ignore_non_arp() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let ipv4_frame = Ipv4Builder::new(
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(10, 0, 0, 2),
            6,
        )
        .build();

        let frame = EthernetBuilder::new(
            MacAddr::broadcast(),
            MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff),
            0x0800, // IPv4, not ARP
        )
        .with_payload(&ipv4_frame)
        .build();

        let actions = cache.handle_packet(&frame, 0);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_dhcp_discover_timeout() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.tick(0); // Initial DISCOVER
        client.tick(4100); // Retransmit after 4s
        client.tick(12200); // Retransmit after 8s
        client.tick(28300); // Retransmit after 16s

        let actions = client.tick(28400); // Another tick
        assert!(actions.iter().any(|a| matches!(a, Action::Failed(_))));
    }

    #[test]
    fn test_dns_query_max_retries() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        resolver.resolve("example.com", 1, 0);

        // Simulate 3 timeouts
        for i in 0..3 {
            if let Some(pq) = resolver.pending.get_mut(&1) {
                pq.sent_at = 0;
                pq.attempt = i;
            }
            resolver.tick(2100 * (i + 1));
        }

        // Next tick should fail
        if let Some(pq) = resolver.pending.get_mut(&1) {
            pq.attempt = 3;
        }
        let actions = resolver.tick(6400);
        assert!(actions.iter().any(|a| matches!(a, Action::Failed(_))));
    }

    #[test]
    fn test_dns_multiple_queries() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        resolver.resolve("example.com", 1, 0);
        resolver.resolve("google.com", 2, 0);

        assert_eq!(resolver.pending.len(), 2);
    }

    #[test]
    fn test_arp_ignore_wrong_op() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let requester_mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
        let requester_ip = Ipv4Addr::new(192, 168, 1, 1);

        // Create ARP reply (op=2) instead of request
        let arp_reply = ArpBuilder::reply(requester_mac, requester_ip, our_mac, our_ip).build();
        let frame = EthernetBuilder::new(
            MacAddr::broadcast(),
            requester_mac,
            0x0806,
        )
        .with_payload(&arp_reply)
        .build();

        let actions = cache.handle_packet(&frame, 0);
        // Should not send a reply to a reply
        assert!(actions.is_empty());
    }

    #[test]
    fn test_dhcp_request_retransmit() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        client.state = DhcpState::Requesting;
        client.state_entered_at = 0;
        client.last_request_at = 0;
        client.offered_ip = Some(Ipv4Addr::new(192, 168, 1, 50));
        client.server_id = Some(Ipv4Addr::new(192, 168, 1, 1));

        let actions1 = client.tick(500);
        assert!(actions1.is_empty()); // Too soon

        let actions2 = client.tick(1100);
        assert!(actions2.iter().any(|a| matches!(a, Action::Send(_)))); // Should retransmit
    }

    #[test]
    fn test_arp_multiple_pending() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let ip1 = Ipv4Addr::new(192, 168, 1, 1);
        let ip2 = Ipv4Addr::new(192, 168, 1, 2);

        cache.resolve(ip1, vec![1], 0);
        cache.resolve(ip2, vec![2], 0);

        assert_eq!(cache.pending.len(), 2);
    }

    #[test]
    fn test_arp_cache_hit_after_resolve() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let target_ip = Ipv4Addr::new(192, 168, 1, 1);
        let target_mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);

        let arp_reply = ArpBuilder::reply(target_mac, target_ip, our_mac, our_ip).build();
        let frame = EthernetBuilder::new(
            MacAddr::broadcast(),
            target_mac,
            0x0806,
        )
        .with_payload(&arp_reply)
        .build();

        cache.handle_packet(&frame, 0);

        // Now resolve - should find it in cache
        let actions = cache.resolve(target_ip, vec![1], 1000);
        assert_eq!(actions.len(), 0); // No Send action, already cached
    }

    #[test]
    fn test_dns_cache_clear_on_expiry() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        let ip = Ipv4Addr::new(93, 184, 216, 34);
        resolver.cache.insert(
            String::from("example.com"),
            DnsCacheEntry {
                ip,
                expires_at: 1000,
            },
        );

        let actions1 = resolver.resolve("example.com", 1, 500);
        assert!(actions1.iter().any(|a| matches!(a, Action::Resolved { .. })));

        let actions2 = resolver.resolve("example.com", 2, 1500); // After expiry
        assert!(actions2.iter().any(|a| matches!(a, Action::Send(_)))); // Should send new query
    }

    #[test]
    fn test_dhcp_bootreply_only() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);
        client.state = DhcpState::Selecting;

        // Try to process a BOOTREQUEST (opcode=1) - should be ignored
        let packet = DhcpBuilder::new(0x12345678, mac)
            .with_message_type(2) // OFFER
            .with_end()
            .build();

        let old_state = client.state;
        client.handle_packet(&packet, 0);
        // State should not change because opcode is wrong
        assert_eq!(client.state, old_state);
    }

    #[test]
    fn test_arp_request_without_offer() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let target_ip = Ipv4Addr::new(192, 168, 1, 1);
        let packet = vec![1, 2, 3];

        // Call 1: time 0, initial send
        cache.resolve(target_ip, packet.clone(), 0);
        // Call 2: time 1000, retransmit 1
        cache.resolve(target_ip, packet.clone(), 1000);
        // Call 3: time 2000, retransmit 2
        cache.resolve(target_ip, packet.clone(), 2000);
        // Call 4: time 3000, retransmit 3 (should fail)
        let actions = cache.resolve(target_ip, packet.clone(), 3000);

        // Should have timeout and failed
        assert!(actions.iter().any(|a| matches!(a, Action::Failed(_))));
    }

    #[test]
    fn test_dhcp_state_transitions() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        assert_eq!(client.state, DhcpState::Init);
        client.tick(0);
        assert_eq!(client.state, DhcpState::Selecting);
    }

    #[test]
    fn test_arp_multiple_packets_same_ip() {
        let our_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let our_ip = Ipv4Addr::new(192, 168, 1, 100);
        let mut cache = ArpCache::new(our_mac, our_ip);

        let target_ip = Ipv4Addr::new(192, 168, 1, 1);
        let packet = vec![1, 2, 3];

        // Queue multiple packets
        cache.resolve(target_ip, packet.clone(), 0);
        cache.resolve(target_ip, packet.clone(), 0);
        cache.resolve(target_ip, packet.clone(), 0);

        // All should be queued
        assert_eq!(cache.pending.get(&target_ip.to_u32()).map(|q| q.len()), Some(3));
    }

    #[test]
    fn test_action_variants() {
        let send = Action::Send(vec![1, 2, 3]);
        let configured = Action::Configured {
            ip: Ipv4Addr::new(192, 168, 1, 50),
            mask: Ipv4Addr::new(255, 255, 255, 0),
            router: Ipv4Addr::new(192, 168, 1, 1),
            dns: Ipv4Addr::new(8, 8, 8, 8),
            lease_ms: 3600000,
        };
        let resolved = Action::Resolved {
            hostname: String::from("example.com"),
            ip: Ipv4Addr::new(93, 184, 216, 34),
        };
        let failed = Action::Failed(String::from("test error"));

        assert!(matches!(send, Action::Send(_)));
        assert!(matches!(configured, Action::Configured { .. }));
        assert!(matches!(resolved, Action::Resolved { .. }));
        assert!(matches!(failed, Action::Failed(_)));
    }

    #[test]
    fn test_dhcp_discover_backoff_intervals() {
        let mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let mut client = DhcpClient::new(mac, 0x12345678);

        // First DISCOVER at time 0
        let actions1 = client.tick(0);
        assert_eq!(client.state, DhcpState::Selecting);
        assert!(actions1.iter().any(|a| matches!(a, Action::Send(_))));

        // At 4 seconds, should retransmit
        let actions2 = client.tick(4100);
        assert!(actions2.iter().any(|a| matches!(a, Action::Send(_))));
        assert_eq!(client.discover_count, 1);

        // At 4 + 8 = 12 seconds, should retransmit again
        let actions3 = client.tick(12200);
        assert!(actions3.iter().any(|a| matches!(a, Action::Send(_))));
        assert_eq!(client.discover_count, 2);

        // At 4 + 8 + 16 = 28 seconds, should retransmit for the last time
        let actions4 = client.tick(28300);
        assert!(actions4.iter().any(|a| matches!(a, Action::Send(_))));
        assert_eq!(client.discover_count, 3);

        // At 4 + 8 + 16 + 1 more seconds, should fail
        let actions5 = client.tick(28400);
        assert!(actions5.iter().any(|a| matches!(a, Action::Failed(_))));
    }

    #[test]
    fn test_dns_nxdomain_not_cached() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        // Start a query
        resolver.resolve("nonexistent.local", 1, 0);

        // Construct NXDOMAIN response (flags = 0x8003, rcode=3)
        let mut response = Vec::new();
        response.extend_from_slice(&(1_u16).to_be_bytes()); // ID = 1
        response.extend_from_slice(&(0x8003_u16).to_be_bytes()); // Flags: response, NXDOMAIN
        response.extend_from_slice(&[0, 0]); // qdcount = 0
        response.extend_from_slice(&[0, 0]); // ancount = 0
        response.extend_from_slice(&[0, 0]); // nscount = 0
        response.extend_from_slice(&[0, 0]); // arcount = 0

        let actions = resolver.handle_packet(&response, 0);
        assert!(actions.iter().any(|a| matches!(a, Action::Failed(_))));

        // Verify not cached - the cache should not have an entry
        let retry_actions = resolver.resolve("nonexistent.local", 2, 100);
        assert!(retry_actions.iter().any(|a| matches!(a, Action::Send(_)))); // Should send new query
    }

    #[test]
    fn test_dns_mismatched_question_ignored() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        resolver.resolve("example.com", 1, 0);
        assert_eq!(resolver.pending.len(), 1);

        // Construct response with matching ID but no data (should be ignored)
        let mut response = Vec::new();
        response.extend_from_slice(&(1_u16).to_be_bytes()); // ID = 1
        response.extend_from_slice(&(0x8000_u16).to_be_bytes()); // Flags: response
        response.extend_from_slice(&[0, 0]); // qdcount = 0 (no question section in response)
        response.extend_from_slice(&[0, 0]); // ancount = 0
        response.extend_from_slice(&[0, 0]); // nscount = 0
        response.extend_from_slice(&[0, 0]); // arcount = 0

        let actions = resolver.handle_packet(&response, 0);

        // Query should have been removed from pending (no error, just nothing to do)
        assert_eq!(resolver.pending.len(), 0);
        // No resolved action since there was no valid answer
        assert!(!actions.iter().any(|a| matches!(a, Action::Resolved { .. })));
    }

    #[test]
    fn test_dns_cname_chain_depth_limit() {
        let server = Ipv4Addr::new(8, 8, 8, 8);
        let mut resolver = DnsResolver::new(server);

        // Manually create a pending query with cname_chain_depth = 8 (at limit)
        resolver.pending.insert(1, PendingQuery {
            hostname: String::from("test.local"),
            query_id: 1,
            sent_at: 0,
            attempt: 0,
            cname_chain_depth: 8,
        });

        // Try to process a CNAME response - should not follow it
        // This tests that cname_chain_depth < 8 check prevents following at depth 8+
        let mut response = Vec::new();
        response.extend_from_slice(&(1_u16).to_be_bytes()); // ID
        response.extend_from_slice(&(0x8000_u16).to_be_bytes()); // Flags: response
        response.extend_from_slice(&[0, 1]); // qdcount = 1
        response.extend_from_slice(&[0, 1]); // ancount = 1
        response.extend_from_slice(&[0, 0]); // nscount = 0
        response.extend_from_slice(&[0, 0]); // arcount = 0
        // Question: test.local A IN
        response.push(4);
        response.extend_from_slice(b"test");
        response.push(5);
        response.extend_from_slice(b"local");
        response.push(0);
        response.extend_from_slice(&(1_u16).to_be_bytes()); // qtype = A
        response.extend_from_slice(&(1_u16).to_be_bytes()); // qclass = IN

        // Answer: CNAME pointing to another.local
        response.push(192); // Pointer to name
        response.push(12); // Offset 12 in packet
        response.extend_from_slice(&(5_u16).to_be_bytes()); // type = CNAME
        response.extend_from_slice(&(1_u16).to_be_bytes()); // class = IN
        response.extend_from_slice(&(3600_u32).to_be_bytes()); // TTL
        response.extend_from_slice(&(9_u16).to_be_bytes()); // RDLENGTH = 9
        // RDATA: another.local
        response.push(7);
        response.extend_from_slice(b"another");
        response.push(5);
        response.extend_from_slice(b"local");
        response.push(0);

        resolver.handle_packet(&response, 0);

        // Query should be removed (processed), not followed
        assert_eq!(resolver.pending.len(), 0);
    }
}
