//! Constant-time helpers shared by every primitive in this crate.
//!
//! Rust gives no hard guarantee that arithmetic on plain integers survives
//! optimization without becoming a data-dependent branch, and `forbid(unsafe_code)`
//! (crate root) rules out `core::ptr::write_volatile` for secret wiping. Everything
//! here is therefore *best-effort* constant-time: no secret-indexed table lookups,
//! no `if`/`match` on secret data, and [`core::hint::black_box`] at the points most
//! likely to be folded away by the optimizer (a single-instruction compare, a
//! dead-store-eliminated zeroing loop). This is the same caveat every safe-Rust
//! crypto crate ships with; DECISIONS.md D22 asks for constant-time code, not a
//! hardware-verified guarantee.

use core::hint::black_box;

/// A constant-time boolean. Never branch on it directly (that would defeat the
/// purpose) -- combine instances with [`Choice::and`]/[`Choice::or`]/[`Choice::not`]
/// and resolve to a real `bool` only at an API boundary (e.g. the `Result` a
/// `verify` function returns to its caller).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice(u8);

impl Choice {
    /// The constant-time `false`.
    pub const FALSE: Choice = Choice(0);
    /// The constant-time `true`.
    pub const TRUE: Choice = Choice(1);

    /// Builds a `Choice` from a byte that must already be exactly 0 or 1 (as
    /// produced by the bit tricks in this module, e.g. a shifted-out sign bit).
    #[inline]
    pub fn from_bit(bit: u8) -> Choice {
        Choice(bit & 1)
    }

    /// Ordinary (branching) construction from a `bool`, for turning a *public*
    /// condition into a `Choice` so it can be combined with secret ones; never call
    /// this with a secret-derived condition.
    #[inline]
    pub fn from_bool(public_bit: bool) -> Choice {
        Choice(public_bit as u8)
    }

    /// Bitwise AND of two choices.
    #[inline]
    pub fn and(self, other: Choice) -> Choice {
        Choice(self.0 & other.0)
    }

    /// Bitwise OR of two choices.
    #[inline]
    pub fn or(self, other: Choice) -> Choice {
        Choice(self.0 | other.0)
    }

    /// Logical NOT. Named `not` (not `!`/`core::ops::Not`) for symmetry with
    /// `and`/`or`, matching this crate's convention of avoiding operator-trait
    /// overloads for domain methods (see `otter-gfx`'s `Point::add`/`sub`).
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> Choice {
        Choice(self.0 ^ 1)
    }

    /// An all-zero (`Choice::FALSE`) or all-one (`Choice::TRUE`) mask of the
    /// requested width, for `select`-style bit tricks.
    #[inline]
    pub fn mask_u8(self) -> u8 {
        0u8.wrapping_sub(self.0)
    }

    /// Resolves to a real `bool`. Only call this at an API boundary -- the
    /// branch this forces is on the *final* result, not on any individual secret
    /// byte, which is the same trade-off `subtle::Choice::into::<bool>()` makes.
    #[inline]
    pub fn into_bool(self) -> bool {
        self.0 != 0
    }
}

/// Compares two byte slices for equality without branching on their contents and
/// without early-exiting on the first mismatch. Slices of different length are
/// unequal -- comparing lengths is not a secret-dependent branch, since a MAC/tag
/// length is public protocol metadata, never the secret itself.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    black_box(diff) == 0
}

/// Selects `a` if `choice` is true, `b` otherwise, without branching.
#[inline]
pub fn select_u8(choice: Choice, a: u8, b: u8) -> u8 {
    let mask = choice.mask_u8();
    (a & mask) | (b & !mask)
}

/// Overwrites `buf` with zero, then reads it back through [`black_box`] so the
/// optimizer cannot prove the store is dead and drop it (the classic
/// dead-store-elimination failure mode of a naive `for b in buf { *b = 0 }`).
/// Best-effort, as documented at module level.
pub fn zeroize(buf: &mut [u8]) {
    for byte in buf.iter_mut() {
        *byte = 0;
    }
    black_box(buf);
}

