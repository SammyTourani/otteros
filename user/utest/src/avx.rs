//! AVX register state across context switches (brief M6-T3), written by the orchestrator: the
//! kernel must pass this unchanged. This process and a spawned copy of itself each load a
//! different pattern into ymm0..=ymm15, yield 300 times (switching to each other) with busy work
//! in between, and store the registers back; any lost upper half (an FXSAVE-only kernel keeps just
//! the low 128 bits) or cross-talk shows up as a mismatch. Everything between the load and the
//! store is one `asm!` block, so no compiler-generated code can touch the registers meanwhile.

use alloc::string::String;

const ITERATIONS: u64 = 300;
const SPIN: u64 = 20_000;

/// CPUID.1:ECX bits 26 (XSAVE), 27 (OSXSAVE: the OS enabled XSAVE) and 28 (AVX).
fn cpuid1_ecx() -> u32 {
    core::arch::x86_64::__cpuid(1).ecx
}

fn xgetbv0() -> u64 {
    let (eax, edx): (u32, u32);
    // SAFETY: only executed after CPUID reported OSXSAVE, which makes XGETBV legal in ring 3;
    // it reads XCR0 into edx:eax and has no other effect.
    unsafe {
        core::arch::asm!("xgetbv", in("ecx") 0u32, out("eax") eax, out("edx") edx, options(nomem, nostack, preserves_flags));
    }
    ((edx as u64) << 32) | eax as u64
}

/// Ok(false) when the CPU has no AVX (nothing to test); Err when it has AVX but the kernel did not
/// enable it for user code.
fn avx_enabled() -> Result<bool, String> {
    let ecx = cpuid1_ecx();
    if ecx & (1 << 26) == 0 || ecx & (1 << 28) == 0 {
        return Ok(false);
    }
    if ecx & (1 << 27) == 0 {
        return Err("the CPU has XSAVE and AVX but CR4.OSXSAVE is clear".into());
    }
    let xcr0 = xgetbv0();
    if xcr0 & 0b111 != 0b111 {
        return Err(alloc::format!("XCR0 is {xcr0:#x}; x87, SSE and AVX state must all be enabled"));
    }
    Ok(true)
}

fn pattern(seed: u64) -> [u8; 512] {
    let mut out = [0u8; 512];
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for b in out.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = (x >> 32) as u8;
    }
    out
}

/// Loads `seed`'s pattern into ymm0..=ymm15, yields ITERATIONS times with busy work in between,
/// stores the registers back and compares.
#[target_feature(enable = "avx")]
fn hold(seed: u64) -> bool {
    let src = pattern(seed);
    let mut dst = [0u8; 512];
    // SAFETY: `src`/`dst` are 512-byte buffers that outlive the block (r14/r15 point at them);
    // vmovdqu has no alignment requirement; syscall 4 (yield) takes no arguments and, like every
    // syscall, clobbers only rax, rcx and r11 (D16), all declared; r12/r13 are loop counters;
    // every ymm register is declared clobbered. The AVX target feature is enabled on this
    // function and the caller checked the CPU and OS support first.
    unsafe {
        core::arch::asm!(
            "vmovdqu ymm0, [r14]",
            "vmovdqu ymm1, [r14 + 32]",
            "vmovdqu ymm2, [r14 + 64]",
            "vmovdqu ymm3, [r14 + 96]",
            "vmovdqu ymm4, [r14 + 128]",
            "vmovdqu ymm5, [r14 + 160]",
            "vmovdqu ymm6, [r14 + 192]",
            "vmovdqu ymm7, [r14 + 224]",
            "vmovdqu ymm8, [r14 + 256]",
            "vmovdqu ymm9, [r14 + 288]",
            "vmovdqu ymm10, [r14 + 320]",
            "vmovdqu ymm11, [r14 + 352]",
            "vmovdqu ymm12, [r14 + 384]",
            "vmovdqu ymm13, [r14 + 416]",
            "vmovdqu ymm14, [r14 + 448]",
            "vmovdqu ymm15, [r14 + 480]",
            "mov r12, {iterations}",
            "2:",
            "mov eax, 4",
            "syscall",
            "mov r13, {spin}",
            "3:",
            "dec r13",
            "jnz 3b",
            "dec r12",
            "jnz 2b",
            "vmovdqu [r15], ymm0",
            "vmovdqu [r15 + 32], ymm1",
            "vmovdqu [r15 + 64], ymm2",
            "vmovdqu [r15 + 96], ymm3",
            "vmovdqu [r15 + 128], ymm4",
            "vmovdqu [r15 + 160], ymm5",
            "vmovdqu [r15 + 192], ymm6",
            "vmovdqu [r15 + 224], ymm7",
            "vmovdqu [r15 + 256], ymm8",
            "vmovdqu [r15 + 288], ymm9",
            "vmovdqu [r15 + 320], ymm10",
            "vmovdqu [r15 + 352], ymm11",
            "vmovdqu [r15 + 384], ymm12",
            "vmovdqu [r15 + 416], ymm13",
            "vmovdqu [r15 + 448], ymm14",
            "vmovdqu [r15 + 480], ymm15",
            iterations = const ITERATIONS,
            spin = const SPIN,
            in("r14") src.as_ptr(),
            in("r15") dst.as_mut_ptr(),
            out("r12") _, out("r13") _, out("rax") _, out("rcx") _, out("r11") _,
            out("ymm0") _, out("ymm1") _, out("ymm2") _, out("ymm3") _,
            out("ymm4") _, out("ymm5") _, out("ymm6") _, out("ymm7") _,
            out("ymm8") _, out("ymm9") _, out("ymm10") _, out("ymm11") _,
            out("ymm12") _, out("ymm13") _, out("ymm14") _, out("ymm15") _,
        );
    }
    src == dst
}

/// `utest avx-worker <seed>`: exit 0 when the registers survived, 1 when they did not, 2 when
/// AVX is unusable.
pub fn worker(seed: &str) -> i32 {
    match avx_enabled() {
        Ok(true) => {}
        _ => return 2,
    }
    let seed = seed.parse::<u64>().unwrap_or(2);
    // SAFETY: avx_enabled() confirmed CPU and OS support for the AVX target feature.
    if unsafe { hold(seed) } { 0 } else { 1 }
}

/// The check: runs a worker alongside this process.
pub fn test_avx_survives_context_switches() -> Result<(), String> {
    if !avx_enabled()? {
        libotter::println!("[utest] note: this CPU has no AVX; avx check skipped");
        return Ok(());
    }
    let child = libotter::spawn("/bin/utest", &["avx-worker", "2"]).map_err(|e| alloc::format!("spawn failed: {e:?}"))?;
    // SAFETY: avx_enabled() confirmed CPU and OS support for the AVX target feature.
    let mine = unsafe { hold(1) };
    let theirs = libotter::wait(child).map_err(|e| alloc::format!("wait failed: {e:?}"))?;
    match (mine, theirs) {
        (true, 0) => Ok(()),
        (false, _) => Err("this process's ymm registers changed across context switches".into()),
        (_, code) => Err(alloc::format!("the worker's ymm registers changed (exit {code})")),
    }
}
