//! Acceptance oracle for brief M3-T1c, written by the orchestrator from the OASIS virtio 1.2
//! specification. The crate must pass this file unchanged; add your own tests elsewhere.
//!
//! The device side (reading descriptor chains, writing the used ring) is implemented here
//! independently, straight from the spec's ring layout (§2.7), and runs both interleaved on one
//! thread and concurrently on a second thread. The Mac's ARM64 CPU is weakly ordered, so a missing
//! release/acquire in the driver shows up as a corrupted payload or a lost token. Test "physical"
//! addresses are the virtual addresses (identity mapping).

use otter_virtio::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------------------------
// Spec constants (virtio 1.2 §6 reserved feature bits, §2.1 device status, §2.7 ring flags).

#[test]
fn feature_bits_and_status_values_match_the_spec() {
    assert_eq!(VIRTIO_F_INDIRECT_DESC, 1u64 << 28);
    assert_eq!(VIRTIO_F_RING_EVENT_IDX, 1u64 << 29);
    assert_eq!(VIRTIO_F_VERSION_1, 1u64 << 32);
    assert_eq!(STATUS_ACKNOWLEDGE, 1);
    assert_eq!(STATUS_DRIVER, 2);
    assert_eq!(STATUS_DRIVER_OK, 4);
    assert_eq!(STATUS_FEATURES_OK, 8);
    assert_eq!(STATUS_FAILED, 128);
}

// ---------------------------------------------------------------------------------------------
// Ring layout (§2.7): descriptor table (16 bytes each, align 16), available ring (flags, idx,
// ring[n], used_event; align 2), used ring (flags, idx, ring[n] of {id u32, len u32}, avail_event;
// align 4).

fn spec_layout(n: usize) -> (usize, usize, usize, usize) {
    let desc = 0;
    let avail = desc + 16 * n;
    let used = (avail + 6 + 2 * n + 3) & !3;
    let total = used + 6 + 8 * n;
    (desc, avail, used, total)
}

#[test]
fn layout_matches_the_spec_for_every_queue_size() {
    for shift in 0..=15 {
        let n = 1usize << shift;
        let l = Layout::for_size(n as u16).expect("power of two");
        let (d, a, u, t) = spec_layout(n);
        assert_eq!((l.desc_offset, l.avail_offset, l.used_offset, l.total_size), (d, a, u, t), "queue size {n}");
    }
    for bad in [0u16, 3, 6, 100, 40000] {
        assert!(Layout::for_size(bad).is_none(), "queue size {bad} is invalid");
    }
}

/// Ring memory for tests: 4 KiB aligned, zeroed, identity-mapped.
struct Ring {
    ptr: *mut u8,
    layout: std::alloc::Layout,
}

impl Ring {
    fn new(size: usize) -> Ring {
        let layout = std::alloc::Layout::from_size_align(size.max(16), 4096).unwrap();
        // SAFETY: the layout has a non-zero size.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!ptr.is_null());
        Ring { ptr, layout }
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        // SAFETY: allocated in Ring::new with this layout.
        unsafe { std::alloc::dealloc(self.ptr, self.layout) }
    }
}

// SAFETY: the ring is shared between the driver and device threads only through the virtio
// protocol (acquire/release on the ring indices), which is what the concurrent test checks.
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

fn new_queue(n: u16) -> (SplitQueue, Arc<Ring>) {
    let l = Layout::for_size(n).unwrap();
    let ring = Arc::new(Ring::new(l.total_size));
    // SAFETY: the region is aligned, zeroed, total_size long and owned by this queue.
    let mem = unsafe { RingMemory::new(ring.ptr, ring.ptr as u64, l.total_size) }.expect("ring memory");
    let q = SplitQueue::new(l, mem).expect("queue");
    assert_eq!(q.desc_phys_addr(), ring.ptr as u64);
    (q, ring)
}

// ---------------------------------------------------------------------------------------------
// The device side, independent of the crate.

struct Device {
    base: usize,
    n: usize,
    avail: usize,
    used: usize,
    last_avail: u16,
    used_idx: u16,
}

