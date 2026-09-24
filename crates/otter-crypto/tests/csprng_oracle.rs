//! Acceptance oracle for the entropy pool and Csprng of brief M8-T6a, written by the orchestrator.
//! The crate must pass this file unchanged. As an integration test it only sees the public,
//! non-test build of the crate, so everything here is what the kernel will actually get.
//!
//! Credits per byte sample (brief M8-T6a): RDSEED 0.5 bit/bit = 4 bits, RDRAND 0.25 bit/bit = 2 bits,
//! TSC jitter 1 bit per sample. Repetition Count Test cutoff C = 1 + ceil(20 / H) with H the credited
//! bits per sample (SP 800-90B 4.4.1): RDSEED 6, RDRAND 11, jitter 21; the C-th identical sample in a
//! row is the failing one.

use otter_crypto::{Csprng, EntropyPool, EntropySource};
use std::collections::HashSet;

struct XorShift(u64);

impl XorShift {
    fn byte(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 24) as u8
    }
}

const SOURCES: [(EntropySource, u32, usize); 3] =
    [(EntropySource::RdSeed, 4, 6), (EntropySource::RdRand, 2, 11), (EntropySource::TscJitter, 1, 21)];

#[test]
fn credits_follow_the_brief() {
    for (source, bits, _) in SOURCES {
        let mut pool = EntropyPool::new();
        let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
        let needed = 256 / bits as usize;
        for i in 0..needed {
            assert!(!pool.is_ready(), "{source:?}: ready after only {i} samples");
            assert_eq!(pool.add_sample(source, rng.byte()), Ok(bits), "{source:?} credit per sample");
        }
        assert_eq!(pool.credited_bits(), 256, "{source:?}");
        assert!(pool.is_ready(), "{source:?}: ready after {needed} samples");
        assert!(pool.extract_seed().is_some());
        assert_eq!(pool.credited_bits(), 0, "extracting the seed consumes the credit");
        assert!(!pool.is_ready());
    }
}

#[test]
fn stuck_source_fails_at_the_repetition_cutoff() {
    for (source, _, cutoff) in SOURCES {
        let mut pool = EntropyPool::new();
        let first_err = (0..100).position(|_| pool.add_sample(source, 0xAA).is_err());
        assert_eq!(first_err, Some(cutoff - 1), "{source:?}: the {cutoff}th identical sample fails");
        let before = pool.credited_bits();
        let mut rng = XorShift(7);
        for _ in 0..100 {
            assert!(pool.add_sample(source, rng.byte()).is_err(), "{source:?} stays unhealthy");
        }
        assert_eq!(pool.credited_bits(), before, "an unhealthy source is never credited");
        assert_eq!(pool.healthy_source_count(), 0);
    }
}

#[test]
fn biased_source_is_flagged_within_two_windows() {
    let mut pool = EntropyPool::new();
    let mut rng = XorShift(0x0123_4567_89ab_cdef);
    let mut flagged_at = None;
    for i in 0..1024 {
        // 90 % one value, otherwise a random byte.
        let sample = if rng.byte() < 230 { 0x55 } else { rng.byte() };
        if pool.add_sample(EntropySource::TscJitter, sample).is_err() {
            flagged_at = Some(i);
            break;
        }
    }
    assert!(flagged_at.is_some(), "a 90 % biased jitter source must be flagged within 1,024 samples");
}

#[test]
fn good_sources_are_never_flagged() {
    for (source, _, _) in SOURCES {
        let mut pool = EntropyPool::new();
        let mut rng = XorShift(0xfeed_face_cafe_beef);
        for i in 0..1_000_000 {
            if pool.add_sample(source, rng.byte()).is_err() {
                panic!("{source:?}: a uniform source was flagged at sample {i}");
            }
            if pool.is_ready() {
                pool.extract_seed();
            }
        }
    }
}

#[test]
fn a_failed_source_does_not_stop_the_others() {
    let mut pool = EntropyPool::new();
    for _ in 0..30 {
        let _ = pool.add_sample(EntropySource::TscJitter, 0);
    }
    let mut rng = XorShift(11);
    for _ in 0..64 {
        assert_eq!(pool.add_sample(EntropySource::RdSeed, rng.byte()), Ok(4));
    }
    assert!(pool.is_ready());
    assert_eq!(pool.healthy_source_count(), 1);
}

