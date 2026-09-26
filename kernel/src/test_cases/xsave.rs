//! XSAVE tests for brief M6-T3, written by the orchestrator: the implementation must pass these
//! unchanged, together with the userspace check in user/utest/src/avx.rs (AVX registers survive
//! context switches between two processes).
//!
//! API: `arch::x86_64::fpu::{mode() -> Mode::{Fxsave, Xsave}, xcr0() -> u64 (0 under Fxsave),
//! area_size() -> usize}`.

use core::arch::x86_64::{__cpuid, __cpuid_count};
use otteros_kernel::arch::x86_64::fpu::{self, Mode};

#[test_case]
fn fpu_uses_xsave_when_the_cpu_has_it() {
    let ecx = __cpuid(1).ecx;
    let (has_xsave, has_avx) = (ecx & (1 << 26) != 0, ecx & (1 << 28) != 0);
    if !has_xsave {
        assert_eq!(fpu::mode(), Mode::Fxsave);
        assert_eq!((fpu::xcr0(), fpu::area_size()), (0, 512));
        return;
    }
    assert_eq!(fpu::mode(), Mode::Xsave);
    assert!(ecx & (1 << 27) != 0, "CPUID reports OSXSAVE once CR4.OSXSAVE is set");
    let want = 0b011 | if has_avx { 0b100 } else { 0 };
    assert_eq!(fpu::xcr0(), want, "XCR0 enables x87, SSE and, when present, AVX; nothing else");
    let sized = __cpuid_count(0xD, 0).ebx as usize;
    assert_eq!(fpu::area_size(), sized, "the save area is sized for XCR0 (CPUID.(EAX=0DH,ECX=0):EBX)");
    assert!(fpu::area_size() >= if has_avx { 832 } else { 576 }, "legacy 512 + header 64 (+ 256 for YMM)");
}

#[test_case]
fn fpu_save_areas_are_64_byte_aligned() {
    for _ in 0..8 {
        let area = fpu::SaveArea::pristine();
        assert_eq!(area.as_ptr() as usize % 64, 0, "XSAVE faults on areas not aligned to 64 bytes");
        assert_eq!(area.len(), fpu::area_size());
    }
}