struct Chain {
    head: u16,
    descs: Vec<u16>,
    readable: Vec<(usize, usize)>,
    writable: Vec<(usize, usize)>,
}

impl Device {
    fn new(ring: &Ring, n: u16) -> Device {
        let (_, avail, used, _) = spec_layout(n as usize);
        Device { base: ring.ptr as usize, n: n as usize, avail, used, last_avail: 0, used_idx: 0 }
    }
    fn u16_at(&self, off: usize) -> &AtomicU16 {
        // SAFETY: off is 2-aligned and inside the ring (spec layout).
        unsafe { AtomicU16::from_ptr((self.base + off) as *mut u16) }
    }
    fn read<T: Copy>(&self, off: usize) -> T {
        // SAFETY: inside the ring; ordered after the acquire load of avail.idx.
        unsafe { std::ptr::read_unaligned((self.base + off) as *const T) }
    }
    fn write<T: Copy>(&self, off: usize, v: T) {
        // SAFETY: inside the ring; published by the release store of used.idx.
        unsafe { std::ptr::write_unaligned((self.base + off) as *mut T, v) }
    }
    /// Takes the next available chain, if any (acquire on avail.idx).
    fn next_chain(&mut self) -> Option<Chain> {
        let avail_idx = self.u16_at(self.avail + 2).load(Ordering::Acquire);
        if avail_idx == self.last_avail {
            return None;
        }
        let head: u16 = self.read(self.avail + 4 + 2 * (self.last_avail as usize % self.n));
        self.last_avail = self.last_avail.wrapping_add(1);
        let mut chain = Chain { head, descs: Vec::new(), readable: Vec::new(), writable: Vec::new() };
        let mut i = head;
        loop {
            assert!((i as usize) < self.n, "descriptor index {i} out of range");
            assert!(chain.descs.len() < self.n, "descriptor chain loops");
            chain.descs.push(i);
            let d = 16 * i as usize;
            let (addr, len, flags, next): (u64, u32, u16, u16) =
                (self.read(d), self.read(d + 8), self.read(d + 12), self.read(d + 14));
            if flags & 2 != 0 {
                chain.writable.push((addr as usize, len as usize));
            } else {
                assert!(chain.writable.is_empty(), "readable descriptor after a writable one");
                chain.readable.push((addr as usize, len as usize));
            }
            if flags & 1 == 0 {
                break;
            }
            i = next;
        }
        Some(chain)
    }
    /// Publishes one used element (release on used.idx).
    fn complete(&mut self, head: u16, written: u32) {
        let e = self.used + 4 + 8 * (self.used_idx as usize % self.n);
        self.write(e, head as u32);
        self.write(e + 4, written);
        self.used_idx = self.used_idx.wrapping_add(1);
        self.u16_at(self.used + 2).store(self.used_idx, Ordering::Release);
    }
    fn set_used_flags(&self, flags: u16) {
        self.u16_at(self.used).store(flags, Ordering::Release);
    }
    fn set_avail_event(&self, v: u16) {
        self.u16_at(self.used + 4 + 8 * self.n).store(v, Ordering::Release);
    }
    fn used_event(&self) -> u16 {
        self.u16_at(self.avail + 4 + 2 * self.n).load(Ordering::Acquire)
    }
}

// ---------------------------------------------------------------------------------------------
// Deterministic interleaving: the driver and the device alternate randomly on one thread.

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn pattern(token: u64, i: usize) -> u8 {
    (token.wrapping_mul(0x9e37_79b9).wrapping_add(i as u64 * 31) >> 3) as u8
}

