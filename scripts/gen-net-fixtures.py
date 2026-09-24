#!/usr/bin/env python3
"""Generate network protocol test fixtures."""

import struct
import socket
import sys
from pathlib import Path

def checksum(data):
    """Compute ones' complement checksum per RFC 1071."""
    if len(data) % 2:
        data = data + b'\x00'
    sum_val = 0
    for i in range(0, len(data), 2):
        word = struct.unpack('!H', data[i:i+2])[0]
        sum_val += word
    while sum_val >> 16:
        sum_val = (sum_val & 0xffff) + (sum_val >> 16)
    return (~sum_val) & 0xffff

def gen_eth_frame():
    """Ethernet II frame."""
    dst = b'\xaa\xbb\xcc\xdd\xee\xff'
    src = b'\x11\x22\x33\x44\x55\x66'
    ethertype = struct.pack('!H', 0x0800)
    payload = b'test payload'
    return dst + src + ethertype + payload

def gen_arp_request():
    """ARP request for Ethernet/IPv4."""
    hwtype = struct.pack('!H', 1)  # Ethernet
    ptype = struct.pack('!H', 0x0800)  # IPv4
    hwlen = b'\x06'
    plen = b'\x04'
    oper = struct.pack('!H', 1)  # REQUEST
    sha = b'\x11\x22\x33\x44\x55\x66'
    spa = socket.inet_aton('192.168.1.100')
    tha = b'\x00\x00\x00\x00\x00\x00'
    tpa = socket.inet_aton('192.168.1.1')
    return hwtype + ptype + hwlen + plen + oper + sha + spa + tha + tpa

def gen_ipv4_header():
    """IPv4 header."""
    version_ihl = b'\x45'  # version=4, ihl=5
    dscp_ecn = b'\x00'
    total_length = struct.pack('!H', 20)  # header only
    identification = struct.pack('!H', 0x1234)
    flags_frag = b'\x00\x00'  # no flags, offset=0
    ttl = b'\x40'  # 64
    protocol = b'\x06'  # TCP
    checksum_field = b'\x00\x00'  # will compute
    src = socket.inet_aton('192.168.1.100')
    dst = socket.inet_aton('192.168.1.1')

    hdr = version_ihl + dscp_ecn + total_length + identification + flags_frag + ttl + protocol + checksum_field + src + dst
    cs = checksum(hdr)
    return hdr[:10] + struct.pack('!H', cs) + hdr[12:]

def gen_icmp_echo_request():
    """ICMP echo request."""
    icmp_type = b'\x08'  # echo request
    code = b'\x00'
    checksum_field = b'\x00\x00'
    identifier = struct.pack('!H', 0x1234)
    sequence = struct.pack('!H', 0x0001)
    payload = b'test echo payload'

    pkt = icmp_type + code + checksum_field + identifier + sequence + payload
    cs = checksum(pkt)
    return icmp_type + code + struct.pack('!H', cs) + identifier + sequence + payload

def gen_icmp_echo_reply():
    """ICMP echo reply."""
    icmp_type = b'\x00'  # echo reply
    code = b'\x00'
    checksum_field = b'\x00\x00'
    identifier = struct.pack('!H', 0x1234)
    sequence = struct.pack('!H', 0x0001)
    payload = b'test echo payload'

    pkt = icmp_type + code + checksum_field + identifier + sequence + payload
    cs = checksum(pkt)
    return icmp_type + code + struct.pack('!H', cs) + identifier + sequence + payload

def gen_udp_packet():
    """UDP packet."""
    src_port = struct.pack('!H', 12345)
    dst_port = struct.pack('!H', 53)
    payload = b'test dns query'
    length = struct.pack('!H', 8 + len(payload))
    checksum_field = b'\x00\x00'

    return src_port + dst_port + length + checksum_field + payload

def gen_dhcp_discover():
    """DHCP DISCOVER message."""
    opcode = b'\x01'  # BOOTREQUEST
    hwtype = b'\x01'  # Ethernet
    hwlen = b'\x06'
    hops = b'\x00'
    xid = struct.pack('!I', 0x12345678)
    secs = struct.pack('!H', 0)
    flags = struct.pack('!H', 0x0000)  # unicast
    ciaddr = socket.inet_aton('0.0.0.0')
    yiaddr = socket.inet_aton('0.0.0.0')
    siaddr = socket.inet_aton('0.0.0.0')
    giaddr = socket.inet_aton('0.0.0.0')
    chaddr = b'\x00\x11\x22\x33\x44\x55' + b'\x00' * 10
    sname = b'\x00' * 64
    file = b'\x00' * 128
    magic_cookie = struct.pack('!I', 0x63825363)

    # Options: Message Type (DISCOVER=1), End
    options = b'\x35\x01\x01'  # tag 53, len 1, value 1 (DISCOVER)
    options += b'\xff'  # End

    return (opcode + hwtype + hwlen + hops + xid + secs + flags +
            ciaddr + yiaddr + siaddr + giaddr + chaddr + sname + file +
            magic_cookie + options)

def gen_dns_query():
    """DNS query for example.com."""
    id_field = struct.pack('!H', 0x1234)
    flags = struct.pack('!H', 0x0000)  # standard query
    qdcount = struct.pack('!H', 1)
    ancount = struct.pack('!H', 0)
    nscount = struct.pack('!H', 0)
    arcount = struct.pack('!H', 0)

    # Question: example.com, type A, class IN
    question = b'\x07example\x03com\x00'  # example.com in DNS format
    qtype = struct.pack('!H', 1)  # A record
    qclass = struct.pack('!H', 1)  # IN

    return id_field + flags + qdcount + ancount + nscount + arcount + question + qtype + qclass

def capture_real_dns_response():
    """Capture a real DNS response from the system resolver."""
    try:
        # Try to query 1.1.1.1 (Cloudflare) or fallback to system resolver
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.settimeout(2)

        # Build a DNS query for example.com
        query = gen_dns_query()

        # Try 1.1.1.1 first, fallback to system resolver
        for resolver in ['1.1.1.1', '8.8.8.8']:
            try:
                sock.sendto(query, (resolver, 53))
                response, _ = sock.recvfrom(1024)
                sock.close()
                return response
            except:
                continue

        sock.close()
    except:
        pass

    # If we can't get a real response, return None
    return None

def write_fixture(name, data):
    """Write a fixture file."""
    fixtures_dir = Path(__file__).parent.parent / 'crates' / 'otter-net-proto' / 'tests' / 'fixtures'
    fixtures_dir.mkdir(parents=True, exist_ok=True)
    path = fixtures_dir / name
    path.write_bytes(data)
    print(f"Generated {path}")

def main():
    write_fixture('eth_frame.bin', gen_eth_frame())
    write_fixture('arp_request.bin', gen_arp_request())
    write_fixture('ipv4_header.bin', gen_ipv4_header())
    write_fixture('icmp_echo_request.bin', gen_icmp_echo_request())
    write_fixture('icmp_echo_reply.bin', gen_icmp_echo_reply())
    write_fixture('udp_packet.bin', gen_udp_packet())
    write_fixture('dhcp_discover.bin', gen_dhcp_discover())
    write_fixture('dns_query.bin', gen_dns_query())

    # Try to capture a real DNS response
    dns_response = capture_real_dns_response()
    if dns_response:
        write_fixture('dns_response_real.bin', dns_response)
        print("Captured real DNS response")
    else:
        print("Warning: Could not capture real DNS response (network unavailable)")

if __name__ == '__main__':
    main()
