//! Record size limits (RFC 8446 section 5.2, brief M8-T5's record-layer
//! design point): `2^14` bytes of plaintext, `2^14 + 256` bytes of
//! ciphertext. Exercised directly against the sans-I/O state machine (no
//! socket needed) by handing it hand-built oversized records.

mod common;

use common::OsRng;
use otter_tls::{ClientConfig, ClientConnection, TlsError};

fn fresh_conn() -> ClientConnection {
    let config = ClientConfig::new(otter_x509::TrustStore::empty());
    let mut conn = ClientConnection::new(config, "example.test", 1_748_736_000, &mut OsRng::new()).unwrap();
    let _ = conn.take_outgoing(); // discard the ClientHello; irrelevant to this test.
    conn
}

fn record(content_type: u8, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + len);
    out.push(content_type);
    out.extend_from_slice(&[0x03, 0x03]);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.resize(5 + len, 0);
    out
}

#[test]
fn a_record_declaring_more_than_the_ciphertext_maximum_is_rejected_outright() {
    let mut conn = fresh_conn();
    // 2^14 + 256 + 1: over the absolute maximum RFC 8446 section 5.2 allows
    // for *any* record, encrypted or not, so this is rejected before this
    // client even considers whether it has read keys yet.
    conn.read_tls(&record(22, 16384 + 256 + 1));
    let err = conn.process().expect_err("an oversized record must be rejected");
    assert_eq!(err, TlsError::RecordOverflow);
    assert!(conn.is_closed());
}

#[test]
fn an_unencrypted_record_over_the_plaintext_maximum_is_rejected() {
    let mut conn = fresh_conn();
    // Under the absolute (ciphertext) maximum, but no read keys are
    // installed yet (no ServerHello has been processed), so RFC 8446
    // section 5.1's 2^14 plaintext limit applies instead of the more
    // generous encrypted-record one.
    conn.read_tls(&record(22, 16384 + 1));
    let err = conn.process().expect_err("an oversized plaintext record must be rejected");
    assert_eq!(err, TlsError::RecordOverflow);
}

#[test]
fn exactly_the_plaintext_maximum_is_not_by_itself_an_overflow() {
    let mut conn = fresh_conn();
    // Exactly 2^14 bytes: a handshake-message header claiming a body far
    // longer than what actually follows, so this can never complete a
    // message -- proving the record itself was accepted (the size boundary
    // is `> 2^14`, not `>= 2^14`) and the state machine correctly reports
    // "no progress yet" rather than any overflow or parse error.
    let mut body = vec![0u8; 16384];
    body[0] = 1; // an arbitrary handshake message type
    body[1..4].copy_from_slice(&[0xff, 0xff, 0xff]); // declared length: far more than follows
    let mut rec = vec![22, 0x03, 0x03];
    rec.extend_from_slice(&(body.len() as u16).to_be_bytes());
    rec.extend_from_slice(&body);

    conn.read_tls(&rec);
    assert!(matches!(conn.process(), Ok(None)));
    assert!(!conn.is_closed());
}
