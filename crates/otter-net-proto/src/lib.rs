#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

// ============================================================================
// MAC Address
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacAddr([u8; 6]);

impl MacAddr {
    pub const fn new(a: u8, b: u8, c: u8, d: u8, e: u8, f: u8) -> Self {
        MacAddr([a, b, c, d, e, f])
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 6 {
            return Err("MAC address requires 6 bytes");
        }
        Ok(MacAddr([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]]))
    }

    pub fn broadcast() -> Self {
        MacAddr([0xff, 0xff, 0xff, 0xff, 0xff, 0xff])
    }

    pub fn as_bytes(&self) -> &[u8; 6] {
        &self.0
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

// ============================================================================
// IPv4 Address
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4Addr([u8; 4]);

impl Ipv4Addr {
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Ipv4Addr([a, b, c, d])
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 4 {
            return Err("IPv4 address requires 4 bytes");
        }
        Ok(Ipv4Addr([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }

    pub fn to_u32(&self) -> u32 {
        u32::from_be_bytes(self.0)
    }

    pub fn from_u32(addr: u32) -> Self {
        Ipv4Addr(addr.to_be_bytes())
    }

    pub fn broadcast() -> Self {
        Ipv4Addr([255, 255, 255, 255])
    }

    pub fn zeros() -> Self {
        Ipv4Addr([0, 0, 0, 0])
    }
}

impl fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

// ============================================================================
// Checksum utilities
// ============================================================================

pub mod checksum {
    /// RFC 1071: ones' complement checksum
    pub fn checksum(data: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        let mut i = 0;
        while i + 1 < data.len() {
            sum += u32::from_be_bytes([0, 0, data[i], data[i + 1]]);
            i += 2;
        }
        if i < data.len() {
            sum += u32::from_be_bytes([0, 0, data[i], 0]);
        }
        while (sum >> 16) > 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    pub fn verify(data: &[u8]) -> bool {
        checksum(data) == 0
    }
}

// ============================================================================
// Ethernet II
// ============================================================================

pub struct EthernetFrame<'a> {
    data: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 14 {
            return Err("Ethernet frame too short");
        }
        Ok(EthernetFrame { data })
    }

    pub fn dst(&self) -> Result<MacAddr, &'static str> {
        MacAddr::from_bytes(&self.data[0..6])
    }

    pub fn src(&self) -> Result<MacAddr, &'static str> {
        MacAddr::from_bytes(&self.data[6..12])
    }

    pub fn ethertype(&self) -> u16 {
        u16::from_be_bytes([self.data[12], self.data[13]])
    }

    pub fn payload(&self) -> &'a [u8] {
        if self.data.len() > 14 {
            &self.data[14..]
        } else {
            &[]
        }
    }
}

pub struct EthernetBuilder {
    data: Vec<u8>,
}

impl EthernetBuilder {
    pub fn new(dst: MacAddr, src: MacAddr, ethertype: u16) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(dst.as_bytes());
        data.extend_from_slice(src.as_bytes());
        data.extend_from_slice(&ethertype.to_be_bytes());
        EthernetBuilder { data }
    }

    pub fn with_payload(mut self, payload: &[u8]) -> Self {
        self.data.extend_from_slice(payload);
        self
    }

    pub fn build(self) -> Vec<u8> {
        self.data
    }
}

// ============================================================================
// ARP
// ============================================================================

pub struct ArpPacket<'a> {
    data: &'a [u8],
}

impl<'a> ArpPacket<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 28 {
            return Err("ARP packet too short");
        }
        Ok(ArpPacket { data })
    }

    pub fn hwtype(&self) -> u16 {
        u16::from_be_bytes([self.data[0], self.data[1]])
    }

    pub fn ptype(&self) -> u16 {
        u16::from_be_bytes([self.data[2], self.data[3]])
    }

    pub fn hwlen(&self) -> u8 {
        self.data[4]
    }

    pub fn plen(&self) -> u8 {
        self.data[5]
    }

    pub fn oper(&self) -> u16 {
        u16::from_be_bytes([self.data[6], self.data[7]])
    }

    pub fn sha(&self) -> Result<MacAddr, &'static str> {
        MacAddr::from_bytes(&self.data[8..14])
    }

    pub fn spa(&self) -> Result<Ipv4Addr, &'static str> {
        Ipv4Addr::from_bytes(&self.data[14..18])
    }

    pub fn tha(&self) -> Result<MacAddr, &'static str> {
        MacAddr::from_bytes(&self.data[18..24])
    }

    pub fn tpa(&self) -> Result<Ipv4Addr, &'static str> {
        Ipv4Addr::from_bytes(&self.data[24..28])
    }
}

pub struct ArpBuilder {
    data: Vec<u8>,
}

