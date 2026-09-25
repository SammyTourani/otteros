//! Acceptance oracle for brief M7-T4 (token sampling for Otter), written by the orchestrator. The
//! crate must pass this file unchanged. References are computed here independently in f64.
//!
//! API this file relies on:
//!   struct Params { temperature: f32, top_k: usize, top_p: f32, repetition_penalty: f32 }
//!     (temperature 0 = greedy; top_k 0 = off; top_p 1.0 = off; repetition_penalty 1.0 = off)
//!   struct Sampler::new(seed: u64); Sampler::sample(&mut self, logits: &[f32], allowed: Option<&dyn Fn(u32) -> bool>,
//!                                                    recent: &[u32], params: &Params) -> Option<u32>
//! Semantics: tokens rejected by `allowed` are never chosen (None when nothing is allowed); the
//! repetition penalty divides positive logits and multiplies negative logits of tokens in `recent`
//! (the llama.cpp / HF convention); then temperature scaling, top-k, then top-p (smallest set whose
//! probability reaches p, always keeping the most likely token), then a draw from the renormalised
//! distribution with the seeded generator. NaN logits are treated as -infinity; greedy ties go to
//! the lowest token id.

use otter_sample::{Params, Sampler};

fn params(temperature: f32, top_k: usize, top_p: f32) -> Params {
    Params { temperature, top_k, top_p, repetition_penalty: 1.0 }
}

/// Independent reference: the exact distribution the sampler must draw from.
fn reference(logits: &[f32], allowed: &dyn Fn(u32) -> bool, recent: &[u32], p: &Params) -> Vec<f64> {
    let mut l: Vec<f64> = logits
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let mut x = if x.is_nan() { f64::NEG_INFINITY } else { x as f64 };
            if recent.contains(&(i as u32)) && p.repetition_penalty != 1.0 {
                x = if x > 0.0 { x / p.repetition_penalty as f64 } else { x * p.repetition_penalty as f64 };
            }
            if allowed(i as u32) { x / p.temperature as f64 } else { f64::NEG_INFINITY }
        })
        .collect();
    let mut order: Vec<usize> = (0..l.len()).filter(|&i| l[i].is_finite()).collect();
    order.sort_by(|&a, &b| l[b].partial_cmp(&l[a]).unwrap().then(a.cmp(&b)));
    if p.top_k > 0 && order.len() > p.top_k {
        order.truncate(p.top_k);
    }
    let m = order.iter().map(|&i| l[i]).fold(f64::NEG_INFINITY, f64::max);
    let z: f64 = order.iter().map(|&i| (l[i] - m).exp()).sum();
    let mut keep = Vec::new();
    let mut cum = 0.0;
    for &i in &order {
        keep.push(i);
        cum += (l[i] - m).exp() / z;
        if cum >= p.top_p as f64 {
            break;
        }
    }
    let z2: f64 = keep.iter().map(|&i| (l[i] - m).exp()).sum();
    let mut probs = vec![0.0; l.len()];
    for &i in &keep {
        probs[i] = (l[i] - m).exp() / z2;
    }
    l.clear();
    probs
}

/// Pearson chi-square; cells expected below 5 are pooled into one cell (standard practice), and a
/// token of probability 0 must never have been drawn.
fn chi_square(counts: &[u64], probs: &[f64], n: u64) -> (f64, usize) {
    let (mut chi, mut cells) = (0.0, 0usize);
    let (mut pooled_count, mut pooled_expected) = (0.0f64, 0.0f64);
    for (c, p) in counts.iter().zip(probs) {
        if *p == 0.0 {
            assert_eq!(*c, 0, "a token with probability 0 was drawn");
            continue;
        }
        let e = p * n as f64;
        if e < 5.0 {
            pooled_count += *c as f64;
            pooled_expected += e;
        } else {
            chi += (*c as f64 - e).powi(2) / e;
            cells += 1;
        }
    }
    if pooled_expected > 0.0 {
        chi += (pooled_count - pooled_expected).powi(2) / pooled_expected;
        cells += 1;
    }
    (chi, cells.saturating_sub(1))
}

