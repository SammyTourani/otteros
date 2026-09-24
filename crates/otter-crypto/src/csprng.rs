//! Cryptographically Secure Pseudo-Random Number Generator combining an entropy pool
//! with HMAC_DRBG for kernel getrandom and similar uses.
//!
//! Provides on-demand reseeding and automatic reseeding after thresholds.

use crate::drbg::HmacDrbg;
use crate::entropy::{EntropyPool, EntropySource};

/// Threshold for reseeding: 2^16 generate calls or 1 MiB of output.
const RESEED_REQUEST_INTERVAL: u64 = 1u64 << 16;  // 2^16 requests
const RESEED_OUTPUT_THRESHOLD: u64 = 1024 * 1024; // 1 MiB

/// The kernel's main CSPRNG, combining a health-tested entropy pool with HMAC_DRBG.
pub struct Csprng {
    /// Entropy pool with health tests.
    pool: EntropyPool,
    /// HMAC_DRBG generator (initialized after pool reaches 256 bits).
    drbg: Option<HmacDrbg>,
    /// Total bytes generated so far (for reseeding threshold).
    generated_bytes: u64,
    /// Number of generate calls since last reseed.
    generate_calls: u64,
    /// Number of times the DRBG has been reseeded.
    reseed_count: u64,
}

impl Csprng {
    /// Creates a new CSPRNG with an empty entropy pool.
    /// The generator will not produce output until at least 256 credited bits
    /// of entropy are accumulated and the DRBG is seeded.
    pub fn new() -> Self {
        Csprng {
            pool: EntropyPool::new(),
            drbg: None,
            generated_bytes: 0,
            generate_calls: 0,
            reseed_count: 0,
        }
    }

    /// Adds a single entropy sample from a specific source.
    /// May trigger DRBG seeding if the pool reaches 256 bits of entropy.
    #[allow(clippy::collapsible_if)]
    pub fn add_sample(&mut self, source: EntropySource, sample: u8) {
        // Try to add to pool (errors are ignored; the source will be marked unhealthy)
        let _ = self.pool.add_sample(source, sample);

        // If DRBG not yet seeded and pool is ready, seed it
        if self.drbg.is_none() && self.pool.is_ready() {
            if let Some(seed) = self.pool.extract_seed() {
                self.drbg = Some(HmacDrbg::instantiate(&seed, &[], &[]));
                self.generated_bytes = 0;
                self.generate_calls = 0;
                self.reseed_count += 1;
            }
        }
    }

    /// Returns true if the DRBG is initialized and ready to generate.
    pub fn is_ready(&self) -> bool {
        self.drbg.is_some()
    }

    /// Generates `out.len()` random bytes into `out`.
    /// Returns true if generation succeeded, false if the DRBG is not yet ready.
    /// Automatically reseeds when thresholds are reached and the pool is ready.
    /// Splits large requests (>64 KiB) into multiple DRBG generate calls.
    pub fn generate(&mut self, out: &mut [u8]) -> bool {
        // Cannot generate if not ready
        if self.drbg.is_none() {
            return false;
        }

        // Check thresholds for automatic reseeding
        if self.generate_calls >= RESEED_REQUEST_INTERVAL
            || self.generated_bytes >= RESEED_OUTPUT_THRESHOLD
        {
            // Attempt automatic reseed from the pool
            if self.pool.is_ready() {
                self.reseed();
            } else {
                // Pool not ready; cannot proceed
                return false;
            }
        }

        // Split large requests into chunks of at most 64 KiB per DRBG request
        const DRBG_MAX_BYTES: usize = 1 << 16; // 64 KiB
        let mut offset = 0;
        while offset < out.len() {
            let chunk_size = (out.len() - offset).min(DRBG_MAX_BYTES);
            let chunk = &mut out[offset..offset + chunk_size];

            // Check thresholds again before each chunk
            if self.generate_calls >= RESEED_REQUEST_INTERVAL
                || self.generated_bytes >= RESEED_OUTPUT_THRESHOLD
            {
                if self.pool.is_ready() {
                    self.reseed();
                } else {
                    return false;
                }
            }

            // Generate this chunk
            let drbg = self.drbg.as_mut().unwrap(); // We know it's Some from above
            match drbg.generate(chunk, &[]) {
                Ok(()) => {
                    self.generated_bytes += chunk.len() as u64;
                    self.generate_calls += 1;
                    offset += chunk_size;
                }
                Err(_) => {
                    // DRBG reseed interval exceeded unexpectedly
                    return false;
                }
            }
        }

        true
    }

