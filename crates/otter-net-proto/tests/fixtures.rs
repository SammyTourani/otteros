// Test fixtures for network protocols
use otter_net_proto::*;

#[test]
fn test_fixture_roundtrip_mac_addr() {
    let bytes = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
    let mac = MacAddr::from_bytes(&bytes).unwrap();
    assert_eq!(mac.as_bytes(), &bytes);
}

#[test]
fn test_fixture_roundtrip_ipv4_addr() {
    let bytes = [192, 168, 1, 100];
    let ip = Ipv4Addr::from_bytes(&bytes).unwrap();
    assert_eq!(ip.as_bytes(), &bytes);
}

#[test]
fn test_fixture_ethernet_roundtrip() {
    let src = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
    let dst = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
    let payload = b"test payload";

    let eth = EthernetBuilder::new(dst, src, 0x0800)
        .with_payload(payload)
        .build();

    let frame = EthernetFrame::new(&eth).unwrap();
    assert_eq!(frame.src().unwrap(), src);
    assert_eq!(frame.dst().unwrap(), dst);
    assert_eq!(frame.ethertype(), 0x0800);
    assert_eq!(frame.payload(), payload);
}

#[test]
fn test_fixture_arp_request_roundtrip() {
    let src_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
    let src_ip = Ipv4Addr::new(192, 168, 1, 100);
    let tgt_ip = Ipv4Addr::new(192, 168, 1, 1);

    let arp = ArpBuilder::request(src_mac, src_ip, tgt_ip).build();
    let pkt = ArpPacket::new(&arp).unwrap();

    assert_eq!(pkt.hwtype(), 1);
    assert_eq!(pkt.ptype(), 0x0800);
    assert_eq!(pkt.oper(), 1);
    assert_eq!(pkt.sha().unwrap(), src_mac);
    assert_eq!(pkt.spa().unwrap(), src_ip);
    assert_eq!(pkt.tpa().unwrap(), tgt_ip);
}

#[test]
fn test_fixture_ipv4_roundtrip() {
    let src = Ipv4Addr::new(192, 168, 1, 100);
    let dst = Ipv4Addr::new(10, 0, 0, 1);
    let payload = b"test ipv4 payload";

    let ipv4 = Ipv4Builder::new(src, dst, 6)
        .with_payload(payload)
        .build();

    let hdr = Ipv4Header::new(&ipv4).unwrap();
    assert_eq!(hdr.version(), 4);
    assert_eq!(hdr.ihl(), 5);
    assert_eq!(hdr.src().unwrap(), src);
    assert_eq!(hdr.dst().unwrap(), dst);
    assert_eq!(hdr.protocol(), 6);
    assert_eq!(hdr.payload(), payload);
    assert!(hdr.verify_checksum());
}

#[test]
fn test_fixture_icmp_echo_roundtrip() {
    let id = 0x1234;
    let seq = 0x5678;
    let payload = b"echo data";

    let echo = IcmpEchoBuilder::request(id, seq)
        .with_payload(payload)
        .build();

    let pkt = IcmpEchoRequest::new(&echo).unwrap();
    assert_eq!(pkt.icmp_type(), 8);
    assert_eq!(pkt.identifier(), id);
    assert_eq!(pkt.sequence(), seq);
    assert_eq!(pkt.payload(), payload);
    assert!(pkt.verify_checksum());
}

#[test]
fn test_fixture_udp_roundtrip() {
    let src_port = 12345;
    let dst_port = 53;
    let payload = b"dns query";

    let udp = UdpBuilder::new(src_port, dst_port)
        .with_payload(payload)
        .build_with_pseudo_header(
            Ipv4Addr::new(192, 168, 1, 100),
            Ipv4Addr::new(8, 8, 8, 8)
        );

    let pkt = UdpPacket::new(&udp).unwrap();
    assert_eq!(pkt.src_port(), src_port);
    assert_eq!(pkt.dst_port(), dst_port);
    assert_eq!(pkt.payload(), payload);
}