fn seeded(seed: u64) -> (Csprng, XorShift) {
    let mut c = Csprng::new();
    let mut rng = XorShift(seed);
    assert!(!c.is_ready());
    assert!(!c.generate(&mut [0u8; 16]), "no output before the pool is ready");
    for _ in 0..64 {
        c.add_sample(EntropySource::RdSeed, rng.byte());
    }
    assert!(c.is_ready(), "64 RDSEED samples = 256 credited bits");
    (c, rng)
}

#[test]
fn csprng_reseeds_automatically_and_output_looks_uniform() {
    let (mut c, mut rng) = seeded(0x5eed_0001);
    let mut counts = [0u64; 256];
    let mut out = [0u8; 4096];
    let mut total = 0usize;
    while total < 10 << 20 {
        // Keep the pool topped up so every threshold crossing can reseed from fresh entropy.
        for _ in 0..8 {
            c.add_sample(EntropySource::RdSeed, rng.byte());
        }
        assert!(c.generate(&mut out), "generate failed after {total} bytes with a ready pool");
        for &b in out.iter() {
            counts[b as usize] += 1;
        }
        total += out.len();
    }
    assert!(c.reseed_count() >= 9, "10 MiB with a 1 MiB threshold reseeds at least 9 times, got {}", c.reseed_count());
    let expected = total as f64 / 256.0;
    let chi: f64 = counts.iter().map(|&n| (n as f64 - expected).powi(2) / expected).sum();
    // 255 degrees of freedom: the p = 0.001 critical value is 330.5.
    assert!(chi < 330.5, "chi-square {chi:.1} over 10 MiB");
}

#[test]
fn csprng_outputs_are_distinct_and_large_requests_work() {
    let (mut c, _) = seeded(0x5eed_0002);
    let mut seen = HashSet::new();
    for _ in 0..1000 {
        let mut out = [0u8; 32];
        assert!(c.generate(&mut out));
        assert!(seen.insert(out), "a repeated 32-byte output");
    }
    let mut big = vec![0u8; 200_000];
    assert!(c.generate(&mut big), "requests above the DRBG's 64 KiB limit are split, not refused");
    assert!(big[150_000..].iter().any(|&b| b != 0));
}

#[test]
fn csprng_is_deterministic_and_reseeding_mixes_in_the_pool() {
    let (mut a, _) = seeded(0x5eed_0003);
    let (mut b, _) = seeded(0x5eed_0003);
    let (mut x, mut y) = ([0u8; 64], [0u8; 64]);
    assert!(a.generate(&mut x) && b.generate(&mut y));
    assert_eq!(x, y, "same inputs, same stream");
    // Cross the 1 MiB threshold on both, feeding different fresh samples: the streams must diverge.
    let mut chunk = vec![0u8; 4096];
    let (mut ra, mut rb) = (XorShift(1), XorShift(2));
    for _ in 0..300 {
        for _ in 0..64 {
            a.add_sample(EntropySource::RdSeed, ra.byte());
            b.add_sample(EntropySource::RdSeed, rb.byte());
        }
        assert!(a.generate(&mut chunk) && b.generate(&mut chunk));
    }
    assert!(a.reseed_count() >= 1 && b.reseed_count() >= 1);
    assert!(a.generate(&mut x) && b.generate(&mut y));
    assert_ne!(x, y, "a reseed must mix in the pool's fresh entropy");
}

#[test]
fn bias_without_long_runs_is_caught_by_the_adaptive_proportion_test() {
    // Runs of five 0x55 separated by one other byte: 83 % one value, but no run reaches any
    // repetition cutoff (6, 11, 21), so only the Adaptive Proportion Test can flag it.
    for (source, _, _) in SOURCES {
        let mut pool = EntropyPool::new();
        let mut rng = XorShift(0xdead_beef);
        let mut flagged_at = None;
        for i in 0..1024 {
            let sample = if i % 6 == 5 { loop { let b = rng.byte(); if b != 0x55 { break b; } } } else { 0x55 };
            if pool.add_sample(source, sample).is_err() {
                flagged_at = Some(i);
                break;
            }
        }
        assert!(flagged_at.is_some(), "{source:?}: an 83 % biased source must be flagged within two 512-sample windows");
    }
}
