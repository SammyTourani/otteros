//! [`ClientConnection`]: the sans-I/O TLS 1.3 client state machine (RFC
//! 8446). Owns no socket -- the caller feeds received bytes to
//! [`ClientConnection::read_tls`], drains bytes to send with
//! [`ClientConnection::take_outgoing`], and drives the handshake and
//! connection forward by calling [`ClientConnection::process`] until it
//! returns `Ok(None)`, at which point the caller must supply more input
//! (either more received bytes, or a [`ClientConnection::send`] /
//! [`ClientConnection::close`] call of its own) before anything else will
//! happen. This is exactly the shape that lets the same state machine run
//! today over an ordinary `std::net::TcpStream` in this crate's own tests
//! and, unchanged, over OtterOS's own TCP stack later (brief M8-T5's goal).

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use otter_crypto::ct::Zeroizing;

use crate::alert::{AlertDescription, AlertLevel};
use crate::certificate;
use crate::cert_verify;
use crate::client_hello;
use crate::codec::{self, Reader, handshake_type};
use crate::config::ClientConfig;
use crate::error::TlsError;
use crate::extensions;
use crate::key_schedule::{KeySchedule, TrafficKeys, update_traffic_secret};
use crate::record::{self, content_type};
use crate::rng::Rng;
use crate::server_hello;
use crate::signature_scheme::{self, SignatureScheme};
use crate::suite::CipherSuite;
use crate::transcript::Transcript;

/// A cap on how many bytes of *reassembled* handshake-layer data this client
/// will buffer while waiting for one logical handshake message to become
/// complete. Each individual record is already bounded by RFC 8446 section
/// 5.2 (checked in [`ClientConnection::pump_one_record`]), so a hostile peer
/// can only reach this by sending many records; generous enough for any
/// real certificate chain, small enough to bound memory use on hostile input.
const MAX_HANDSHAKE_REASSEMBLY: usize = 1 << 20;

/// An event [`ClientConnection::process`] reports back to the caller.
#[derive(Debug, Clone)]
pub enum Event {
    /// The handshake finished and application data can now be sent/received.
    HandshakeComplete {
        /// The negotiated cipher suite.
        suite: CipherSuite,
        /// The negotiated ALPN protocol, if the caller offered any and the
        /// server selected one.
        alpn: Option<Vec<u8>>,
        /// The leaf certificate's subject, for display (`otter_x509::VerifiedChain::leaf_subject`).
        peer: String,
    },
    /// Decrypted application data from the peer.
    ApplicationData(Vec<u8>),
    /// The peer sent `close_notify`: it will send no more data.
    PeerClosed,
    /// The peer sent an alert other than `close_notify`.
    Alert {
        /// The alert's severity.
        level: AlertLevel,
        /// The alert's reason.
        description: AlertDescription,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    WaitServerHello,
    WaitEncryptedExtensions,
    WaitCertificate,
    WaitCertificateVerify,
    WaitServerFinished,
    Connected,
    Closed,
}

/// A sans-I/O TLS 1.3 client connection (RFC 8446). See the module doc for
/// the overall shape.
pub struct ClientConnection {
    config: ClientConfig,
    host: String,
    now_unix: u64,

    state: State,
    transcript: Transcript,
    suite: Option<CipherSuite>,
    hrr_seen: bool,
    cookie: Option<Vec<u8>>,

    client_random: [u8; 32],
    legacy_session_id: [u8; 32],
    x25519_private: Zeroizing<[u8; 32]>,
    x25519_public: [u8; 32],

    key_schedule: Option<KeySchedule>,
    client_hs_secret: Option<Zeroizing<Vec<u8>>>,
    server_hs_secret: Option<Zeroizing<Vec<u8>>>,
    client_ap_secret: Option<Zeroizing<Vec<u8>>>,
    server_ap_secret: Option<Zeroizing<Vec<u8>>>,
    read_keys: Option<TrafficKeys>,
    write_keys: Option<TrafficKeys>,

    verified_chain: Option<otter_x509::VerifiedChain>,
    alpn_negotiated: Option<Vec<u8>>,

