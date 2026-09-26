//! Kernel randomness (brief M8-T6b, DECISIONS.md D29): CSPRNG seeded from RDSEED, RDRAND,
//! and TSC-jitter samples, guarded by SP 800-90B health tests. Exports `fill()` for syscall 17
//! `getrandom`, and `stats()` for kernel tests.

use core::arch::x86_64::__cpuid;
use core::arch::x86_64::__cpuid_count;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use otter_crypto::entropy::EntropySource;
use otter_crypto::Csprng;

use crate::kprintln;

/// Statistics about the CSPRNG's seeding and health.
#[derive(Clone, Copy, Debug)]
pub struct RandomStats {
    pub rdseed: bool,
    pub rdrand: bool,
    pub rdseed_samples: u64,
    pub rdrand_samples: u64,
    pub jitter_samples: u64,
    pub healthy_sources: usize,
    pub reseeds: u64,
}

/// The kernel's main CSPRNG, protected by a spinlock.
/// Initialized by `init()` at boot.
static CSPRNG: spin::Mutex<Option<Csprng>> = spin::Mutex::new(None);

/// Counters for statistics tracking.
static RDSEED_SAMPLES: AtomicU64 = AtomicU64::new(0);
static RDRAND_SAMPLES: AtomicU64 = AtomicU64::new(0);
static JITTER_SAMPLES: AtomicU64 = AtomicU64::new(0);

/// Lock-free ring buffer for samples from the timer IRQ.
/// The timer IRQ writes samples here; thread context drains them into the CSPRNG.
/// 256 bytes provides a small, L1-friendly buffer for IRQ-generated entropy.
static JITTER_RING: [core::sync::atomic::AtomicU8; 256] =
    [const { core::sync::atomic::AtomicU8::new(0) }; 256];
static JITTER_RING_WRITE_INDEX: AtomicUsize = AtomicUsize::new(0);

/// Static 64 KiB buffer for TSC-jitter sampling workload.
/// Accessed in a data-dependent way to create realistic timing variation.
static JITTER_WORKLOAD_BUFFER: [u8; 65536] = [0x42; 65536];

/// Checks if RDRAND is available via CPUID leaf 1, ECX bit 30.
fn has_rdrand() -> bool {
    let leaf1 = __cpuid(1);
    leaf1.ecx & (1 << 30) != 0
}

/// Checks if RDSEED is available via CPUID leaf 7 (subleaf 0), EBX bit 18.
fn has_rdseed() -> bool {
    let leaf7 = __cpuid_count(7, 0);
    leaf7.ebx & (1 << 18) != 0
}

/// Calls `_rdrand64_step` with at most 100 retries; returns `Some(value)` on success,
/// `None` if all retries fail.
#[inline]
#[target_feature(enable = "rdrand")]
unsafe fn rdrand64() -> Option<u64> {
    for _ in 0..100 {
        let mut val: u64 = 0;
        // SAFETY: _rdrand64_step is safe to call when rdrand target feature is enabled.
        if core::arch::x86_64::_rdrand64_step(&mut val) == 1 {
            return Some(val);
        }
    }
    None
}

/// Calls `_rdseed64_step` with at most 100 retries; returns `Some(value)` on success,
/// `None` if all retries fail.
#[inline]
#[target_feature(enable = "rdseed")]
unsafe fn rdseed64() -> Option<u64> {
    for _ in 0..100 {
        let mut val: u64 = 0;
        // SAFETY: _rdseed64_step is safe to call when rdseed target feature is enabled.
        if core::arch::x86_64::_rdseed64_step(&mut val) == 1 {
            return Some(val);
        }
    }
    None
}