#[test]
fn interleaved_device_model_every_token_exactly_once() {
    for (n, target) in [(1u16, 70_000u64), (2, 70_000), (8, 70_000), (256, 70_000), (32768, 70_000)] {
        let (mut q, ring) = new_queue(n);
        assert_eq!(q.free_descriptors(), n as usize);
        let mut dev = Device::new(&ring, n);
        let mut rng = Rng(0x5eed ^ n as u64);
        let mut buffers: HashMap<u64, (Vec<Vec<u8>>, usize)> = HashMap::new(); // token -> (buffers, readable count)
        let mut expected_len: HashMap<u64, u32> = HashMap::new();
        let mut at_device: Vec<(Chain, u64)> = Vec::new(); // chains the device holds
        let mut in_flight_descs: HashSet<u16> = HashSet::new();
        let mut next_token = 1u64;
        let mut completed = 0u64;
        let mut outstanding = 0u64;
        while completed < target {
            match rng.below(3) {
                0 => {
                    // Driver adds a chain of 1..=4 buffers (at most n).
                    let k = 1 + rng.below(4.min(n as usize));
                    let nread = rng.below(k + 1);
                    let mut bufs: Vec<Vec<u8>> = (0..k).map(|_| vec![0u8; 1 + rng.below(64)]).collect();
                    for (bi, b) in bufs.iter_mut().enumerate().take(nread) {
                        for (i, x) in b.iter_mut().enumerate() {
                            *x = pattern(next_token, bi * 100 + i);
                        }
                    }
                    let descs: Vec<Buffer> = bufs
                        .iter()
                        .enumerate()
                        .map(|(i, b)| Buffer { addr: b.as_ptr() as u64, len: b.len() as u32, device_writes: i >= nread })
                        .collect();
                    let free_before = q.free_descriptors();
                    let r = q.add(&descs, next_token);
                    if free_before >= k {
                        assert!(r.is_ok(), "add of {k} with {free_before} free descriptors failed");
                        assert_eq!(q.free_descriptors(), free_before - k);
                        buffers.insert(next_token, (bufs, nread));
                        next_token += 1;
                        outstanding += 1;
                    } else {
                        assert!(r.is_err(), "add of {k} with only {free_before} free descriptors must fail");
                        assert_eq!(q.free_descriptors(), free_before, "a failed add changes nothing");
                    }
                }
                1 => {
                    // Device takes some available chains.
                    for _ in 0..1 + rng.below(4) {
                        let Some(c) = dev.next_chain() else { break };
                        for d in &c.descs {
                            assert!(in_flight_descs.insert(*d), "descriptor {d} reused while in flight");
                        }
                        // Identify the token by content: the readable bytes carry its pattern.
                        let token = *buffers
                            .iter()
                            .find(|(_, (bufs, _))| bufs[0].as_ptr() as usize == c.readable.first().or(c.writable.first()).unwrap().0)
                            .map(|(t, _)| t)
                            .expect("chain matches a submitted token");
                        for (bi, &(addr, len)) in c.readable.iter().enumerate() {
                            for i in 0..len {
                                // SAFETY: addr/len come from a live buffer of this token.
                                let v = unsafe { *(addr as *const u8).add(i) };
                                assert_eq!(v, pattern(token, bi * 100 + i), "readable payload of token {token}");
                            }
                        }
                        at_device.push((c, token));
                    }
                }
                _ => {
                    // Device completes held chains in random order; the driver pops everything used.
                    for _ in 0..rng.below(at_device.len() + 1) {
                        let (c, token) = at_device.swap_remove(rng.below(at_device.len()));
                        let mut written = 0u32;
                        for &(addr, len) in &c.writable {
                            for i in 0..len {
                                // SAFETY: device-writable buffer of a live token.
                                unsafe { *(addr as *mut u8).add(i) = !pattern(token, 7000 + written as usize + i) };
                            }
                            written += len as u32;
                        }
                        for d in &c.descs {
                            in_flight_descs.remove(d);
                        }
                        expected_len.insert(token, written);
                        dev.complete(c.head, written);
                    }
                    while let Some((token, len)) = q.pop_used() {
                        let want = expected_len.remove(&token).expect("popped a token that was not completed (or twice)");
                        assert_eq!(len, want, "used length of token {token}");
                        let (bufs, nread) = buffers.remove(&token).expect("token submitted");
                        let mut w = 0usize;
                        for b in &bufs[nread..] {
                            for (i, &x) in b.iter().enumerate() {
                                assert_eq!(x, !pattern(token, 7000 + w + i), "device-written byte of token {token}");
                            }
                            w += b.len();
                        }
                        completed += 1;
                        outstanding -= 1;
                    }
                }
            }
        }
        // Drain: the device completes everything it holds and everything still available.
        while let Some(c) = dev.next_chain() {
            at_device.push((c, 0));
        }
        for (c, _) in at_device.drain(..) {
            dev.complete(c.head, 0);
        }
        while q.pop_used().is_some() {
            outstanding -= 1;
        }
        assert_eq!(outstanding, 0, "queue size {n}: every submitted token came back exactly once");
        assert_eq!(q.free_descriptors(), n as usize, "queue size {n}: free list whole again");
        assert!(q.pop_used().is_none());
        eprintln!("virtio oracle: queue size {n}: {completed}+ completions, indices wrapped");
    }
}

