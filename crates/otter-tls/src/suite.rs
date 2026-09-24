//! The three TLS 1.3 cipher suites this client offers (RFC 8446 section
//! B.4), and the hash-algorithm dispatch ([`SuiteHash`]) the key schedule
//! ([`crate::key_schedule`]) needs. `otter_crypto::hash::Hash` is a trait
//! with an associated `fn new() -> Self`, which is not object-safe, so
//! rather than fight `dyn Hash` this module just matches on a two-variant
//! enum everywhere a hash algorithm is needed generically -- exactly as
//! plain and "boring" as a generic `H: Hash` parameter, and simpler to
//! thread through a state machine whose cipher suite (and therefore hash)
//! is only known once the `ServerHello` arrives.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use otter_crypto::hash::Digest;
use otter_crypto::{Aead, Aes128Gcm, Aes256Gcm, Hmac, Sha256, Sha384};

/// A TLS 1.3 cipher suite this client can negotiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherSuite {
    /// `TLS_AES_128_GCM_SHA256` (0x1301).
    Aes128GcmSha256,
    /// `TLS_AES_256_GCM_SHA384` (0x1302).
    Aes256GcmSha384,
    /// `TLS_CHACHA20_POLY1305_SHA256` (0x1303).
    Chacha20Poly1305Sha256,
}

impl CipherSuite {
    pub(crate) fn id(self) -> u16 {
        match self {
            CipherSuite::Aes128GcmSha256 => 0x1301,
            CipherSuite::Aes256GcmSha384 => 0x1302,
            CipherSuite::Chacha20Poly1305Sha256 => 0x1303,
        }
    }

    pub(crate) fn from_id(id: u16) -> Option<CipherSuite> {
        match id {
            0x1301 => Some(CipherSuite::Aes128GcmSha256),
            0x1302 => Some(CipherSuite::Aes256GcmSha384),
            0x1303 => Some(CipherSuite::Chacha20Poly1305Sha256),
            _ => None,
        }
    }

    pub(crate) fn hash(self) -> SuiteHash {
        match self {
            CipherSuite::Aes128GcmSha256 => SuiteHash::Sha256,
            CipherSuite::Aes256GcmSha384 => SuiteHash::Sha384,
            CipherSuite::Chacha20Poly1305Sha256 => SuiteHash::Sha256,
        }
    }

    /// The AEAD key length in bytes (the nonce/IV is always 12 bytes and the
    /// tag always 16, for every suite this client offers).
    pub(crate) fn key_len(self) -> usize {
        match self {
            CipherSuite::Aes128GcmSha256 => 16,
            CipherSuite::Aes256GcmSha384 => 32,
            CipherSuite::Chacha20Poly1305Sha256 => 32,
        }
    }

    /// Builds the AEAD for this suite, bound to `key` (exactly
    /// [`CipherSuite::key_len`] bytes).
    pub(crate) fn aead(self, key: &[u8]) -> Box<dyn Aead> {
        match self {
            CipherSuite::Aes128GcmSha256 => Box::new(Aes128Gcm::new(&to_array::<16>(key))),
            CipherSuite::Aes256GcmSha384 => Box::new(Aes256Gcm::new(&to_array::<32>(key))),
            CipherSuite::Chacha20Poly1305Sha256 => {
                Box::new(otter_crypto::aead::ChaCha20Poly1305::new(&to_array::<32>(key)))
            }
        }
    }

    /// The three suites this client offers, in `ClientHello.cipher_suites`
    /// order: `prefer_chacha` (true when the CPU lacks AES-NI, brief M8-T5)
    /// puts the software-friendly ChaCha20-Poly1305 suite first.
    pub(crate) fn offered(prefer_chacha: bool) -> [CipherSuite; 3] {
        if prefer_chacha {
            [CipherSuite::Chacha20Poly1305Sha256, CipherSuite::Aes128GcmSha256, CipherSuite::Aes256GcmSha384]
        } else {
            [CipherSuite::Aes128GcmSha256, CipherSuite::Aes256GcmSha384, CipherSuite::Chacha20Poly1305Sha256]
        }
    }
}

fn to_array<const N: usize>(s: &[u8]) -> [u8; N] {
    s.try_into().expect("caller passed a key of the wrong length for this suite")
}

/// The hash algorithm a [`CipherSuite`] uses for its transcript hash, HKDF
/// and `Finished` MAC (RFC 8446 section 7.1). Two of this client's three
/// suites use SHA-256; `TLS_AES_256_GCM_SHA384` uses SHA-384.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuiteHash {
    Sha256,
    Sha384,
}