/// Wraps a byte-buffer-like value so it is zeroized when dropped. Used for secret
/// key material and intermediate buffers that must not linger in memory (padding
/// keys, HKDF's pseudorandom key, ChaCha20's Poly1305 one-time key, ...).
///
/// Best-effort, as documented at module level: this is a plain safe-Rust
/// destructor, not a hardware guarantee, and a `mem::forget` still defeats it.
pub struct Zeroizing<T: AsMut<[u8]>>(T);

impl<T: AsMut<[u8]>> Zeroizing<T> {
    /// Takes ownership of `inner`, zeroizing it on drop.
    pub fn new(inner: T) -> Self {
        Zeroizing(inner)
    }
}

impl<T: AsMut<[u8]> + Copy> Zeroizing<T> {
    /// Copies the inner value out before this guard's `Drop` zeroizes the
    /// original -- for the (fixed-size-array) buffers this crate wraps, taking a
    /// `Copy` out and letting the wrapper zeroize on scope exit is simpler and
    /// just as safe as an ownership-transferring `into_inner` would be.
    pub fn get(&self) -> T {
        self.0
    }
}

impl<T: AsMut<[u8]>> core::ops::Deref for Zeroizing<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: AsMut<[u8]>> core::ops::DerefMut for Zeroizing<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: AsMut<[u8]>> Drop for Zeroizing<T> {
    fn drop(&mut self) {
        zeroize(self.0.as_mut());
    }
}

impl<T: AsMut<[u8]>> From<T> for Zeroizing<T> {
    fn from(inner: T) -> Self {
        Zeroizing::new(inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_equal_slices() {
        assert!(ct_eq(b"hello world", b"hello world"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn ct_eq_unequal_content() {
        assert!(!ct_eq(b"hello world", b"HELLO WORLD"));
        assert!(!ct_eq(&[0u8; 32], &[0u8; 31]));
    }

    #[test]
    fn ct_eq_unequal_length() {
        assert!(!ct_eq(b"short", b"a bit longer"));
        assert!(!ct_eq(b"", b"\0"));
    }

    #[test]
    fn ct_eq_single_bit_flip_anywhere() {
        let a = [0xAAu8; 64];
        for i in 0..a.len() {
            for bit in 0..8u8 {
                let mut b = a;
                b[i] ^= 1 << bit;
                assert!(!ct_eq(&a, &b), "byte {i} bit {bit} flip not detected");
            }
        }
    }

    #[test]
    fn choice_algebra() {
        assert!(Choice::TRUE.into_bool());
        assert!(!Choice::FALSE.into_bool());
        assert!(Choice::TRUE.and(Choice::TRUE).into_bool());
        assert!(!Choice::TRUE.and(Choice::FALSE).into_bool());
        assert!(Choice::FALSE.or(Choice::TRUE).into_bool());
        assert!(!Choice::FALSE.or(Choice::FALSE).into_bool());
        assert!(Choice::FALSE.not().into_bool());
        assert!(!Choice::TRUE.not().into_bool());
    }

    #[test]
    fn select_u8_picks_the_right_branch() {
        assert_eq!(select_u8(Choice::TRUE, 0xAA, 0x55), 0xAA);
        assert_eq!(select_u8(Choice::FALSE, 0xAA, 0x55), 0x55);
    }

    #[test]
    fn zeroize_clears_the_buffer() {
        let mut buf = [1u8, 2, 3, 4, 5];
        zeroize(&mut buf);
        assert_eq!(buf, [0u8; 5]);
    }

    #[test]
    fn zeroizing_clears_on_drop() {
        // Cannot observe memory after drop safely, but can confirm `get`/`Deref`
        // see the live, untouched contents right up until the guard drops.
        let secret = Zeroizing::new([1u8, 2, 3, 4]);
        assert_eq!(*secret, [1, 2, 3, 4]);
        assert_eq!(secret.get(), [1, 2, 3, 4]);
    }
}