impl ArpBuilder {
    // Ethernet/IPv4 ARP
    pub fn request(sender_mac: MacAddr, sender_ip: Ipv4Addr, target_ip: Ipv4Addr) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&(1_u16).to_be_bytes()); // hwtype: Ethernet
        data.extend_from_slice(&(0x0800_u16).to_be_bytes()); // ptype: IPv4
        data.push(6); // hwlen
        data.push(4); // plen
        data.extend_from_slice(&(1_u16).to_be_bytes()); // oper: REQUEST
        data.extend_from_slice(sender_mac.as_bytes());
        data.extend_from_slice(sender_ip.as_bytes());
        data.extend_from_slice(MacAddr([0, 0, 0, 0, 0, 0]).as_bytes());
        data.extend_from_slice(target_ip.as_bytes());
        ArpBuilder { data }
    }

    pub fn reply(sender_mac: MacAddr, sender_ip: Ipv4Addr, target_mac: MacAddr, target_ip: Ipv4Addr) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&(1_u16).to_be_bytes()); // hwtype: Ethernet
        data.extend_from_slice(&(0x0800_u16).to_be_bytes()); // ptype: IPv4
        data.push(6); // hwlen
        data.push(4); // plen
        data.extend_from_slice(&(2_u16).to_be_bytes()); // oper: REPLY
        data.extend_from_slice(sender_mac.as_bytes());
        data.extend_from_slice(sender_ip.as_bytes());
        data.extend_from_slice(target_mac.as_bytes());
        data.extend_from_slice(target_ip.as_bytes());
        ArpBuilder { data }
    }

    pub fn build(self) -> Vec<u8> {
        self.data
    }
}

// ============================================================================
// IPv4
// ============================================================================

pub struct Ipv4Header<'a> {
    data: &'a [u8],
}

impl<'a> Ipv4Header<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 20 {
            return Err("IPv4 header too short");
        }
        let version_ihl = data[0];
        let ihl = (version_ihl & 0x0f) as usize;
        let header_len = ihl * 4;
        if header_len < 20 || data.len() < header_len {
            return Err("Invalid IPv4 IHL");
        }
        // The packet ends at Total Length, not at the end of the frame: Ethernet pads short
        // frames to 60 bytes, and that padding is not payload (RFC 791, RFC 894).
        let total = u16::from_be_bytes([data[2], data[3]]) as usize;
        if total < header_len || total > data.len() {
            return Err("Invalid IPv4 total length");
        }
        Ok(Ipv4Header { data: &data[..total] })
    }

    pub fn version(&self) -> u8 {
        (self.data[0] >> 4) & 0x0f
    }

    pub fn ihl(&self) -> u8 {
        self.data[0] & 0x0f
    }

    pub fn header_len(&self) -> usize {
        (self.ihl() as usize) * 4
    }

    pub fn dscp(&self) -> u8 {
        (self.data[1] >> 2) & 0x3f
    }

    pub fn ecn(&self) -> u8 {
        self.data[1] & 0x03
    }

    pub fn total_length(&self) -> u16 {
        u16::from_be_bytes([self.data[2], self.data[3]])
    }

    pub fn identification(&self) -> u16 {
        u16::from_be_bytes([self.data[4], self.data[5]])
    }

    pub fn flags(&self) -> u8 {
        (self.data[6] >> 5) & 0x07
    }

    pub fn frag_offset(&self) -> u16 {
        u16::from_be_bytes([self.data[6] & 0x1f, self.data[7]]) & 0x1fff
    }

    pub fn ttl(&self) -> u8 {
        self.data[8]
    }

    pub fn protocol(&self) -> u8 {
        self.data[9]
    }

    pub fn checksum(&self) -> u16 {
        u16::from_be_bytes([self.data[10], self.data[11]])
    }

    pub fn src(&self) -> Result<Ipv4Addr, &'static str> {
        Ipv4Addr::from_bytes(&self.data[12..16])
    }

    pub fn dst(&self) -> Result<Ipv4Addr, &'static str> {
        Ipv4Addr::from_bytes(&self.data[16..20])
    }

    pub fn verify_checksum(&self) -> bool {
        checksum::verify(&self.data[..self.header_len()])
    }

    pub fn payload(&self) -> &'a [u8] {
        let offset = self.header_len();
        if offset < self.data.len() {
            &self.data[offset..]
        } else {
            &[]
        }
    }
}

pub struct Ipv4Builder {
    data: Vec<u8>,
}

impl Ipv4Builder {
    pub fn new(src: Ipv4Addr, dst: Ipv4Addr, protocol: u8) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x45, 0, 0, 0, 0, 0, 0, 0, 64, protocol, 0, 0]);
        data.extend_from_slice(src.as_bytes());
        data.extend_from_slice(dst.as_bytes());
        Ipv4Builder { data }
    }

    pub fn with_payload(mut self, payload: &[u8]) -> Self {
        self.data.extend_from_slice(payload);
        self
    }

    pub fn build(mut self) -> Vec<u8> {
        let total_len = self.data.len() as u16;
        self.data[2..4].copy_from_slice(&total_len.to_be_bytes());

        // Compute and set checksum
        self.data[10..12].copy_from_slice(&[0, 0]);
        let cs = checksum::checksum(&self.data[..20]);
        self.data[10..12].copy_from_slice(&cs.to_be_bytes());

        self.data
    }
}

// ============================================================================
// ICMP
// ============================================================================

pub struct IcmpEchoRequest<'a> {
    data: &'a [u8],
}

impl<'a> IcmpEchoRequest<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 8 {
            return Err("ICMP echo request too short");
        }
        Ok(IcmpEchoRequest { data })
    }

    pub fn icmp_type(&self) -> u8 {
        self.data[0]
    }

    pub fn code(&self) -> u8 {
        self.data[1]
    }

    pub fn checksum(&self) -> u16 {
        u16::from_be_bytes([self.data[2], self.data[3]])
    }

    pub fn identifier(&self) -> u16 {
        u16::from_be_bytes([self.data[4], self.data[5]])
    }

    pub fn sequence(&self) -> u16 {
        u16::from_be_bytes([self.data[6], self.data[7]])
    }

    pub fn payload(&self) -> &'a [u8] {
        if self.data.len() > 8 {
            &self.data[8..]
        } else {
            &[]
        }
    }

    pub fn verify_checksum(&self) -> bool {
        checksum::verify(self.data)
    }
}

