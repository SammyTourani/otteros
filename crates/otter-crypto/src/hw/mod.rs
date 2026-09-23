//! CPUID-gated hardware acceleration: AES-NI and PCLMULQDQ.
//!
//! DECISIONS.md D22 requires the software paths ([`crate::aes`], [`crate::ghash`])
//! to stay constant-time and D2/D27 forbid dependencies, so this crate cannot use
//! a crate like `raw-cpuid` or `cpufeatures` to query the CPU; `core::arch::x86_64`
//! (part of `core`, not a dependency) gives `__cpuid` and the AES-NI/PCLMULQDQ
//! intrinsics directly. The crate root is `#![deny(unsafe_code)]`, not `forbid`,
//! specifically so this module -- and only this module -- can carry
//! `#![allow(unsafe_code)]`; every `unsafe` block below has a `// SAFETY:` comment.
//!
//! On any host that is not `x86_64` (this crate's tests run on `aarch64-apple-darwin`
//! by default, see `crates/.cargo/config.toml`), the actual intrinsic-wrapping code
//! in [`aes`] and [`ghash`] does not exist at all -- `aes_ni_available`/
//! `pclmulqdq_available` are trivial `false`-returning stubs with no `unsafe` and no
//! CPU-specific code, so callers ([`crate::gcm`]) never need their own
//! `cfg(target_arch = "x86_64")` guards.
//!
//! [`aes`]/[`ghash`] additionally need `target_feature = "sse2"` (true by default
//! for any ordinary hosted x86_64 target, including `x86_64-apple-darwin`): the
//! kernel's own `x86_64-unknown-none` target is soft-float with SSE disabled at
//! the ABI level (DECISIONS.md D13), and LLVM refuses to enable an SSE-dependent
//! feature like `aes`/`pclmulqdq` at all on a target whose baseline has SSE off,
//! so this module simply has nothing to offer there -- CPUID detection alone
//! (harmless without SSE) still works, it would just always have no hardware
//! path to hand back.
#![allow(unsafe_code)]

#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
pub mod aes;
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
pub mod ghash;

#[cfg(target_arch = "x86_64")]
mod cpuid {
    use core::arch::x86_64::__cpuid;
    use core::sync::atomic::{AtomicU8, Ordering};

    const UNKNOWN: u8 = 0;
    const UNSUPPORTED: u8 = 1;
    const SUPPORTED: u8 = 2;

    static AES_NI: AtomicU8 = AtomicU8::new(UNKNOWN);
    static PCLMULQDQ: AtomicU8 = AtomicU8::new(UNKNOWN);

    /// Reads `slot`, computing and caching `detect()` the first time (CPUID leaf 1
    /// never changes for the lifetime of a running CPU, so a racy first write from
    /// two threads is harmless -- both computations agree, `Relaxed` is enough).
    fn cached(slot: &AtomicU8, detect: fn() -> bool) -> bool {
        match slot.load(Ordering::Relaxed) {
            SUPPORTED => true,
            UNSUPPORTED => false,
            _ => {
                let supported = detect();
                slot.store(if supported { SUPPORTED } else { UNSUPPORTED }, Ordering::Relaxed);
                supported
            }
        }
    }

    fn cpuid_leaf1_ecx() -> u32 {
        // `__cpuid` is a safe function (core::arch::x86_64): CPUID leaf 1 (basic
        // feature information) is available on every x86_64 CPU -- CPUID support
        // itself is architecturally mandatory for x86_64 (unlike on 32-bit x86,
        // where its presence must be probed via EFLAGS.ID), so it can never fault.
        __cpuid(1).ecx
    }

    /// AES-NI: CPUID leaf 1, ECX bit 25.
    pub fn aes_ni_available() -> bool {
        cached(&AES_NI, || cpuid_leaf1_ecx() & (1 << 25) != 0)
    }

    /// PCLMULQDQ: CPUID leaf 1, ECX bit 1.
    pub fn pclmulqdq_available() -> bool {
        cached(&PCLMULQDQ, || cpuid_leaf1_ecx() & (1 << 1) != 0)
    }
}

#[cfg(target_arch = "x86_64")]
pub use cpuid::{aes_ni_available, pclmulqdq_available};

/// Always `false` off x86_64: there is no AES-NI to detect.
#[cfg(not(target_arch = "x86_64"))]
pub fn aes_ni_available() -> bool {
    false
}

/// Always `false` off x86_64: there is no PCLMULQDQ to detect.
#[cfg(not(target_arch = "x86_64"))]
pub fn pclmulqdq_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_is_stable_across_repeated_calls() {
        // Exercises the atomic cache's "already cached" path, whatever the answer
        // is on this host (aarch64 in normal `cargo test`, x86_64 under Rosetta).
        let a = aes_ni_available();
        let b = aes_ni_available();
        assert_eq!(a, b);
        let c = pclmulqdq_available();
        let d = pclmulqdq_available();
        assert_eq!(c, d);
    }

    #[test]
    #[cfg(not(target_arch = "x86_64"))]
    fn off_x86_64_nothing_is_available() {
        assert!(!aes_ni_available());
        assert!(!pclmulqdq_available());
    }
}
