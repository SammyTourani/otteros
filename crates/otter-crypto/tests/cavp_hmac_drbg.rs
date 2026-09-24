//! NIST CAVP HMAC_DRBG test vectors for SHA-256.
//! Parses and validates the test vectors from the CAVP suite.

use otter_crypto::HmacDrbg;
use std::fs;

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join("")
}

#[derive(Debug)]
struct CavpTestCase {
    count: u32,
    entropy_input: Vec<u8>,
    nonce: Vec<u8>,
    personalization_string: Vec<u8>,
    entropy_input_reseed: Vec<u8>,
    additional_input_reseed: Vec<u8>,
    additional_inputs_seen: usize,
    additional_input_gen1: Vec<u8>,
    returned_bits: Vec<u8>,
    additional_input_gen2: Vec<u8>,
    // Intermediate state values from CAVP file
    v_after_instantiate: Option<Vec<u8>>,
    key_after_instantiate: Option<Vec<u8>>,
    v_after_reseed: Option<Vec<u8>>,
    key_after_reseed: Option<Vec<u8>>,
    v_after_generate1: Option<Vec<u8>>,
    key_after_generate1: Option<Vec<u8>>,
}

fn parse_cavp_file(content: &str) -> Vec<CavpTestCase> {
    let mut cases = Vec::new();
    let mut current = None;
    let mut current_section = "";  // Track which section we're in: "INSTANTIATE", "RESEED", "GENERATE"

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("COUNT = ") {
            if let Some(case) = current {
                cases.push(case);
            }
            let count = trimmed.strip_prefix("COUNT = ").unwrap().parse().unwrap();
            current = Some(CavpTestCase {
                count,
                entropy_input: Vec::new(),
                nonce: Vec::new(),
                personalization_string: Vec::new(),
                entropy_input_reseed: Vec::new(),
                additional_input_reseed: Vec::new(),
                additional_inputs_seen: 0,
                additional_input_gen1: Vec::new(),
                returned_bits: Vec::new(),
                additional_input_gen2: Vec::new(),
                v_after_instantiate: None,
                key_after_instantiate: None,
                v_after_reseed: None,
                key_after_reseed: None,
                v_after_generate1: None,
                key_after_generate1: None,
            });
            current_section = "";
        } else if let Some(ref mut case) = current {
            if trimmed.starts_with("EntropyInput = ") {
                let hex_str = trimmed.strip_prefix("EntropyInput = ").unwrap();
                case.entropy_input = if hex_str.is_empty() { Vec::new() } else { hex_decode(hex_str) };
            } else if trimmed.starts_with("Nonce = ") {
                let hex_str = trimmed.strip_prefix("Nonce = ").unwrap();
                case.nonce = if hex_str.is_empty() { Vec::new() } else { hex_decode(hex_str) };
            } else if trimmed.starts_with("PersonalizationString = ") {
                let hex_str = trimmed.strip_prefix("PersonalizationString = ").unwrap();
                case.personalization_string = if hex_str.is_empty() { Vec::new() } else { hex_decode(hex_str) };
            } else if trimmed.starts_with("EntropyInputReseed = ") {
                let hex_str = trimmed.strip_prefix("EntropyInputReseed = ").unwrap();
                case.entropy_input_reseed = if hex_str.is_empty() { Vec::new() } else { hex_decode(hex_str) };
            } else if trimmed.starts_with("AdditionalInputReseed = ") {
                let hex_str = trimmed.strip_prefix("AdditionalInputReseed = ").unwrap();
                case.additional_input_reseed = if hex_str.is_empty() { Vec::new() } else { hex_decode(hex_str) };
            } else if trimmed.starts_with("** INSTANTIATE:") {
                current_section = "INSTANTIATE";
            } else if trimmed.starts_with("** RESEED:") {
                current_section = "RESEED";
            } else if trimmed.starts_with("** GENERATE (FIRST CALL):") {
                current_section = "GENERATE_1";
            } else if trimmed.starts_with("** GENERATE (SECOND CALL):") {
                current_section = "GENERATE_2";
            } else if trimmed.starts_with("V   = ") && current_section == "INSTANTIATE" {
                let hex_str = trimmed.strip_prefix("V   = ").unwrap();
                case.v_after_instantiate = Some(hex_decode(hex_str));
            } else if trimmed.starts_with("Key = ") && current_section == "INSTANTIATE" {
                let hex_str = trimmed.strip_prefix("Key = ").unwrap();
                case.key_after_instantiate = Some(hex_decode(hex_str));
            } else if trimmed.starts_with("V   = ") && current_section == "RESEED" {
                let hex_str = trimmed.strip_prefix("V   = ").unwrap();
                case.v_after_reseed = Some(hex_decode(hex_str));
            } else if trimmed.starts_with("Key = ") && current_section == "RESEED" {
                let hex_str = trimmed.strip_prefix("Key = ").unwrap();
                case.key_after_reseed = Some(hex_decode(hex_str));
            } else if trimmed.starts_with("V   = ") && current_section == "GENERATE_1" {
                let hex_str = trimmed.strip_prefix("V   = ").unwrap();
                case.v_after_generate1 = Some(hex_decode(hex_str));
            } else if trimmed.starts_with("Key = ") && current_section == "GENERATE_1" {
                let hex_str = trimmed.strip_prefix("Key = ").unwrap();
                case.key_after_generate1 = Some(hex_decode(hex_str));
            } else if trimmed.starts_with("AdditionalInput = ") {
                let hex_str = trimmed.strip_prefix("AdditionalInput = ").unwrap();
                let value = if hex_str.is_empty() { Vec::new() } else { hex_decode(hex_str) };
                // The first AdditionalInput line of a case (printed before the "GENERATE (FIRST
                // CALL)" trace) belongs to the first generate call, the second one to the second.
                if case.additional_inputs_seen == 0 {
                    case.additional_input_gen1 = value;
                } else {
                    case.additional_input_gen2 = value;
                }
                case.additional_inputs_seen += 1;
            } else if trimmed.starts_with("ReturnedBits = ") {
                let hex_str = trimmed.strip_prefix("ReturnedBits = ").unwrap();
                case.returned_bits = if hex_str.is_empty() { Vec::new() } else { hex_decode(hex_str) };
            }
        }
    }

    if let Some(case) = current {
        cases.push(case);
    }

    cases
}

