//! Acceptance oracle for brief M5-T0e, written by the orchestrator. The crate must pass this file
//! unchanged; add your own tests elsewhere.
//!
//! The core is a network simulation: a client and a server `Tcb` exchange real encoded segments
//! (build_segment / parse_segment, checksums included) over a seeded link with delay, jitter, loss,
//! duplication, reordering and a bandwidth cap, while applications on both ends write, read and
//! close. Every run must deliver both byte streams exactly, show EOF on both sides, and end with
//! both connections CLOSED without error. Simulated time is in microseconds; the Tcbs see ms.
//!
//! API this file relies on (beyond the brief): `Ipv4Addr::new`, `OutSegment::mss` (the MSS option
//! on SYN and SYN-ACK, encoded by build_segment), and `TcpError::WouldBlock` from `recv` when no
//! data is available yet (`Ok(0)` means EOF).

use otter_tcp::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn pct(&mut self, p: u64) -> bool {
        self.below(10_000) < p
    }
}

fn payload(seed: u64, len: usize) -> Vec<u8> {
    let mut r = Rng(seed | 1);
    (0..len).map(|_| r.next() as u8).collect()
}

fn seq_le(a: u32, b: u32) -> bool {
    (b.wrapping_sub(a) as i32) >= 0
}

// ---------------------------------------------------------------------------------------------
// The simulated link.

#[derive(Clone, Copy)]
struct LinkParams {
    loss_bp: u64,      // loss in basis points (1/100 of a percent)
    dup_bp: u64,       // duplication
    reorder_bp: u64,   // extra random delay for some packets
    delay_us: (u64, u64), // one-way delay range
    bandwidth_bps: u64,
}

/// Packets in flight, ordered by (deliver_at, order); each carries (to, bytes).
type Queue = BinaryHeap<Reverse<(u64, u64, usize, Vec<u8>)>>;

struct Net {
    p: LinkParams,
    rng: Rng,
    queue: Queue,
    order: u64,
    busy_until: [u64; 2],
    sent: u64,
    overflow: bool,
}

/// Hard bounds that turn a runaway implementation into a test failure instead of an out-of-memory
/// machine (an unbounded segment storm once took this 16 GB Mac to 61.7 GB and a kernel panic).
const MAX_IN_FLIGHT: usize = 100_000;
const MAX_SENT: u64 = 5_000_000;
const MAX_BURST: usize = 10_000;