// ---------------------------------------------------------------------------------------------
// Concurrent device on a second thread: 1,000,000 requests, payload checked end to end.

#[test]
fn concurrent_device_model_one_million_requests() {
    const N: u16 = 256;
    const REQUESTS: u64 = 1_000_000;
    const SLOTS: usize = N as usize / 2; // two descriptors per request
    let (mut q, ring) = new_queue(N);
    // Each slot: 16 readable bytes (token, token ^ K) then 16 device-writable bytes.
    let mut slots = vec![[0u64; 4]; SLOTS];
    let slots_ptr = slots.as_mut_ptr() as usize;
    let stop = Arc::new(AtomicBool::new(false));
    let device = {
        let ring = ring.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut dev = Device::new(&ring, N);
            let mut served = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let Some(c) = dev.next_chain() else {
                    std::hint::spin_loop();
                    continue;
                };
                assert_eq!((c.readable.len(), c.writable.len()), (1, 1));
                let (ra, rl) = c.readable[0];
                let (wa, wl) = c.writable[0];
                assert_eq!((rl, wl), (16, 16));
                // SAFETY: the driver published these buffers before its release of avail.idx.
                let (a, b) = unsafe { (*(ra as *const u64), *((ra + 8) as *const u64)) };
                assert_eq!(b, a ^ 0x5a5a_5a5a_5a5a_5a5a, "request payload torn: missing driver release/acquire");
                // SAFETY: device-writable half of the same slot.
                unsafe {
                    *(wa as *mut u64) = a.wrapping_mul(3);
                    *((wa + 8) as *mut u64) = !a;
                }
                dev.complete(c.head, 16);
                served += 1;
            }
            served
        })
    };
    let mut next = 0u64;
    let mut done = 0u64;
    while done < REQUESTS {
        if device.is_finished() {
            panic!("device thread stopped early: {:?}", device.join().err());
        }
        while next < REQUESTS && next - done < SLOTS as u64 {
            let s = (next % SLOTS as u64) as usize;
            let base = slots_ptr + s * 32;
            // SAFETY: slot s is free (its previous request completed; in-flight requests < SLOTS).
            unsafe {
                *(base as *mut u64) = next;
                *((base + 8) as *mut u64) = next ^ 0x5a5a_5a5a_5a5a_5a5a;
                *((base + 16) as *mut u64) = 0;
                *((base + 24) as *mut u64) = 0;
            }
            let bufs = [
                Buffer { addr: base as u64, len: 16, device_writes: false },
                Buffer { addr: base as u64 + 16, len: 16, device_writes: true },
            ];
            q.add(&bufs, next).expect("room for a request");
            next += 1;
        }
        while let Some((token, len)) = q.pop_used() {
            assert_eq!(token, done, "in-order device, in-order completions");
            assert_eq!(len, 16);
            let base = slots_ptr + (token % SLOTS as u64) as usize * 32;
            // SAFETY: the device wrote this half before its release of used.idx.
            let (x, y) = unsafe { (*((base + 16) as *const u64), *((base + 24) as *const u64)) };
            assert_eq!((x, y), (token.wrapping_mul(3), !token), "response torn: missing driver acquire");
            done += 1;
        }
    }
    stop.store(true, Ordering::Relaxed);
    assert_eq!(device.join().expect("device thread"), REQUESTS);
    assert_eq!(q.free_descriptors(), N as usize);
    drop(slots);
}