impl SuiteHash {
    /// The digest length in bytes (32 or 48).
    pub(crate) fn len(self) -> usize {
        match self {
            SuiteHash::Sha256 => 32,
            SuiteHash::Sha384 => 48,
        }
    }

    pub(crate) fn digest(self, data: &[u8]) -> Digest {
        match self {
            SuiteHash::Sha256 => Digest::from_slice(&Sha256::digest(data)),
            SuiteHash::Sha384 => Digest::from_slice(&Sha384::digest(data)),
        }
    }

    pub(crate) fn hkdf_extract(self, salt: &[u8], ikm: &[u8]) -> Digest {
        match self {
            SuiteHash::Sha256 => otter_crypto::hkdf_extract::<Sha256>(salt, ikm),
            SuiteHash::Sha384 => otter_crypto::hkdf_extract::<Sha384>(salt, ikm),
        }
    }

    pub(crate) fn hkdf_expand(self, prk: &[u8], info: &[u8], out: &mut [u8]) -> Result<(), otter_crypto::HkdfError> {
        match self {
            SuiteHash::Sha256 => otter_crypto::hkdf_expand::<Sha256>(prk, info, out),
            SuiteHash::Sha384 => otter_crypto::hkdf_expand::<Sha384>(prk, info, out),
        }
    }

    pub(crate) fn hmac(self, key: &[u8], data: &[u8]) -> Vec<u8> {
        match self {
            SuiteHash::Sha256 => Hmac::<Sha256>::mac(key, data).as_bytes().to_vec(),
            SuiteHash::Sha384 => Hmac::<Sha384>::mac(key, data).as_bytes().to_vec(),
        }
    }

    /// Verifies `tag` in constant time (`otter_crypto::hmac::Hmac::verify`),
    /// used for the `Finished` MAC check (DECISIONS.md D22).
    pub(crate) fn hmac_verify(self, key: &[u8], data: &[u8], tag: &[u8]) -> bool {
        match self {
            SuiteHash::Sha256 => Hmac::<Sha256>::verify(key, data, tag),
            SuiteHash::Sha384 => Hmac::<Sha384>::verify(key, data, tag),
        }
    }

    /// `HKDF-Expand-Label` (RFC 8446 section 7.1): builds the `HkdfLabel`
    /// structure (`uint16 length; opaque label<7..255> = "tls13 " + label;
    /// opaque context<0..255> = context;`) and expands into `out`. Every
    /// label this crate uses is a short ASCII constant and every context is
    /// at most a 48-byte transcript hash, both far under the one-byte
    /// length prefixes' 255-byte limit, so this never hits
    /// `HkdfError::OutputTooLong` either (`out` is at most a 48-byte digest
    /// or a 32-byte AEAD key, both `<< 255 * hash_len`).
    pub(crate) fn expand_label(self, secret: &[u8], label: &[u8], context: &[u8], out: &mut [u8]) {
        let mut hkdf_label = Vec::with_capacity(2 + 1 + 6 + label.len() + 1 + context.len());
        hkdf_label.extend_from_slice(&(out.len() as u16).to_be_bytes());
        hkdf_label.push((6 + label.len()) as u8);
        hkdf_label.extend_from_slice(b"tls13 ");
        hkdf_label.extend_from_slice(label);
        hkdf_label.push(context.len() as u8);
        hkdf_label.extend_from_slice(context);
        self.hkdf_expand(secret, &hkdf_label, out).expect("HkdfLabel expansion is always within RFC 5869 bounds");
    }

    /// `Derive-Secret(Secret, Label, Messages)` (RFC 8446 section 7.1):
    /// `HKDF-Expand-Label(Secret, Label, Transcript-Hash(Messages),
    /// Hash.length)`. `transcript_hash` is the caller's already-computed
    /// `Transcript-Hash(Messages)`.
    pub(crate) fn derive_secret(self, secret: &[u8], label: &[u8], transcript_hash: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; self.len()];
        self.expand_label(secret, label, transcript_hash, &mut out);
        out
    }

    /// `Derive-Secret(Secret, Label, "")`: the two "derived" steps in RFC
    /// 8446 section 7.1's key schedule diagram, whose `Messages` is the
    /// empty string rather than any real transcript.
    pub(crate) fn derive_secret_empty(self, secret: &[u8], label: &[u8]) -> Vec<u8> {
        let empty_hash = self.digest(&[]);
        self.derive_secret(secret, label, empty_hash.as_bytes())
    }
}