pub struct IcmpEchoBuilder {
    data: Vec<u8>,
}

impl IcmpEchoBuilder {
    pub fn request(identifier: u16, sequence: u16) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&[8, 0, 0, 0]);
        data.extend_from_slice(&identifier.to_be_bytes());
        data.extend_from_slice(&sequence.to_be_bytes());
        IcmpEchoBuilder { data }
    }

    pub fn reply(identifier: u16, sequence: u16) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&[0, 0, 0, 0]);
        data.extend_from_slice(&identifier.to_be_bytes());
        data.extend_from_slice(&sequence.to_be_bytes());
        IcmpEchoBuilder { data }
    }

    pub fn with_payload(mut self, payload: &[u8]) -> Self {
        self.data.extend_from_slice(payload);
        self
    }

    pub fn build(mut self) -> Vec<u8> {
        self.data[2..4].copy_from_slice(&[0, 0]);
        let cs = checksum::checksum(&self.data);
        self.data[2..4].copy_from_slice(&cs.to_be_bytes());
        self.data
    }
}

// ============================================================================
// UDP
// ============================================================================

pub struct UdpPacket<'a> {
    data: &'a [u8],
}

impl<'a> UdpPacket<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 8 {
            return Err("UDP packet too short");
        }
        // The datagram ends at its Length field (RFC 768); anything after it is not payload.
        let length = u16::from_be_bytes([data[4], data[5]]) as usize;
        if length < 8 || length > data.len() {
            return Err("Invalid UDP length");
        }
        Ok(UdpPacket { data: &data[..length] })
    }

    pub fn src_port(&self) -> u16 {
        u16::from_be_bytes([self.data[0], self.data[1]])
    }

    pub fn dst_port(&self) -> u16 {
        u16::from_be_bytes([self.data[2], self.data[3]])
    }

    pub fn length(&self) -> u16 {
        u16::from_be_bytes([self.data[4], self.data[5]])
    }

    pub fn checksum(&self) -> u16 {
        u16::from_be_bytes([self.data[6], self.data[7]])
    }

    pub fn payload(&self) -> &'a [u8] {
        if self.data.len() > 8 {
            &self.data[8..]
        } else {
            &[]
        }
    }

    /// Verify UDP checksum (RFC 768). A transmitted checksum of 0 means "no checksum computed".
    /// Otherwise the one's-complement sum over the pseudo-header and the datagram as received
    /// (checksum field included) must be 0xFFFF; this also accepts the 0xFFFF a sender transmits
    /// when the computed checksum is 0.
    pub fn verify_checksum(&self, src: Ipv4Addr, dst: Ipv4Addr) -> bool {
        if self.checksum() == 0 {
            return true;
        }
        let mut check_data = Vec::with_capacity(12 + self.data.len());
        check_data.extend_from_slice(src.as_bytes());
        check_data.extend_from_slice(dst.as_bytes());
        check_data.push(0);
        check_data.push(17);
        check_data.extend_from_slice(&self.length().to_be_bytes());
        check_data.extend_from_slice(self.data);
        checksum::verify(&check_data)
    }
}

pub struct UdpBuilder {
    data: Vec<u8>,
}

impl UdpBuilder {
    pub fn new(src_port: u16, dst_port: u16) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&src_port.to_be_bytes());
        data.extend_from_slice(&dst_port.to_be_bytes());
        data.push(0); // length (will update)
        data.push(0);
        data.push(0); // checksum (will update)
        data.push(0);
        UdpBuilder { data }
    }

    pub fn with_payload(mut self, payload: &[u8]) -> Self {
        self.data.extend_from_slice(payload);
        self
    }

    pub fn build_with_pseudo_header(mut self, src: Ipv4Addr, dst: Ipv4Addr) -> Vec<u8> {
        let length = self.data.len() as u16;
        self.data[4..6].copy_from_slice(&length.to_be_bytes());

        // Compute pseudo-header checksum
        let mut pseudo = Vec::new();
        pseudo.extend_from_slice(src.as_bytes());
        pseudo.extend_from_slice(dst.as_bytes());
        pseudo.push(0); // reserved
        pseudo.push(17); // protocol: UDP
        pseudo.extend_from_slice(&length.to_be_bytes());
        pseudo.extend_from_slice(&self.data);

        self.data[6..8].copy_from_slice(&[0, 0]);
        let cs = checksum::checksum(&pseudo);
        self.data[6..8].copy_from_slice(&cs.to_be_bytes());

        self.data
    }
}

// ============================================================================
// DHCP
// ============================================================================

pub struct DhcpPacket<'a> {
    data: &'a [u8],
}

