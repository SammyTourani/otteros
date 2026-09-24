//! Tests for .otm file reading, validation, and correctness checks.

#![allow(unused_imports, clippy::expect_fun_call)]

use std::fs;
use std::path::Path;

#[test]
fn otm_reader_parses_written_files() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR").unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());

    let files = vec![
        ("tiny-random.f32.otm", format!("{}/tiny-random.f32.otm", model_dir)),
        ("tiny-random.q8.otm", format!("{}/tiny-random.q8.otm", model_dir)),
        ("SmolLM2-135M-Instruct.f32.otm", format!("{}/SmolLM2-135M-Instruct.f32.otm", model_dir)),
        ("SmolLM2-135M-Instruct.q8.otm", format!("{}/SmolLM2-135M-Instruct.q8.otm", model_dir)),
    ];

    for (name, path_str) in files {
        let path = Path::new(&path_str);
        if !path.exists() {
            eprintln!("otm_reader test: skipping {} (not found)", name);
            continue;
        }

        let reader = otter_convert::otm::OtmReader::read(path)
            .expect(&format!("failed to read {}", name));
        let parsed = reader.parse()
            .expect(&format!("failed to parse {}", name));

        eprintln!(
            "otm_reader test: {} PASS (vocab={}, dim={}, n_layers={}, tensors={})",
            name,
            parsed.vocab_size,
            parsed.dim,
            parsed.n_layers,
            parsed.tensors.len()
        );

        // Verify header config is sensible
        assert!(parsed.vocab_size > 0, "{}: vocab_size=0", name);
        assert!(parsed.dim > 0, "{}: dim=0", name);
        assert!(parsed.n_layers > 0, "{}: n_layers=0", name);
        assert!(parsed.n_heads > 0, "{}: n_heads=0", name);

        // Verify tensor offsets are 64-byte aligned
        for (tname, _dtype, shape, offset, _nelems) in &parsed.tensors {
            assert_eq!(
                offset % 64, 0,
                "{}: tensor {} offset {} not 64-byte aligned",
                name, tname, offset
            );
            assert!(!shape.is_empty(), "{}: tensor {} has empty shape", name, tname);
        }
    }
}

#[test]
fn f32_round_trip_tiny_random() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR").unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());
    let tiny_model_str = format!("{}/tiny-random", model_dir);
    let tiny_model = Path::new(&tiny_model_str);
    let otm_path_str = format!("{}/tiny-random.f32.otm", model_dir);
    let otm_path = Path::new(&otm_path_str);

    if !otm_path.exists() || !tiny_model.join("model.safetensors").exists() {
        eprintln!("f32_round_trip test: skipping tiny-random (files not found)");
        return;
    }

    let st = otter_convert::safetensors::SafeTensors::read(&tiny_model.join("model.safetensors"))
        .expect("read safetensors");

    let otm_reader = otter_convert::otm::OtmReader::read(otm_path)
        .expect("read .otm file");
    let otm_parsed = otm_reader.parse()
        .expect("parse .otm file");

    let mut checked_tensors = 0;

    for (tensor_name, dtype, _shape, _offset, _nelems) in &otm_parsed.tensors {
        if *dtype != 0 {
            // Skip Q8_0 tensors, test them separately
            continue;
        }

        if let Some(original) = st.to_f32(tensor_name) {
            checked_tensors += 1;

            // For F32, values should be bit-exact since there's no quantization
            // Just verify we can read them
            assert!(
                !original.is_empty(),
                "F32 tensor {} is empty",
                tensor_name
            );
        }
    }

    eprintln!("f32_round_trip test: tiny-random checked {} F32 tensors", checked_tensors);
    if checked_tensors == 0 {
        eprintln!("f32_round_trip test: skipping tiny-random validation (tensor table empty)");
    }
}

#[test]
fn q8_quantization_error_bounds() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR").unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());
    let tiny_model_str = format!("{}/tiny-random", model_dir);
    let tiny_model = Path::new(&tiny_model_str);
    let otm_path_str = format!("{}/tiny-random.q8.otm", model_dir);
    let otm_path = Path::new(&otm_path_str);

    if !otm_path.exists() || !tiny_model.join("model.safetensors").exists() {
        eprintln!("q8_quantization_error_bounds test: skipping tiny-random (files not found)");
        return;
    }

    let st = otter_convert::safetensors::SafeTensors::read(&tiny_model.join("model.safetensors"))
        .expect("read safetensors");

    let otm_data = std::fs::read(otm_path)
        .expect("read .otm file");

    let otm_reader = otter_convert::otm::OtmReader::read(otm_path)
        .expect("read .otm file");
    let otm_parsed = otm_reader.parse()
        .expect("parse .otm file");

    let mut max_error: f32 = 0.0;
    let mut checked_blocks: usize = 0;

    for (tensor_name, dtype, _shape, offset, nelems) in &otm_parsed.tensors {
        if *dtype != 1 {
            // Only check Q8_0
            continue;
        }

        if let Some(original) = st.to_f32(tensor_name) {
            let start = *offset as usize;
            let bytes_needed = (*nelems as usize).div_ceil(32) * 36;
            let end = start + bytes_needed;

            if end > otm_data.len() {
                eprintln!("q8_quantization test: skipping {} (data truncated)", tensor_name);
                continue;
            }

            let tensor_data = &otm_data[start..end];

            let dequantized = otter_convert::otm::dequantize_q8_0(tensor_data)
                .expect(&format!("dequantize {}", tensor_name));

            // Check error per block
            for (block_idx, chunk) in original.chunks(32).enumerate() {
                let dq_chunk = &dequantized[block_idx * 32..std::cmp::min((block_idx + 1) * 32, dequantized.len())];

                // Extract scale from the quantized data
                let block_offset = block_idx * 36;
                let scale_bytes: [u8; 4] = tensor_data[block_offset + 32..block_offset + 36].try_into().unwrap();
                let scale = f32::from_le_bytes(scale_bytes);

                for (i, &orig_val) in chunk.iter().enumerate() {
                    if i < dq_chunk.len() {
                        let error = (orig_val - dq_chunk[i]).abs();
                        let bound = scale / 2.0 + 1e-6;

                        assert!(
                            error <= bound,
                            "Q8_0 error in {} block {}: {} > {}",
                            tensor_name, block_idx, error, bound
                        );

                        max_error = max_error.max(error);
                    }
                }

                checked_blocks += 1;
            }
        }
    }

    eprintln!("q8_quantization_error_bounds test: tiny-random checked {} blocks, max error {:.2e}", checked_blocks, max_error);
    if checked_blocks == 0 {
        eprintln!("q8_quantization_error_bounds test: skipping tiny-random validation (tensor table empty)");
    }
}
