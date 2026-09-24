# Brief M7-T2b — `otter-llm` part 2: the forward pass and generation, matched against transformers

## Goal
Run the Llama-family forward pass from scratch (scalar code) and prove it produces the same logits
and the same greedy text as Hugging Face `transformers` for the tiny test model and SmolLM2-135M.

## Applies
D2, D21, D27, D28.

## Scope
Only `crates/otter-llm/` (it already has the loader, tokenizer, ChatML builder and `mathf`). Use
`mathf` for exp/sqrt/sin/cos/pow/silu (the crate is no_std). Do not run gmake or QEMU.

## Exact algorithm (HF LlamaForCausalLM semantics; single token at a time with a KV cache)
Notation: D = hidden size, H = heads, G = kv heads, hd = head_dim = D / H, F = intermediate size.
- Weights are row-major [out_features, in_features]; y = W·x means y[o] = Σ_i W[o][i]·x[i].
  Q8_0 blocks run over the flattened tensor in 32-element groups, so a block may straddle two rows
  when in_features is not a multiple of 32 (the tiny model's down_proj has in_features 176): index
  weights by global element position (block = idx / 32, lane = idx % 32).
- x = embed_tokens[token] (dequantized row).
- Per layer: h = rmsnorm(x, input_layernorm) where rmsnorm(v, w)[i] = v[i] / sqrt(mean(v²) + eps)
  · w[i] (accumulate the mean in f64 or f32 carefully; eps from the config).
  q = W_q·h (+ q bias if the config says so), k = W_k·h, v = W_v·h. If qk-norm is set (Qwen3), apply
  rmsnorm per head to q and k with q_norm / k_norm weights.
  RoPE, HF "rotate_half" convention, per head, for i in 0..hd/2: inv_freq_i = theta^(−2i/hd);
  angle = pos·inv_freq_i (compute in f64); (a, b) = (x[i], x[i + hd/2]);
  x[i] = a·cos − b·sin; x[i + hd/2] = a·sin + b·cos. Apply to every q head and k head.
  Append k and v to this layer's cache at `pos`.
  For q head j: kv head g = j / (H / G); score_t = (q_j · k_{t,g}) / sqrt(hd) for t = 0..=pos;
  p = softmax(scores) (subtract the max); out_j = Σ_t p_t·v_{t,g}.
  x = x + W_o·concat(out_0 … out_{H−1}).
  h2 = rmsnorm(x, post_attention_layernorm); m = silu(W_gate·h2) ⊙ (W_up·h2); x = x + W_down·m.
- Final: x = rmsnorm(x, norm); logits = W_lm·x, with W_lm = embed_tokens when embeddings are tied.
- Kernels behind a small trait (`matvec_f32`, `matvec_q8`) so SIMD versions can plug in later.

## Generation
`Session::new(&model, max_ctx)` owning the KV cache; `feed(token) -> logits`; prefill by feeding the
prompt tokens in order; sampling: greedy, temperature, top-p (sorted, cumulative), seeded xorshift
RNG; stop on EOS or `<|im_end|>`; a streaming callback with decoded text via the streaming decoder.

## Tests (host)
Use the goldens in tools/otter-convert/tests/fixtures/{tiny-random,smollm2}/golden.json (read their
exact schema first) and the .otm files in $OTTEROS_MODEL_DIR:
- tiny-random f32: at every position of the 3 detailed prompts, the top-10 ids match and each
  logit is within 1e-4 absolute; the 32-token greedy continuation is identical.
- SmolLM2 f32: same with 1e-3 tolerance; greedy continuation identical.
- SmolLM2 q8: greedy continuations identical for at least the first 16 tokens of each prompt
  (print where they diverge, if at all); report the top-1 agreement rate over all positions.
- Performance printed: tokens/s for SmolLM2 f32 and q8 on the host (release build).
If a tolerance fails, debug by comparing one layer at a time (the reference script can be extended
to dump intermediate activations for the tiny model); never loosen a tolerance.

## Acceptance
- `cd crates && cargo test --release -p otter-llm` -> 0 with the numbers above reported
- `cargo clippy -p otter-llm --tests -- -D warnings` -> 0; `cargo build -p otter-llm --target x86_64-unknown-none` -> 0

## Report
<=10 lines with the observed numbers. Do not commit.
