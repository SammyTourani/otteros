//! The RFC 8446 section 7.1 key schedule: no PSK and no early data (brief
//! M8-T5's scope), so this is the diagram's plain `(EC)DHE`-only path --
//! Early Secret is derived from an all-zero IKM purely so the "derived"
//! step's shape stays uniform, never used for a binder or early-traffic
//! secret. [`TrafficKeys`] is the record layer's other half: a suite's AEAD
//! bound to a derived key, plus the static IV and running sequence number
//! RFC 8446 section 5.3 XORs together for each record's nonce.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use otter_crypto::Aead;
use otter_crypto::ct::Zeroizing;

use crate::error::TlsError;
use crate::suite::{CipherSuite, SuiteHash};

/// The running secrets of RFC 8446 section 7.1's key schedule, in the order
/// they become available: Early -> Handshake -> Master. Traffic secrets
/// derived from each are returned to the caller ([`crate::conn`]) rather
/// than stored here, since it also needs to build [`TrafficKeys`] from them
/// and (for the application secrets) rotate them on a `KeyUpdate`.
pub(crate) struct KeySchedule {
    hash: SuiteHash,
    early_secret: Zeroizing<Vec<u8>>,
    handshake_secret: Zeroizing<Vec<u8>>,
    master_secret: Zeroizing<Vec<u8>>,
}

impl KeySchedule {
    /// `Early Secret = HKDF-Extract(0, PSK)`; this client never offers a
    /// PSK, so both the salt and IKM are `Hash.length` zero bytes (RFC 8446
    /// section 7.1: "if no PSK is selected, ... a string of Hash.length
    /// zeros is used" for IKM; salt "0" means the same for HKDF-Extract's
    /// first argument, exactly like `otter_crypto::hkdf::hkdf_extract`'s own
    /// documented `&[]`-for-absent-salt convention).
    pub(crate) fn new(hash: SuiteHash) -> KeySchedule {
        let zeros = vec![0u8; hash.len()];
        let early_secret = hash.hkdf_extract(&[], &zeros);
        KeySchedule {
            hash,
            early_secret: Zeroizing::new(early_secret.as_bytes().to_vec()),
            handshake_secret: Zeroizing::new(Vec::new()),
            master_secret: Zeroizing::new(Vec::new()),
        }
    }

    /// `Handshake Secret = HKDF-Extract(Derive-Secret(Early Secret,
    /// "derived", ""), (EC)DHE)`, and the two handshake traffic secrets
    /// derived from it over `transcript_hash` (`Transcript-Hash(ClientHello
    /// ... ServerHello)`). Returns `(client_handshake_traffic_secret,
    /// server_handshake_traffic_secret)`.
    pub(crate) fn advance_to_handshake(&mut self, dhe_shared_secret: &[u8], transcript_hash: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let derived = self.hash.derive_secret_empty(&self.early_secret, b"derived");
        let hs = self.hash.hkdf_extract(&derived, dhe_shared_secret);
        self.handshake_secret = Zeroizing::new(hs.as_bytes().to_vec());
        let c = self.hash.derive_secret(&self.handshake_secret, b"c hs traffic", transcript_hash);
        let s = self.hash.derive_secret(&self.handshake_secret, b"s hs traffic", transcript_hash);
        (c, s)
    }

    /// `Master Secret = HKDF-Extract(Derive-Secret(Handshake Secret,
    /// "derived", ""), 0)`, and the two application traffic secrets derived
    /// from it over `transcript_hash` (`Transcript-Hash(ClientHello ...
    /// server Finished)`). Returns `(client_application_traffic_secret_0,
    /// server_application_traffic_secret_0)`.
    pub(crate) fn advance_to_application(&mut self, transcript_hash: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let derived = self.hash.derive_secret_empty(&self.handshake_secret, b"derived");
        let zeros = vec![0u8; self.hash.len()];
        let ms = self.hash.hkdf_extract(&derived, &zeros);
        self.master_secret = Zeroizing::new(ms.as_bytes().to_vec());
        let c = self.hash.derive_secret(&self.master_secret, b"c ap traffic", transcript_hash);
        let s = self.hash.derive_secret(&self.master_secret, b"s ap traffic", transcript_hash);
        (c, s)
    }

