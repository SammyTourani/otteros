//! The one piece of non-determinism this sans-I/O crate needs: random bytes
//! for the ClientHello's `random`, its legacy session id (RFC 8446 section
//! 4.1.2's middlebox-compatibility field) and the X25519 ephemeral private
//! key. A `no_std` crate cannot read `/dev/urandom` or call `getrandom(2)`
//! itself (DECISIONS.md D2/D27: no OS-facing crates here), so the caller --
//! the kernel, eventually seeded from real hardware entropy, or a host test
//! harness meanwhile -- supplies it through this trait.

/// A source of random bytes. Implementations do not need to be
/// cryptographically secure for every conceivable use of this trait, but
/// [`crate::ClientConnection::new`] uses it only for values that RFC 8446
/// requires to be unpredictable (the ClientHello random and the X25519
/// private key), so a real implementation must be.
pub trait Rng {
    /// Fills `dest` with random bytes.
    fn fill(&mut self, dest: &mut [u8]);
}