impl<'a> DhcpPacket<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 240 {
            return Err("DHCP packet too short");
        }
        Ok(DhcpPacket { data })
    }

    pub fn opcode(&self) -> u8 {
        self.data[0]
    }

    pub fn hwtype(&self) -> u8 {
        self.data[1]
    }

    pub fn hwlen(&self) -> u8 {
        self.data[2]
    }

    pub fn hops(&self) -> u8 {
        self.data[3]
    }

    pub fn xid(&self) -> u32 {
        u32::from_be_bytes([self.data[4], self.data[5], self.data[6], self.data[7]])
    }

    pub fn secs(&self) -> u16 {
        u16::from_be_bytes([self.data[8], self.data[9]])
    }

    pub fn flags(&self) -> u16 {
        u16::from_be_bytes([self.data[10], self.data[11]])
    }

    pub fn ciaddr(&self) -> Result<Ipv4Addr, &'static str> {
        Ipv4Addr::from_bytes(&self.data[12..16])
    }

    pub fn yiaddr(&self) -> Result<Ipv4Addr, &'static str> {
        Ipv4Addr::from_bytes(&self.data[16..20])
    }

    pub fn siaddr(&self) -> Result<Ipv4Addr, &'static str> {
        Ipv4Addr::from_bytes(&self.data[20..24])
    }

    pub fn giaddr(&self) -> Result<Ipv4Addr, &'static str> {
        Ipv4Addr::from_bytes(&self.data[24..28])
    }

    pub fn chaddr(&self) -> Result<MacAddr, &'static str> {
        MacAddr::from_bytes(&self.data[28..34])
    }

    pub fn magic_cookie(&self) -> u32 {
        u32::from_be_bytes([self.data[236], self.data[237], self.data[238], self.data[239]])
    }

    pub fn options(&self) -> &'a [u8] {
        if self.data.len() > 240 {
            &self.data[240..]
        } else {
            &[]
        }
    }

    pub fn get_message_type(&self) -> Option<u8> {
        let mut offset = 0;
        let opts = self.options();
        while offset < opts.len() {
            let tag = opts[offset];
            if tag == 255 { // End
                break;
            }
            if tag == 0 { // Pad
                offset += 1;
                continue;
            }
            if offset + 1 >= opts.len() {
                break;
            }
            let len = opts[offset + 1] as usize;
            if tag == 53 && len == 1 && offset + 2 < opts.len() {
                return Some(opts[offset + 2]);
            }
            offset += 2 + len;
        }
        None
    }
}

pub struct DhcpBuilder {
    data: Vec<u8>,
}

impl DhcpBuilder {
    pub fn new(xid: u32, client_mac: MacAddr) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&[1, 1, 6, 0]);
        data.extend_from_slice(&xid.to_be_bytes());
        data.extend_from_slice(&[0, 0]); // secs
        data.extend_from_slice(&[0, 0]); // flags
        data.extend_from_slice(Ipv4Addr::zeros().as_bytes()); // ciaddr
        data.extend_from_slice(Ipv4Addr::zeros().as_bytes()); // yiaddr
        data.extend_from_slice(Ipv4Addr::zeros().as_bytes()); // siaddr
        data.extend_from_slice(Ipv4Addr::zeros().as_bytes()); // giaddr
        data.extend_from_slice(client_mac.as_bytes());
        data.extend_from_slice(&[0; 10]); // chaddr padding (to make chaddr 16 bytes)
        data.extend_from_slice(&[0; 64]); // sname
        data.extend_from_slice(&[0; 128]); // file
        data.extend_from_slice(&(0x63825363_u32).to_be_bytes()); // magic cookie
        DhcpBuilder { data }
    }

    pub fn with_message_type(mut self, msg_type: u8) -> Self {
        self.data.push(53); // Message Type tag
        self.data.push(1); // Length
        self.data.push(msg_type);
        self
    }

    pub fn with_requested_ip(mut self, ip: Ipv4Addr) -> Self {
        self.data.push(50); // Requested IP tag
        self.data.push(4); // Length
        self.data.extend_from_slice(ip.as_bytes());
        self
    }

    pub fn with_server_id(mut self, ip: Ipv4Addr) -> Self {
        self.data.push(54); // Server Identifier tag
        self.data.push(4); // Length
        self.data.extend_from_slice(ip.as_bytes());
        self
    }

    pub fn with_end(mut self) -> Self {
        self.data.push(255); // End
        self
    }

    pub fn build(self) -> Vec<u8> {
        self.data
    }
}

// ============================================================================
// DNS
// ============================================================================

pub struct DnsHeader<'a> {
    data: &'a [u8],
}

impl<'a> DnsHeader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 12 {
            return Err("DNS header too short");
        }
        Ok(DnsHeader { data })
    }

    pub fn id(&self) -> u16 {
        u16::from_be_bytes([self.data[0], self.data[1]])
    }

    pub fn flags(&self) -> u16 {
        u16::from_be_bytes([self.data[2], self.data[3]])
    }

    pub fn qdcount(&self) -> u16 {
        u16::from_be_bytes([self.data[4], self.data[5]])
    }

    pub fn ancount(&self) -> u16 {
        u16::from_be_bytes([self.data[6], self.data[7]])
    }

    pub fn nscount(&self) -> u16 {
        u16::from_be_bytes([self.data[8], self.data[9]])
    }

    pub fn arcount(&self) -> u16 {
        u16::from_be_bytes([self.data[10], self.data[11]])
    }
}