    /// `finished_key = HKDF-Expand-Label(BaseKey, "finished", "",
    /// Hash.length)` (RFC 8446 section 4.4.4): `BaseKey` is the relevant
    /// side's handshake traffic secret.
    pub(crate) fn finished_key(&self, base_secret: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; self.hash.len()];
        self.hash.expand_label(base_secret, b"finished", &[], &mut out);
        out
    }

}

/// `application_traffic_secret_{N+1} = HKDF-Expand-Label(
/// application_traffic_secret_N, "traffic upd", "", Hash.length)` (RFC 8446
/// section 7.2), used both for a `KeyUpdate` this client receives and one it
/// sends in response.
pub(crate) fn update_traffic_secret(hash: SuiteHash, secret: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; hash.len()];
    hash.expand_label(secret, b"traffic upd", &[], &mut out);
    out
}

/// One direction's live record-protection state: the suite's AEAD bound to
/// a key derived from a traffic secret, the secret's static IV, and the
/// running sequence number RFC 8446 section 5.3 combines with the IV for
/// each record's nonce. Rebuilt from scratch (fresh sequence number too)
/// whenever the underlying traffic secret changes -- installing handshake
/// keys, installing application keys, and any later `KeyUpdate`.
pub(crate) struct TrafficKeys {
    aead: Box<dyn Aead>,
    iv: [u8; 12],
    seq: u64,
}

impl TrafficKeys {
    pub(crate) fn new(suite: CipherSuite, secret: &[u8]) -> TrafficKeys {
        let hash = suite.hash();
        let mut key = Zeroizing::new(vec![0u8; suite.key_len()]);
        hash.expand_label(secret, b"key", &[], &mut key);
        let mut iv = [0u8; 12];
        hash.expand_label(secret, b"iv", &[], &mut iv);
        TrafficKeys { aead: suite.aead(&key), iv, seq: 0 }
    }

    fn nonce(&self) -> [u8; 12] {
        let mut n = self.iv;
        let seq_bytes = self.seq.to_be_bytes();
        for i in 0..8 {
            n[4 + i] ^= seq_bytes[i];
        }
        n
    }

    /// The nonce for the next record, advancing the sequence number.
    /// Errors if the sequence number would wrap past 2^64 (RFC 8446 section
    /// 5.3: "if the sequence number would wrap ... the party must ...
    /// terminate the connection"); unreachable in any real connection's
    /// lifetime.
    fn next_nonce(&mut self) -> Result<[u8; 12], TlsError> {
        let n = self.nonce();
        self.seq = self.seq.checked_add(1).ok_or(TlsError::SequenceNumberOverflow)?;
        Ok(n)
    }

    /// Encrypts `plaintext` (already including its trailing inner content
    /// type and any padding) in place, authenticating `header` (the 5-byte
    /// record header that precedes it on the wire) as associated data, and
    /// appends `header`, the resulting ciphertext and the tag to `out`.
    pub(crate) fn seal(&mut self, header: [u8; 5], mut plaintext: Vec<u8>, out: &mut Vec<u8>) -> Result<(), TlsError> {
        let nonce = self.next_nonce()?;
        let tag = self.aead.seal_in_place(&nonce, &header, &mut plaintext);
        out.extend_from_slice(&header);
        out.extend_from_slice(&plaintext);
        out.extend_from_slice(&tag);
        Ok(())
    }

    /// Decrypts `ciphertext_and_tag` (the record payload after its 5-byte
    /// header) in place, authenticating `header` as associated data.
    /// [`TlsError::BadRecordMac`] on any tag mismatch, matching
    /// `otter_crypto::aead`'s "verify before touching plaintext" contract
    /// (DECISIONS.md D22): a forged or corrupted record is never even
    /// partially decrypted.
    pub(crate) fn open(&mut self, header: [u8; 5], mut ciphertext_and_tag: Vec<u8>) -> Result<Vec<u8>, TlsError> {
        if ciphertext_and_tag.len() < 16 {
            return Err(TlsError::Decode("encrypted record shorter than one AEAD tag".into()));
        }
        let nonce = self.next_nonce()?;
        let tag_start = ciphertext_and_tag.len() - 16;
        let tag: [u8; 16] = ciphertext_and_tag[tag_start..].try_into().expect("exactly 16 bytes sliced");
        ciphertext_and_tag.truncate(tag_start);
        self.aead.open_in_place(&nonce, &header, &mut ciphertext_and_tag, &tag).map_err(|_| TlsError::BadRecordMac)?;
        Ok(ciphertext_and_tag)
    }
}