    incoming: VecDeque<u8>,
    hs_reassembly: Vec<u8>,
    outgoing: Vec<u8>,
    pending_events: VecDeque<Event>,

    /// Set once this side must send/process no more data: a local failure
    /// (a fatal alert was queued), a fatal alert was received, or
    /// [`ClientConnection::close`] was called.
    closed: bool,
}

impl ClientConnection {
    /// Starts a new connection: generates the ClientHello's random values
    /// and X25519 key share via `rng`, and queues the ClientHello (plus a
    /// middlebox-compatibility `change_cipher_spec`, RFC 8446 Appendix D.4)
    /// for [`ClientConnection::take_outgoing`].
    pub fn new(config: ClientConfig, server_name: &str, now_unix: u64, rng: &mut dyn Rng) -> Result<ClientConnection, TlsError> {
        if server_name.is_empty() || !server_name.is_ascii() {
            return Err(TlsError::InvalidConfig("server_name must be a non-empty ASCII DNS name or IP literal".to_string()));
        }

        let mut client_random = [0u8; 32];
        rng.fill(&mut client_random);
        let mut legacy_session_id = [0u8; 32];
        rng.fill(&mut legacy_session_id);
        let mut x25519_private = Zeroizing::new([0u8; 32]);
        rng.fill(&mut *x25519_private);
        let x25519_public = otter_crypto::x25519_base(&x25519_private).map_err(|_| TlsError::KeyExchangeFailed)?;

        let sni = if is_ip_literal(server_name) { None } else { Some(server_name.as_bytes()) };

        let mut conn = ClientConnection {
            config,
            host: server_name.to_string(),
            now_unix,
            state: State::WaitServerHello,
            transcript: Transcript::new(),
            suite: None,
            hrr_seen: false,
            cookie: None,
            client_random,
            legacy_session_id,
            x25519_private,
            x25519_public,
            key_schedule: None,
            client_hs_secret: None,
            server_hs_secret: None,
            client_ap_secret: None,
            server_ap_secret: None,
            read_keys: None,
            write_keys: None,
            verified_chain: None,
            alpn_negotiated: None,
            incoming: VecDeque::new(),
            hs_reassembly: Vec::new(),
            outgoing: Vec::new(),
            pending_events: VecDeque::new(),
            closed: false,
        };

        let ch1 = client_hello::build(client_random, legacy_session_id, &conn.config, sni, &x25519_public, None);
        conn.transcript.add(&ch1);
        // Order matters here: RFC 8446 Appendix D.4's compatibility record
        // follows the ClientHello it accompanies, not the other way around.
        conn.queue_handshake(&ch1).expect("no write keys exist yet, so this cannot fail");
        conn.queue_change_cipher_spec();
        Ok(conn)
    }

    /// Feeds bytes received from the peer. Pure buffering -- call
    /// [`ClientConnection::process`] afterward to actually make progress.
    pub fn read_tls(&mut self, data: &[u8]) {
        self.incoming.extend(data.iter().copied());
    }

    /// Returns (and clears) the bytes queued to send to the peer.
    pub fn take_outgoing(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.outgoing)
    }

    /// Advances the state machine as far as currently-buffered input
    /// allows, returning the next event, or `Ok(None)` once no more
    /// progress can be made without more input. A local protocol failure
    /// queues a fatal alert (available from [`ClientConnection::take_outgoing`])
    /// and is returned as `Err`; the connection is then closed.
    pub fn process(&mut self) -> Result<Option<Event>, TlsError> {
        loop {
            if let Some(event) = self.pending_events.pop_front() {
                return Ok(Some(event));
            }
            match self.pump_one_record() {
                Ok(true) => continue,
                Ok(false) => return Ok(None),
                Err(e) => return Err(self.fail(e)),
            }
        }
    }

    /// Queues `data` as application data. Errors if the handshake has not
    /// completed or the connection is closed.
    pub fn send(&mut self, data: &[u8]) -> Result<(), TlsError> {
        if self.closed {
            return Err(TlsError::ConnectionClosed);
        }
        if self.state != State::Connected {
            return Err(TlsError::Unsupported("send() before the handshake completed".to_string()));
        }
        if data.is_empty() {
            return Ok(());
        }
        self.queue_content(content_type::APPLICATION_DATA, data)
    }