// ---------------------------------------------------------------------------------------------
// Notification suppression (§2.7.7, §2.7.10).

fn need_event(event: u16, new: u16, old: u16) -> bool {
    new.wrapping_sub(event).wrapping_sub(1) < new.wrapping_sub(old)
}

#[test]
fn notification_suppression() {
    // Without EVENT_IDX: notify unless the device set VIRTQ_USED_F_NO_NOTIFY.
    let (mut q, ring) = new_queue(8);
    let dev = Device::new(&ring, 8);
    let buf = [0u8; 4];
    let b = [Buffer { addr: buf.as_ptr() as u64, len: 4, device_writes: false }];
    q.add(&b, 1).unwrap();
    assert!(q.should_notify());
    dev.set_used_flags(1);
    q.add(&b, 2).unwrap();
    assert!(!q.should_notify(), "NO_NOTIFY honoured");

    // With EVENT_IDX: notify exactly when avail.idx crossed avail_event since the last check.
    let mut rng = Rng(99);
    for trial in 0..200 {
        let (mut q, ring) = new_queue(8);
        q.set_event_idx_enabled(true);
        let mut dev = Device::new(&ring, 8);
        let mut idx = 0u16;
        // The first trial walks the avail index up to just below the 16-bit wrap.
        for _ in 0..if trial == 0 { 65_530 } else { 0 } {
            q.add(&b, 7).unwrap();
            let c = dev.next_chain().unwrap();
            dev.complete(c.head, 0);
            q.pop_used().unwrap();
            idx = idx.wrapping_add(1);
        }
        let _ = q.should_notify(); // resynchronise the driver's last-notified index
        let mut old = idx;
        for _ in 0..30 {
            let adds = 1 + rng.below(3);
            for _ in 0..adds {
                if q.add(&b, 7).is_err() {
                    while let Some(c) = dev.next_chain() {
                        dev.complete(c.head, 0);
                    }
                    while q.pop_used().is_some() {}
                    q.add(&b, 7).unwrap();
                }
                idx = idx.wrapping_add(1);
            }
            let event = old.wrapping_add(rng.below(5) as u16);
            dev.set_avail_event(event);
            assert_eq!(q.should_notify(), need_event(event, idx, old), "event {event} new {idx} old {old}");
            old = idx;
        }
    }
    // The driver asks for an interrupt after a given used index.
    let (mut q, ring) = new_queue(8);
    q.set_event_idx_enabled(true);
    q.set_used_event(0xfffe);
    assert_eq!(Device::new(&ring, 8).used_event(), 0xfffe);
}

// ---------------------------------------------------------------------------------------------
// Device initialisation (§3.1.1) and PCI common configuration (§4.1.4.3) against a mock device.

#[derive(Default)]
struct MockQueue {
    max: u16,
    size: u16,
    msix: u16,
    enabled: bool,
    addrs: (u64, u64, u64),
    notify_off: u16,
}

#[derive(Default)]
struct Mock {
    features: u64,
    reject_features: bool,
    msix_fails: bool,
    feature_select: Cell<u32>,
    driver_features: Cell<u64>,
    status: Cell<u8>,
    status_writes: RefCell<Vec<u8>>,
    selected: Cell<u16>,
    queues: RefCell<Vec<MockQueue>>,
    generation: Cell<u32>,
    config: RefCell<Vec<u8>>,
    bump_generation_on_read: Cell<bool>,
    log: RefCell<Vec<String>>,
}