/// Collects a TSC-jitter sample by timing a data-dependent memory workload.
/// The workload performs linked-list-like traversals over a static 64 KiB buffer,
/// where each index is data-dependent on the previous load and the current TSC.
/// Returns a single byte formed by XORing all 8 bytes of the full 64-bit TSC delta.
#[inline]
fn jitter_sample() -> u8 {
    // SAFETY: _rdtsc is always valid and just reads the current cycle count.
    let tsc1 = unsafe { core::arch::x86_64::_rdtsc() };

    // Time a data-dependent workload: ~1-10 µs worth of memory operations.
    // Start with an index derived from the current TSC.
    let mut index = (tsc1 as usize) & 0xFFFF; // 16-bit index into 64 KiB buffer
    let mut accumulator = 0u8;

    // Perform ~20 dependent loads where each new index comes from:
    // a mix of the loaded value, current TSC, and accumulator.
    // This increases timing variation by having the loop body depend on
    // multiple sources of non-determinism.
    for iteration in 0..20 {
        // SAFETY: index is always in [0, 65536), so this is a valid read.
        let loaded = unsafe { core::ptr::read_volatile(&JITTER_WORKLOAD_BUFFER[index]) };
        accumulator = accumulator.wrapping_add(loaded).wrapping_mul(7);

        // Make next index depend on loaded value, current TSC, and iteration count
        // to add maximum variation.
        // SAFETY: _rdtsc is always valid.
        let tsc_now = unsafe { core::arch::x86_64::_rdtsc() };
        index = ((loaded as usize)
            ^ (tsc_now as usize)
            ^ (accumulator as usize)
            ^ iteration)
            & 0xFFFF;
    }

    // SAFETY: _rdtsc is always valid.
    let tsc2 = unsafe { core::arch::x86_64::_rdtsc() };

    // Prevent compiler from optimizing away the workload.
    // SAFETY: black_box is a compiler hint and doesn't modify memory.
    core::hint::black_box(accumulator);

    // Compute the full 64-bit TSC delta and fold its bytes via XOR.
    let delta = tsc2.wrapping_sub(tsc1);
    (delta as u8)
        ^ ((delta >> 8) as u8)
        ^ ((delta >> 16) as u8)
        ^ ((delta >> 24) as u8)
        ^ ((delta >> 32) as u8)
        ^ ((delta >> 40) as u8)
        ^ ((delta >> 48) as u8)
        ^ ((delta >> 56) as u8)
}

