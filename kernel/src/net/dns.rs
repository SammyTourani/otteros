//! DNS resolution via UDP.

use alloc::boxed::Box;
use crate::time;
use otter_netlogic::dns::Lookup;

use super::{NetError, UdpSocket};

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
    if let Some(netif) = super::interface() {
        if let Some(lease) = netif.dhcp_lease() {
            resolve_via(name, lease.dns, 53, timeout_ms)
        } else {
            Err(NetError::NoInterface)
        }
    } else {
        Err(NetError::NoInterface)
    }
}
