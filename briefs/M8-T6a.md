# Brief M8-T6a — CSPRNG core: SP 800-90A HMAC_DRBG and a health-tested entropy pool (pure)

## Goal
The kernel's `getrandom` (TLS keys, sequence numbers, ASLR later) needs a random generator whose
correctness can be proven. It is built here as a pure crate and checked against NIST's official
known-answer vectors. The kernel task (M8-T6b) adds RDSEED/RDRAND, TSC-jitter sampling and the
syscall.

## Applies
D2, D27, D29 (HMAC_DRBG instead of the ChaCha20 DRBG named in the first PLAN draft). Lives in
crates/otter-crypto as a `drbg` module and an `entropy` module (forbid(unsafe_code) stays).

## Design
1. `HmacDrbg` (SHA-256), NIST SP 800-90A Rev. 1 §10.1.2: `instantiate(entropy, nonce,
   personalization)`, `reseed(entropy, additional)`, `generate(out, additional) ->
   Result<(), ReseedRequired>`, reseed counter with a reseed interval of 2^20 requests and at most
   2^16 bytes per request. Zeroise the state on drop.
2. `EntropyPool`: accumulates samples from named sources, each with a conservative credit:
   RDSEED 0.5 bit per bit, RDRAND 0.25 bit per bit, TSC-jitter samples 1 bit each, and only after
   that source passes its health tests. Condensing uses SHA-256 over everything absorbed. The pool
   refuses to produce seed material before 256 credited bits.
3. NIST SP 800-90B §4.4 continuous health tests on each raw noise source: the Repetition Count Test
   (cutoff 1 + ⌈20/H⌉) and the Adaptive Proportion Test (window 512, cutoff from the standard's
   formula for α = 2^-20). A failure marks the source unhealthy, and it is no longer credited.
4. `Csprng` = pool + HmacDrbg: seeds once the pool is ready, reseeds from the pool every 2^16
   generate calls or 1 MiB of output, and supports on-demand reseeding.

## Tests (host; `cargo test -p otter-crypto`)
- NIST CAVP HMAC_DRBG vectors for SHA-256 (drbgtestvectors.zip, the "no prediction resistance"
  file), covering every combination of personalization string and additional input present or
  absent. Download to the external SSD (/Volumes/SammyDisk/Downloads), and commit a subset of the
  SHA-256 sections as a test fixture under crates/otter-crypto/tests/vectors/ with a README line
  naming the source. All cases in that subset must pass.
- Health tests: synthetic sources that are stuck (a constant), biased (90 % one value) and good
  (a seeded PRNG). The stuck and biased sources are flagged within the expected sample counts; the
  good source is not flagged in 10^6 samples.
- Pool: no output before 256 credited bits; unhealthy sources are not credited; the same inputs
  give the same seed (deterministic), and different inputs give different seeds.
- Csprng: the reseed happens at the thresholds; after 10 MiB of output, the byte frequency
  chi-square has p > 0.001, and 1,000 successive 32-byte outputs are all distinct.

## Acceptance
`scripts/verify-crate.sh otter-crypto` passes: 0 failed, clippy ok, no_std ok, external deps 0.
The existing test count rises by ≥ 15. The CAVP subset has ≥ 60 cases, all passing.

## Report
<=10 lines: the verify-crate line, CAVP case count, health-test detection sample counts,
deviations. Do not commit.