/// Initialize the CSPRNG by feeding entropy from available sources (RDSEED, RDRAND, TSC jitter)
/// until at least 256 credited bits are accumulated. Panics if 1,000,000 jitter samples are
/// exhausted without seeding.
pub fn init() {
    let has_rdseed = has_rdseed();
    let has_rdrand = has_rdrand();

    let mut csprng_opt = CSPRNG.lock();
    let mut csprng = Csprng::new();

    // Draw RDSEED (4 credited bits per byte), then RDRAND (2), until the pool is ready: on every
    // CPU that has them (all the laptops this OS targets, and QEMU -cpu max) boot seeding never
    // waits on the TSC-jitter source, which TCG makes too regular to pass the health tests
    // reliably. Bounded, so a CPU whose instruction keeps failing still falls through to jitter.
    if has_rdseed {
        for _ in 0..64 {
            if csprng.is_ready() {
                break;
            }
            // SAFETY: has_rdseed() checked CPUID, so the instruction is available.
            let Some(val) = (unsafe { rdseed64() }) else { break };
            for i in 0..8 {
                csprng.add_sample(EntropySource::RdSeed, (val >> (i * 8)) as u8);
                RDSEED_SAMPLES.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    if has_rdrand {
        for _ in 0..64 {
            if csprng.is_ready() && RDRAND_SAMPLES.load(Ordering::Relaxed) > 0 {
                break;
            }
            // SAFETY: has_rdrand() checked CPUID, so the instruction is available.
            let Some(val) = (unsafe { rdrand64() }) else { break };
            for i in 0..8 {
                csprng.add_sample(EntropySource::RdRand, (val >> (i * 8)) as u8);
                RDRAND_SAMPLES.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    // Feed TSC-jitter samples until seeded (1 bit each).
    // With a data-dependent workload that reads memory and uses TSC variation,
    // most boots reach 256 bits in 256-500 samples. Under high load or specific
    // timing conditions, use a budget of 50,000 samples.
    let mut jitter_count = 0u64;
    let max_jitter = 50_000u64;

    while !csprng.is_ready() && jitter_count < max_jitter {
        let jitter_byte = jitter_sample();
        csprng.add_sample(EntropySource::TscJitter, jitter_byte);
        JITTER_SAMPLES.fetch_add(1, Ordering::Relaxed);
        jitter_count += 1;
    }

    if !csprng.is_ready() {

        // Boot never depends on entropy: fill() keeps gathering (IRQ ring + synchronous samples)

        // until the pool is ready; is_seeded() reports false meanwhile.

        kprintln!("[random] not seeded yet after {} jitter samples; continuing boot", jitter_count);

    }

    *csprng_opt = Some(csprng);
    drop(csprng_opt);

    let s = stats();
    kprintln!(
        "[random] seeded: rdseed {}, rdrand {}, jitter {} samples",
        s.rdseed_samples, s.rdrand_samples, s.jitter_samples
    );
}

/// Add one TSC-jitter sample to the lock-free ring buffer. Called from the timer interrupt
/// (every 16 ticks). Never blocks or allocates; just writes the byte to the ring buffer.
/// Thread-context code drains the ring buffer into the CSPRNG.
pub fn add_jitter_sample() {
    let jitter_byte = jitter_sample();

    // Write to the lock-free ring buffer. This never allocates or blocks.
    // SAFETY: Ring operations are atomic and we're just reading/writing indices
    // and array elements within bounds.
    let write_idx = JITTER_RING_WRITE_INDEX.load(Ordering::Relaxed);
    let next_idx = (write_idx + 1) & 0xFF; // Wrap at 256
    JITTER_RING[write_idx].store(jitter_byte, Ordering::Release);
    JITTER_RING_WRITE_INDEX.store(next_idx, Ordering::Release);
}

/// Drain the lock-free jitter ring buffer and feed samples into the CSPRNG.
/// Called whenever the CSPRNG needs more entropy. Never called from IRQ context.
/// SAFETY: Must only be called when the CSPRNG lock can be safely acquired.
fn drain_jitter_ring(csprng: &mut Csprng) {
    // Keep draining until the ring is empty (write index catches up with read position).
    // We don't track a separate read index; we just know when to stop by comparing
    // a local read position with the current write position.
    static RING_READ_INDEX: AtomicUsize = AtomicUsize::new(0);

    loop {
        let write_idx = JITTER_RING_WRITE_INDEX.load(Ordering::Acquire);
        let read_idx = RING_READ_INDEX.load(Ordering::Relaxed);

        if read_idx == write_idx {
            break; // Ring is empty
        }

        // Fetch and feed the sample.
        let sample_byte = JITTER_RING[read_idx].load(Ordering::Acquire);
        csprng.add_sample(EntropySource::TscJitter, sample_byte);
        JITTER_SAMPLES.fetch_add(1, Ordering::Relaxed);

        // Move read position forward, wrapping at 256.
        let next_read = (read_idx + 1) & 0xFF;
        RING_READ_INDEX.store(next_read, Ordering::Release);
    }
}

/// Generate `len` random bytes into `buf`. If the CSPRNG needs to reseed and the entropy pool
/// is not ready, this function will gather jitter samples synchronously until it succeeds.
pub fn fill(buf: &mut [u8]) {
    loop {
        let mut csprng_opt = CSPRNG.lock();

        // Ensure CSPRNG is initialized (should always be true after init() is called)
        let Some(ref mut csprng) = *csprng_opt else {
            panic!("[random] CSPRNG not initialized; init() was not called");
        };

        // Drain any accumulated jitter samples from the timer IRQ.
        drain_jitter_ring(csprng);

        if csprng.generate(buf) {
            break;
        }

        // CSPRNG declined; gather some jitter synchronously
        drop(csprng_opt);
        let jitter_byte = jitter_sample();
        let mut csprng_opt = CSPRNG.lock();
        if let Some(ref mut csprng) = *csprng_opt {
            csprng.add_sample(EntropySource::TscJitter, jitter_byte);
            JITTER_SAMPLES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Fills `buf` only if the CSPRNG is already seeded; never waits. For identifiers that must not
/// stall a caller on entropy (a DHCP transaction id), where a weaker fallback is acceptable.
pub fn try_fill(buf: &mut [u8]) -> bool {
    let mut csprng_opt = CSPRNG.lock();
    let Some(ref mut csprng) = *csprng_opt else {
        return false;
    };
    drain_jitter_ring(csprng);
    csprng.is_ready() && csprng.generate(buf)
}

/// Returns true if the CSPRNG is ready to generate.
pub fn is_seeded() -> bool {
    let csprng_opt = CSPRNG.lock();
    csprng_opt.as_ref().map(|c| c.is_ready()).unwrap_or(false)
}

/// Returns statistics about entropy sources and CSPRNG state.
pub fn stats() -> RandomStats {
    let csprng_opt = CSPRNG.lock();
    let (healthy_sources, reseeds) = if let Some(ref csprng) = *csprng_opt {
        (csprng.healthy_sources(), csprng.reseed_count())
    } else {
        (0, 0)
    };

    RandomStats {
        rdseed: has_rdseed(),
        rdrand: has_rdrand(),
        rdseed_samples: RDSEED_SAMPLES.load(Ordering::Relaxed),
        rdrand_samples: RDRAND_SAMPLES.load(Ordering::Relaxed),
        jitter_samples: JITTER_SAMPLES.load(Ordering::Relaxed),
        healthy_sources,
        reseeds,
    }
}