pub struct DnsQuery<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> DnsQuery<'a> {
    pub fn new(data: &'a [u8], offset: usize) -> Result<Self, &'static str> {
        if offset >= data.len() {
            return Err("Query offset out of bounds");
        }
        Ok(DnsQuery { data, offset })
    }

    pub fn parse_name(&self) -> Result<(usize, alloc::string::String), &'static str> {
        use alloc::string::String;
        let mut result = String::new();
        let mut pos = self.offset;
        let mut visited = alloc::vec::Vec::new();

        loop {
            if pos >= self.data.len() {
                return Err("Name extends beyond packet");
            }

            let byte = self.data[pos];
            if byte & 0xc0 == 0xc0 {
                // Pointer
                if pos + 1 >= self.data.len() {
                    return Err("Pointer extends beyond packet");
                }
                if visited.len() > 10 {
                    return Err("Name compression loop detected");
                }
                visited.push(pos);
                let ptr = ((byte & 0x3f) as usize) << 8 | self.data[pos + 1] as usize;
                pos = ptr;
            } else if byte == 0 {
                pos += 1;
                break;
            } else {
                // Label
                if pos + 1 + byte as usize > self.data.len() {
                    return Err("Label extends beyond packet");
                }
                if !result.is_empty() {
                    result.push('.');
                }
                if result.len() + (byte as usize) > 255 {
                    return Err("Name too long");
                }
                for i in 0..(byte as usize) {
                    result.push(self.data[pos + 1 + i] as char);
                }
                pos += 1 + byte as usize;
            }
        }

        Ok((pos, result))
    }

    pub fn qtype(&self) -> Result<u16, &'static str> {
        let (pos, _) = self.parse_name()?;
        if pos + 1 >= self.data.len() {
            return Err("QType extends beyond packet");
        }
        Ok(u16::from_be_bytes([self.data[pos], self.data[pos + 1]]))
    }

    pub fn qclass(&self) -> Result<u16, &'static str> {
        let (pos, _) = self.parse_name()?;
        if pos + 3 >= self.data.len() {
            return Err("QClass extends beyond packet");
        }
        Ok(u16::from_be_bytes([self.data[pos + 2], self.data[pos + 3]]))
    }
}

pub struct DnsBuilder {
    data: Vec<u8>,
}

