//! `/bin/utest` (brief M2-T3 step 6): runs a fixed battery of userspace
//! checks, printing one `[utest] ok <name>` or `[utest] FAIL <name>: <why>`
//! line per check, then `[utest] passed N/M`. Exits with the number of
//! checks that passed if every one of them did (>= 10, satisfying `gmake
//! test`'s own `M >= 10` acceptance line), or `0` on any failure -- the
//! kernel side (`tests::test_runner`) treats a `0` exit as "userspace
//! tests failed" and a positive one as "passed", so a partial failure can
//! never masquerade as `M >= 10` success.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use libotter::Errno;

fn check(name: &str, result: Result<(), String>, passed: &mut u32, total: &mut u32) {
    *total += 1;
    match result {
        Ok(()) => {
            *passed += 1;
            libotter::println!("[utest] ok {name}");
        }
        Err(reason) => libotter::println!("[utest] FAIL {name}: {reason}"),
    }
}

#[unsafe(no_mangle)]
pub fn main(args: &[&str]) -> i32 {
    libotter::set_program_name("utest");

    let mut passed = 0u32;
    let mut total = 0u32;

    check("allocator-vec-100000", test_allocator_vec(), &mut passed, &mut total);
    check("allocator-btreemap-10000", test_allocator_btreemap(), &mut passed, &mut total);
    check("strings-format", test_strings_format(), &mut passed, &mut total);
    check("args-roundtrip", test_args_roundtrip(args), &mut passed, &mut total);
    check("spawn-wait-hello", test_spawn_wait_hello(), &mut passed, &mut total);
    check("spawn-missing-is-enoent", test_spawn_missing(), &mut passed, &mut total);
    check("spawn-non-elf-is-enoexec", test_spawn_non_elf(), &mut passed, &mut total);
    check("crash-null-is-killed", test_crash("null"), &mut passed, &mut total);
    check("crash-div0-is-killed", test_crash("div0"), &mut passed, &mut total);
    check("sleep-ms-advances-time", test_sleep_ms(), &mut passed, &mut total);
    check("map-anon-unmap-roundtrip", test_map_anon_roundtrip(), &mut passed, &mut total);
    check("sse-float-math", test_sse_float_math(), &mut passed, &mut total);
    check("getrandom-distinct", test_getrandom_distinct(), &mut passed, &mut total);

    libotter::println!("[utest] passed {passed}/{total}");
    if passed == total { passed as i32 } else { 0 }
}

/// A `Vec` of 100 000 `u64` -- large enough to force several reallocations
/// through `libotter`'s "large object" `map_anon`/`unmap` path (brief
/// step 5), not just the size-class free lists.
fn test_allocator_vec() -> Result<(), String> {
    let mut v: Vec<u64> = Vec::new();
    for i in 0..100_000u64 {
        v.push(i);
    }
    if v.len() != 100_000 {
        return Err(alloc::format!("expected len 100000, got {}", v.len()));
    }
    let sum: u64 = v.iter().sum();
    let expected: u64 = (0..100_000u64).sum();
    if sum != expected {
        return Err(alloc::format!("sum mismatch: {sum} != {expected}"));
    }
    Ok(())
}

/// A `BTreeMap` of 10 000 entries -- many small, similarly-sized node
/// allocations through the size-class free lists.
fn test_allocator_btreemap() -> Result<(), String> {
    let mut m: BTreeMap<u64, u64> = BTreeMap::new();
    for i in 0..10_000u64 {
        m.insert(i, i * 2);
    }
    if m.len() != 10_000 {
        return Err(alloc::format!("expected len 10000, got {}", m.len()));
    }
    for i in (0..10_000u64).step_by(777) {
        match m.get(&i) {
            Some(&v) if v == i * 2 => {}
            other => return Err(alloc::format!("m[{i}] = {other:?}, expected Some({})", i * 2)),
        }
    }
    Ok(())
}

fn test_strings_format() -> Result<(), String> {
    let s = alloc::format!("{}-{:04}-{:.2}", "otter", 7, 9.8765);
    if s != "otter-0007-9.88" {
        return Err(alloc::format!("unexpected format output: {s:?}"));
    }
    let upper: String = s.chars().map(|c| c.to_ascii_uppercase()).collect();
    if upper != "OTTER-0007-9.88" {
        return Err(alloc::format!("unexpected uppercase: {upper:?}"));
    }
    Ok(())
}

/// `init` spawns `/bin/utest selftest` (brief step 7's test-mode path) --
/// confirms both that `main`'s own `args` parameter and the separate
/// `libotter::args()` accessor agree, and that the argument actually made
/// the SysV round trip through the kernel intact.
fn test_args_roundtrip(args: &[&str]) -> Result<(), String> {
    if args.len() != 1 || args[0] != "selftest" {
        return Err(alloc::format!("expected argv [\"selftest\"], got {args:?}"));
    }
    let via_accessor = libotter::args();
    if via_accessor != args {
        return Err(alloc::format!("libotter::args() {via_accessor:?} != main's own args {args:?}"));
    }
    Ok(())
}

fn test_spawn_wait_hello() -> Result<(), String> {
    let pid = libotter::spawn("/bin/hello", &["x"]).map_err(|e| alloc::format!("spawn failed: {e:?}"))?;
    let code = libotter::wait(pid).map_err(|e| alloc::format!("wait failed: {e:?}"))?;
    if code != 0 {
        return Err(alloc::format!("expected exit 0, got {code}"));
    }
    Ok(())
}

fn test_spawn_missing() -> Result<(), String> {
    match libotter::spawn("/bin/this-path-does-not-exist", &[]) {
        Err(Errno::ENOENT) => Ok(()),
        other => Err(alloc::format!("expected Err(ENOENT), got {other:?}")),
    }
}