#[test]
fn test_fixture_dhcp_discover_roundtrip() {
    let xid = 0x12345678;
    let mac = MacAddr::new(0x00, 0x11, 0x22, 0x33, 0x44, 0x55);

    let dhcp = DhcpBuilder::new(xid, mac)
        .with_message_type(1)  // DISCOVER
        .with_end()
        .build();

    let pkt = DhcpPacket::new(&dhcp).unwrap();
    assert_eq!(pkt.opcode(), 1);
    assert_eq!(pkt.xid(), xid);
    assert_eq!(pkt.chaddr().unwrap(), mac);
    assert_eq!(pkt.magic_cookie(), 0x63825363);
    assert_eq!(pkt.get_message_type(), Some(1));
}

#[test]
fn test_fixture_dhcp_request_roundtrip() {
    let xid = 0x87654321;
    let mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
    let requested_ip = Ipv4Addr::new(192, 168, 1, 100);
    let server_ip = Ipv4Addr::new(192, 168, 1, 1);

    let dhcp = DhcpBuilder::new(xid, mac)
        .with_message_type(3)  // REQUEST
        .with_requested_ip(requested_ip)
        .with_server_id(server_ip)
        .with_end()
        .build();

    let pkt = DhcpPacket::new(&dhcp).unwrap();
    assert_eq!(pkt.get_message_type(), Some(3));
    assert_eq!(pkt.magic_cookie(), 0x63825363);
}

#[test]
fn test_fixture_dns_query_roundtrip() {
    let id = 0x5678;
    let query = DnsBuilder::query(id)
        .with_question("example.com", 1, 1)
        .unwrap()
        .build();

    let hdr = DnsHeader::new(&query).unwrap();
    assert_eq!(hdr.id(), id);
    assert_eq!(hdr.qdcount(), 1);
}

#[test]
fn test_fixture_dns_name_parsing() {
    let query = DnsBuilder::query(0x1111)
        .with_question("test.example.org", 1, 1)
        .unwrap()
        .build();

    let q = DnsQuery::new(&query, 12).unwrap();
    let (_, name) = q.parse_name().unwrap();
    assert_eq!(name, "test.example.org");
}

#[test]
fn test_fixture_truncated_packets_error() {
    assert!(EthernetFrame::new(&[0; 10]).is_err());
    assert!(ArpPacket::new(&[0; 20]).is_err());
    assert!(Ipv4Header::new(&[0; 15]).is_err());
    assert!(IcmpEchoRequest::new(&[0; 5]).is_err());
    assert!(UdpPacket::new(&[0; 5]).is_err());
    assert!(DhcpPacket::new(&[0; 100]).is_err());
    assert!(DnsHeader::new(&[0; 10]).is_err());
}

#[test]
fn test_fixture_checksum_verification() {
    let src = Ipv4Addr::new(10, 0, 0, 1);
    let dst = Ipv4Addr::new(10, 0, 0, 2);

    let ipv4 = Ipv4Builder::new(src, dst, 17).build();
    let hdr = Ipv4Header::new(&ipv4).unwrap();
    assert!(hdr.verify_checksum());
}

#[test]
fn test_fixture_icmp_checksum_verification() {
    let echo = IcmpEchoBuilder::request(1, 1)
        .with_payload(b"test")
        .build();

    let pkt = IcmpEchoRequest::new(&echo).unwrap();
    assert!(pkt.verify_checksum());
}