impl DnsBuilder {
    pub fn query(id: u16) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&id.to_be_bytes());
        data.extend_from_slice(&[0, 0]); // flags: standard query
        data.extend_from_slice(&[0, 1]); // qdcount: 1
        data.extend_from_slice(&[0, 0]); // ancount: 0
        data.extend_from_slice(&[0, 0]); // nscount: 0
        data.extend_from_slice(&[0, 0]); // arcount: 0
        DnsBuilder { data }
    }

    pub fn with_question(mut self, name: &str, qtype: u16, qclass: u16) -> Result<Self, &'static str> {
        // Encode domain name
        for label in name.split('.') {
            if label.len() > 63 {
                return Err("Label too long");
            }
            self.data.push(label.len() as u8);
            for ch in label.as_bytes() {
                self.data.push(*ch);
            }
        }
        self.data.push(0); // root label
        self.data.extend_from_slice(&qtype.to_be_bytes());
        self.data.extend_from_slice(&qclass.to_be_bytes());
        Ok(self)
    }

    pub fn build(self) -> Vec<u8> {
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mac_addr_creation() {
        let mac = MacAddr::new(0x00, 0x11, 0x22, 0x33, 0x44, 0x55);
        assert_eq!(mac.as_bytes(), &[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    }

    #[test]
    fn test_mac_addr_broadcast() {
        let mac = MacAddr::broadcast();
        assert_eq!(mac.as_bytes(), &[0xff; 6]);
    }

    #[test]
    fn test_mac_addr_from_bytes() {
        let bytes = &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        let mac = MacAddr::from_bytes(bytes).unwrap();
        assert_eq!(mac.as_bytes(), bytes);
    }

    #[test]
    fn test_ipv4_addr_creation() {
        let ip = Ipv4Addr::new(192, 168, 1, 1);
        assert_eq!(ip.as_bytes(), &[192, 168, 1, 1]);
    }

    #[test]
    fn test_ipv4_addr_to_u32() {
        let ip = Ipv4Addr::new(192, 168, 1, 1);
        assert_eq!(ip.to_u32(), 0xc0a80101);
    }

    #[test]
    fn test_ipv4_addr_from_u32() {
        let ip = Ipv4Addr::from_u32(0xc0a80101);
        assert_eq!(ip.as_bytes(), &[192, 168, 1, 1]);
    }

    #[test]
    fn test_ipv4_addr_broadcast() {
        let ip = Ipv4Addr::broadcast();
        assert_eq!(ip.as_bytes(), &[255, 255, 255, 255]);
    }

    #[test]
    fn test_ipv4_addr_zeros() {
        let ip = Ipv4Addr::zeros();
        assert_eq!(ip.as_bytes(), &[0, 0, 0, 0]);
    }

    #[test]
    fn test_checksum_basic() {
        let data = [0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00, 0x40, 0x06, 0, 0, 0xac, 0x10, 0x0a, 0x63, 0xac, 0x10, 0x0a, 0x0c];
        let cs = checksum::checksum(&data);
        assert_eq!(cs, 0xb1e6);
    }

    #[test]
    fn test_ethernet_builder() {
        let src = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let dst = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
        let eth = EthernetBuilder::new(dst, src, 0x0800);
        let data = eth.with_payload(&[1, 2, 3, 4]).build();
        assert_eq!(data.len(), 18);
        assert_eq!(&data[0..6], dst.as_bytes());
        assert_eq!(&data[6..12], src.as_bytes());
        assert_eq!(&data[12..14], &[0x08, 0x00]);
        assert_eq!(&data[14..], &[1, 2, 3, 4]);
    }

    #[test]
    fn test_ethernet_frame_parse() {
        let src = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let dst = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
        let eth = EthernetBuilder::new(dst, src, 0x0800).with_payload(&[1, 2, 3]).build();
        let frame = EthernetFrame::new(&eth).unwrap();
        assert_eq!(frame.src().unwrap(), src);
        assert_eq!(frame.dst().unwrap(), dst);
        assert_eq!(frame.ethertype(), 0x0800);
        assert_eq!(frame.payload(), &[1, 2, 3]);
    }

    #[test]
    fn test_arp_request_builder() {
        let src_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let src_ip = Ipv4Addr::new(192, 168, 1, 100);
        let tgt_ip = Ipv4Addr::new(192, 168, 1, 1);
        let arp = ArpBuilder::request(src_mac, src_ip, tgt_ip).build();
        assert_eq!(arp.len(), 28);
        let pkt = ArpPacket::new(&arp).unwrap();
        assert_eq!(pkt.oper(), 1); // REQUEST
        assert_eq!(pkt.sha().unwrap(), src_mac);
        assert_eq!(pkt.spa().unwrap(), src_ip);
        assert_eq!(pkt.tpa().unwrap(), tgt_ip);
    }

    #[test]
    fn test_arp_reply_builder() {
        let src_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let src_ip = Ipv4Addr::new(192, 168, 1, 1);
        let tgt_mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
        let tgt_ip = Ipv4Addr::new(192, 168, 1, 100);
        let arp = ArpBuilder::reply(src_mac, src_ip, tgt_mac, tgt_ip).build();
        let pkt = ArpPacket::new(&arp).unwrap();
        assert_eq!(pkt.oper(), 2); // REPLY
        assert_eq!(pkt.sha().unwrap(), src_mac);
        assert_eq!(pkt.tha().unwrap(), tgt_mac);
    }

    #[test]
    fn test_ipv4_builder_checksum() {
        let src = Ipv4Addr::new(192, 168, 1, 100);
        let dst = Ipv4Addr::new(192, 168, 1, 1);
        let ipv4 = Ipv4Builder::new(src, dst, 6).build();
        let hdr = Ipv4Header::new(&ipv4).unwrap();
        assert!(hdr.verify_checksum());
    }

    #[test]
    fn test_ipv4_header_parse() {
        let src = Ipv4Addr::new(192, 168, 1, 100);
        let dst = Ipv4Addr::new(192, 168, 1, 1);
        let ipv4 = Ipv4Builder::new(src, dst, 17).build();
        let hdr = Ipv4Header::new(&ipv4).unwrap();
        assert_eq!(hdr.version(), 4);
        assert_eq!(hdr.ihl(), 5);
        assert_eq!(hdr.ttl(), 64);
        assert_eq!(hdr.protocol(), 17);
        assert_eq!(hdr.src().unwrap(), src);
        assert_eq!(hdr.dst().unwrap(), dst);
    }

    #[test]
    fn test_icmp_echo_request_builder() {
        let echo = IcmpEchoBuilder::request(0x1234, 0x5678).build();
        let pkt = IcmpEchoRequest::new(&echo).unwrap();
        assert_eq!(pkt.icmp_type(), 8); // echo request
        assert_eq!(pkt.identifier(), 0x1234);
        assert_eq!(pkt.sequence(), 0x5678);
        assert!(pkt.verify_checksum());
    }

    #[test]
    fn test_icmp_echo_reply_builder() {
        let echo = IcmpEchoBuilder::reply(0x1234, 0x5678).build();
        let pkt = IcmpEchoRequest::new(&echo).unwrap();
        assert_eq!(pkt.icmp_type(), 0); // echo reply
        assert_eq!(pkt.identifier(), 0x1234);
        assert_eq!(pkt.sequence(), 0x5678);
        assert!(pkt.verify_checksum());
    }

    #[test]
    fn test_icmp_with_payload() {
        let payload = b"Hello";
        let echo = IcmpEchoBuilder::request(1, 1).with_payload(payload).build();
        let pkt = IcmpEchoRequest::new(&echo).unwrap();
        assert_eq!(pkt.payload(), payload);
        assert!(pkt.verify_checksum());
    }

    #[test]
    fn test_udp_builder() {
        let udp = UdpBuilder::new(12345, 53)
            .with_payload(b"test data")
            .build_with_pseudo_header(
                Ipv4Addr::new(192, 168, 1, 100),
                Ipv4Addr::new(192, 168, 1, 1)
            );
        let pkt = UdpPacket::new(&udp).unwrap();
        assert_eq!(pkt.src_port(), 12345);
        assert_eq!(pkt.dst_port(), 53);
        assert_eq!(pkt.length(), udp.len() as u16);
        assert_eq!(pkt.payload(), b"test data");
    }

    #[test]
    fn test_dhcp_discover_builder() {
        let xid = 0x12345678;
        let mac = MacAddr::new(0x00, 0x11, 0x22, 0x33, 0x44, 0x55);
        let dhcp = DhcpBuilder::new(xid, mac)
            .with_message_type(1) // DISCOVER
            .with_end()
            .build();
        let pkt = DhcpPacket::new(&dhcp).unwrap();
        assert_eq!(pkt.opcode(), 1); // BOOTREQUEST
        assert_eq!(pkt.xid(), xid);
        assert_eq!(pkt.chaddr().unwrap(), mac);
        assert_eq!(pkt.magic_cookie(), 0x63825363);
        assert_eq!(pkt.get_message_type(), Some(1));
    }

    #[test]
    fn test_dhcp_request_builder() {
        let xid = 0x12345678;
        let mac = MacAddr::new(0x00, 0x11, 0x22, 0x33, 0x44, 0x55);
        let requested_ip = Ipv4Addr::new(192, 168, 1, 100);
        let server_ip = Ipv4Addr::new(192, 168, 1, 1);
        let dhcp = DhcpBuilder::new(xid, mac)
            .with_message_type(3) // REQUEST
            .with_requested_ip(requested_ip)
            .with_server_id(server_ip)
            .with_end()
            .build();
        let pkt = DhcpPacket::new(&dhcp).unwrap();
        assert_eq!(pkt.get_message_type(), Some(3));
    }

    #[test]
    fn test_dns_query_builder() {
        let query = DnsBuilder::query(0x1234)
            .with_question("example.com", 1, 1)
            .unwrap()
            .build();
        assert!(query.len() > 12);
    }

    #[test]
    fn test_dns_header_parse() {
        let query = DnsBuilder::query(0x5678)
            .with_question("test.com", 1, 1)
            .unwrap()
            .build();
        let hdr = DnsHeader::new(&query).unwrap();
        assert_eq!(hdr.id(), 0x5678);
        assert_eq!(hdr.qdcount(), 1);
    }

    #[test]
    fn test_rfc1071_checksum_example1() {
        // Example from RFC 1071
        let data = [0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00, 0x40, 0x06, 0xb1, 0xe6, 0xac, 0x10, 0x0a, 0x63, 0xac, 0x10, 0x0a, 0x0c];
        assert!(checksum::verify(&data));
    }

    #[test]
    fn test_checksum_odd_length() {
        let data = [0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00, 0x40, 0x06, 0, 0, 0xac, 0x10, 0x0a, 0x63, 0xac, 0x10, 0x0a];
        let cs = checksum::checksum(&data);
        assert_ne!(cs, 0);
    }

    #[test]
    fn test_ethernet_short_frame() {
        let data = &[0; 10];
        assert!(EthernetFrame::new(data).is_err());
    }

    #[test]
    fn test_arp_short_packet() {
        let data = &[0; 20];
        assert!(ArpPacket::new(data).is_err());
    }

    #[test]
    fn test_ipv4_short_header() {
        let data = &[0; 10];
        assert!(Ipv4Header::new(data).is_err());
    }

    #[test]
    fn test_icmp_short_packet() {
        let data = &[0; 5];
        assert!(IcmpEchoRequest::new(data).is_err());
    }

    #[test]
    fn test_udp_short_packet() {
        let data = &[0; 5];
        assert!(UdpPacket::new(data).is_err());
    }

    #[test]
    fn test_dhcp_short_packet() {
        let data = &[0; 100];
        assert!(DhcpPacket::new(data).is_err());
    }

    #[test]
    fn test_dns_short_header() {
        let data = &[0; 5];
        assert!(DnsHeader::new(data).is_err());
    }

    #[test]
    fn test_ipv4_with_payload() {
        let src = Ipv4Addr::new(10, 0, 0, 1);
        let dst = Ipv4Addr::new(10, 0, 0, 2);
        let payload = b"test payload data";
        let ipv4 = Ipv4Builder::new(src, dst, 6)
            .with_payload(payload)
            .build();
        let hdr = Ipv4Header::new(&ipv4).unwrap();
        assert_eq!(hdr.payload(), payload);
    }

    #[test]
    fn test_dns_name_parsing() {
        let query = DnsBuilder::query(1)
            .with_question("example.com", 1, 1)
            .unwrap()
            .build();
        let q = DnsQuery::new(&query, 12).unwrap();
        let (_, name) = q.parse_name().unwrap();
        assert_eq!(name, "example.com");
    }

    #[test]
    fn test_dns_multi_label_name() {
        let query = DnsBuilder::query(2)
            .with_question("sub.domain.example.org", 1, 1)
            .unwrap()
            .build();
        let q = DnsQuery::new(&query, 12).unwrap();
        let (_, name) = q.parse_name().unwrap();
        assert_eq!(name, "sub.domain.example.org");
    }

    #[test]
    fn test_ipv4_ttl() {
        let src = Ipv4Addr::new(10, 0, 0, 1);
        let dst = Ipv4Addr::new(10, 0, 0, 2);
        let ipv4 = Ipv4Builder::new(src, dst, 6).build();
        let hdr = Ipv4Header::new(&ipv4).unwrap();
        assert_eq!(hdr.ttl(), 64);
    }

    #[test]
    fn test_mac_display() {
        let mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
        let s = alloc::format!("{}", mac);
        assert_eq!(s, "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn test_ipv4_display() {
        let ip = Ipv4Addr::new(192, 168, 1, 1);
        let s = alloc::format!("{}", ip);
        assert_eq!(s, "192.168.1.1");
    }

    #[test]
    fn test_arp_hwtype_and_ptype() {
        let src_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let src_ip = Ipv4Addr::new(192, 168, 1, 100);
        let tgt_ip = Ipv4Addr::new(192, 168, 1, 1);
        let arp = ArpBuilder::request(src_mac, src_ip, tgt_ip).build();
        let pkt = ArpPacket::new(&arp).unwrap();
        assert_eq!(pkt.hwtype(), 1); // Ethernet
        assert_eq!(pkt.ptype(), 0x0800); // IPv4
        assert_eq!(pkt.hwlen(), 6);
        assert_eq!(pkt.plen(), 4);
    }

    #[test]
    fn test_udp_zero_checksum() {
        let udp = UdpBuilder::new(5353, 5353)
            .with_payload(b"dns")
            .build_with_pseudo_header(
                Ipv4Addr::new(224, 0, 0, 251),
                Ipv4Addr::new(192, 168, 1, 100)
            );
        let pkt = UdpPacket::new(&udp).unwrap();
        assert_eq!(pkt.dst_port(), 5353);
    }

    #[test]
    fn test_ipv4_fragment_offset() {
        let src = Ipv4Addr::new(10, 0, 0, 1);
        let dst = Ipv4Addr::new(10, 0, 0, 2);
        let ipv4 = Ipv4Builder::new(src, dst, 6).build();
        let hdr = Ipv4Header::new(&ipv4).unwrap();
        assert_eq!(hdr.frag_offset(), 0);
    }

    #[test]
    fn test_dhcp_yiaddr() {
        let xid = 0x12345678;
        let mac = MacAddr::new(0x00, 0x11, 0x22, 0x33, 0x44, 0x55);
        let dhcp = DhcpBuilder::new(xid, mac).with_end().build();
        let pkt = DhcpPacket::new(&dhcp).unwrap();
        assert_eq!(pkt.yiaddr().unwrap(), Ipv4Addr::zeros());
    }

    #[test]
    fn test_ethernet_min_frame() {
        let data = &[0xff; 14];
        let frame = EthernetFrame::new(data).unwrap();
        assert_eq!(frame.payload(), &[]);
    }

    #[test]
    fn test_ipv4_header_version_ihl() {
        let src = Ipv4Addr::new(10, 0, 0, 1);
        let dst = Ipv4Addr::new(10, 0, 0, 2);
        let ipv4 = Ipv4Builder::new(src, dst, 6).build();
        let hdr = Ipv4Header::new(&ipv4).unwrap();
        assert_eq!(hdr.version(), 4);
        assert_eq!(hdr.ihl(), 5);
        assert_eq!(hdr.header_len(), 20);
    }

    #[test]
    fn test_dns_query_minimal() {
        let query = DnsBuilder::query(0x0001)
            .with_question("a", 1, 1)
            .unwrap()
            .build();
        assert!(query.len() >= 12);
    }

    #[test]
    fn test_icmp_payload_empty() {
        let echo = IcmpEchoBuilder::request(1, 1).build();
        let pkt = IcmpEchoRequest::new(&echo).unwrap();
        assert_eq!(pkt.payload(), &[]);
    }

    #[test]
    fn test_ipv4_dscp_ecn() {
        let src = Ipv4Addr::new(10, 0, 0, 1);
        let dst = Ipv4Addr::new(10, 0, 0, 2);
        let ipv4 = Ipv4Builder::new(src, dst, 6).build();
        let hdr = Ipv4Header::new(&ipv4).unwrap();
        assert_eq!(hdr.dscp(), 0);
        assert_eq!(hdr.ecn(), 0);
    }

    #[test]
    fn test_arp_target_hardware_zero() {
        let src_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
        let src_ip = Ipv4Addr::new(192, 168, 1, 100);
        let tgt_ip = Ipv4Addr::new(192, 168, 1, 1);
        let arp = ArpBuilder::request(src_mac, src_ip, tgt_ip).build();
        let pkt = ArpPacket::new(&arp).unwrap();
        assert_eq!(pkt.tha().unwrap(), MacAddr::new(0, 0, 0, 0, 0, 0));
    }

    #[test]
    fn test_mac_from_bytes_short() {
        let bytes = &[0xaa, 0xbb, 0xcc];
        assert!(MacAddr::from_bytes(bytes).is_err());
    }

    #[test]
    fn test_ipv4_from_bytes_short() {
        let bytes = &[192, 168, 1];
        assert!(Ipv4Addr::from_bytes(bytes).is_err());
    }

    #[test]
    fn test_checksum_all_zeros() {
        let data = [0; 20];
        let cs = checksum::checksum(&data);
        assert_eq!(cs, 0xffff);
    }

    #[test]
    fn test_udp_length_field() {
        let udp = UdpBuilder::new(1234, 5678)
            .with_payload(&[1, 2, 3, 4, 5])
            .build_with_pseudo_header(
                Ipv4Addr::new(10, 0, 0, 1),
                Ipv4Addr::new(10, 0, 0, 2)
            );
        let pkt = UdpPacket::new(&udp).unwrap();
        assert_eq!(pkt.length(), (8 + 5) as u16);
    }

    #[test]
    fn test_dhcp_magic_cookie_constant() {
        let xid = 0x11223344;
        let mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
        let dhcp = DhcpBuilder::new(xid, mac).build();
        let pkt = DhcpPacket::new(&dhcp).unwrap();
        assert_eq!(pkt.magic_cookie(), 0x63825363);
    }
}
