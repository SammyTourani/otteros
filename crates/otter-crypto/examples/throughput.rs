//! Reports approximate throughput (MB/s) for every primitive in this crate, for
//! the record (brief M8-T1: "report MB/s" for the hash block function,
//! "throughput numbers reported for the record" overall; brief M8-T2: "throughput
//! for AES-128-GCM software and hardware paths"). Not a rigorous criterion-style
//! benchmark -- no warm-up isolation, no statistics -- just a single wall-clock
//! measurement over a large buffer, run with `--release`:
//!
//!   cd crates && cargo run -p otter-crypto --release --example throughput
//!   cd crates && cargo run -p otter-crypto --release --example throughput --target x86_64-apple-darwin

use otter_crypto::{Aes128Gcm, Sha256, Sha384, Sha512, poly1305::poly1305_mac, seal_in_place};
use std::time::Instant;

const SIZE: usize = 16 * 1024 * 1024; // 16 MiB

fn report(name: &str, bytes: usize, elapsed: std::time::Duration) {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    let secs = elapsed.as_secs_f64();
    println!("{name:<28} {mb:>8.1} MiB in {secs:>7.3} s -> {:>8.1} MB/s", mb / secs);
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
}