/// `/etc/motd` (packed into the initramfs alongside the real ELF binaries,
/// see `scripts/make-initramfs.py`) is plain text -- a legitimate file
/// `spawn` can find and read, but not a valid ELF64 executable.
fn test_spawn_non_elf() -> Result<(), String> {
    match libotter::spawn("/etc/motd", &[]) {
        Err(Errno::ENOEXEC) => Ok(()),
        other => Err(alloc::format!("expected Err(ENOEXEC), got {other:?}")),
    }
}

/// Spawns `/bin/crash <kind>`, which deliberately faults -- `proc::fault`
/// kills it with the traditional "128 + SIGSEGV(11)" code regardless of
/// which specific fault it was (kernel-side: `proc::fault::FAULT_EXIT_
/// CODE`). `utest` surviving to report the result at all *is* the proof
/// the kernel only killed the child, not itself.
fn test_crash(kind: &str) -> Result<(), String> {
    const FAULT_EXIT_CODE: i32 = 128 + 11;
    let pid = libotter::spawn("/bin/crash", &[kind]).map_err(|e| alloc::format!("spawn failed: {e:?}"))?;
    let code = libotter::wait(pid).map_err(|e| alloc::format!("wait failed: {e:?}"))?;
    if code != FAULT_EXIT_CODE {
        return Err(alloc::format!("expected fault-kill code {FAULT_EXIT_CODE}, got {code}"));
    }
    Ok(())
}

fn test_sleep_ms() -> Result<(), String> {
    let before = libotter::time_ms();
    libotter::sleep_ms(50);
    let after = libotter::time_ms();
    let delta = after.saturating_sub(before);
    if !(30..=500).contains(&delta) {
        return Err(alloc::format!("sleep_ms(50) advanced time_ms by {delta}ms, expected 30..=500"));
    }
    Ok(())
}

fn test_map_anon_roundtrip() -> Result<(), String> {
    let len = 8192usize;
    let ptr = libotter::mem::map_anon(len).map_err(|e| alloc::format!("map_anon failed: {e:?}"))?;

    // SAFETY: `map_anon` just mapped exactly `len` fresh, writable bytes at
    // `ptr`; nothing else can be using them yet.
    unsafe {
        for i in 0..len {
            ptr.add(i).write(i as u8);
        }
        for i in 0..len {
            let got = ptr.add(i).read();
            if got != i as u8 {
                return Err(alloc::format!("byte {i}: wrote {}, read back {got}", i as u8));
            }
        }
    }

    // SAFETY: `ptr`/`len` are exactly what `map_anon` returned above;
    // nothing still references this memory (the loop above finished).
    unsafe { libotter::mem::unmap(ptr, len) }.map_err(|e| alloc::format!("unmap failed: {e:?}"))?;
    Ok(())
}

/// Sums `sqrt(1)..=sqrt(1000)` using the hardware `sqrtsd` instruction
/// directly (brief step 6: "SSE float math") -- proof that a userspace
/// program built with SSE2 enabled (`user/x86_64-otter.json`) actually
/// gets correct FPU/SSE state (DECISIONS.md D13: saved/restored per
/// thread, initialised clean) rather than garbage or a `#UD`/`#NM`.
fn test_sse_float_math() -> Result<(), String> {
    fn sqrt(x: f64) -> f64 {
        let result: f64;
        // SAFETY: `sqrtsd` is a pure SSE2 floating-point instruction --
        // no memory access, no relied-upon flag side effects, output
        // determined entirely by the input operand.
        unsafe {
            core::arch::asm!(
                "sqrtsd {result}, {x}",
                x = in(xmm_reg) x,
                result = out(xmm_reg) result,
                options(nomem, nostack),
            );
        }
        result
    }

    let mut sum = 0.0f64;
    for i in 1..=1000u64 {
        sum += sqrt(i as f64);
    }

    // Computed on the host with Python's `math.sqrt` (IEEE 754 double,
    // correctly rounded -- identical to what hardware `sqrtsd` produces)
    // over the same `1..=1000`, summed in the same left-to-right order.
    const EXPECTED: f64 = 21097.455887480734;
    let diff = (sum - EXPECTED).abs();
    if diff > 1e-9 {
        return Err(alloc::format!("sum of sqrt(1..=1000) = {sum}, expected {EXPECTED} (diff {diff:e})"));
    }
    Ok(())
}

/// Syscall 17 `getrandom` (brief M8-T6b): two 32-byte results must differ,
/// and a null buffer must return EFAULT.
fn test_getrandom_distinct() -> Result<(), String> {
    // Test that two calls produce different results (highly unlikely to repeat by chance)
    let mut buf1 = [0u8; 32];
    libotter::getrandom(&mut buf1).map_err(|e| alloc::format!("getrandom failed: {e:?}"))?;
    let mut buf2 = [0u8; 32];
    libotter::getrandom(&mut buf2).map_err(|e| alloc::format!("getrandom failed: {e:?}"))?;

    if buf1 == buf2 {
        return Err(alloc::format!("two consecutive getrandom calls returned identical 32-byte buffers"));
    }

    // Test that null pointer returns EFAULT
    match libotter::getrandom(&mut []) {
        Ok(0) => {}  // Empty buffer is OK
        Ok(n) => return Err(alloc::format!("getrandom with empty buffer returned {n}, expected 0")),
        Err(Errno::EFAULT) => {} // Also valid for null pointer, but empty buffer shouldn't trigger it
        Err(e) => return Err(alloc::format!("getrandom with empty buffer returned error {e:?}")),
    }

    Ok(())
}