    /// Sends `close_notify` (RFC 8446 section 6.1) and marks the connection
    /// closed. Idempotent.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.queue_alert(AlertLevel::Warning, AlertDescription::CloseNotify);
        self.closed = true;
    }

    /// Whether this connection can no longer send or make progress (a fatal
    /// alert was sent or received, or [`ClientConnection::close`] was called).
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    // ---- Internal driving logic -----------------------------------------

    /// Applies a locally detected failure's side effects (queue the
    /// corresponding fatal alert, mark the connection closed) and returns
    /// the same error, so every fallible entry point can just
    /// `.map_err(|e| self.fail(e))` once at its own boundary.
    fn fail(&mut self, err: TlsError) -> TlsError {
        if let Some(description) = err.alert() {
            self.queue_alert(AlertLevel::Fatal, description);
        }
        self.closed = true;
        self.state = State::Closed;
        err
    }

    fn queue_change_cipher_spec(&mut self) {
        record::write_plaintext_record(&mut self.outgoing, content_type::CHANGE_CIPHER_SPEC, &[0x01]);
    }

    fn queue_alert(&mut self, level: AlertLevel, description: AlertDescription) {
        let body = [level.to_u8(), description.to_u8()];
        // Only failure mode is a sequence-number wraparound on the write
        // side, astronomically unlikely; nothing better to do with it than
        // drop the alert we can no longer protect anyway.
        let _ = self.queue_content(content_type::ALERT, &body);
    }

    fn queue_handshake(&mut self, message: &[u8]) -> Result<(), TlsError> {
        self.queue_content(content_type::HANDSHAKE, message)
    }

    /// Encrypts (if write keys are installed) or sends in the clear
    /// (otherwise -- only ever true for the plaintext `ClientHello`(s))
    /// `payload` under `content_type`, chunked to RFC 8446 section 5.2's
    /// 2^14-byte plaintext limit.
    fn queue_content(&mut self, content_type: u8, payload: &[u8]) -> Result<(), TlsError> {
        let chunks: Vec<&[u8]> = if payload.is_empty() { alloc::vec![payload] } else { payload.chunks(record::MAX_PLAINTEXT_LEN).collect() };
        for chunk in chunks {
            match self.write_keys.as_mut() {
                Some(keys) => {
                    let mut inner = chunk.to_vec();
                    inner.push(content_type);
                    let header = record::build_header(record::content_type::APPLICATION_DATA, inner.len() + 16);
                    keys.seal(header, inner, &mut self.outgoing)?;
                }
                None => record::write_plaintext_record(&mut self.outgoing, content_type, chunk),
            }
        }
        Ok(())
    }

    /// Tries to consume exactly one record from `self.incoming`. `Ok(true)`
    /// means progress was made (try again immediately); `Ok(false)` means
    /// there is not yet a complete record buffered.
    fn pump_one_record(&mut self) -> Result<bool, TlsError> {
        if self.closed {
            return Ok(false);
        }
        if self.incoming.len() < 5 {
            return Ok(false);
        }
        let header: [u8; 5] = {
            let contiguous = self.incoming.make_contiguous();
            contiguous[..5].try_into().expect("checked len >= 5 above")
        };
        let declared_len = u16::from_be_bytes([header[3], header[4]]) as usize;
        if declared_len > record::MAX_CIPHERTEXT_LEN {
            return Err(TlsError::RecordOverflow);
        }
        if self.incoming.len() < 5 + declared_len {
            return Ok(false);
        }
        let payload: Vec<u8> = self.incoming.iter().skip(5).take(declared_len).copied().collect();
        self.incoming.drain(..5 + declared_len);

        let outer_content_type = header[0];
        if outer_content_type == content_type::CHANGE_CIPHER_SPEC {
            // RFC 8446 section 5.1: dropped unconditionally if well-formed;
            // never encrypted, so this check happens before any decryption.
            if payload != [0x01] {
                return Err(TlsError::UnexpectedMessage("malformed change_cipher_spec record".to_string()));
            }
            return Ok(true);
        }

        let (inner_type, inner_bytes) = if let Some(keys) = self.read_keys.as_mut() {
            let plaintext = keys.open(header, payload)?;
            if plaintext.len() > record::MAX_PLAINTEXT_LEN + 1 {
                return Err(TlsError::RecordOverflow);
            }
            record::strip_inner_plaintext(plaintext).ok_or_else(|| TlsError::UnexpectedMessage("all-zero TLSInnerPlaintext (no content type)".to_string()))?
        } else {
            if declared_len > record::MAX_PLAINTEXT_LEN {
                return Err(TlsError::RecordOverflow);
            }
            match outer_content_type {
                content_type::HANDSHAKE | content_type::ALERT => (outer_content_type, payload),
                _ => return Err(TlsError::UnexpectedMessage("unexpected content type before any keys are established".to_string())),
            }
        };
        // A decrypted inner type must itself be a real content type other
        // than change_cipher_spec (RFC 8446 section 5.1: "a protected
        // change_cipher_spec record" is always invalid).
        match inner_type {
            content_type::HANDSHAKE => self.on_handshake_bytes(&inner_bytes)?,
            content_type::ALERT => self.on_alert_bytes(&inner_bytes)?,
            content_type::APPLICATION_DATA => {
                if self.state != State::Connected {
                    return Err(TlsError::UnexpectedMessage("application_data before the handshake completed".to_string()));
                }
                self.pending_events.push_back(Event::ApplicationData(inner_bytes));
            }
            _ => return Err(TlsError::UnexpectedMessage("invalid (or protected change_cipher_spec) inner content type".to_string())),
        }
        Ok(true)
    }

    fn on_alert_bytes(&mut self, bytes: &[u8]) -> Result<(), TlsError> {
        if bytes.len() != 2 {
            return Err(TlsError::Decode("alert record is not exactly 2 bytes".to_string()));
        }
        let level = AlertLevel::from_u8(bytes[0]);
        let description = AlertDescription::from_u8(bytes[1]);
        if description == AlertDescription::CloseNotify {
            self.pending_events.push_back(Event::PeerClosed);
        } else {
            if level.is_fatal() {
                self.closed = true;
            }
            self.pending_events.push_back(Event::Alert { level, description });
        }
        Ok(())
    }

    fn on_handshake_bytes(&mut self, bytes: &[u8]) -> Result<(), TlsError> {
        self.hs_reassembly.extend_from_slice(bytes);
        if self.hs_reassembly.len() > MAX_HANDSHAKE_REASSEMBLY {
            return Err(TlsError::RecordOverflow);
        }
        loop {
            let Some((msg_type, body_len, total_len)) = codec::parse_handshake_message(&self.hs_reassembly).map(|(m, n)| (m.msg_type, m.body.len(), n)) else {
                return Ok(());
            };
            let raw = self.hs_reassembly[..total_len].to_vec();
            let body = raw[total_len - body_len..total_len].to_vec();
            self.hs_reassembly.drain(..total_len);
            self.on_handshake_message(msg_type, &body, &raw)?;
        }
    }

    fn on_handshake_message(&mut self, msg_type: u8, body: &[u8], raw: &[u8]) -> Result<(), TlsError> {
        if self.state == State::Connected {
            return match msg_type {
                handshake_type::NEW_SESSION_TICKET => Ok(()), // RFC 8446 section 4.6.1: discarded.
                handshake_type::KEY_UPDATE => self.on_key_update(body),
                other => Err(TlsError::UnexpectedMessage(format!("handshake message type {other} after the handshake completed"))),
            };
        }
        match (self.state, msg_type) {
            (State::WaitServerHello, handshake_type::SERVER_HELLO) => self.on_server_hello(body, raw),
            (State::WaitEncryptedExtensions, handshake_type::ENCRYPTED_EXTENSIONS) => self.on_encrypted_extensions(body, raw),
            (State::WaitCertificate, handshake_type::CERTIFICATE_REQUEST) => {
                Err(TlsError::Unsupported("CertificateRequest: client-certificate authentication is not supported".to_string()))
            }
            (State::WaitCertificate, handshake_type::CERTIFICATE) => self.on_certificate(body, raw),
            (State::WaitCertificateVerify, handshake_type::CERTIFICATE_VERIFY) => self.on_certificate_verify(body, raw),
            (State::WaitServerFinished, handshake_type::FINISHED) => self.on_server_finished(body, raw),
            (state, other) => Err(TlsError::UnexpectedMessage(format!("handshake message type {other} not expected in state {state:?}"))),
        }
    }

    fn on_server_hello(&mut self, body: &[u8], raw: &[u8]) -> Result<(), TlsError> {
        let sh = server_hello::parse(body)?;
        if server_hello::is_downgrade_sentinel(&sh.random) {
            return Err(TlsError::InappropriateFallback);
        }
        if sh.is_hello_retry_request {
            return self.on_hello_retry_request(sh, raw);
        }

        let supported_versions = extensions::find(&sh.extensions, extensions::ext_type::SUPPORTED_VERSIONS).ok_or(TlsError::ProtocolVersion)?;
        if extensions::parse_supported_versions_server(supported_versions)? != extensions::TLS_1_3_VERSION {
            return Err(TlsError::ProtocolVersion);
        }

        let suite = CipherSuite::from_id(sh.cipher_suite)
            .ok_or_else(|| TlsError::IllegalParameter("ServerHello selected a cipher suite this client did not offer".to_string()))?;
        if let Some(expected) = self.suite
            && expected != suite
        {
            return Err(TlsError::IllegalParameter("ServerHello.cipher_suite does not match the HelloRetryRequest's".to_string()));
        }

        if sh.legacy_session_id_echo != self.legacy_session_id {
            return Err(TlsError::IllegalParameter("legacy_session_id_echo does not match what this client sent".to_string()));
        }

        let key_share = extensions::find(&sh.extensions, extensions::ext_type::KEY_SHARE)
            .ok_or_else(|| TlsError::IllegalParameter("ServerHello has no key_share extension".to_string()))?;
        let (group, peer_key) = extensions::parse_key_share_server_hello(key_share)?;
        if group != extensions::X25519_GROUP {
            return Err(TlsError::IllegalParameter("ServerHello key_share names a group other than x25519".to_string()));
        }
        let peer_key: [u8; 32] = peer_key.try_into().map_err(|_| TlsError::Decode("ServerHello key_share key_exchange is not 32 bytes".to_string()))?;

        self.suite = Some(suite);
        self.transcript.add(raw);

        let shared_secret = otter_crypto::x25519(&self.x25519_private, &peer_key).map_err(|_| TlsError::KeyExchangeFailed)?;

        let hash = suite.hash();
        let mut schedule = KeySchedule::new(hash);
        let transcript_hash = self.transcript.hash(hash);
        let (client_hs, server_hs) = schedule.advance_to_handshake(&shared_secret, &transcript_hash);
        self.read_keys = Some(TrafficKeys::new(suite, &server_hs));
        self.write_keys = Some(TrafficKeys::new(suite, &client_hs));
        self.client_hs_secret = Some(Zeroizing::new(client_hs));
        self.server_hs_secret = Some(Zeroizing::new(server_hs));
        self.key_schedule = Some(schedule);
        self.state = State::WaitEncryptedExtensions;
        Ok(())
    }

    fn on_hello_retry_request(&mut self, sh: server_hello::ServerHello<'_>, raw: &[u8]) -> Result<(), TlsError> {
        if self.hrr_seen {
            return Err(TlsError::UnexpectedMessage("a second HelloRetryRequest is not allowed".to_string()));
        }
        self.hrr_seen = true;

        let supported_versions = extensions::find(&sh.extensions, extensions::ext_type::SUPPORTED_VERSIONS).ok_or(TlsError::ProtocolVersion)?;
        if extensions::parse_supported_versions_server(supported_versions)? != extensions::TLS_1_3_VERSION {
            return Err(TlsError::ProtocolVersion);
        }

        let suite = CipherSuite::from_id(sh.cipher_suite)
            .ok_or_else(|| TlsError::IllegalParameter("HelloRetryRequest selected a cipher suite this client did not offer".to_string()))?;

        if sh.legacy_session_id_echo != self.legacy_session_id {
            return Err(TlsError::IllegalParameter("legacy_session_id_echo does not match what this client sent".to_string()));
        }

        if let Some(key_share) = extensions::find(&sh.extensions, extensions::ext_type::KEY_SHARE) {
            let group = extensions::parse_key_share_hrr(key_share)?;
            if group != extensions::X25519_GROUP {
                return Err(TlsError::IllegalParameter("HelloRetryRequest requested a group other than x25519".to_string()));
            }
            // Same group already offered: no new key share to generate.
        }

        self.cookie = match extensions::find(&sh.extensions, extensions::ext_type::COOKIE) {
            Some(c) => Some(extensions::parse_cookie(c)?.to_vec()),
            None => None,
        };

        self.suite = Some(suite);
        self.transcript.replace_client_hello1_with_message_hash(suite.hash());
        self.transcript.add(raw);

        let sni = if is_ip_literal(&self.host) { None } else { Some(self.host.as_bytes()) };
        let ch2 = client_hello::build(self.client_random, self.legacy_session_id, &self.config, sni, &self.x25519_public, self.cookie.as_deref());
        self.transcript.add(&ch2);
        self.queue_handshake(&ch2).expect("no write keys exist yet, so this cannot fail");
        self.queue_change_cipher_spec();
        Ok(())
    }

    fn on_encrypted_extensions(&mut self, body: &[u8], raw: &[u8]) -> Result<(), TlsError> {
        let mut r = Reader::new(body);
        let ext_bytes = r.opaque16()?;
        if !r.is_empty() {
            return Err(TlsError::Decode("EncryptedExtensions has trailing bytes".to_string()));
        }
        let exts = extensions::parse_extensions(ext_bytes)?;
        if let Some(alpn_body) = extensions::find(&exts, extensions::ext_type::ALPN) {
            self.alpn_negotiated = Some(extensions::parse_alpn_response(alpn_body)?);
        }
        self.transcript.add(raw);
        self.state = State::WaitCertificate;
        Ok(())
    }

    fn on_certificate(&mut self, body: &[u8], raw: &[u8]) -> Result<(), TlsError> {
        let chain = certificate::parse(body)?;
        let chain_refs: Vec<&[u8]> = chain.iter().map(Vec::as_slice).collect();
        let verified = otter_x509::verify_server_chain(&chain_refs, &self.host, self.now_unix, &self.config.roots)?;
        self.verified_chain = Some(verified);
        self.transcript.add(raw);
        self.state = State::WaitCertificateVerify;
        Ok(())
    }

    fn on_certificate_verify(&mut self, body: &[u8], raw: &[u8]) -> Result<(), TlsError> {
        let suite = self.suite.expect("set once ServerHello is processed");
        let hash = suite.hash();
        // "Transcript-Hash(Handshake Context, Certificate)": up to and
        // including Certificate, not including this message.
        let transcript_hash = self.transcript.hash(hash);

        let (scheme_code, signature) = cert_verify::parse(body)?;
        let scheme = SignatureScheme::from_code(scheme_code).ok_or(TlsError::BadSignature)?;
        let leaf_key = self.verified_chain.as_ref().expect("Certificate is processed before CertificateVerify").leaf_public_key();
        let content = cert_verify::server_signed_content(&transcript_hash);
        signature_scheme::verify(leaf_key, scheme, &content, signature)?;

        self.transcript.add(raw);
        self.state = State::WaitServerFinished;
        Ok(())
    }

    fn on_server_finished(&mut self, body: &[u8], raw: &[u8]) -> Result<(), TlsError> {
        let suite = self.suite.expect("set once ServerHello is processed");
        let hash = suite.hash();
        let schedule = self.key_schedule.as_ref().expect("set once ServerHello is processed");
        let server_hs_secret = self.server_hs_secret.as_ref().expect("set once ServerHello is processed");
        let finished_key = schedule.finished_key(server_hs_secret);
        // "Transcript-Hash(Handshake Context, Certificate, CertificateVerify)".
        let transcript_hash = self.transcript.hash(hash);
        if !hash.hmac_verify(&finished_key, &transcript_hash, body) {
            return Err(TlsError::DecryptError);
        }
        self.transcript.add(raw);

        // Application traffic secrets and the client's own Finished both use
        // "Transcript-Hash(... server Finished)" -- exactly the transcript
        // as it stands right now, having just added the server's Finished.
        let ap_hash = self.transcript.hash(hash);
        let schedule = self.key_schedule.as_mut().expect("set once ServerHello is processed");
        let (client_ap, server_ap) = schedule.advance_to_application(&ap_hash);

        let client_hs_secret = self.client_hs_secret.as_ref().expect("set once ServerHello is processed");
        let client_finished_key = schedule.finished_key(client_hs_secret);
        let verify_data = hash.hmac(&client_finished_key, &ap_hash);
        let mut finished_msg = Vec::new();
        codec::write_handshake_message(&mut finished_msg, handshake_type::FINISHED, &verify_data);
        // Still under the handshake write key: application keys are
        // installed only after this is queued (RFC 8446 section 7.1: the
        // client's Finished is the last message protected under
        // client_handshake_traffic_secret).
        self.queue_handshake(&finished_msg)?;
        self.transcript.add(&finished_msg);

        self.read_keys = Some(TrafficKeys::new(suite, &server_ap));
        self.write_keys = Some(TrafficKeys::new(suite, &client_ap));
        self.client_ap_secret = Some(Zeroizing::new(client_ap));
        self.server_ap_secret = Some(Zeroizing::new(server_ap));

        let peer = self.verified_chain.as_ref().expect("set in on_certificate").leaf_subject().to_string();
        let alpn = self.alpn_negotiated.clone();
        self.state = State::Connected;
        self.pending_events.push_back(Event::HandshakeComplete { suite, alpn, peer });
        Ok(())
    }

    fn on_key_update(&mut self, body: &[u8]) -> Result<(), TlsError> {
        let mut r = Reader::new(body);
        let request_update = r.u8()?;
        if !r.is_empty() {
            return Err(TlsError::Decode("KeyUpdate has trailing bytes".to_string()));
        }
        if request_update > 1 {
            return Err(TlsError::IllegalParameter("KeyUpdate.request_update has an invalid value".to_string()));
        }
        let suite = self.suite.expect("suite is set once ServerHello is processed, long before any KeyUpdate");
        let hash = suite.hash();

        let old_read = self.server_ap_secret.take().expect("application traffic secrets are set before entering Connected");
        let updated_read = update_traffic_secret(hash, &old_read);
        self.read_keys = Some(TrafficKeys::new(suite, &updated_read));
        self.server_ap_secret = Some(Zeroizing::new(updated_read));

        if request_update == 1 {
            let mut msg = Vec::new();
            codec::write_handshake_message(&mut msg, handshake_type::KEY_UPDATE, &[0]); // update_not_requested
            self.queue_handshake(&msg)?; // still under the current (pre-update) write key.
            let old_write = self.client_ap_secret.take().expect("application traffic secrets are set before entering Connected");
            let updated_write = update_traffic_secret(hash, &old_write);
            self.write_keys = Some(TrafficKeys::new(suite, &updated_write));
            self.client_ap_secret = Some(Zeroizing::new(updated_write));
        }
        Ok(())
    }
}

/// Whether `host` looks like an IPv4/IPv6 literal rather than a DNS name
/// (RFC 6066 section 3: `server_name` is never sent for an IP-literal
/// host). `otter_x509::verify_server_chain` does the authoritative parsing
/// of `host` either way; this only decides whether to send SNI at all.
fn is_ip_literal(host: &str) -> bool {
    if host.contains(':') {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.len() <= 3 && part.bytes().all(|b| b.is_ascii_digit()) && part.parse::<u16>().is_ok_and(|v| v <= 255))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_literal_detection() {
        assert!(is_ip_literal("127.0.0.1"));
        assert!(is_ip_literal("203.0.113.42"));
        assert!(is_ip_literal("::1"));
        assert!(is_ip_literal("2001:db8::1"));
        assert!(!is_ip_literal("example.com"));
        assert!(!is_ip_literal("leaf.otter-test.example"));
        assert!(!is_ip_literal("999.999.999.999")); // out of range octets
        assert!(!is_ip_literal("1.2.3.4.5"));
    }
}