#[test]
fn test_fixture_combined_stack() {
    // Simulate building a complete IPv4/UDP/DNS packet
    let src_ip = Ipv4Addr::new(192, 168, 1, 100);
    let dst_ip = Ipv4Addr::new(8, 8, 8, 8);

    let dns_query = DnsBuilder::query(0x1234)
        .with_question("example.com", 1, 1)
        .unwrap()
        .build();

    let udp = UdpBuilder::new(54321, 53)
        .with_payload(&dns_query)
        .build_with_pseudo_header(src_ip, dst_ip);

    let ipv4 = Ipv4Builder::new(src_ip, dst_ip, 17)
        .with_payload(&udp)
        .build();

    let hdr = Ipv4Header::new(&ipv4).unwrap();
    assert_eq!(hdr.src().unwrap(), src_ip);
    assert_eq!(hdr.dst().unwrap(), dst_ip);
    assert_eq!(hdr.protocol(), 17); // UDP
}

#[test]
fn test_fixture_mac_equality() {
    let mac1 = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
    let mac2 = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
    assert_eq!(mac1, mac2);
}

#[test]
fn test_fixture_ipv4_equality() {
    let ip1 = Ipv4Addr::new(192, 168, 1, 1);
    let ip2 = Ipv4Addr::new(192, 168, 1, 1);
    assert_eq!(ip1, ip2);
}

#[test]
fn test_fixture_arp_reply_roundtrip() {
    let src_mac = MacAddr::new(0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
    let src_ip = Ipv4Addr::new(192, 168, 1, 1);
    let tgt_mac = MacAddr::new(0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff);
    let tgt_ip = Ipv4Addr::new(192, 168, 1, 100);

    let arp = ArpBuilder::reply(src_mac, src_ip, tgt_mac, tgt_ip).build();
    let pkt = ArpPacket::new(&arp).unwrap();

    assert_eq!(pkt.oper(), 2); // REPLY
    assert_eq!(pkt.sha().unwrap(), src_mac);
    assert_eq!(pkt.spa().unwrap(), src_ip);
    assert_eq!(pkt.tha().unwrap(), tgt_mac);
    assert_eq!(pkt.tpa().unwrap(), tgt_ip);
}

#[test]
fn test_fixture_icmp_reply_roundtrip() {
    let id = 0xabcd;
    let seq = 0xef01;
    let payload = b"reply data";

    let echo = IcmpEchoBuilder::reply(id, seq)
        .with_payload(payload)
        .build();

    let pkt = IcmpEchoRequest::new(&echo).unwrap();
    assert_eq!(pkt.icmp_type(), 0); // echo reply
    assert_eq!(pkt.identifier(), id);
    assert_eq!(pkt.sequence(), seq);
    assert_eq!(pkt.payload(), payload);
    assert!(pkt.verify_checksum());
}

#[test]
fn test_fixture_various_ipv4_addresses() {
    let loopback = Ipv4Addr::new(127, 0, 0, 1);
    let broadcast = Ipv4Addr::broadcast();
    let zeros = Ipv4Addr::zeros();

    assert_eq!(loopback.as_bytes(), &[127, 0, 0, 1]);
    assert_eq!(broadcast.as_bytes(), &[255, 255, 255, 255]);
    assert_eq!(zeros.as_bytes(), &[0, 0, 0, 0]);
}

#[test]
fn test_fixture_dns_qtype_qclass() {
    let query = DnsBuilder::query(0x0001)
        .with_question("example.com", 28, 1)  // AAAA, IN
        .unwrap()
        .build();

    let q = DnsQuery::new(&query, 12).unwrap();
    assert_eq!(q.qtype().unwrap(), 28);
    assert_eq!(q.qclass().unwrap(), 1);
}

#[test]
fn test_fixture_dhcp_no_options() {
    let xid = 0x11111111;
    let mac = MacAddr::new(0x11, 0x11, 0x11, 0x11, 0x11, 0x11);

    let dhcp = DhcpBuilder::new(xid, mac).build();
    let pkt = DhcpPacket::new(&dhcp).unwrap();

    assert_eq!(pkt.opcode(), 1);
    assert_eq!(pkt.xid(), xid);
    assert_eq!(pkt.magic_cookie(), 0x63825363);
}