    /// Explicitly reseed the DRBG with additional entropy.
    /// Does nothing if the DRBG is not yet initialized or the pool is not ready.
    #[allow(clippy::collapsible_if)]
    pub fn reseed(&mut self) {
        if let Some(seed) = self.pool.extract_seed() {
            if let Some(drbg) = &mut self.drbg {
                drbg.reseed(&seed, &[]);
                self.generated_bytes = 0;
                self.generate_calls = 0;
                self.reseed_count += 1;
            }
        }
    }

    /// Returns the number of healthy entropy sources.
    pub fn healthy_sources(&self) -> usize {
        self.pool.healthy_source_count()
    }

    /// Returns the current number of credited entropy bits.
    pub fn credited_bits(&self) -> u32 {
        self.pool.credited_bits()
    }

    /// Returns the number of times the DRBG has been reseeded.
    pub fn reseed_count(&self) -> u64 {
        self.reseed_count
    }
}

impl Default for Csprng {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csprng_not_ready_initially() {
        let csprng = Csprng::new();
        assert!(!csprng.is_ready());
    }

    #[test]
    fn csprng_ready_after_256_bits() {
        let mut csprng = Csprng::new();

        // Add TscJitter samples: each contributes 1 credited bit
        // So we need 256 samples to reach 256 bits
        for i in 0..256u16 {
            csprng.add_sample(EntropySource::TscJitter, ((i as u32 * 17) % 256) as u8);
        }

        assert!(csprng.is_ready());
    }

    #[test]
    fn csprng_generate_before_ready_fails() {
        let mut csprng = Csprng::new();
        let mut out = [0u8; 32];

        assert!(!csprng.generate(&mut out), "Generate should fail when not ready");
    }

    #[test]
    fn csprng_generate_after_seeding() {
        let mut csprng = Csprng::new();

        // Seed with entropy (256 samples * 1 bit = 256 bits)
        for i in 0..256u16 {
            csprng.add_sample(EntropySource::TscJitter, ((i as u32 * 17) % 256) as u8);
        }

        assert!(csprng.is_ready());

        // Generate should now work
        let mut out = [0u8; 32];
        assert!(csprng.generate(&mut out), "Generate should succeed when ready");

        // Output should not be all zeros (highly unlikely for good DRBG)
        assert!(out.iter().any(|&b| b != 0), "Generated bytes should not all be zero");
    }

    #[test]
    fn csprng_successive_outputs_differ() {
        let mut csprng = Csprng::new();

        // Seed
        for i in 0..256u16 {
            csprng.add_sample(EntropySource::TscJitter, ((i as u32 * 17) % 256) as u8);
        }

        // Generate two blocks
        let mut out1 = [0u8; 32];
        let mut out2 = [0u8; 32];

        assert!(csprng.generate(&mut out1));
        assert!(csprng.generate(&mut out2));

        assert_ne!(out1, out2, "Successive outputs should differ");
    }

    #[test]
    fn csprng_automatic_reseed() {
        let mut csprng = Csprng::new();

        // Seed initially
        for i in 0..256u16 {
            csprng.add_sample(EntropySource::TscJitter, ((i as u32 * 17) % 256) as u8);
        }

        assert!(csprng.is_ready());
        assert_eq!(csprng.reseed_count(), 1);

        // Generate until we hit the output threshold (1 MiB) to trigger automatic reseed
        let mut out = [0u8; 65536]; // 64 KiB chunks
        let initial_reseed = csprng.reseed_count();
        let mut total = 0u64;

        // Need to generate at least 1 MiB to trigger reseed at the 1 MiB boundary
        while total < 1 << 21 {
            // Add fresh entropy to allow automatic reseeding
            for i in 0..256u16 {
                csprng.add_sample(
                    EntropySource::TscJitter,
                    ((i as u32 * 19 + total as u32) % 256) as u8,
                );
            }
            if csprng.generate(&mut out) {
                total += out.len() as u64;
            }
        }

        // Should have reseeded at least once (initial + at least one automatic)
        assert!(csprng.reseed_count() > initial_reseed);
    }
}
