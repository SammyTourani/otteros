//! Brief M8-T2's differential test: on a CPU with AES-NI and PCLMULQDQ (this
//! crate's tests run under Rosetta 2 on `x86_64-apple-darwin` for exactly this
//! reason), 10,000 random `(key, nonce, aad, plaintext)` cases must seal to the
//! identical ciphertext and tag whether `Aes128Gcm`/`Aes256Gcm` use the
//! constant-time software path or the AES-NI/PCLMULQDQ hardware path. Skips
//! (printing why) on any host without both features, rather than failing --
//! there is nothing to differential-test without a hardware path to compare.

use otter_crypto::{Aes128Gcm, Aes256Gcm};

/// A small, dependency-free splitmix64-style PRNG: deterministic and fast,
/// good enough for generating varied (not cryptographically secret) test
/// inputs -- the same role `rand` would play if this crate were allowed one
/// (DECISIONS.md D2/D27: it is not).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }

    fn len_in(&mut self, max: usize) -> usize {
        (self.next_u64() as usize) % (max + 1)
    }
}

const CASES: usize = 10_000;

#[test]
fn aes128_gcm_hardware_matches_software_on_10_000_random_cases() {
    let Some(_probe) = Aes128Gcm::new_hardware(&[0u8; 16]) else {
        eprintln!("skipping: AES-NI/PCLMULQDQ not available on this host");
        return;
    };

    let mut rng = Rng(0x1234_5678_9abc_def0);
    for i in 0..CASES {
        let mut key = [0u8; 16];
        let mut nonce = [0u8; 12];
        rng.fill(&mut key);
        rng.fill(&mut nonce);
        let mut aad = vec![0u8; rng.len_in(64)];
        rng.fill(&mut aad);
        let mut plaintext = vec![0u8; rng.len_in(512)];
        rng.fill(&mut plaintext);

        let hw = Aes128Gcm::new_hardware(&key).expect("checked available above");
        let sw = Aes128Gcm::new_software(&key);

        let mut hw_buf = plaintext.clone();
        let mut sw_buf = plaintext.clone();
        let hw_tag = hw.seal_in_place(&nonce, &aad, &mut hw_buf);
        let sw_tag = sw.seal_in_place(&nonce, &aad, &mut sw_buf);
        assert_eq!(hw_buf, sw_buf, "case {i}: ciphertext mismatch");
        assert_eq!(hw_tag, sw_tag, "case {i}: tag mismatch");

        hw.open_in_place(&nonce, &aad, &mut hw_buf, &hw_tag).expect("hw tag must open under hw");
        assert_eq!(hw_buf, plaintext, "case {i}: hw round-trip mismatch");
        sw.open_in_place(&nonce, &aad, &mut sw_buf, &sw_tag).expect("sw tag must open under sw");
        assert_eq!(sw_buf, plaintext, "case {i}: sw round-trip mismatch");
    }
}

#[test]
fn aes256_gcm_hardware_matches_software_on_10_000_random_cases() {
    let Some(_probe) = Aes256Gcm::new_hardware(&[0u8; 32]) else {
        eprintln!("skipping: AES-NI/PCLMULQDQ not available on this host");
        return;
    };

    let mut rng = Rng(0xfedc_ba98_7654_3210);
    for i in 0..CASES {
        let mut key = [0u8; 32];
        let mut nonce = [0u8; 12];
        rng.fill(&mut key);
        rng.fill(&mut nonce);
        let mut aad = vec![0u8; rng.len_in(64)];
        rng.fill(&mut aad);
        let mut plaintext = vec![0u8; rng.len_in(512)];
        rng.fill(&mut plaintext);

        let hw = Aes256Gcm::new_hardware(&key).expect("checked available above");
        let sw = Aes256Gcm::new_software(&key);

        let mut hw_buf = plaintext.clone();
        let mut sw_buf = plaintext.clone();
        let hw_tag = hw.seal_in_place(&nonce, &aad, &mut hw_buf);
        let sw_tag = sw.seal_in_place(&nonce, &aad, &mut sw_buf);
        assert_eq!(hw_buf, sw_buf, "case {i}: ciphertext mismatch");
        assert_eq!(hw_tag, sw_tag, "case {i}: tag mismatch");
    }
}