#[test]
fn cavp_hmac_drbg_sha256() {
    let content = fs::read_to_string("tests/vectors/cavp_hmac_drbg_sha256.txt")
        .expect("Could not read CAVP test vectors");

    let cases = parse_cavp_file(&content);
    println!("Running {} CAVP test cases", cases.len());

    let mut passed = 0;
    let mut failed = 0;

    for (idx, case) in cases.iter().enumerate() {
        if idx == 0 {
            // Print details for first test case for debugging
            println!("Test case 0 details:");
            println!("  EntropyInput:       {}", hex(&case.entropy_input));
            println!("  Nonce:              {}", hex(&case.nonce));
            println!("  PersonalizationStr: {}", hex(&case.personalization_string));
            println!("  EntropyReseed:      {}", hex(&case.entropy_input_reseed));
            println!("  AddInputReseed:     {}", hex(&case.additional_input_reseed));
            println!("  AddInputGen1:       {}", hex(&case.additional_input_gen1));
            println!("  Expected bytes:     {}", case.returned_bits.len());
        }

        // Instantiate
        let mut drbg = HmacDrbg::instantiate(&case.entropy_input, &case.nonce, &case.personalization_string);

        // Check state after instantiate
        if idx == 0 {
            let (key, v) = drbg.test_state();
            println!("\nAfter INSTANTIATE:");
            if let Some(ref exp_v) = case.v_after_instantiate {
                let match_v = v == &exp_v[..];
                println!("  V:   {} (expected: {}) {}", hex(v), hex(exp_v), if match_v { "✓" } else { "✗" });
            }
            if let Some(ref exp_key) = case.key_after_instantiate {
                let match_key = key == &exp_key[..];
                println!("  Key: {} (expected: {}) {}", hex(key), hex(exp_key), if match_key { "✓" } else { "✗" });
            }
        }

        // Reseed
        drbg.reseed(&case.entropy_input_reseed, &case.additional_input_reseed);

        // Check state after reseed
        if idx == 0 {
            let (key, v) = drbg.test_state();
            println!("\nAfter RESEED:");
            if let Some(ref exp_v) = case.v_after_reseed {
                let match_v = v == &exp_v[..];
                println!("  V:   {} (expected: {}) {}", hex(v), hex(exp_v), if match_v { "✓" } else { "✗" });
            }
            if let Some(ref exp_key) = case.key_after_reseed {
                let match_key = key == &exp_key[..];
                println!("  Key: {} (expected: {}) {}", hex(key), hex(exp_key), if match_key { "✓" } else { "✗" });
            }
        }

        // First generate - discard output
        let mut discard_output = vec![0u8; case.returned_bits.len()];
        if drbg.generate(&mut discard_output, &case.additional_input_gen1).is_err() {
            eprintln!("Case {}: first generate failed", case.count);
            failed += 1;
            continue;
        }

        // Second generate - this is what we compare
        let mut output = vec![0u8; case.returned_bits.len()];
        if drbg.generate(&mut output, &case.additional_input_gen2).is_err() {
            eprintln!("Case {}: second generate failed", case.count);
            failed += 1;
            continue;
        }

        // Check second generate output
        if output == case.returned_bits {
            passed += 1;
        } else {
            if idx < 3 {
                eprintln!(
                    "Case {}: Second generate mismatch\nExpected: {}\nGot:      {}",
                    case.count,
                    hex(&case.returned_bits),
                    hex(&output)
                );
            }
            failed += 1;
        }
    }

    println!("CAVP Results: {} passed, {} failed", passed, failed);
    assert_eq!(failed, 0, "All CAVP test cases should pass");
    assert!(passed >= 60 && passed == cases.len(), "expected >= 60 parsed cases, all passing; got {passed}/{}", cases.len());
}
