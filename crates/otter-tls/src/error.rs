//! The one error type every fallible operation in this crate returns
//! (mirroring `otter_x509::X509Error`'s design, see its module doc): DECISIONS.md
//! D22 says any verification failure aborts the handshake, so a TLS client only
//! ever needs to know *that* something failed and which alert to send back, not
//! a rich recoverable-error hierarchy.

use alloc::string::String;

use crate::alert::AlertDescription;

/// Why a handshake or connection operation failed. Every variant maps to a
/// specific outgoing alert via [`TlsError::alert`], except the three that mean
/// there is no connection left to send one on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsError {
    /// A message did not parse: too short, a length field pointed past the
    /// end of the message, or a fixed value was not what RFC 8446 requires.
    Decode(String),
    /// A message was received in a state that does not expect it.
    UnexpectedMessage(String),
    /// A record's declared length exceeded RFC 8446 section 5.2's limits.
    RecordOverflow,
    /// An AEAD tag did not verify.
    BadRecordMac,
    /// A field was structurally fine but its value was invalid (a session ID
    /// echo mismatch, an unacceptable cipher suite or group, ...).
    IllegalParameter(String),
    /// No compatible parameters could be found for this connection.
    HandshakeFailure(String),
    /// The peer did not negotiate TLS 1.3.
    ProtocolVersion,
    /// The peer's `ServerHello.random` carried a downgrade sentinel value
    /// (RFC 8446 section 4.1.3): a real TLS 1.3 server never sends this, so
    /// it means a downgrade attack (a fatal, deliberately non-recoverable
    /// condition, not merely a protocol-version mismatch).
    InappropriateFallback,
    /// `otter_x509::verify_server_chain` rejected the certificate chain.
    Certificate(otter_x509::X509Error),
    /// A `CertificateVerify` signature did not verify, or named a scheme
    /// this client did not offer or that does not match the leaf key's type.
    BadSignature,
    /// A `Finished` MAC did not verify.
    DecryptError,
    /// A message this client's scope does not support (e.g. a
    /// `CertificateRequest`: no client-certificate authentication).
    Unsupported(String),
    /// The X25519 key exchange produced the all-zero shared secret (a
    /// low-order point from the peer); RFC 8446 section 7.4.1 requires
    /// aborting.
    KeyExchangeFailed,
    /// A write-direction sequence number would have wrapped past 2^64 (RFC
    /// 8446 section 5.3); practically unreachable, but specified.
    SequenceNumberOverflow,
    /// A `ClientConfig`/server name given to [`crate::ClientConnection::new`]
    /// was invalid (no cipher suites, a server name that is not valid UTF-8
    /// ASCII, ...).
    InvalidConfig(String),
    /// The peer sent a fatal alert; the connection is over.
    PeerAborted(AlertDescription),
    /// `send`/`process` was called after [`crate::ClientConnection::close`]
    /// or after the connection otherwise ended.
    ConnectionClosed,
}

impl TlsError {
    /// The alert this client sends when it aborts locally for this reason.
    /// `None` for the three variants that mean there is nothing left to send
    /// an alert on.
    pub(crate) fn alert(&self) -> Option<AlertDescription> {
        use AlertDescription as A;
        Some(match self {
            TlsError::Decode(_) => A::DecodeError,
            TlsError::UnexpectedMessage(_) => A::UnexpectedMessage,
            TlsError::RecordOverflow => A::RecordOverflow,
            TlsError::BadRecordMac => A::BadRecordMac,
            TlsError::IllegalParameter(_) => A::IllegalParameter,
            TlsError::HandshakeFailure(_) => A::HandshakeFailure,
            TlsError::ProtocolVersion => A::ProtocolVersion,
            TlsError::InappropriateFallback => A::InappropriateFallback,
            TlsError::Certificate(e) => certificate_alert(*e),
            TlsError::BadSignature => A::DecryptError,
            TlsError::DecryptError => A::DecryptError,
            TlsError::Unsupported(_) => A::UnexpectedMessage,
            TlsError::KeyExchangeFailed => A::IllegalParameter,
            TlsError::SequenceNumberOverflow => A::InternalError,
            TlsError::InvalidConfig(_) => A::InternalError,
            TlsError::PeerAborted(_) | TlsError::ConnectionClosed => return None,
        })
    }
}

/// Maps an X.509 validation failure to the closest RFC 8446 section 6.2
/// alert. Every case aborts the handshake identically (DECISIONS.md D22);
/// this only picks which byte goes on the wire.
fn certificate_alert(e: otter_x509::X509Error) -> AlertDescription {
    use otter_x509::X509Error as X;
    match e {
        X::CertificateExpired | X::CertificateNotYetValid => AlertDescription::CertificateExpired,
        X::NoPathFound => AlertDescription::UnknownCa,
        X::HostnameMismatch => AlertDescription::BadCertificate,
        X::UnsupportedPublicKey | X::UnsupportedSignatureAlgorithm => AlertDescription::UnsupportedCertificate,
        _ => AlertDescription::BadCertificate,
    }
}

impl From<otter_x509::X509Error> for TlsError {
    fn from(e: otter_x509::X509Error) -> TlsError {
        TlsError::Certificate(e)
    }
}

impl core::fmt::Display for TlsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, f)
    }
}
