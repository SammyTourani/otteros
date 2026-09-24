//! TLS alerts (RFC 8446 section 6): the `AlertLevel`/`AlertDescription` wire
//! values, decoded from a received 2-byte alert record body and encoded
//! into one this crate sends when it aborts a handshake locally.

/// An alert's severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertLevel {
    /// A non-fatal alert (only `close_notify` and `user_canceled` are
    /// ordinarily sent at this level).
    Warning,
    /// A fatal alert: the connection is over, per RFC 8446 section 6,
    /// "Upon transmission or receipt of a fatal alert message, both parties
    /// MUST immediately close the connection".
    Fatal,
    /// A level byte other than 1 (warning) or 2 (fatal). RFC 8446 says
    /// implementations MUST treat any unrecognized alert level as fatal
    /// (section 6), so callers should do the same with this variant.
    Unknown(u8),
}

impl AlertLevel {
    pub(crate) fn from_u8(b: u8) -> AlertLevel {
        match b {
            1 => AlertLevel::Warning,
            2 => AlertLevel::Fatal,
            other => AlertLevel::Unknown(other),
        }
    }

    pub(crate) fn to_u8(self) -> u8 {
        match self {
            AlertLevel::Warning => 1,
            AlertLevel::Fatal => 2,
            AlertLevel::Unknown(b) => b,
        }
    }

    /// Whether this level must be treated as connection-ending: an
    /// explicit `Fatal`, or an unrecognized level (RFC 8446 section 6).
    pub(crate) fn is_fatal(self) -> bool {
        !matches!(self, AlertLevel::Warning)
    }
}

/// An alert's reason (RFC 8446 section 6.2's full registry; this client
/// only ever sends a handful of these, but decodes any of them from a peer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertDescription {
    /// Normal connection shutdown, not a failure.
    CloseNotify,
    /// A message was received out of the order this state machine expects.
    UnexpectedMessage,
    /// An AEAD tag did not verify.
    BadRecordMac,
    /// A record's declared length is outside RFC 8446 section 5.2's limits.
    RecordOverflow,
    /// No acceptable set of handshake parameters could be negotiated.
    HandshakeFailure,
    /// A certificate was structurally invalid or otherwise unacceptable.
    BadCertificate,
    /// A certificate was of an unsupported type.
    UnsupportedCertificate,
    /// A certificate has been revoked (this client never checks revocation;
    /// only decoded for a peer that sends it back).
    CertificateRevoked,
    /// A certificate's validity period does not cover the current time.
    CertificateExpired,
    /// A certificate could not be validated for a reason not covered above.
    CertificateUnknown,
    /// A field in a handshake message was out of range or inconsistent.
    IllegalParameter,
    /// The certificate chain did not chain to a trusted root.
    UnknownCa,
    /// The sender decided not to proceed with the handshake.
    AccessDenied,
    /// A message could not be decoded because a field was out of its
    /// specified range or the message was too short.
    DecodeError,
    /// A handshake cryptographic operation (here: a `Finished` MAC) failed.
    DecryptError,
    /// The protocol version the peer attempted to negotiate is not
    /// supported (this client speaks TLS 1.3 only).
    ProtocolVersion,
    /// The negotiated security parameters are not strong enough.
    InsufficientSecurity,
    /// An internal, not-protocol-related error.
    InternalError,
    /// A downgrade was requested that is not allowed (RFC 8446's downgrade
    /// sentinel values in `ServerHello.random`).
    InappropriateFallback,
    /// The user canceled the handshake for a reason unrelated to protocol
    /// failure.
    UserCanceled,
    /// A message lacked a required extension.
    MissingExtension,
    /// A message contained an extension that was not expected.
    UnsupportedExtension,
    /// The server does not recognize the requested server name.
    UnrecognizedName,
    /// The certificate status response (e.g. OCSP staple) was invalid.
    BadCertificateStatusResponse,
    /// A PSK identity was not recognized.
    UnknownPskIdentity,
    /// The server requires a client certificate.
    CertificateRequired,
    /// ALPN negotiation failed to find a common protocol.
    NoApplicationProtocol,
    /// A description byte not in RFC 8446's registry.
    Unknown(u8),
}

impl AlertDescription {
    pub(crate) fn from_u8(b: u8) -> AlertDescription {
        use AlertDescription::*;
        match b {
            0 => CloseNotify,
            10 => UnexpectedMessage,
            20 => BadRecordMac,
            22 => RecordOverflow,
            40 => HandshakeFailure,
            42 => BadCertificate,
            43 => UnsupportedCertificate,
            44 => CertificateRevoked,
            45 => CertificateExpired,
            46 => CertificateUnknown,
            47 => IllegalParameter,
            48 => UnknownCa,
            49 => AccessDenied,
            50 => DecodeError,
            51 => DecryptError,
            70 => ProtocolVersion,
            71 => InsufficientSecurity,
            80 => InternalError,
            86 => InappropriateFallback,
            90 => UserCanceled,
            109 => MissingExtension,
            110 => UnsupportedExtension,
            112 => UnrecognizedName,
            113 => BadCertificateStatusResponse,
            115 => UnknownPskIdentity,
            116 => CertificateRequired,
            120 => NoApplicationProtocol,
            other => Unknown(other),
        }
    }

    pub(crate) fn to_u8(self) -> u8 {
        use AlertDescription::*;
        match self {
            CloseNotify => 0,
            UnexpectedMessage => 10,
            BadRecordMac => 20,
            RecordOverflow => 22,
            HandshakeFailure => 40,
            BadCertificate => 42,
            UnsupportedCertificate => 43,
            CertificateRevoked => 44,
            CertificateExpired => 45,
            CertificateUnknown => 46,
            IllegalParameter => 47,
            UnknownCa => 48,
            AccessDenied => 49,
            DecodeError => 50,
            DecryptError => 51,
            ProtocolVersion => 70,
            InsufficientSecurity => 71,
            InternalError => 80,
            InappropriateFallback => 86,
            UserCanceled => 90,
            MissingExtension => 109,
            UnsupportedExtension => 110,
            UnrecognizedName => 112,
            BadCertificateStatusResponse => 113,
            UnknownPskIdentity => 115,
            CertificateRequired => 116,
            NoApplicationProtocol => 120,
            Unknown(b) => b,
        }
    }
}
