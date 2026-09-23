//! The [`Hash`] trait: lets [`crate::hmac::Hmac`] and [`crate::hkdf`] be generic
//! over which digest TLS's cipher suite selects (SHA-256 or SHA-384), instead of
//! duplicating HMAC/HKDF once per hash.

/// A digest of at most 64 bytes (SHA-512's output, the largest this crate has).
/// A fixed-capacity buffer plus a length sidesteps needing a `[u8; H::OUTPUT_SIZE]`
/// return type, which Rust does not allow from a trait method without the
/// unstable `generic_const_exprs`.
#[derive(Clone, Copy)]
pub struct Digest {
    bytes: [u8; 64],
    len: usize,
}

impl Digest {
    /// Builds a `Digest` from a slice of at most 64 bytes.
    pub fn from_slice(data: &[u8]) -> Digest {
        assert!(data.len() <= 64, "digest longer than the largest hash this crate supports");
        let mut bytes = [0u8; 64];
        bytes[..data.len()].copy_from_slice(data);
        Digest { bytes, len: data.len() }
    }

    /// The digest bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl AsRef<[u8]> for Digest {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl core::ops::Deref for Digest {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl PartialEq for Digest {
    /// Ordinary (non-constant-time) equality, for tests and non-secret
    /// comparisons. Comparing a *received* MAC/tag against an expected one must
    /// go through [`crate::ct::ct_eq`] instead -- see [`crate::hmac::Hmac::verify`].
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Digest {}

impl core::fmt::Debug for Digest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Digest(")?;
        for byte in self.as_bytes() {
            write!(f, "{byte:02x}")?;
        }
        write!(f, ")")
    }
}

/// A streaming hash function: SHA-256, SHA-384 or SHA-512 (`sha256.rs`/`sha512.rs`).
/// Generic code (HMAC, HKDF, and eventually TLS's cipher-suite dispatch) is
/// written once against this trait instead of once per concrete hash.
pub trait Hash: Clone {
    /// Digest size in bytes (32 for SHA-256, 48 for SHA-384, 64 for SHA-512).
    const OUTPUT_SIZE: usize;
    /// Internal block size in bytes (64 for SHA-256, 128 for SHA-384/512) -- what
    /// HMAC pads the key to.
    const BLOCK_SIZE: usize;

    /// Starts a new hash computation.
    fn new() -> Self;
    /// Feeds more input. May be called any number of times.
    fn update(&mut self, data: &[u8]);
    /// Consumes the hasher and returns the digest.
    fn finalize(self) -> Digest;

    /// One-shot convenience: `Self::new()` + `update(data)` + `finalize()`.
    fn digest(data: &[u8]) -> Digest {
        let mut h = Self::new();
        h.update(data);
        h.finalize()
    }
}
