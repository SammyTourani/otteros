# Brief M7-T3a — `otter-simd`: SIMD kernels for the language model (pure crate)

## Goal
The inner loops of Otter's inference: f32 and Q8_0 matrix-vector products and a few vector helpers,
in scalar, SSE2 and AVX2+FMA versions chosen at runtime by CPUID, proven equal to the scalar
reference and measured for speed. This is what lets a cheap laptop run the model at interactive
speed.

## Applies
D2, D21 (Q8_0: blocks of 32 int8 weights with one f32 scale, row-major, blocks may straddle rows),
D27. Unsafe intrinsics are allowed only inside the per-ISA modules, each block with a `// SAFETY:`
comment, and only reachable after the CPUID check.

## Design
- API (all safe): `dot_f32(a, b) -> f32`; `matvec_f32(w: &[f32], rows, cols, x, out)`;
  `matvec_q8(w_blocks: &[u8], rows, cols, x, out)` where element (r, c) lives at global index
  r*cols + c (block = idx/32, lane = idx%32, each block = 32 int8 + 4-byte f32 scale, 36 bytes);
  `rmsnorm`, `axpy`, `softmax_inplace` helpers; a `Kernels` struct holding function pointers chosen
  once by `Kernels::detect()` (CPUID leaf 1 for SSE2/FMA, leaf 7 for AVX2, XGETBV for OS YMM
  support); `Kernels::scalar()` for tests.
- x86_64 paths: SSE2 (4-wide), AVX2+FMA (8-wide, int8 → i16 → i32 widening or
  `_mm256_cvtepi8_epi32` + float FMA for Q8). Handle tails and straddling blocks correctly.
- On non-x86 hosts only the scalar path exists (the crate must still build and test on aarch64).
- Threading hook: `matvec_*_rows(row_range, …)` variants so the OS can split rows across cores.

## Tests (host)
- aarch64 native: scalar path unit tests (hand-computed small cases, row-straddling Q8 blocks with
  cols = 176 and 64).
- x86_64 under Rosetta (`cargo test -p otter-simd --target x86_64-apple-darwin`): every ISA path
  vs scalar on 1,000 random shapes (rows 1..300, cols 1..600 including non-multiples of 8/32),
  relative error ≤ 1e-5 for f32 and ≤ 1e-4 for Q8 (summation order differs); report which ISAs
  were detected under Rosetta.
- Benchmarks printed (release, Rosetta): GFLOP/s for matvec_f32 and matvec_q8 at 576x1536 and
  1536x576 (SmolLM2 shapes) per ISA.

## Acceptance
`scripts/verify-crate.sh otter-simd` all clear (≥ 25 tests) AND
`cd crates && cargo test -p otter-simd --target x86_64-apple-darwin` -> 0 with the ISA report.