impl Net {
    fn send(&mut self, now: u64, to: usize, bytes: Vec<u8>) {
        self.sent += 1;
        if self.queue.len() >= MAX_IN_FLIGHT || self.sent > MAX_SENT {
            self.overflow = true;
            return;
        }
        if self.rng.pct(self.p.loss_bp) {
            return;
        }
        let copies = if self.rng.pct(self.p.dup_bp) { 2 } else { 1 };
        for _ in 0..copies {
            let tx = (bytes.len() as u64 + 40) * 8 * 1_000_000 / self.p.bandwidth_bps;
            let start = now.max(self.busy_until[to]);
            self.busy_until[to] = start + tx;
            let (lo, hi) = self.p.delay_us;
            let mut at = start + tx + lo + self.rng.below(hi - lo + 1);
            if self.rng.pct(self.p.reorder_bp) {
                at += self.rng.below(30_000);
            }
            self.order += 1;
            self.queue.push(Reverse((at, self.order, to, bytes.clone())));
        }
    }
    fn next_due(&self) -> Option<u64> {
        self.queue.peek().map(|Reverse((t, ..))| *t)
    }
    fn pop_due(&mut self, now: u64) -> Option<(usize, Vec<u8>)> {
        if self.next_due()? <= now {
            let Reverse((_, _, to, b)) = self.queue.pop()?;
            Some((to, b))
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Applications on both ends.

struct End {
    tcb: Tcb,
    addr: Ipv4Addr,
    port: u16,
    out: Vec<u8>,
    written: usize,
    closed: bool,
    got: Vec<u8>,
    eof: bool,
    read_chunk: usize, // bytes per read call; a slow reader reads little and rarely
    slow: bool,
    all_received_at: Option<u64>,
}

impl End {
    fn app(&mut self, rng: &mut Rng, now_us: u64, reading_tick: bool, expect_len: usize) -> Result<(), String> {
        while self.written < self.out.len() {
            let chunk = 1 + rng.below(8192) as usize;
            let end = (self.written + chunk).min(self.out.len());
            let n = self.tcb.send(&self.out[self.written..end]);
            if n == 0 {
                break;
            }
            self.written += n;
        }
        let st = self.tcb.state();
        if self.written == self.out.len()
            && !self.closed
            && matches!(st, TcpState::Established | TcpState::CloseWait)
        {
            self.tcb.close();
            self.closed = true;
        }
        if self.slow && !reading_tick {
            return Ok(());
        }
        let mut buf = vec![0u8; self.read_chunk];
        loop {
            match self.tcb.recv(&mut buf) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => {
                    if self.got.len() + n > expect_len {
                        return Err(format!("received {} bytes but the peer only sent {expect_len}", self.got.len() + n));
                    }
                    self.got.extend_from_slice(&buf[..n]);
                    if self.got.len() == expect_len && self.all_received_at.is_none() {
                        self.all_received_at = Some(now_us);
                    }
                    if self.slow {
                        break;
                    }
                }
                Err(TcpError::WouldBlock) => break,
                Err(e) => return Err(format!("recv error {e:?} in state {:?}", self.tcb.state())),
            }
        }
        Ok(())
    }
    fn done(&self) -> bool {
        self.written == self.out.len() && self.closed && self.eof && self.tcb.state() == TcpState::Closed
    }
}

struct Run {
    link: LinkParams,
    client_bytes: usize,
    server_bytes: usize,
    seed: u64,
    delayed_ack_ms: u32,
    slow_server: bool,
    max_sim_us: u64,
}

struct Outcome {
    end_us: u64,
    client_data_received_at: u64,
}

fn simulate(r: &Run) -> Result<Outcome, String> {
    let cfg = TcpConfig { msl_ms: 5_000, max_syn_retries: 12, delayed_ack_ms: r.delayed_ack_ms, ..TcpConfig::default() };
    let (ca, sa) = (Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 0, 0, 2));
    let cdata = payload(r.seed ^ 0xc1, r.client_bytes);
    let sdata = payload(r.seed ^ 0x5e, r.server_bytes);
    let mut ends = [
        End {
            tcb: Tcb::connect(cfg, ca, 40_000, sa, 80, (r.seed as u32).wrapping_mul(2_654_435_761), 0),
            addr: ca,
            port: 40_000,
            out: cdata.clone(),
            written: 0,
            closed: false,
            got: Vec::new(),
            eof: false,
            read_chunk: 64 * 1024,
            slow: false,
            all_received_at: None,
        },
        End {
            tcb: Tcb::listen(cfg, sa, 80),
            addr: sa,
            port: 80,
            out: sdata.clone(),
            written: 0,
            closed: false,
            got: Vec::new(),
            eof: false,
            read_chunk: if r.slow_server { 512 } else { 64 * 1024 },
            slow: r.slow_server,
            all_received_at: None,
        },
    ];
    let expect = [sdata.len(), cdata.len()];
    let mut net = Net { p: r.link, rng: Rng(r.seed ^ 0xabcd), queue: BinaryHeap::new(), order: 0, busy_until: [0; 2], sent: 0, overflow: false };
    let mut app_rng = Rng(r.seed ^ 0x77);
    let mut now: u64 = 0;
    let read_tick_us = 100_000;
    let mut next_read_tick = 0u64;
    for _ in 0..20_000_000u64 {
        let ms = now / 1000;
        let reading_tick = now >= next_read_tick;
        if reading_tick {
            next_read_tick = now + read_tick_us;
        }
        for i in 0..2 {
            ends[i].app(&mut app_rng, now, reading_tick, expect[i])?;
            let mut burst = 0;
            while let Some(seg) = ends[i].tcb.poll_transmit(ms) {
                burst += 1;
                if burst > MAX_BURST {
                    return Err(format!("end {i}: poll_transmit returned over {MAX_BURST} segments in a row; it must return None when nothing is due"));
                }
                let peer = 1 - i;
                let bytes = build_segment(&seg, ends[i].port, ends[peer].port, ends[i].addr, ends[peer].addr);
                net.send(now, peer, bytes);
            }
        }
        if net.overflow {
            return Err(format!(
                "segment storm: {} packets in flight, {} sent (limits {MAX_IN_FLIGHT} and {MAX_SENT})",
                net.queue.len(),
                net.sent
            ));
        }
        while let Some((to, bytes)) = net.pop_due(now) {
            let from = 1 - to;
            let seg = parse_segment(&bytes, ends[from].addr, ends[to].addr)
                .map_err(|e| format!("a segment we built failed to parse: {e}"))?;
            ends[to].tcb.on_segment(&seg, ms);
        }
        for e in ends.iter_mut() {
            if e.tcb.poll_timeout().is_some_and(|t| t <= ms) {
                e.tcb.on_timer(ms);
            }
        }
        if ends.iter().all(End::done) {
            for (i, e) in ends.iter().enumerate() {
                if let Some(err) = e.tcb.error() {
                    return Err(format!("end {i} finished with error {err:?}"));
                }
            }
            if ends[0].got != sdata || ends[1].got != cdata {
                return Err(format!(
                    "byte streams differ: client got {} of {} bytes, server got {} of {}",
                    ends[0].got.len(),
                    sdata.len(),
                    ends[1].got.len(),
                    cdata.len()
                ));
            }
            return Ok(Outcome { end_us: now, client_data_received_at: ends[1].all_received_at.unwrap_or(now) });
        }
        if now > r.max_sim_us {
            return Err(format!(
                "not finished after {} simulated s: client {:?} (sent {}/{}, got {}/{}), server {:?} (sent {}/{}, got {}/{})",
                now / 1_000_000,
                ends[0].tcb.state(),
                ends[0].written,
                cdata.len(),
                ends[0].got.len(),
                sdata.len(),
                ends[1].tcb.state(),
                ends[1].written,
                sdata.len(),
                ends[1].got.len(),
                cdata.len()
            ));
        }
        let mut next = u64::MAX;
        if let Some(t) = net.next_due() {
            next = next.min(t);
        }
        for e in ends.iter_mut() {
            if let Some(t) = e.tcb.poll_timeout() {
                next = next.min(t.saturating_mul(1000));
            }
        }
        if r.slow_server {
            next = next.min(next_read_tick);
        }
        if next == u64::MAX {
            return Err(format!(
                "stalled: nothing in flight and no timer (client {:?}, server {:?})",
                ends[0].tcb.state(),
                ends[1].tcb.state()
            ));
        }
        now = next.max(now + 1);
    }
    Err("livelock: 20 million simulation steps".into())
}

fn lossy(loss_bp: u64) -> LinkParams {
    LinkParams { loss_bp, dup_bp: 100, reorder_bp: 300, delay_us: (1_000, 50_000), bandwidth_bps: 100_000_000 }
}

fn run_seeds(loss_bp: u64, seeds: u64, base: u64) {
    let mut worst = 0u64;
    for s in 0..seeds {
        let seed = base + s;
        let mut rng = Rng(seed * 0x9e37_79b9 + 1);
        let size = |rng: &mut Rng| if rng.pct(500) { 1 << 20 } else { rng.below(256 * 1024 + 1) as usize };
        let run = Run {
            link: lossy(loss_bp),
            client_bytes: size(&mut rng),
            server_bytes: size(&mut rng),
            seed,
            delayed_ack_ms: if rng.pct(5000) { 40 } else { 0 },
            slow_server: false,
            max_sim_us: 1_800_000_000,
        };
        match simulate(&run) {
            Ok(o) => worst = worst.max(o.end_us),
            Err(e) => panic!(
                "loss {}%, seed {seed} ({} B up, {} B down): {e}",
                loss_bp as f64 / 100.0,
                run.client_bytes,
                run.server_bytes
            ),
        }
    }
    eprintln!("tcp oracle: loss {}%: {seeds} seeds ok, max simulated time {:.1} s", loss_bp as f64 / 100.0, worst as f64 / 1e6);
}

#[test]
fn netsim_no_loss() {
    run_seeds(0, 100, 1_000);
}

#[test]
fn netsim_one_percent_loss() {
    run_seeds(100, 100, 2_000);
}

#[test]
fn netsim_five_percent_loss() {
    run_seeds(500, 100, 3_000);
}

#[test]
fn netsim_twenty_percent_loss() {
    run_seeds(2_000, 100, 4_000);
}

#[test]
#[ignore = "stress: 5,000 seeds"]
fn netsim_stress() {
    for (i, loss) in [0, 100, 500, 2_000].into_iter().enumerate() {
        run_seeds(loss, 1_250, 100_000 * (i as u64 + 1));
    }
}

#[test]
fn slow_reader_forces_zero_windows_and_persist_probes() {
    for seed in 0..20 {
        let run = Run {
            link: lossy(500),
            client_bytes: 256 * 1024,
            server_bytes: 1000,
            seed: 9_000 + seed,
            delayed_ack_ms: 40,
            slow_server: true,
            max_sim_us: 1_800_000_000,
        };
        simulate(&run).unwrap_or_else(|e| panic!("slow reader seed {seed}: {e}"));
    }
}

fn goodput_mbps(loss_bp: u64) -> f64 {
    let run = Run {
        link: LinkParams { loss_bp, dup_bp: 0, reorder_bp: 0, delay_us: (10_000, 10_000), bandwidth_bps: 100_000_000 },
        client_bytes: 16 << 20,
        server_bytes: 0,
        seed: 77,
        delayed_ack_ms: 40,
        slow_server: false,
        max_sim_us: 3_600_000_000,
    };
    let o = simulate(&run).unwrap_or_else(|e| panic!("throughput run at {loss_bp} bp: {e}"));
    (16u64 << 20) as f64 * 8.0 / (o.client_data_received_at as f64 / 1e6) / 1e6
}

#[test]
fn throughput_floors() {
    let clean = goodput_mbps(0);
    let lossy = goodput_mbps(100);
    eprintln!("tcp oracle: 16 MiB over 20 ms RTT, 100 Mbit/s: {clean:.1} Mbit/s clean, {lossy:.1} Mbit/s at 1% loss");
    assert!(clean >= 20.0, "{clean:.1} Mbit/s with no loss (64 KiB window allows ~26)");
    assert!(lossy >= 5.0, "{lossy:.1} Mbit/s at 1% loss");
}

// ---------------------------------------------------------------------------------------------
// Exact behaviours.

fn addrs() -> (Ipv4Addr, Ipv4Addr) {
    (Ipv4Addr::new(192, 168, 1, 10), Ipv4Addr::new(192, 168, 1, 20))
}

/// Moves every pending segment between two Tcbs (through the codec) until both are quiet.
fn pump(a: &mut Tcb, b: &mut Tcb, now: u64) -> Vec<(bool, Segment)> {
    let (aa, ba) = addrs();
    let mut log = Vec::new();
    loop {
        assert!(log.len() < MAX_BURST, "pump: more than {MAX_BURST} segments exchanged; the connection never goes quiet");
        let mut moved = false;
        while let Some(s) = a.poll_transmit(now) {
            let seg = parse_segment(&build_segment(&s, 1111, 80, aa, ba), aa, ba).expect("parse");
            b.on_segment(&seg, now);
            log.push((true, seg));
            moved = true;
        }
        while let Some(s) = b.poll_transmit(now) {
            let seg = parse_segment(&build_segment(&s, 80, 1111, ba, aa), ba, aa).expect("parse");
            a.on_segment(&seg, now);
            log.push((false, seg));
            moved = true;
        }
        if !moved {
            return log;
        }
    }
}

fn established(client_mss: u16, server_mss: u16) -> (Tcb, Tcb, u32, u32) {
    let (aa, ba) = addrs();
    let cc = TcpConfig { mss: client_mss, delayed_ack_ms: 0, ..TcpConfig::default() };
    let sc = TcpConfig { mss: server_mss, delayed_ack_ms: 0, ..TcpConfig::default() };
    let mut c = Tcb::connect(cc, aa, 1111, ba, 80, 1_000, 0);
    let mut s = Tcb::listen(sc, ba, 80);
    let log = pump(&mut c, &mut s, 0);
    let syn = &log[0].1;
    assert!(log[0].0 && syn.flags & TCP_SYN != 0 && syn.flags & TCP_ACK == 0, "first segment is the SYN");
    assert_eq!(syn.mss, Some(client_mss), "the SYN carries the MSS option");
    let synack = &log[1].1;
    assert!(!log[1].0 && synack.flags & (TCP_SYN | TCP_ACK) == TCP_SYN | TCP_ACK, "then the SYN-ACK");
    assert_eq!(synack.mss, Some(server_mss), "the SYN-ACK carries the MSS option");
    assert_eq!(synack.ack, 1_001);
    assert_eq!((c.state(), s.state()), (TcpState::Established, TcpState::Established));
    (c, s, 1_001, synack.seq.wrapping_add(1))
}

#[test]
fn handshake_and_peer_mss_limit_segment_sizes() {
    let (mut c, mut s, _, _) = established(1460, 536);
    assert_eq!(c.send(&payload(1, 5000)), 5000);
    let log = pump(&mut c, &mut s, 10);
    let sizes: Vec<usize> = log.iter().filter(|(up, _)| *up).map(|(_, g)| g.payload.len()).filter(|&n| n > 0).collect();
    assert!(!sizes.is_empty() && sizes.iter().all(|&n| n <= 536), "segments respect the peer's MSS: {sizes:?}");
    let mut got = Vec::new();
    let mut buf = vec![0u8; 1000];
    while let Ok(n) = s.recv(&mut buf) {
        assert!(n > 0, "no EOF: the client has not closed");
        got.extend_from_slice(&buf[..n]);
        assert!(got.len() <= 5000, "received more than was sent");
    }
    assert_eq!(got, payload(1, 5000), "all 5,000 bytes, in order");
}

#[test]
fn syn_backoff_then_timeout() {
    let (aa, ba) = addrs();
    let mut c = Tcb::connect(TcpConfig::default(), aa, 1111, ba, 80, 5, 0);
    let mut times = Vec::new();
    let mut now = 0u64;
    while now <= 200_000 && c.state() != TcpState::Closed {
        while let Some(seg) = c.poll_transmit(now) {
            assert!(seg.flags & TCP_SYN != 0);
            times.push(now);
            assert!(times.len() <= 20, "far too many SYNs: {times:?}");
        }
        match c.poll_timeout() {
            Some(t) => {
                now = t.max(now + 1);
                c.on_timer(now);
            }
            None => break,
        }
    }
    assert_eq!(&times[..7], &[0, 1_000, 3_000, 7_000, 15_000, 31_000, 63_000], "RTO 1 s doubling (RFC 6298)");
    assert_eq!(times.len(), 7, "6 retries then give up");
    assert_eq!(c.state(), TcpState::Closed);
    assert_eq!(c.error(), Some(TcpError::TimedOut));
    assert!((63_000 + 32_000..=63_000 + 60_000).contains(&now), "gave up at {now} ms");
}

#[test]
fn rst_rules_rfc5961() {
    let rst = |seq: u32| Segment {
        seq,
        ack: 0,
        flags: TCP_RST,
        window: 0,
        payload: Vec::new(),
        mss: None,
        wscale: None,
        sack_permitted: false,
        timestamps: None,
    };
    // Outside the window: ignored, nothing sent.
    let (mut c, _s, _, rcv_nxt) = established(1460, 1460);
    c.on_segment(&rst(rcv_nxt.wrapping_add(10_000_000)), 5);
    assert_eq!(c.state(), TcpState::Established);
    assert!(c.poll_transmit(5).is_none());
    // In the window but not exact: challenge ACK, connection stays up.
    c.on_segment(&rst(rcv_nxt.wrapping_add(100)), 6);
    assert_eq!(c.state(), TcpState::Established);
    let ack = c.poll_transmit(6).expect("challenge ACK");
    assert!(ack.flags & TCP_ACK != 0 && ack.flags & TCP_RST == 0);
    assert_eq!(ack.ack, rcv_nxt);
    // Exact: reset.
    c.on_segment(&rst(rcv_nxt), 7);
    assert_eq!(c.state(), TcpState::Closed);
    assert_eq!(c.error(), Some(TcpError::Reset));
    assert!(matches!(c.recv(&mut [0u8; 8]), Err(TcpError::Reset)));
}

#[test]
fn syn_in_established_gets_a_challenge_ack() {
    let (mut c, _s, _, rcv_nxt) = established(1460, 1460);
    let syn = Segment {
        seq: rcv_nxt.wrapping_add(5),
        ack: 0,
        flags: TCP_SYN,
        window: 1000,
        payload: Vec::new(),
        mss: Some(1000),
        wscale: None,
        sack_permitted: false,
        timestamps: None,
    };
    c.on_segment(&syn, 9);
    assert_eq!(c.state(), TcpState::Established);
    let ack = c.poll_transmit(9).expect("challenge ACK");
    assert!(ack.flags & TCP_ACK != 0 && ack.flags & (TCP_RST | TCP_SYN) == 0);
    assert_eq!(ack.ack, rcv_nxt);
}

#[test]
fn recv_would_block_then_eof() {
    let (mut c, mut s, _, _) = established(1460, 1460);
    assert!(matches!(s.recv(&mut [0u8; 16]), Err(TcpError::WouldBlock)));
    c.send(b"hello");
    c.close();
    pump(&mut c, &mut s, 20);
    let mut buf = [0u8; 16];
    assert_eq!(s.recv(&mut buf), Ok(5));
    assert_eq!(&buf[..5], b"hello");
    assert_eq!(s.recv(&mut buf), Ok(0), "EOF after the peer's FIN");
    assert_eq!(s.state(), TcpState::CloseWait);
}

// ---------------------------------------------------------------------------------------------
// Codec.

fn internet_checksum(pseudo_and_segment: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in pseudo_and_segment.chunks(2) {
        let w = if chunk.len() == 2 { u16::from_be_bytes([chunk[0], chunk[1]]) } else { u16::from_be_bytes([chunk[0], 0]) };
        sum += w as u32;
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn random_out(rng: &mut Rng) -> OutSegment {
    let flags = [TCP_ACK, TCP_SYN, TCP_SYN | TCP_ACK, TCP_FIN | TCP_ACK, TCP_RST, TCP_PSH | TCP_ACK][rng.below(6) as usize];
    OutSegment {
        seq: rng.next() as u32,
        ack: rng.next() as u32,
        flags,
        window: rng.next() as u16,
        payload: payload(rng.next(), rng.below(1461) as usize),
        mss: if flags & TCP_SYN != 0 { Some(536 + rng.below(1000) as u16) } else { None },
    }
}

#[test]
fn codec_round_trip_checksum_and_every_bit_flip() {
    let (a, b) = addrs();
    let mut rng = Rng(42);
    for _ in 0..300 {
        let out = random_out(&mut rng);
        let bytes = build_segment(&out, 1234, 443, a, b);
        // Independent checksum over the IPv4 pseudo-header.
        let mut pseudo = Vec::new();
        pseudo.extend_from_slice(a.as_bytes());
        pseudo.extend_from_slice(b.as_bytes());
        pseudo.extend_from_slice(&[0, 6]);
        pseudo.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        let mut zeroed = bytes.clone();
        zeroed[16] = 0;
        zeroed[17] = 0;
        pseudo.extend_from_slice(&zeroed);
        assert_eq!(u16::from_be_bytes([bytes[16], bytes[17]]), internet_checksum(&pseudo), "checksum field");
        let seg = parse_segment(&bytes, a, b).expect("our own segment parses");
        assert_eq!((seg.seq, seg.ack, seg.flags, seg.window), (out.seq, out.ack, out.flags, out.window));
        assert_eq!(seg.payload, out.payload);
        assert_eq!(seg.mss, out.mss);
        assert!(parse_segment(&bytes, a, Ipv4Addr::new(192, 168, 1, 21)).is_err(), "the pseudo-header addresses are checksummed");
        for bit in 0..bytes.len() * 8 {
            let mut m = bytes.clone();
            m[bit / 8] ^= 1 << (bit % 8);
            assert!(parse_segment(&m, a, b).is_err(), "a single bit flip at bit {bit} must fail the checksum");
        }
        for len in 0..bytes.len() {
            let _ = parse_segment(&bytes[..len], a, b);
        }
    }
}

#[test]
fn random_segments_never_break_invariants() {
    let mut rng = Rng(7);
    for trial in 0..200 {
        let (mut c, mut s, _, _) = established(1460, 1460);
        c.send(&payload(trial, rng.below(20_000) as usize));
        pump(&mut c, &mut s, 1);
        let tcb = if trial % 2 == 0 { &mut c } else { &mut s };
        if trial % 3 == 0 {
            tcb.close();
        }
        for i in 0..500 {
            let near = tcb.snd_una.wrapping_add(rng.below(200_000) as u32).wrapping_sub(100_000);
            let seg = Segment {
                seq: if rng.pct(5000) { rng.next() as u32 } else { near },
                ack: if rng.pct(5000) { rng.next() as u32 } else { near },
                flags: rng.next() as u8 & 0x3f,
                window: rng.next() as u16,
                payload: payload(rng.next(), rng.below(3000) as usize),
                mss: None,
                wscale: None,
                sack_permitted: false,
                timestamps: None,
            };
            let now = 2 + i;
            tcb.on_segment(&seg, now);
            let drained = (0..MAX_BURST).any(|_| tcb.poll_transmit(now).is_none());
            assert!(drained, "trial {trial}: poll_transmit never returned None");
            if tcb.poll_timeout().is_some_and(|t| t <= now + 50) {
                tcb.on_timer(now + 50);
            }
            assert!(seq_le(tcb.snd_una, tcb.snd_nxt), "trial {trial}: snd_una {} > snd_nxt {}", tcb.snd_una, tcb.snd_nxt);
            assert!(tcb.rx_buf.len() <= 65536 && tcb.tx_buf.len() <= 65536, "buffers stay within capacity");
        }
    }
}
