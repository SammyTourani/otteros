#![no_std]
extern crate alloc;

use alloc::vec::Vec;

/// Sampling parameters.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Temperature for scaling logits. 0.0 means greedy (argmax).
    pub temperature: f32,
    /// Top-k filtering; 0 means off.
    pub top_k: usize,
    /// Top-p (nucleus) filtering; 1.0 means off.
    pub top_p: f32,
    /// Repetition penalty; 1.0 means off. Divides positive logits, multiplies negative.
    pub repetition_penalty: f32,
}

/// Seeded token sampler using xoshiro256**.
pub struct Sampler {
    prng: Xoshiro256,
}

impl Sampler {
    /// Create a new sampler with the given seed.
    pub fn new(seed: u64) -> Self {
        Sampler {
            prng: Xoshiro256::new(seed),
        }
    }

    /// Sample a token from the given logits.
    ///
    /// Returns `None` if no token is allowed or all logits are non-finite.
    pub fn sample(
        &mut self,
        logits: &[f32],
        allowed: Option<&dyn Fn(u32) -> bool>,
        recent: &[u32],
        params: &Params,
    ) -> Option<u32> {
        // Convert logits to f64 for accuracy; treat NaN as -infinity.
        let mut l: Vec<f64> = logits
            .iter()
            .map(|&x| if x.is_nan() { f64::NEG_INFINITY } else { x as f64 })
            .collect();

        let n = l.len();
        if n == 0 {
            return None;
        }

        // Apply repetition penalty to recent tokens.
        if params.repetition_penalty != 1.0 {
            let penalty = params.repetition_penalty as f64;
            for &token_id in recent {
                let idx = token_id as usize;
                if idx < n && l[idx].is_finite() {
                    if l[idx] > 0.0 {
                        l[idx] /= penalty;
                    } else {
                        l[idx] *= penalty;
                    }
                }
            }
        }

        // Apply allowed mask (set disallowed tokens to -infinity).
        if let Some(allowed_fn) = allowed {
            for (i, val) in l.iter_mut().enumerate() {
                if !allowed_fn(i as u32) {
                    *val = f64::NEG_INFINITY;
                }
            }
        }

        // Check if any token is allowed.
        if l.iter().all(|x| !x.is_finite()) {
            return None;
        }

        // Greedy case: return the argmax with lowest id tie-breaking.
        if params.temperature == 0.0 && params.top_k == 0 && params.top_p == 1.0 {
            let mut best_idx = None;
            let mut best_val = f64::NEG_INFINITY;
            for (i, &x) in l.iter().enumerate() {
                if x.is_finite() && x > best_val {
                    best_val = x;
                    best_idx = Some(i);
                }
            }
            return best_idx.map(|i| i as u32);
        }

        // Apply temperature scaling.
        let temp = params.temperature as f64;
        if temp > 0.0 {
            for x in &mut l {
                if x.is_finite() {
                    *x /= temp;
                }
            }
        }

        // Find finite logits and their indices.
        let mut indices: Vec<usize> = (0..n)
            .filter(|&i| l[i].is_finite())
            .collect();

        if indices.is_empty() {
            return None;
        }

        // Apply top-k filtering.
        if params.top_k > 0 && indices.len() > params.top_k {
            // Sort by logit (descending), then by index (ascending) for tie-breaking.
            indices.sort_by(|&a, &b| {
                l[b].partial_cmp(&l[a]).unwrap().then(a.cmp(&b))
            });
            indices.truncate(params.top_k);
        }

        // Apply top-p (nucleus) filtering.
        let top_p = params.top_p as f64;
        if top_p < 1.0 {
            // Sort by logit (descending), then by index (ascending) for tie-breaking.
            indices.sort_by(|&a, &b| {
                l[b].partial_cmp(&l[a]).unwrap().then(a.cmp(&b))
            });

            // Compute softmax and cumulative probabilities.
            let max_l = indices.iter().map(|&i| l[i]).fold(f64::NEG_INFINITY, f64::max);
            let sum_exp: f64 = indices.iter().map(|&i| exp(l[i] - max_l)).sum();

            let mut cum_prob = 0.0;
            let mut keep_count = 0;
            for (rank, &idx) in indices.iter().enumerate() {
                let prob = exp(l[idx] - max_l) / sum_exp;
                cum_prob += prob;
                keep_count = rank + 1;
                if cum_prob >= top_p {
                    break;
                }
            }
            // Always keep the best token (rank 0).
            keep_count = keep_count.max(1);
            indices.truncate(keep_count);
        } else {
            // No top-p filtering, but ensure they're sorted for consistency.
            indices.sort_by(|&a, &b| {
                l[b].partial_cmp(&l[a]).unwrap().then(a.cmp(&b))
            });
        }

        // Compute softmax over the remaining tokens.
        let max_l = indices.iter().map(|&i| l[i]).fold(f64::NEG_INFINITY, f64::max);
        let sum_exp: f64 = indices.iter().map(|&i| exp(l[i] - max_l)).sum();

        if sum_exp <= 0.0 {
            return None;
        }

        // Build cumulative probabilities.
        let mut cum_probs: Vec<f64> = Vec::with_capacity(indices.len());
        let mut cum = 0.0;
        for &idx in &indices {
            cum += exp(l[idx] - max_l) / sum_exp;
            cum_probs.push(cum);
        }

        // Draw a random value in [0, 1).
        let r = self.prng.next_f64();

        // Find the token corresponding to this probability.
        for (rank, &cum_prob) in cum_probs.iter().enumerate() {
            if r < cum_prob {
                return Some(indices[rank] as u32);
            }
        }

        // Fallback to the last token (should not happen due to normalization).
        indices.last().map(|&i| i as u32)
    }
}

