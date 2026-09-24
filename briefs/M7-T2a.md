# Brief M7-T2a — `otter-llm` part 1: .otm loader, byte-level BPE tokenizer, no_std math

## Goal
The first half of the in-OS language-model runtime, as a pure crate proven on the Mac: load `.otm`
files safely, tokenize and detokenize exactly like Hugging Face for SmolLM2, and provide the float
math the forward pass needs without `std`.

## Applies
D2, D21, D27, D28 (model files in $OTTEROS_MODEL_DIR on the SSD).

## Scope and boundaries (another agent works in kernel/ and user/ concurrently)
New crate `crates/otter-llm/` (add to the crates workspace; zero external dependencies). Read-only
use of tools/otter-convert (FORMAT.md, its tests' reference BPE, the golden fixtures under
tools/otter-convert/tests/fixtures/). Do not touch kernel/, user/, tools/ sources or GNUmakefile.
Do not run gmake or QEMU.

## Design
1. `Model::load(bytes: &[u8]) -> Result<Model<'_>, LoadError>` following tools/otter-convert/FORMAT.md
   exactly: header, config, tokenizer section, chat template kind, tensor table. Tensor views borrow
   the input slice (no copies). Every offset, length, count and shape is validated with checked
   arithmetic against the slice; any inconsistency is an error value, never a panic.
2. Tokenizer: GPT-2 byte-to-unicode mapping, merges applied by rank, the pre-tokenizer SmolLM2 uses
   (port the reference BPE from tools/otter-convert's tests, which already reproduces the 20 golden
   prompts, and keep its behaviour identical), special tokens such as `<|im_start|>` and
   `<|im_end|>` matched before BPE, `encode(&str) -> Vec<u32>`, `decode(&[u32]) -> Vec<u8>`, and a
   streaming decoder that holds back incomplete UTF-8 sequences. ChatML builder for
   system/user/assistant turns producing exactly the template strings the converter recorded.
3. `mathf` (no_std): `expf`, `logf`, `sinf`, `cosf`, `sqrtf`, `powf`, `tanhf` if needed, `silu`, and
   a numerically stable softmax over a slice; accuracy targets documented (e.g. expf within 2 ulp
   over [-87, 88]).

## Tests (host `cargo test`)
- Loader: tiny-random and SmolLM2 f32/Q8 files load with the expected config (layers, heads, dims,
  vocab 49152, 272 tensors for SmolLM2) and every tensor view has the right byte length; truncating
  the tiny file at 300 evenly spaced offsets and flipping 300 random bytes never panics.
- Tokenizer: `encode` reproduces the golden token ids for all 20 prompts of both golden fixtures;
  `decode(encode(s)) == s` for those prompts; the streaming decoder emits the same bytes as a
  one-shot decode when fed one token at a time, including across multi-byte characters and emoji.
- mathf: compare against the host's std for 1,000,000 sampled inputs per function; report the
  maximum ulp error; softmax sums to 1 within 1e-6 and handles large inputs without overflow.
- Tests that need the SmolLM2 files skip with a printed note only when $OTTEROS_MODEL_DIR is absent;
  on this machine they must run.

## Acceptance (run all; report exit codes)
- `cd crates && cargo test -p otter-llm` -> 0; report how many prompts matched (must be 40/40) and the mathf ulp errors
- `cd crates && cargo clippy -p otter-llm --tests -- -D warnings` -> 0; `cargo build -p otter-llm --target x86_64-unknown-none` -> 0
- zero dependencies

## Report
<=12 lines: files, acceptance results with exit codes, prompt matches, ulp errors, deviations. Do not commit.
