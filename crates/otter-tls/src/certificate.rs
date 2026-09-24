//! Parses a `Certificate` handshake message body (RFC 8446 section 4.4.2)
//! into the raw DER certificates `otter_x509::verify_server_chain` wants,
//! leaf first.

use alloc::vec::Vec;

use crate::codec::Reader;
use crate::error::TlsError;

pub(crate) fn parse(body: &[u8]) -> Result<Vec<Vec<u8>>, TlsError> {
    let mut r = Reader::new(body);
    let context = r.opaque8()?;
    if !context.is_empty() {
        return Err(TlsError::Decode("Certificate.certificate_request_context must be empty (no post-handshake or client-certificate authentication)".into()));
    }
    let list_bytes = r.opaque24()?;
    if !r.is_empty() {
        return Err(TlsError::Decode("Certificate message has trailing bytes".into()));
    }

    let mut list_r = Reader::new(list_bytes);
    let mut certs = Vec::new();
    while !list_r.is_empty() {
        let cert_data = list_r.opaque24()?;
        if cert_data.is_empty() {
            return Err(TlsError::Decode("CertificateEntry.cert_data is empty".into()));
        }
        let _extensions = list_r.opaque16()?; // per-certificate extensions (e.g. OCSP staples): not in this client's scope.
        certs.push(cert_data.to_vec());
    }
    if certs.is_empty() {
        return Err(TlsError::Decode("Certificate message carried an empty certificate_list".into()));
    }
    Ok(certs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{write_opaque8, write_opaque16, write_opaque24};

    fn build_entry(out: &mut alloc::vec::Vec<u8>, cert: &[u8]) {
        write_opaque24(out, cert);
        write_opaque16(out, &[]);
    }

    #[test]
    fn parses_two_certificates() {
        let mut body = Vec::new();
        write_opaque8(&mut body, &[]);
        let mut list = Vec::new();
        build_entry(&mut list, b"leaf-cert-bytes");
        build_entry(&mut list, b"intermediate-cert-bytes");
        write_opaque24(&mut body, &list);

        let certs = parse(&body).unwrap();
        assert_eq!(certs, vec![b"leaf-cert-bytes".to_vec(), b"intermediate-cert-bytes".to_vec()]);
    }

    #[test]
    fn rejects_nonempty_context() {
        let mut body = Vec::new();
        write_opaque8(&mut body, &[0x01]);
        write_opaque24(&mut body, &[]);
        assert!(parse(&body).is_err());
    }

    #[test]
    fn rejects_empty_chain() {
        let mut body = Vec::new();
        write_opaque8(&mut body, &[]);
        write_opaque24(&mut body, &[]);
        assert!(parse(&body).is_err());
    }
}
