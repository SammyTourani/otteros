# Brief M8-T6b — kernel randomness: RDSEED/RDRAND/TSC jitter into otter-crypto's Csprng, `getrandom`

## Goal
TLS key shares, TCP initial sequence numbers and every other secret the OS makes come from a
seeded, health-tested CSPRNG. The generator itself is proven on the host (M8-T6a, D29); this task
feeds it inside the kernel and exposes it to userspace. The orchestrator's acceptance tests are
kernel/src/test_cases/random.rs (must pass unchanged).

## Applies
D2 (path dependency on the repo's own `otter-crypto`), D29, SYSCALLS.md. Every `unsafe` block gets
a `// SAFETY:` comment. No floating point in the kernel.

## Design
1. `kernel/Cargo.toml`: `otter-crypto = { path = "../crates/otter-crypto" }`; use
   `otter_crypto::{Csprng, EntropySource}` (do not re-implement the DRBG or the health tests).
2. `kernel/src/random.rs`:
   - CPUID: RDRAND = leaf 1 ECX bit 30, RDSEED = leaf 7 (subleaf 0) EBX bit 18 (`__cpuid` is a
     safe function on this toolchain; `_rdrand64_step`/`_rdseed64_step` need
     `#[target_feature(enable = "rdrand")]` / `"rdseed"` wrappers, called only when CPUID says so,
     with retry loops of at most 100 attempts).
   - `init()` at boot, before anything that needs randomness: feed each available instruction's
     64-bit results as 8 byte samples (RdSeed / RdRand sources), then TSC-jitter byte samples (the
     low byte of the difference between successive `rdtsc` readings around a short memory-touching
     loop) until `Csprng::is_ready()`; give up with a panic only after 1,000,000 jitter samples.
     Log `[random] seeded: rdseed <n>, rdrand <n>, jitter <n> samples`.
   - The timer interrupt adds one jitter sample every 16 ticks with a `try_lock` (never block or
     allocate in IRQ context), so later reseeds draw on fresh entropy.
   - `pub fn fill(buf: &mut [u8])`: generate; when the Csprng declines (reseed due and pool not
     ready), gather jitter samples synchronously until it succeeds.
   - `pub fn is_seeded() -> bool`, `pub fn stats() -> RandomStats { rdseed: bool, rdrand: bool,
     rdseed_samples: u64, rdrand_samples: u64, jitter_samples: u64, healthy_sources: usize,
     reseeds: u64 }` (reseeds = the Csprng's reseed_count).
3. Syscall 17 `getrandom(buf_ptr, len, flags) -> bytes written`: flags must be 0 (else EINVAL),
   len ≤ 4096 per call (else EINVAL), user buffer checked and copied with the existing usermem
   helpers (EFAULT on a bad pointer). Document it in SYSCALLS.md; add `libotter::getrandom(&mut
   [u8]) -> Result<usize, Errno>`; add a check to /bin/utest: two 32-byte results differ and a null
   buffer gives EFAULT (the gate's userspace check count rises by one).
4. `scripts/qemu.py --cpu <model>` (default qemu64); a gate target `test-max` = the in-kernel
   test suite booted with `-cpu max` (TCG implements RDRAND and RDSEED there), added to `gmake check`.
5. Add `mod random;` to kernel/src/test_cases/mod.rs.

## Acceptance
- `source scripts/env.sh && gmake check` → every target PASS including `test-max`; serial of both
  runs shows the `[random] seeded:` line and the test's `[random] rdseed ...` line (jitter only under
  qemu64, RDSEED/RDRAND under max).
- `gmake lint` clean.

## Report
<=15 lines: files, the [check] table, both [random] lines, deviations. Do not commit.
