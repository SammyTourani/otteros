//! NIST SP 800-90B continuous health tests and entropy pool.
//!
//! Accumulates samples from named sources with conservative credits and health tests.
//! Only provides seed material after reaching 256 credited bits.

use crate::sha256::Sha256;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// A source of entropy samples (RDSEED, RDRAND, TSC-jitter, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntropySource {
    /// RDSEED: 0.5 bits of entropy per bit, credited as 4 bits per byte sample.
    /// High-quality Intel/AMD hardware instruction.
    RdSeed,
    /// RDRAND: 0.25 bits of entropy per bit, credited as 2 bits per byte sample.
    /// Lower-quality fallback.
    RdRand,
    /// TSC-jitter: 1 bit of entropy per sample.
    TscJitter,
}

/// Health test failures mark a source as unhealthy and stop crediting it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthTestFailure {
    /// Repetition Count Test failed: too many identical samples in a row.
    RepetitionCount,
    /// Adaptive Proportion Test failed: distribution too skewed.
    AdaptiveProportion,
}

/// State for SP 800-90B §4.4.1 Repetition Count Test.
struct RepetitionCountTest {
    /// Last sample seen (or None if not initialized).
    last_sample: Option<u8>,
    /// Current count of consecutive identical samples.
    count: u32,
    /// Cutoff: 1 + ⌈20 / H⌉ where H is the credited bits per sample.
    /// RDSEED (H=4): 1 + ⌈20/4⌉ = 1 + 5 = 6
    /// RDRAND (H=2): 1 + ⌈20/2⌉ = 1 + 10 = 11
    /// TscJitter (H=1): 1 + ⌈20/1⌉ = 1 + 20 = 21
    cutoff: u32,
}

impl RepetitionCountTest {
    fn new(source: EntropySource) -> Self {
        let cutoff = match source {
            EntropySource::RdSeed => 6,     // 1 + ceil(20/4)
            EntropySource::RdRand => 11,    // 1 + ceil(20/2)
            EntropySource::TscJitter => 21, // 1 + ceil(20/1)
        };
        RepetitionCountTest { last_sample: None, count: 0, cutoff }
    }

    /// Tests a single sample. Returns false if the test fails (too many repeats).
    fn test(&mut self, sample: u8) -> bool {
        if Some(sample) == self.last_sample {
            self.count += 1;
            if self.count >= self.cutoff {
                return false; // Failed
            }
        } else {
            self.last_sample = Some(sample);
            self.count = 1;
        }
        true
    }
}

/// SP 800-90B §4.4.2 Adaptive Proportion Test over non-overlapping windows of W = 512 samples:
/// the first sample A of a window is counted through the rest of the window, and a count reaching
/// the cutoff C = 1 + CRITBINOM(W, 2^-H, 1 - 2^-20) is a failure (false-positive rate 2^-20 per
/// window for a source that really delivers H bits per sample). C for H = 4, 2, 1 bits per byte
/// sample (RDSEED, RDRAND, TSC jitter) is 62, 177 and 311, computed from the exact binomial CDF.
struct AdaptiveProportionTest {
    cutoff: u32,
    first: u8,
    count: u32,
    seen: u32,
}

const APT_WINDOW: u32 = 512;

impl AdaptiveProportionTest {
    fn new(source: EntropySource) -> Self {
        let cutoff = match source {
            EntropySource::RdSeed => 62,
            EntropySource::RdRand => 177,
            EntropySource::TscJitter => 311,
        };
        AdaptiveProportionTest { cutoff, first: 0, count: 0, seen: 0 }
    }

    /// Tests one sample; false means the source failed.
    fn test(&mut self, sample: u8) -> bool {
        if self.seen == 0 {
            self.first = sample;
            self.count = 1;
        } else if sample == self.first {
            self.count += 1;
            if self.count >= self.cutoff {
                return false;
            }
        }
        self.seen = (self.seen + 1) % APT_WINDOW;
        true
    }
}

struct SourceHealth {
    rep_count: RepetitionCountTest,
    adaptive_prop: AdaptiveProportionTest,
    healthy: bool,
}

impl SourceHealth {
    fn new(source: EntropySource) -> Self {
        SourceHealth {
            rep_count: RepetitionCountTest::new(source),
            adaptive_prop: AdaptiveProportionTest::new(source),
            healthy: true,
        }
    }

    /// Tests a sample and returns Ok(()) if it passed all health tests,
    /// or Err(failure) if a test failed.
    fn test(&mut self, sample: u8) -> Result<(), HealthTestFailure> {
        if !self.healthy {
            return Err(HealthTestFailure::RepetitionCount); // Already marked unhealthy
        }

        if !self.rep_count.test(sample) {
            self.healthy = false;
            return Err(HealthTestFailure::RepetitionCount);
        }

        if !self.adaptive_prop.test(sample) {
            self.healthy = false;
            return Err(HealthTestFailure::AdaptiveProportion);
        }

        Ok(())
    }