/// xoshiro256** PRNG, seeded via splitmix64.
struct Xoshiro256 {
    s: [u64; 4],
}

impl Xoshiro256 {
    fn new(seed: u64) -> Self {
        let mut x = seed;
        let s0 = splitmix64(&mut x);
        let s1 = splitmix64(&mut x);
        let s2 = splitmix64(&mut x);
        let s3 = splitmix64(&mut x);
        Xoshiro256 {
            s: [s0, s1, s2, s3],
        }
    }

    fn next(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    fn next_f64(&mut self) -> f64 {
        // Map [0, 2^53) to [0, 1).
        ((self.next() >> 11) as f64) * (1.0 / 9007199254740992.0)
    }
}

/// Splitmix64 initialization function.
fn splitmix64(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// Approximate exponential function using Taylor series.
fn exp(x: f64) -> f64 {
    // Range reduction: x = k*ln2 + r with |r| <= ln2/2, so the series below converges fast and without
    // cancellation for any x (a plain Taylor series of e^-30 cancels catastrophically).
    // fdlibm's split of ln 2: the high part has trailing zero bits so k*LN2_HI is exact.
    const LN2_HI: f64 = f64::from_bits(0x3fe6_2e42_fee0_0000);
    const LN2_LO: f64 = f64::from_bits(0x3dea_39ef_3579_3c76);
    if x.is_nan() {
        return f64::NAN;
    }
    if x > 709.0 {
        return f64::INFINITY;
    }
    if x < -745.0 {
        return 0.0;
    }
    let k = (x * core::f64::consts::LOG2_E + if x >= 0.0 { 0.5 } else { -0.5 }) as i64;
    let kf = k as f64;
    let r = (x - kf * LN2_HI) - kf * LN2_LO;
    // |r| <= 0.347: terms through r^13/13! leave a truncation error below 1e-17.
    let (mut term, mut sum) = (1.0, 1.0);
    for n in 1..14 {
        term *= r / n as f64;
        sum += term;
    }
    // Multiply by 2^k in two halves so each factor is a normal f64 (k ranges over -1075..=1023).
    let pow2 = |e: i64| f64::from_bits(((e + 1023) as u64) << 52);
    sum * pow2(k / 2) * pow2(k - k / 2)
}