impl Transport for Mock {
    fn device_feature(&self, select: u32) -> u32 {
        self.feature_select.set(select);
        (self.features >> (32 * select)) as u32
    }
    fn set_driver_feature(&self, select: u32, value: u32) {
        let mask = 0xffff_ffffu64 << (32 * select);
        self.driver_features.set((self.driver_features.get() & !mask) | ((value as u64) << (32 * select)));
    }
    fn status(&self) -> u8 {
        self.status.get()
    }
    fn set_status(&self, s: u8) {
        self.status_writes.borrow_mut().push(s);
        if s & STATUS_DRIVER_OK != 0 {
            assert!(self.queues.borrow().iter().all(|q| q.enabled || q.size == 0), "DRIVER_OK before the queues were enabled");
            self.log.borrow_mut().push("driver_ok".into());
        }
        let s = if s & STATUS_FEATURES_OK != 0 && self.reject_features { s & !STATUS_FEATURES_OK } else { s };
        self.status.set(s);
    }
    fn num_queues(&self) -> u16 {
        self.queues.borrow().len() as u16
    }
    fn select_queue(&self, index: u16) {
        self.selected.set(index);
    }
    fn queue_size(&self) -> u16 {
        let qs = self.queues.borrow();
        let q = &qs[self.selected.get() as usize];
        if q.size == 0 { q.max } else { q.size }
    }
    fn set_queue_size(&self, size: u16) {
        let mut qs = self.queues.borrow_mut();
        let q = &mut qs[self.selected.get() as usize];
        assert!(size <= q.max && size.is_power_of_two(), "queue size {size} (max {})", q.max);
        assert!(!q.enabled);
        q.size = size;
        self.log.borrow_mut().push("size".into());
    }
    fn set_queue_msix_vector(&self, v: u16) {
        self.queues.borrow_mut()[self.selected.get() as usize].msix = if self.msix_fails { 0xffff } else { v };
    }
    fn queue_msix_vector(&self) -> u16 {
        self.queues.borrow()[self.selected.get() as usize].msix
    }
    fn set_queue_addresses(&self, desc: u64, driver: u64, device: u64) {
        let mut qs = self.queues.borrow_mut();
        let q = &mut qs[self.selected.get() as usize];
        assert!(!q.enabled);
        q.addrs = (desc, driver, device);
        self.log.borrow_mut().push("addresses".into());
    }
    fn set_queue_enable(&self, enable: bool) {
        self.queues.borrow_mut()[self.selected.get() as usize].enabled = enable;
        self.log.borrow_mut().push("enable".into());
    }
    fn queue_notify_off(&self) -> u16 {
        self.queues.borrow()[self.selected.get() as usize].notify_off
    }
    fn config_generation(&self) -> u32 {
        self.generation.get()
    }
    fn read_config(&self, offset: usize, buf: &mut [u8]) {
        if self.bump_generation_on_read.replace(false) {
            // A config change lands in the middle of the driver's read.
            self.config.borrow_mut()[offset] ^= 0xff;
            self.generation.set(self.generation.get() + 1);
        }
        buf.copy_from_slice(&self.config.borrow()[offset..offset + buf.len()]);
    }
    fn notify(&self, queue: u16, _notify_off: u16) {
        self.log.borrow_mut().push(format!("notify {queue}"));
    }
}

fn mock(features: u64) -> Mock {
    let m = Mock { features, ..Default::default() };
    m.queues.borrow_mut().push(MockQueue { max: 128, notify_off: 7, ..Default::default() });
    m.config.borrow_mut().extend_from_slice(&[0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]);
    m
}

const BLK_F_FLUSH: u64 = 1 << 9;
const BLK_F_BLK_SIZE: u64 = 1 << 6;