    fn is_healthy(&self) -> bool {
        self.healthy
    }
}

/// Entropy pool that accumulates and condenses samples from multiple sources.
pub struct EntropyPool {
    /// Raw entropy bytes: condensed via SHA-256 to produce seed material.
    pool: Vec<u8>,
    /// Total credited bits (entropy estimate).
    credited_bits: u32,
    /// Per-source health test state.
    sources: BTreeMap<EntropySource, SourceHealth>,
}

impl EntropyPool {
    /// Creates a new empty entropy pool.
    pub fn new() -> Self {
        EntropyPool {
            pool: Vec::new(),
            credited_bits: 0,
            sources: BTreeMap::new(),
        }
    }

    /// Adds a sample from a specific source and applies health tests.
    /// Returns `Ok(credited_bits)` if the sample passed health tests and was credited,
    /// or `Err(failure)` if it failed.
    pub fn add_sample(&mut self, source: EntropySource, sample: u8) -> Result<u32, HealthTestFailure> {
        let health = self.sources.entry(source).or_insert_with(|| SourceHealth::new(source));

        // Perform health tests (returns Err if any test fails)
        health.test(sample)?;

        // Add to pool since the source is healthy
        self.pool.push(sample);

        // Credit bits based on source (brief M8-T6a)
        let credit_bits = match source {
            EntropySource::RdSeed => 4,   // 0.5 bits/bit × 8 bits/byte = 4 bits per sample
            EntropySource::RdRand => 2,   // 0.25 bits/bit × 8 bits/byte = 2 bits per sample
            EntropySource::TscJitter => 1, // 1 bit per sample
        };

        self.credited_bits = self.credited_bits.saturating_add(credit_bits);
        Ok(credit_bits)
    }

    /// Returns true if the pool has accumulated at least 256 credited bits.
    pub fn is_ready(&self) -> bool {
        self.credited_bits >= 256
    }

    /// Returns the current number of credited bits.
    pub fn credited_bits(&self) -> u32 {
        self.credited_bits
    }

    /// Condenses the pool's entropy into a 32-byte seed via SHA-256 hashing.
    /// Clears the pool afterwards. Returns None if not enough entropy yet.
    pub fn extract_seed(&mut self) -> Option<[u8; 32]> {
        if !self.is_ready() {
            return None;
        }

        let digest = Sha256::digest(&self.pool);
        self.pool.clear();
        self.credited_bits = 0;

        let mut seed = [0u8; 32];
        seed.copy_from_slice(&digest[..32]);
        Some(seed)
    }

    /// Returns the number of healthy sources.
    pub fn healthy_source_count(&self) -> usize {
        self.sources.iter().filter(|(_, h)| h.is_healthy()).count()
    }
}

impl Default for EntropyPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_pool_rejects_early_seed() {
        let mut pool = EntropyPool::new();

        // Add a few samples (not enough for 256 bits)
        for i in 0..10u8 {
            let _ = pool.add_sample(EntropySource::TscJitter, i);
        }

        // Should not be ready yet (10 samples * 1 bit = 10 bits < 256)
        assert!(!pool.is_ready());
        assert!(pool.extract_seed().is_none());
    }

    #[test]
    fn entropy_pool_ready_at_256_bits() {
        let mut pool = EntropyPool::new();

        // Add TscJitter samples: each contributes 1 credited bit
        // So we need 256 samples to reach 256 bits
        for i in 0..256u16 {
            let _ = pool.add_sample(EntropySource::TscJitter, ((i as u32 * 17) % 256) as u8);
        }

        assert!(pool.is_ready());
        let seed = pool.extract_seed();
        assert!(seed.is_some());

        // After extraction, pool is reset
        assert!(!pool.is_ready());
    }

    #[test]
    fn extract_seed_is_deterministic() {
        let mut pool1 = EntropyPool::new();
        let mut pool2 = EntropyPool::new();

        // Add the same samples to both pools
        for i in 0..256u16 {
            let sample = (i as u8).wrapping_mul(13);
            let _ = pool1.add_sample(EntropySource::TscJitter, sample);
            let _ = pool2.add_sample(EntropySource::TscJitter, sample);
        }

        let seed1 = pool1.extract_seed();
        let seed2 = pool2.extract_seed();

        assert_eq!(seed1, seed2, "Same inputs should produce same seed");
    }

    #[test]
    fn different_inputs_different_seeds() {
        let mut pool1 = EntropyPool::new();
        let mut pool2 = EntropyPool::new();

        // Add different samples to each pool
        for i in 0..256u16 {
            let _ = pool1.add_sample(EntropySource::TscJitter, (i as u8).wrapping_mul(13));
            let _ = pool2.add_sample(EntropySource::TscJitter, (i as u8).wrapping_mul(17));
        }

        let seed1 = pool1.extract_seed();
        let seed2 = pool2.extract_seed();

        assert_ne!(seed1, seed2, "Different inputs should produce different seeds");
    }
}
