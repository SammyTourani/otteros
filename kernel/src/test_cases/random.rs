//! Kernel random-number tests for brief M8-T6b, written by the orchestrator: the implementation must
//! pass these unchanged. The generator is otter-crypto's Csprng (HMAC_DRBG over a health-tested
//! entropy pool, D29) fed by RDSEED, RDRAND and TSC jitter. The gate boots with `-cpu qemu64`
//! (no RDRAND/RDSEED: jitter alone must seed it) and again with `-cpu max` (both instructions).

use alloc::collections::BTreeSet;
use alloc::vec;
use core::arch::x86_64::{__cpuid, __cpuid_count};
use otteros_kernel::{kprintln, random};

#[test_case]
fn random_is_seeded_at_boot_from_healthy_sources() {
    assert!(random::is_seeded(), "the CSPRNG is seeded before the first test runs");
    let s = random::stats();
    kprintln!(
        "[random] rdseed {} ({} samples), rdrand {} ({} samples), jitter {} samples, healthy sources {}",
        s.rdseed, s.rdseed_samples, s.rdrand, s.rdrand_samples, s.jitter_samples, s.healthy_sources
    );
    assert!(s.healthy_sources >= 1);
    if !s.rdseed && !s.rdrand {
        assert!(s.jitter_samples >= 256, "jitter alone must have supplied 256 credited bits");
    }
}

#[test_case]
fn random_instruction_use_matches_cpuid() {
    let (leaf1, leaf7) = (__cpuid(1), __cpuid_count(7, 0));
    let (has_rdrand, has_rdseed) = (leaf1.ecx & (1 << 30) != 0, leaf7.ebx & (1 << 18) != 0);
    let s = random::stats();
    assert_eq!(s.rdrand, has_rdrand, "RDRAND use follows CPUID");
    assert_eq!(s.rdseed, has_rdseed, "RDSEED use follows CPUID");
    if has_rdseed {
        assert!(s.rdseed_samples > 0, "RDSEED present but never sampled");
    } else if has_rdrand {
        assert!(s.rdrand_samples > 0, "RDRAND present but never sampled");
    }
}

#[test_case]
fn random_outputs_are_distinct() {
    let mut seen = BTreeSet::new();
    for _ in 0..1000 {
        let mut out = [0u8; 32];
        random::fill(&mut out);
        assert!(seen.insert(out), "a repeated 32-byte output");
    }
}

#[test_case]
fn random_output_is_uniform_and_reseeds() {
    let before = random::stats().reseeds;
    let mut counts = [0u64; 256];
    let mut chunk = vec![0u8; 4096];
    let total = 2 * 1024 * 1024;
    for _ in 0..total / chunk.len() {
        random::fill(&mut chunk);
        for &b in chunk.iter() {
            counts[b as usize] += 1;
        }
    }
    let expected = total as u64 / 256;
    let chi_x100: u64 = counts
        .iter()
        .map(|&n| {
            let d = n.abs_diff(expected);
            d * d * 100 / expected
        })
        .sum();
    // 255 degrees of freedom: the p = 0.001 critical value is 330.5 (x100 to stay in integers).
    assert!(chi_x100 < 33_050, "chi-square {}.{:02} over 2 MiB", chi_x100 / 100, chi_x100 % 100);
    assert!(random::stats().reseeds > before, "2 MiB crosses the 1 MiB reseed threshold");
}