#[test]
fn full_initialisation_sequence() {
    let m = mock(VIRTIO_F_VERSION_1 | VIRTIO_F_RING_EVENT_IDX | VIRTIO_F_INDIRECT_DESC | BLK_F_FLUSH | BLK_F_BLK_SIZE);
    let got = negotiate(&m, VIRTIO_F_RING_EVENT_IDX | BLK_F_FLUSH | (1 << 40)).expect("negotiates");
    assert_eq!(got, VIRTIO_F_VERSION_1 | VIRTIO_F_RING_EVENT_IDX | BLK_F_FLUSH, "wanted ∩ offered, VERSION_1 always");
    assert_eq!(m.driver_features.get(), got, "both 32-bit feature words written");
    assert_eq!(
        *m.status_writes.borrow(),
        vec![0, STATUS_ACKNOWLEDGE, STATUS_ACKNOWLEDGE | STATUS_DRIVER, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK]
    );
    let size = queue_size_for(&m, 0, 256).expect("queue 0 exists");
    assert_eq!(size, 128, "capped at the device maximum");
    let (q, _ring) = new_queue(size);
    let notify_off = setup_queue(&m, 0, &q, 3).expect("queue set up");
    assert_eq!(notify_off, 7);
    {
        let qs = m.queues.borrow();
        assert_eq!(qs[0].size, 128);
        assert_eq!(qs[0].msix, 3);
        assert!(qs[0].enabled);
        assert_eq!(qs[0].addrs, (q.desc_phys_addr(), q.avail_phys_addr(), q.used_phys_addr()));
    }
    assert_eq!(m.log.borrow().last().map(String::as_str), Some("enable"), "enable comes last");
    finish_init(&m);
    assert_eq!(m.status.get(), STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);
}

#[test]
fn initialisation_failures() {
    // No VERSION_1: refuse, set FAILED, never FEATURES_OK.
    let m = mock(BLK_F_FLUSH);
    assert!(negotiate(&m, BLK_F_FLUSH).is_err());
    assert!(m.status.get() & STATUS_FAILED != 0);
    assert!(m.status_writes.borrow().iter().all(|s| s & STATUS_FEATURES_OK == 0));

    // The device clears FEATURES_OK: refuse and set FAILED.
    let mut m = mock(VIRTIO_F_VERSION_1);
    m.reject_features = true;
    assert!(negotiate(&m, 0).is_err());
    assert!(m.status.get() & STATUS_FAILED != 0);
    assert!(m.status_writes.borrow().iter().all(|s| s & STATUS_DRIVER_OK == 0));

    // MSI-X vector not accepted: the queue is not enabled.
    let mut m = mock(VIRTIO_F_VERSION_1);
    m.msix_fails = true;
    negotiate(&m, 0).unwrap();
    let (q, _ring) = new_queue(queue_size_for(&m, 0, 64).unwrap());
    assert!(setup_queue(&m, 0, &q, 1).is_err());
    assert!(!m.queues.borrow()[0].enabled);

    // A queue the device does not offer.
    let m = mock(VIRTIO_F_VERSION_1);
    m.queues.borrow_mut()[0].max = 0;
    assert!(queue_size_for(&m, 0, 64).is_err());
    assert!(queue_size_for(&m, 5, 64).is_err(), "queue index beyond num_queues");
}

#[test]
fn config_reads_retry_on_generation_change() {
    let m = mock(VIRTIO_F_VERSION_1);
    m.bump_generation_on_read.set(true);
    let mut buf = [0u8; 8];
    read_config_consistent(&m, 0, &mut buf);
    assert_eq!(buf, [0xef, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80], "the value after the change, read consistently");
}

// ---------------------------------------------------------------------------------------------
// Request headers (§5.2.6 block, §5.1.6 net), little-endian.

#[test]
fn block_and_net_headers() {
    assert_eq!((BLOCK_T_IN, BLOCK_T_OUT, BLOCK_T_FLUSH, BLOCK_T_GET_ID), (0, 1, 4, 8));
    assert_eq!((BLOCK_S_OK, BLOCK_S_IOERR, BLOCK_S_UNSUPP), (0, 1, 2));
    assert_eq!(
        BlockRequest::new(BLOCK_T_OUT, 0x1122_3344_5566_7788).to_bytes(),
        [1, 0, 0, 0, 0, 0, 0, 0, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
    );
    let h = NetHeader {
        flags: 1,
        gso_type: 0,
        hdr_len: 0x0102,
        gso_size: 0x0304,
        csum_start: 0x0506,
        csum_offset: 0x0708,
        num_buffers: 0x090a,
    };
    let bytes = [1, 0, 2, 1, 4, 3, 6, 5, 8, 7, 0x0a, 9];
    assert_eq!(h.to_bytes(), bytes);
    assert_eq!(NetHeader::from_bytes(&bytes), h);
}
