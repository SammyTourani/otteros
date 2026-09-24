//! Reports approximate throughput (MB/s) for every primitive in this crate, for
//! the record (brief M8-T1: "report MB/s" for the hash block function,
//! "throughput numbers reported for the record" overall; brief M8-T2: "throughput
//! for AES-128-GCM software and hardware paths"; brief M8-T3: "verification
//! timings for RSA-2048, RSA-4096, P-256 and P-384 reported for the record").
//! Not a rigorous criterion-style benchmark -- no warm-up isolation, no
//! statistics -- just a single wall-clock measurement over a large buffer (or,
//! for the signature-verification timings, a repeated single-signature
//! verification), run with `--release`:
//!
//!   cd crates && cargo run -p otter-crypto --release --example throughput
//!   cd crates && cargo run -p otter-crypto --release --example throughput --target x86_64-apple-darwin
//!
//! The signature-verification timings read one real "valid" case out of the
//! Wycheproof-derived fixtures under `tests/vectors/` (run
//! `../scripts/gen-crypto-vectors.py` first if they are missing), the same
//! data brief M8-T3's own correctness tests use, rather than inventing
//! throwaway key material.

use otter_crypto::hash::Digest;
use otter_crypto::sha512::Sha384;
use otter_crypto::{Aes128Gcm, Sha256, Sha512, poly1305::poly1305_mac, seal_in_place};
use std::time::Instant;

const SIZE: usize = 16 * 1024 * 1024; // 16 MiB

fn report(name: &str, bytes: usize, elapsed: std::time::Duration) {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    let secs = elapsed.as_secs_f64();
    println!("{name:<28} {mb:>8.1} MiB in {secs:>7.3} s -> {:>8.1} MB/s", mb / secs);
}

fn report_verify(name: &str, iterations: u32, elapsed: std::time::Duration) {
    let per_op_ms = elapsed.as_secs_f64() * 1000.0 / iterations as f64;
    println!("{name:<28} {iterations:>6} verifies in {:>7.3} s -> {per_op_ms:>8.3} ms/verify", elapsed.as_secs_f64());
}

/// The first `result == "valid"` row of a `tests/vectors/*.txt` fixture
/// (brief M8-T3's own compact pipe-delimited format -- see
/// `tests/common/mod.rs`'s `read_fixture` for the format this mirrors).
fn read_first_valid_row(fixture_name: &str) -> Vec<String> {
    let path = format!("{}/tests/vectors/{fixture_name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {path} ({e}); run scripts/gen-crypto-vectors.py first"));
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('|').map(str::to_string).collect::<Vec<_>>())
        .find(|fields| fields.get(1).map(String::as_str) == Some("valid"))
        .unwrap_or_else(|| panic!("no valid case found in {path}"))
}

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn main() {
    let data = vec![0x5au8; SIZE];

    let start = Instant::now();
    let _ = Sha256::digest(&data);
    report("SHA-256", SIZE, start.elapsed());

    let start = Instant::now();
    let _ = Sha384::digest(&data);
    report("SHA-384", SIZE, start.elapsed());

    let start = Instant::now();
    let _ = Sha512::digest(&data);
    report("SHA-512", SIZE, start.elapsed());

    let key = [0x11u8; 32];
    let nonce = [0x22u8; 12];
    let mut buf = data.clone();
    let start = Instant::now();
    otter_crypto::chacha20::xor_keystream(&key, &nonce, 0, &mut buf);
    report("ChaCha20 keystream", SIZE, start.elapsed());

    let start = Instant::now();
    let _ = poly1305_mac(&key, &data);
    report("Poly1305", SIZE, start.elapsed());

    let mut aead_buf = data.clone();
    let start = Instant::now();
    let _ = seal_in_place(&key, &nonce, b"", &mut aead_buf);
    report("ChaCha20-Poly1305 seal", SIZE, start.elapsed());

    let gcm_key = [0x33u8; 16];
    let sw_gcm = Aes128Gcm::new_software(&gcm_key);
    let mut gcm_buf = data.clone();
    let start = Instant::now();
    let _ = sw_gcm.seal_in_place(&nonce, b"", &mut gcm_buf);
    report("AES-128-GCM seal (software)", SIZE, start.elapsed());

    match Aes128Gcm::new_hardware(&gcm_key) {
        Some(hw_gcm) => {
            let mut gcm_buf = data.clone();
            let start = Instant::now();
            let _ = hw_gcm.seal_in_place(&nonce, b"", &mut gcm_buf);
            report("AES-128-GCM seal (AES-NI/PCLMULQDQ)", SIZE, start.elapsed());
        }
        None => println!("AES-128-GCM seal (hardware)     -- AES-NI/PCLMULQDQ not available on this host"),
    }

    // brief M8-T3: RSA-2048/4096 PKCS1v15 and P-256/P-384 ECDSA verification
    // timings, each against a real Wycheproof "valid" case.
    for (fixture, name, key_size) in [
        ("rsa_pkcs1v15_2048_sha256_wycheproof.txt", "RSA-2048 PKCS1v15 verify", 2048),
        ("rsa_pkcs1v15_4096_sha256_wycheproof.txt", "RSA-4096 PKCS1v15 verify", 4096),
    ] {
        let row = read_first_valid_row(fixture);
        let n = hex_decode(&row[2]);
        let e = hex_decode(&row[3]);
        let msg = hex_decode(&row[4]);
        let sig = hex_decode(&row[5]);
        let key = otter_crypto::rsa::PublicKey::from_components(&n, &e)
            .unwrap_or_else(|err| panic!("{key_size}-bit fixture key rejected: {err:?}"));
        let digest = Digest::from_slice(&Sha256::digest(&msg));

        let iterations = 50;
        let start = Instant::now();
        for _ in 0..iterations {
            key.verify_pkcs1v15(otter_crypto::rsa::HashAlg::Sha256, &digest, &sig)
                .expect("fixture signature is valid");
        }
        report_verify(name, iterations, start.elapsed());
    }

    {
        let row = read_first_valid_row("ecdsa_p256_sha256_wycheproof.txt");
        let pubkey = hex_decode(&row[3]);
        let msg = hex_decode(&row[4]);
        let sig = hex_decode(&row[5]);
        let key = otter_crypto::p256::PublicKey::from_sec1(&pubkey).expect("fixture key is on-curve");
        let digest = Sha256::digest(&msg);

        let iterations = 20;
        let start = Instant::now();
        for _ in 0..iterations {
            key.verify(&digest, &sig).expect("fixture signature is valid");
        }
        report_verify("P-256 ECDSA verify", iterations, start.elapsed());
    }

    {
        let row = read_first_valid_row("ecdsa_p384_sha384_wycheproof.txt");
        let pubkey = hex_decode(&row[3]);
        let msg = hex_decode(&row[4]);
        let sig = hex_decode(&row[5]);
        let key = otter_crypto::p384::PublicKey::from_sec1(&pubkey).expect("fixture key is on-curve");
        let digest = Sha384::digest(&msg);

        let iterations = 20;
        let start = Instant::now();
        for _ in 0..iterations {
            key.verify(&digest, &sig).expect("fixture signature is valid");
        }
        report_verify("P-384 ECDSA verify", iterations, start.elapsed());
    }
}
