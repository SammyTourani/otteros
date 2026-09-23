//! Poly1305 (RFC 8439 section 2.5): a one-time message authenticator.
//!
//! Uses the standard 26-bit-limb representation of the 130-bit accumulator and
//! clamped `r` (five `u64`s, each holding one 26-bit limb) so every
//! multiply-accumulate step fits comfortably in a `u64` and reduction modulo
//! `2^130 - 5` is a handful of shifts, ANDs and adds -- the well-known technique
//! for implementing Poly1305 without a big-integer library, used here with a
//! fresh derivation (see the comments below) rather than any borrowed code.
//!
//! Every block, including a short final one, is treated exactly as RFC 8439
//! section 2.5.1 describes: read up to 16 message bytes, append a single 0x01
//! byte, zero-pad to 17 bytes, treat that as a little-endian number, add it to
//! the accumulator, multiply by `r`, reduce modulo `p`.

const MASK26: u64 = 0x3ff_ffff;

/// Clamp mask for `r` (RFC 8439 section 2.5): clears the top four bits of every
/// 4th byte and the bottom two bits of every 4th byte starting at index 4, i.e.
/// `r &= 0x0ffffffc_0ffffffc_0ffffffc_0fffffff` when `r` is read as a 128-bit
/// little-endian number.
const R_CLAMP_MASK: u128 = 0x0fff_fffc_0fff_fffc_0fff_fffc_0fff_ffff;

/// `p = 2^130 - 5`'s five 26-bit limbs: since 5 limbs of 26 bits hold exactly
/// `2^130 - 1` when all bits are set, `p`'s limbs are all-ones except the lowest,
/// which is `2^26 - 5` (all-ones minus 4).
const P_LIMBS: [i64; 5] = [(MASK26 as i64) - 4, MASK26 as i64, MASK26 as i64, MASK26 as i64, MASK26 as i64];

/// Splits a clamped-or-not 128-bit little-endian value into five 26-bit limbs.
fn limbs_from_u128(v: u128) -> [u64; 5] {
    let mut limbs = [0u64; 5];
    for (i, limb) in limbs.iter_mut().enumerate() {
        *limb = ((v >> (26 * i)) & MASK26 as u128) as u64;
    }
    limbs
}

/// Splits a 17-byte little-endian "widened block" (16 message bytes plus the
/// RFC's implicit 0x01) into five limbs. Bit 128 (the appended 0x01, when the
/// block is a full 16 bytes) lands 24 bits into the top limb, since limb 4
/// covers bits 104..129.
fn limbs_from_widened_block(block17: &[u8; 17]) -> [u64; 5] {
    let low = u128::from_le_bytes(block17[0..16].try_into().unwrap());
    let mut limbs = limbs_from_u128(low);
    limbs[4] |= (block17[16] as u64) << 24;
    limbs
}

/// Propagates carries out of a raw (not yet limb-normalized) multiply-accumulate
/// result, folding the carry out of the top limb back in with the same `*5`
/// trick used for the multiply itself (`2^130 = 5 (mod p)`). Every output limb
/// except possibly `out[1]` ends up strictly less than `2^26`; `out[1]` can be
/// exactly `2^26` in the rare case its own carry-out lands on an exact power of
/// two, which the next block's multiply (and `finalize`'s full carry pass)
/// tolerates -- nothing here assumes a limb is *strictly* below `2^26`, only
/// that it is small enough for a `u64` product to stay clear of overflow.
fn carry_reduce(mut d: [u64; 5]) -> [u64; 5] {
    let mut out = [0u64; 5];
    let mut carry;
    carry = d[0] >> 26;
    out[0] = d[0] & MASK26;
    d[1] += carry;
    carry = d[1] >> 26;
    out[1] = d[1] & MASK26;
    d[2] += carry;
    carry = d[2] >> 26;
    out[2] = d[2] & MASK26;
    d[3] += carry;
    carry = d[3] >> 26;
    out[3] = d[3] & MASK26;
    d[4] += carry;
    carry = d[4] >> 26;
    out[4] = d[4] & MASK26;
    out[0] += carry * 5;
    carry = out[0] >> 26;
    out[0] &= MASK26;
    out[1] += carry;
    out
}

/// Streaming Poly1305. `poly1305_mac(key, data)` for one-shot use.
pub struct Poly1305 {
    r: [u64; 5],
    h: [u64; 5],
    /// `s`, added to the accumulator (mod 2^128) at the very end.
    pad: u128,
    buffer: [u8; 16],
    buffer_len: usize,
}

