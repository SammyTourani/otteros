//! Acceptance oracle for brief M7-T6 (otter-local: Otter's chat engine), written by the
//! orchestrator. The crate must pass this file unchanged. Reference: Hugging Face transformers'
//! greedy continuation of the ChatML prompt p11_chatml_math for SmolLM2-135M-Instruct (f32),
//! tools/otter-convert/tests/fixtures/smollm2/golden.json. The model file comes from
//! $OTTEROS_MODEL_DIR (`scripts/env.sh`), like otter-llm's own golden tests.
//!
//! API this file relies on (crate `otter_local`):
//!   ChatConfig { temperature: f32, top_k: usize, top_p: f32, repetition_penalty: f32, seed: u64,
//!                max_new_tokens: usize, stop_ids: Vec<u32> }   (Default: greedy, 256 tokens, stop at <|im_end|>)
//!   trait Clock { fn now_ms(&self) -> u64; }
//!   ChatEngine::new(&otter_llm::Model, ChatConfig, Box<dyn Clock>) -> Result<ChatEngine, ChatError>
//!   ChatEngine::start(&mut self, messages: &[(Role, &str)]) -> Result<(), ChatError>
//!     (Role = otter_llm::chatml::Role; builds the ChatML prompt ending in "<|im_start|>assistant\n"
//!      and feeds it)
//!   ChatEngine::step(&mut self, budget: usize) -> Step   (generates at most `budget` new tokens)
//!   Step::{Text(String), Done(DoneReason)}  DoneReason::{EndOfTurn, MaxTokens, Cancelled}
//!   ChatEngine::cancel(&mut self); prompt_ids(&self) -> &[u32]; generated_ids(&self) -> &[u32];
//!   stats(&self) -> Stats { prompt_tokens: usize, generated_tokens: usize, prefill_ms: u64,
//!                           generate_ms: u64 }
//!
//! Semantics: the first new token comes from the logits of the last prompt position and every
//! generated token is fed exactly once. `Text` carries the newly decoded UTF-8 of this step (the
//! streaming decoder holds back incomplete characters, so a step's text may be empty); once a step
//! returns `Done`, every later step returns the same `Done` and generates nothing. A token in
//! `stop_ids` ends the turn (EndOfTurn) and is neither emitted as text nor counted as generated;
//! `max_new_tokens` generated tokens end it with MaxTokens; `cancel` makes the next step return
//! Done(Cancelled). The default stop id is the tokenizer's <|im_end|>.

use otter_llm::chatml::Role;
use otter_llm::Model;
use otter_local::{ChatConfig, ChatEngine, Clock, DoneReason, Step};
use std::cell::Cell;
use std::rc::Rc;

/// The 539 MB model, read once per test binary and shared by the tests running in parallel.
fn model_bytes() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        let dir = std::env::var("OTTEROS_MODEL_DIR").expect("source scripts/env.sh first (OTTEROS_MODEL_DIR)");
        std::fs::read(format!("{dir}/SmolLM2-135M-Instruct.f32.otm")).expect("run `gmake models`: SmolLM2 f32 .otm is missing")
    })
}

fn golden() -> (Vec<u32>, Vec<u32>) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/otter-convert/tests/fixtures/smollm2/golden.json");
    let g = otter_json::parse_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let d = g
        .get("detailed")
        .and_then(|v| match v {
            otter_json::Value::Array(a) => a.iter().find(|x| x.get("id").and_then(|i| i.as_str()) == Some("p11_chatml_math")).cloned(),
            _ => None,
        })
        .expect("p11_chatml_math");
    let ids = |key: &str| -> Vec<u32> {
        match d.get(key).unwrap() {
            otter_json::Value::Array(a) => a.iter().map(|n| n.as_number().unwrap().as_f64() as u32).collect(),
            _ => panic!("{key}"),
        }
    };
    (ids("token_ids"), ids("greedy_continuation"))
}

struct FakeClock(Rc<Cell<u64>>);

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        let t = self.0.get();
        self.0.set(t + 5); // every reading advances 5 ms
        t
    }
}

fn greedy(max_new_tokens: usize) -> ChatConfig {
    ChatConfig { temperature: 0.0, max_new_tokens, ..ChatConfig::default() }
}

const MESSAGES: [(Role, &str); 2] = [(Role::System, "You are a helpful assistant."), (Role::User, "What is 84 * 3 / 2?")];