fn logits(seed: u64, n: usize) -> Vec<f32> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ((x >> 11) as f64 / (1u64 << 53) as f64 * 8.0 - 4.0) as f32
        })
        .collect()
}

#[test]
fn greedy_is_argmax_with_lowest_id_ties() {
    let mut s = Sampler::new(1);
    let l = [0.5, 2.0, -1.0, 2.0, f32::NAN, 1.9];
    assert_eq!(s.sample(&l, None, &[], &params(0.0, 0, 1.0)), Some(1));
    let only_odd = |t: u32| t % 2 == 1;
    assert_eq!(s.sample(&l, Some(&only_odd), &[], &params(0.0, 0, 1.0)), Some(1));
    let only_high = |t: u32| t >= 4;
    assert_eq!(s.sample(&l, Some(&only_high), &[], &params(0.0, 0, 1.0)), Some(5), "NaN is -infinity");
    let nothing = |_: u32| false;
    assert_eq!(s.sample(&l, Some(&nothing), &[], &params(0.7, 0, 0.9)), None);
    assert_eq!(s.sample(&[f32::NAN, f32::NAN], None, &[], &params(1.0, 0, 1.0)), None, "no finite logit");
}

#[test]
fn draws_follow_the_exact_distribution() {
    let cases: [(u64, usize, Params); 5] = [
        (11, 20, params(1.0, 0, 1.0)),
        (12, 50, params(0.7, 0, 1.0)),
        (13, 50, params(1.0, 10, 1.0)),
        (14, 50, params(0.8, 0, 0.9)),
        (15, 100, Params { temperature: 1.2, top_k: 40, top_p: 0.95, repetition_penalty: 1.3 }),
    ];
    for (seed, n, p) in cases {
        let l = logits(seed, n);
        let recent = [0u32, 3, 7, 11];
        let allowed = |t: u32| t % 5 != 2;
        let probs = reference(&l, &allowed, &recent, &p);
        let mut s = Sampler::new(seed * 1000);
        let draws = 200_000u64;
        let mut counts = vec![0u64; n];
        for _ in 0..draws {
            let t = s.sample(&l, Some(&allowed), &recent, &p).expect("a token") as usize;
            counts[t] += 1;
        }
        let (chi, dof) = chi_square(&counts, &probs, draws);
        // Wilson-Hilferty approximation of the chi-square 0.999 quantile.
        let k = dof.max(1) as f64;
        let crit = k * (1.0 - 2.0 / (9.0 * k) + 3.09 * (2.0 / (9.0 * k)).sqrt()).powi(3);
        assert!(chi < crit, "seed {seed}: chi-square {chi:.1} with {dof} degrees of freedom exceeds {crit:.1}");
    }
}

#[test]
fn seeded_and_deterministic() {
    let l = logits(21, 64);
    let p = params(0.9, 0, 0.95);
    let run = |seed: u64| {
        let mut s = Sampler::new(seed);
        (0..500).map(|_| s.sample(&l, None, &[], &p).unwrap()).collect::<Vec<u32>>()
    };
    assert_eq!(run(7), run(7), "same seed, same tokens");
    assert_ne!(run(7), run(8), "different seeds, different tokens");
}

#[test]
fn masked_tokens_are_never_drawn_even_when_they_dominate() {
    let mut l = vec![-5.0f32; 1000];
    l[500] = 30.0; // overwhelmingly likely, but disallowed
    let allowed = |t: u32| t != 500;
    let mut s = Sampler::new(3);
    for _ in 0..20_000 {
        let t = s.sample(&l, Some(&allowed), &[], &params(1.5, 0, 1.0)).unwrap();
        assert_ne!(t, 500);
    }
}

#[test]
fn top_p_always_keeps_the_best_token() {
    let l = [10.0f32, 0.0, 0.0];
    let mut s = Sampler::new(5);
    for _ in 0..1000 {
        assert_eq!(s.sample(&l, None, &[], &params(1.0, 0, 0.0001)), Some(0));
    }
}