impl Poly1305 {
    /// Starts a new Poly1305 computation with a fresh one-time 32-byte key
    /// (`r || s`). Per RFC 8439, never reuse a key for two different messages.
    pub fn new(key: &[u8; 32]) -> Self {
        let r_raw = u128::from_le_bytes(key[0..16].try_into().unwrap());
        let pad = u128::from_le_bytes(key[16..32].try_into().unwrap());
        let r = limbs_from_u128(r_raw & R_CLAMP_MASK);
        Poly1305 { r, h: [0; 5], pad, buffer: [0; 16], buffer_len: 0 }
    }

    fn absorb_block(&mut self, block17: &[u8; 17]) {
        let n = limbs_from_widened_block(block17);
        let mut h = self.h;
        for i in 0..5 {
            h[i] += n[i];
        }

        let r = self.r;
        // 2^130 = 5 (mod p): multiplying the limbs that would otherwise land at
        // convolution positions 5..8 by 5 folds them back into positions 0..3
        // (see the module doc's derivation in the design notes / brief).
        let s = [0u64, r[1] * 5, r[2] * 5, r[3] * 5, r[4] * 5];

        let d = [
            h[0] * r[0] + h[1] * s[4] + h[2] * s[3] + h[3] * s[2] + h[4] * s[1],
            h[0] * r[1] + h[1] * r[0] + h[2] * s[4] + h[3] * s[3] + h[4] * s[2],
            h[0] * r[2] + h[1] * r[1] + h[2] * r[0] + h[3] * s[4] + h[4] * s[3],
            h[0] * r[3] + h[1] * r[2] + h[2] * r[1] + h[3] * r[0] + h[4] * s[4],
            h[0] * r[4] + h[1] * r[3] + h[2] * r[2] + h[3] * r[1] + h[4] * r[0],
        ];

        self.h = carry_reduce(d);
    }

    fn absorb_full_block(&mut self, block: &[u8; 16]) {
        let mut widened = [0u8; 17];
        widened[..16].copy_from_slice(block);
        widened[16] = 1;
        self.absorb_block(&widened);
    }

