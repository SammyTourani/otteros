//! DHCP lease and configuration.

use crate::kprintln;
use otter_net_proto::Ipv4Addr;

use super::NetIf;

/// DHCP lease information.
#[derive(Clone, Copy, Debug)]
pub struct Lease {
    pub address: [u8; 4],
    pub router: [u8; 4],
    pub dns: [u8; 4],
    pub lease_secs: u32,
}

/// Convert a netmask to prefix length (e.g., [255, 255, 255, 0] -> 24).
fn netmask_to_prefix(mask: [u8; 4]) -> u8 {
    let mask_u32 = u32::from_be_bytes(mask);
    (!mask_u32).leading_zeros() as u8
}

/// Apply a DHCP lease to the network interface.
pub fn apply_lease(netif: &NetIf, ip: Ipv4Addr, router: Ipv4Addr, dns: Ipv4Addr, lease_ms: u32, mask: Ipv4Addr) {
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