fn run_to_done(engine: &mut ChatEngine, budget: usize) -> (String, DoneReason) {
    let mut text = String::new();
    for _ in 0..10_000 {
        match engine.step(budget) {
            Step::Text(t) => text.push_str(&t),
            Step::Done(reason) => return (text, reason),
        }
    }
    panic!("the engine never finished");
}

#[test]
fn chatml_prompt_and_greedy_reply_match_transformers() {
    let model = Model::load(model_bytes()).unwrap();
    let (prompt, continuation) = golden();
    let mut engine = ChatEngine::new(&model, greedy(32), Box::new(FakeClock(Rc::new(Cell::new(0))))).unwrap();
    engine.start(&MESSAGES).unwrap();
    assert_eq!(engine.prompt_ids(), &prompt[..], "the ChatML prompt tokenises exactly like transformers");
    let (text, reason) = run_to_done(&mut engine, 4);
    assert_eq!(engine.generated_ids(), &continuation[..], "greedy tokens match transformers");
    assert_eq!(reason, DoneReason::MaxTokens);
    let tok = model.tokenizer().unwrap();
    let expected = String::from_utf8(tok.decode(&continuation).unwrap()).unwrap();
    assert_eq!(text, expected, "the streamed text is the decoded reply");
    let s = engine.stats();
    assert_eq!((s.prompt_tokens, s.generated_tokens), (32, 32));
    assert!(s.prefill_ms > 0 && s.generate_ms > 0, "timed with the injected clock");
}

#[test]
fn stop_tokens_end_the_turn_without_being_emitted() {
    let model = Model::load(model_bytes()).unwrap();
    let (_, continuation) = golden();
    let cfg = ChatConfig { stop_ids: vec![continuation[5]], ..greedy(32) };
    let mut engine = ChatEngine::new(&model, cfg, Box::new(FakeClock(Rc::new(Cell::new(0))))).unwrap();
    engine.start(&MESSAGES).unwrap();
    let (text, reason) = run_to_done(&mut engine, 32);
    assert_eq!(reason, DoneReason::EndOfTurn);
    assert_eq!(engine.generated_ids(), &continuation[..5], "the stop token is not part of the reply");
    let tok = model.tokenizer().unwrap();
    assert_eq!(text, String::from_utf8(tok.decode(&continuation[..5]).unwrap()).unwrap());
    assert_eq!(engine.step(4), Step::Done(DoneReason::EndOfTurn), "Done is final");
    assert_eq!(engine.generated_ids().len(), 5);
}

#[test]
fn budgets_limits_and_cancellation() {
    let model = Model::load(model_bytes()).unwrap();
    let mut engine = ChatEngine::new(&model, greedy(7), Box::new(FakeClock(Rc::new(Cell::new(0))))).unwrap();
    engine.start(&MESSAGES).unwrap();
    let _ = engine.step(3);
    assert_eq!(engine.generated_ids().len(), 3, "a step never exceeds its budget");
    let (_, reason) = run_to_done(&mut engine, 100);
    assert_eq!((reason, engine.generated_ids().len()), (DoneReason::MaxTokens, 7));

    let mut engine = ChatEngine::new(&model, greedy(32), Box::new(FakeClock(Rc::new(Cell::new(0))))).unwrap();
    engine.start(&MESSAGES).unwrap();
    let _ = engine.step(2);
    engine.cancel();
    assert_eq!(engine.step(8), Step::Done(DoneReason::Cancelled));
    assert_eq!(engine.generated_ids().len(), 2, "nothing is generated after cancel");
}

#[test]
fn sampling_is_seeded_and_streaming_stays_valid_utf8() {
    let model = Model::load(model_bytes()).unwrap();
    let sampled = |seed: u64| {
        let cfg = ChatConfig { temperature: 0.9, top_p: 0.9, seed, max_new_tokens: 16, ..ChatConfig::default() };
        let mut engine = ChatEngine::new(&model, cfg, Box::new(FakeClock(Rc::new(Cell::new(0))))).unwrap();
        engine.start(&[(Role::User, "Write one sentence about otters. Use an emoji like 🦦 or é.")]).unwrap();
        let mut text = String::new();
        let tok = model.tokenizer().unwrap();
        while let Step::Text(t) = engine.step(1) {
            text.push_str(&t);
            let full = tok.decode(engine.generated_ids()).unwrap();
            assert!(full.starts_with(text.as_bytes()), "streamed text is always a prefix of the full decode");
        }
        engine.generated_ids().to_vec()
    };
    let a = sampled(42);
    assert_eq!(a, sampled(42), "same seed, same reply");
    assert_ne!(a, sampled(43), "another seed, another reply");
}