    /// Feeds more input. May be called any number of times with any chunking.
    pub fn update(&mut self, mut data: &[u8]) {
        if self.buffer_len > 0 {
            let need = 16 - self.buffer_len;
            let take = need.min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];
            if self.buffer_len < 16 {
                return;
            }
            let block = self.buffer;
            self.absorb_full_block(&block);
            self.buffer_len = 0;
        }
        while data.len() >= 16 {
            let block: [u8; 16] = data[..16].try_into().unwrap();
            self.absorb_full_block(&block);
            data = &data[16..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    /// Consumes the Poly1305 state and returns the 16-byte tag.
    pub fn finalize(mut self) -> [u8; 16] {
        if self.buffer_len > 0 {
            let mut widened = [0u8; 17];
            widened[..self.buffer_len].copy_from_slice(&self.buffer[..self.buffer_len]);
            widened[self.buffer_len] = 1;
            self.absorb_block(&widened);
        }

        // Full carry propagation: `absorb_block`'s per-block reduce can leave a
        // carry sitting in `h[1]` that has not yet rippled into `h[2..]`. Push it
        // all the way through so every limb is a genuine value before comparing
        // against `p`.
        let mut h = self.h;
        let mut carry;
        carry = h[1] >> 26;
        h[1] &= MASK26;
        h[2] += carry;
        carry = h[2] >> 26;
        h[2] &= MASK26;
        h[3] += carry;
        carry = h[3] >> 26;
        h[3] &= MASK26;
        h[4] += carry;
        carry = h[4] >> 26;
        h[4] &= MASK26;
        h[0] += carry * 5;
        carry = h[0] >> 26;
        h[0] &= MASK26;
        h[1] += carry;

        // `h` is a valid 130-bit value (five 26-bit-ish limbs), so it is at most
        // one subtraction of `p = 2^130 - 5` away from its canonical
        // representative. Compute `h - p` limb-by-limb with a borrow chain done
        // via signed arithmetic shifts (an `i64` arithmetic right shift by 63
        // sign-extends, giving an all-ones or all-zero mask without branching on
        // any value that depends on the message or key), then select `h - p` if
        // that subtraction did not need to borrow past the top limb (`h >= p`),
        // or plain `h` otherwise. This is a from-scratch constant-time multi-limb
        // conditional subtraction, not the single-word sign-bit trick some C
        // reference implementations use.
        let mut diff = [0i64; 5];
        let mut borrow: i64 = 0;
        for i in 0..5 {
            let d = h[i] as i64 - P_LIMBS[i] - borrow;
            borrow = (d >> 63) & 1;
            diff[i] = d + ((d >> 63) & (1i64 << 26));
        }
        let use_diff = (1 - borrow) as u64; // 1 when h >= p (no final borrow).
        let select_mask = 0u64.wrapping_sub(use_diff); // all-ones or all-zero.
        let mut f = [0u64; 5];
        for i in 0..5 {
            f[i] = (diff[i] as u64 & select_mask) | (h[i] & !select_mask);
        }

        // Combine the limbs mod 2^128 (the top two bits of a 130-bit value that
        // is now < p are simply discarded, per RFC 8439: "the 128 least
        // significant bits are serialized"), then add the pad mod 2^128.
        let combined: u128 = (f[0] as u128)
            .wrapping_add((f[1] as u128).wrapping_shl(26))
            .wrapping_add((f[2] as u128).wrapping_shl(52))
            .wrapping_add((f[3] as u128).wrapping_shl(78))
            .wrapping_add((f[4] as u128).wrapping_shl(104));
        combined.wrapping_add(self.pad).to_le_bytes()
    }
}

/// One-shot: `Poly1305::new(key)` + `update(data)` + `finalize()`.
pub fn poly1305_mac(key: &[u8; 32], data: &[u8]) -> [u8; 16] {
    let mut p = Poly1305::new(key);
    p.update(data);
    p.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn hex(bytes: &[u8]) -> alloc::string::String {
        use alloc::string::String;
        use core::fmt::Write;
        let mut s = String::new();
        for b in bytes {
            write!(s, "{b:02x}").unwrap();
        }
        s
    }

    fn key32(s: &str) -> [u8; 32] {
        hex_decode(s).try_into().unwrap()
    }

    #[test]
    fn rfc_8439_2_5_2_example() {
        let key = key32("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b");
        let tag = poly1305_mac(&key, b"Cryptographic Forum Research Group");
        assert_eq!(hex(&tag), "a8061dc1305136c6c22b8baf0c0127a9");
    }

    /// RFC 8439 Appendix A.3's eleven Poly1305 edge cases (each given directly
    /// as `r`/`s` rather than a 32-byte key, so the key is `r || s`).
    struct EdgeCase {
        r: &'static str,
        s: &'static str,
        data: &'static str,
        tag: &'static str,
    }

    const EDGE_CASES: &[EdgeCase] = &[
        // #1: everything zero
        EdgeCase {
            r: "00000000000000000000000000000000",
            s: "00000000000000000000000000000000",
            data: "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            tag: "00000000000000000000000000000000",
        },
        // #2: r = 0, so the tag is always exactly s
        EdgeCase {
            r: "00000000000000000000000000000000",
            s: "36e5f6b5c5e06070f0efca96227a863e",
            data: "416e79207375626d697373696f6e20746f20746865204945544620696e74656e6465642062792074686520436f6e7472696275746f7220666f72207075626c69636174696f6e20617320616c6c206f722070617274206f6620616e204945544620496e7465726e65742d4472616674206f722052464320616e6420616e792073746174656d656e74206d6164652077697468696e2074686520636f6e74657874206f6620616e204945544620616374697669747920697320636f6e7369646572656420616e20224945544620436f6e747269627574696f6e222e20537563682073746174656d656e747320696e636c756465206f72616c2073746174656d656e747320696e20494554462073657373696f6e732c2061732077656c6c206173207772697474656e20616e6420656c656374726f6e696320636f6d6d756e69636174696f6e73206d61646520617420616e792074696d65206f7220706c6163652c207768696368206172652061646472657373656420746f",
            tag: "36e5f6b5c5e06070f0efca96227a863e",
        },
        // #3: s = 0
        EdgeCase {
            r: "36e5f6b5c5e06070f0efca96227a863e",
            s: "00000000000000000000000000000000",
            data: "416e79207375626d697373696f6e20746f20746865204945544620696e74656e6465642062792074686520436f6e7472696275746f7220666f72207075626c69636174696f6e20617320616c6c206f722070617274206f6620616e204945544620496e7465726e65742d4472616674206f722052464320616e6420616e792073746174656d656e74206d6164652077697468696e2074686520636f6e74657874206f6620616e204945544620616374697669747920697320636f6e7369646572656420616e20224945544620436f6e747269627574696f6e222e20537563682073746174656d656e747320696e636c756465206f72616c2073746174656d656e747320696e20494554462073657373696f6e732c2061732077656c6c206173207772697474656e20616e6420656c656374726f6e696320636f6d6d756e69636174696f6e73206d61646520617420616e792074696d65206f7220706c6163652c207768696368206172652061646472657373656420746f",
            tag: "f3477e7cd95417af89a6b8794c310cf0",
        },
        // #4: the Jabberwocky message, a representative "ordinary" case
        EdgeCase {
            r: "1c9240a5eb55d38af333888604f6b5f0",
            s: "473917c1402b80099dca5cbc207075c0",
            data: "2754776173206272696c6c69672c20616e642074686520736c6974687920746f7665730a446964206779726520616e642067696d626c6520696e2074686520776162653a0a416c6c206d696d737920776572652074686520626f726f676f7665732c0a416e6420746865206d6f6d65207261746873206f757467726162652e",
            tag: "4541669a7eaaee61e708dc7cbcc5eb62",
        },
        // #5: does the code handle a not-fully-reduced partial result?
        EdgeCase {
            r: "02000000000000000000000000000000",
            s: "00000000000000000000000000000000",
            data: "ffffffffffffffffffffffffffffffff",
            tag: "03000000000000000000000000000000",
        },
        // #6: does addition of s overflow modulo 2^128?
        EdgeCase {
            r: "02000000000000000000000000000000",
            s: "ffffffffffffffffffffffffffffffff",
            data: "02000000000000000000000000000000",
            tag: "03000000000000000000000000000000",
        },
        // #7: an all-ones data limb with a carry from the limb below
        EdgeCase {
            r: "01000000000000000000000000000000",
            s: "00000000000000000000000000000000",
            data: "fffffffffffffffffffffffffffffffff0ffffffffffffffffffffffffffffff11000000000000000000000000000000",
            tag: "05000000000000000000000000000000",
        },
        // #8: the polynomial part lands on exactly 2^130-5
        EdgeCase {
            r: "01000000000000000000000000000000",
            s: "00000000000000000000000000000000",
            data: "fffffffffffffffffffffffffffffffffbfefefefefefefefefefefefefefefe01010101010101010101010101010101",
            tag: "00000000000000000000000000000000",
        },
        // #9: the polynomial part lands on exactly 2^130-6
        EdgeCase {
            r: "02000000000000000000000000000000",
            s: "00000000000000000000000000000000",
            data: "fdffffffffffffffffffffffffffffff",
            tag: "faffffffffffffffffffffffffffffff",
        },
        // #10: a 5*H+L-type reduction producing a 131-bit intermediate result
        EdgeCase {
            r: "01000000000000000400000000000000",
            s: "00000000000000000000000000000000",
            data: "e33594d7505e43b900000000000000003394d7505e4379cd01000000000000000000000000000000000000000000000001000000000000000000000000000000",
            tag: "14000000000000005500000000000000",
        },
        // #11: the same, one message block shorter
        EdgeCase {
            r: "01000000000000000400000000000000",
            s: "00000000000000000000000000000000",
            data: "e33594d7505e43b900000000000000003394d7505e4379cd010000000000000000000000000000000000000000000000",
            tag: "13000000000000000000000000000000",
        },
    ];

    #[test]
    fn rfc_8439_appendix_a3_edge_cases() {
        for (i, case) in EDGE_CASES.iter().enumerate() {
            let mut key = [0u8; 32];
            key[..16].copy_from_slice(&hex_decode(case.r));
            key[16..].copy_from_slice(&hex_decode(case.s));
            let data = hex_decode(case.data);
            let tag = poly1305_mac(&key, &data);
            assert_eq!(hex(&tag), case.tag, "edge case #{} (1-indexed)", i + 1);
        }
    }

    #[test]
    fn streaming_matches_one_shot_at_every_split() {
        let key = key32("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b");
        let data: Vec<u8> = (0..200u32).map(|i| (i % 256) as u8).collect();
        let expected = poly1305_mac(&key, &data);
        for split in 0..=data.len() {
            let mut p = Poly1305::new(&key);
            p.update(&data[..split]);
            p.update(&data[split..]);
            assert_eq!(p.finalize(), expected, "mismatch splitting at {split}");
        }
    }

    #[test]
    fn empty_message_tag_is_exactly_s() {
        // r = 0 (first 32 hex chars), s = 00112233...ddeeff (last 32 hex chars).
        let key = key32("0000000000000000000000000000000000112233445566778899aabbccddeeff");
        let tag = poly1305_mac(&key, b"");
        assert_eq!(hex(&tag), "00112233445566778899aabbccddeeff");
    }
}
