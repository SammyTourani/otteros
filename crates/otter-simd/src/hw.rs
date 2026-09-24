//! Hardware capability detection via CPUID and XGETBV.
//! Follows the pattern in crates/otter-crypto/src/hw/mod.rs (D27).

#![allow(unsafe_code)]

use core::sync::atomic::{AtomicU8, Ordering};

const UNKNOWN: u8 = 0;
const UNSUPPORTED: u8 = 1;
const SUPPORTED: u8 = 2;

static SSE2: AtomicU8 = AtomicU8::new(UNKNOWN);
static AVX2: AtomicU8 = AtomicU8::new(UNKNOWN);
static FMA: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Cached CPUID feature detection.
fn cached(slot: &AtomicU8, detect: fn() -> bool) -> bool {
    match slot.load(Ordering::Relaxed) {
        SUPPORTED => true,
        UNSUPPORTED => false,
        _ => {
            let supported = detect();
            slot.store(
                if supported { SUPPORTED } else { UNSUPPORTED },
                Ordering::Relaxed,
            );
            supported
        }
    }
}

/// CPUID leaf 1: basic feature flags.
fn cpuid_leaf1() -> (u32, u32) {
    // core::arch::x86_64::__cpuid is a safe function: CPUID leaf 1 is architecturally
    // available on all x86_64 (mandatory in the ISA).
    let result = core::arch::x86_64::__cpuid(1);
    (result.ecx, result.edx)
}

/// CPUID leaf 7 sub-leaf 0: extended feature flags.
fn cpuid_leaf7() -> u32 {
    // core::arch::x86_64::__cpuid is a safe function: CPUID leaf 7 is available on
    // all modern x86_64 and gracefully returns 0 for unavailable sub-leaves.
    let result = core::arch::x86_64::__cpuid(7);
    result.ebx
}

/// XGETBV with ECX=0: get XCR0 (processor extended state control register).
fn xgetbv_xcr0() -> u32 {
    // SAFETY: XGETBV is safe to call on x86_64 systems that report XGETBV support in CPUID.
    // We only call this after verifying the XGETBV bit (leaf 1 ECX bit 26), so it cannot fault.
    // Reads a read-only state register (XCR0).
    unsafe { core::arch::x86_64::_xgetbv(0) as u32 }
}

/// SSE2 support: CPUID leaf 1 EDX bit 26.
/// Always available on x86_64 (baseline ISA).
pub fn sse2_supported() -> bool {
    cached(&SSE2, || {
        let (_ecx, edx) = cpuid_leaf1();
        edx & (1 << 26) != 0
    })
}

/// FMA support: CPUID leaf 1 ECX bit 12 + XGETBV YMM support.
pub fn fma_supported() -> bool {
    cached(&FMA, || {
        let (ecx, _edx) = cpuid_leaf1();
        if ecx & (1 << 12) == 0 {
            return false; // FMA bit not set
        }
        // Check if XGETBV is available (ECX bit 26)
        if ecx & (1 << 26) == 0 {
            return false;
        }
        // XGETBV: check if OS supports YMM (XCR0 bit 2)
        let xcr0 = xgetbv_xcr0();
        xcr0 & (1 << 2) != 0
    })
}

/// AVX2 support: CPUID leaf 7 EBX bit 5 + XGETBV YMM support.
pub fn avx2_supported() -> bool {
    cached(&AVX2, || {
        // First check AVX2 bit in leaf 7
        let ebx = cpuid_leaf7();
        if ebx & (1 << 5) == 0 {
            return false; // AVX2 bit not set
        }

        // Check if XGETBV is available (leaf 1 ECX bit 26)
        let (ecx, _edx) = cpuid_leaf1();
        if ecx & (1 << 26) == 0 {
            return false;
        }

        // XGETBV: check if OS supports YMM (XCR0 bit 2)
        let xcr0 = xgetbv_xcr0();
        xcr0 & (1 << 2) != 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpuid_detection_is_stable() {
        let a = sse2_supported();
        let b = sse2_supported();
        assert_eq!(a, b, "SSE2 detection should be stable");

        let c = avx2_supported();
        let d = avx2_supported();
        assert_eq!(c, d, "AVX2 detection should be stable");

        let e = fma_supported();
        let f = fma_supported();
        assert_eq!(e, f, "FMA detection should be stable");
    }
}
