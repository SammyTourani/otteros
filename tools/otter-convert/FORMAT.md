# OtterOS `.otm` Model Format

Binary format for Llama-family LLMs on OtterOS. All multi-byte integers are little-endian.

## File Structure

```
[Header (128 bytes)]
[Tokenizer Section (variable)]
[Tensor Data (64-byte aligned blocks)]
[Tensor Table (variable, follows all tensor data)]
```

## Header (128 bytes, fixed)

```
u32  magic = 0x004D_544F          // "OTM\0"
u32  version = 1

// Model config
u32  vocab_size
u32  dim
u32  n_layers
u32  n_heads
u32  n_kv_heads
u32  head_dim
u32  hidden_dim
f32  rope_theta
f32  norm_eps
u32  max_context

// Flags
u32  tied_embeddings              // 0 or 1
u32  qkv_bias                     // 0 or 1
u32  qk_norm                      // 0 or 1

// Tokenizer metadata
u32  bos_id                       // u32::MAX if absent
u32  eos_id                       // u32::MAX if absent
u32  isolate_digits               // 0=no, 1=yes (pre-tokenizer)

// Chat template
u32  chat_template_kind           // 0=ChatML, 1+=reserved
```

Total header size: 8 + 8 + 36 + 12 + 8 + 8 = 80 bytes (leaves 48 bytes reserved for future use, padded to 128)

## Tokenizer Section

Begins immediately after header (offset 128).

```
u32  vocab_len                    // number of vocab entries (should match vocab_size)

// For each vocab entry in order (id 0 to vocab_size-1):
  u16 byte_string_len            // length of the raw byte string
  u8[byte_string_len]            // raw bytes (after reversing GPT-2 byte-to-unicode mapping)

u32  merges_len                   // number of merge pairs

// For each merge (in priority order, earliest first):
  u32  left_id
  u32  right_id

u32  added_tokens_len             // number of special/added tokens

// For each added token:
  u32  id
  u16  content_len
  u8[content_len]                // literal UTF-8 string
```

## Tensor Table

Begins after tokenizer section. Describes all weight tensors.

```
u32  num_tensors

// For each tensor:
  u32  name_len
  u8[name_len]                   // ASCII tensor name
  u32  dtype                     // 0=F32, 1=Q8_0
  u32  num_dims
  u32[num_dims]                  // shape dimensions
  u64  offset                    // byte offset in data section (64-byte aligned)
  u64  num_elements              // total count (product of shape)
```

## Data Section

All tensors follow, each 64-byte aligned. Padding bytes between tensors are undefined.

### F32 Format
Raw f32 values in little-endian (one per 4 bytes), no compression.

### Q8_0 Format
Blocks of 32 int8 weights followed by one f32 scale.
- 32 int8 values (one byte each) = 32 bytes
- 1 f32 scale = 4 bytes
- Total per block: 36 bytes

**Layout rule**: tensor flattened to 1D in row-major order; blocks are formed sequentially from the flattened array (no per-row padding, no transpose). For a tensor of shape [A, B], elements are visited as: (0,0), (0,1), ..., (0,B-1), (1,0), (1,1), ..., etc., and every 32 elements form one block with a separate scale.

Dequantization: `dequantized[i] = quantized[i] * scale`

Block boundary: each new block starts at `offset + (block_index * 36)`.
Error bound per block: `max |weight - dequantized_weight| <= scale / 2`

## Example: tiny-random model

A 3.5 MB deterministic test model (seed-based):
- dim=64, n_layers=2, n_heads=4, n_kv_heads=2, head_dim=16, hidden_dim=176
- vocab_size=512
- Weights: Q8_0 quantized
- Used to validate the reference tokenizer (SmolLM2's) and the converter itself

## Notes

- No compression, no checksum. Runtime validates integrity by checking shape consistency.
- Chat template kind=0 (ChatML) uses hard-coded template: `<|im_start|>{role}\n{content}<|im_end|>\n`
- Tokenizer is the spec: runtime must implement this exact BPE logic (the reference implementation in otter-convert's tests).
