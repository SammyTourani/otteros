//! A minimal safetensors reader (BF16/F16/F32) and writer (F32 only, for
//! `--tiny-random`'s synthetic checkpoints), from scratch -- zero
//! dependencies (DECISIONS.md D27). Format: an 8-byte little-endian header
//! length, that many bytes of UTF-8 JSON (a `{tensor_name: {dtype, shape,
//! data_offsets}}` map, plus an optional non-tensor `__metadata__` entry),
//! then the raw tensor bytes back-to-back.

use std::collections::BTreeMap;

use otter_json::Value;

use crate::half::{bf16_to_f32, f16_to_f32};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceDtype {
    F32,
    F16,
    Bf16,
}

impl SourceDtype {
    fn from_str(s: &str) -> Result<SourceDtype, String> {
        match s {
            "F32" => Ok(SourceDtype::F32),
            "F16" => Ok(SourceDtype::F16),
            "BF16" => Ok(SourceDtype::Bf16),
            other => Err(format!("unsupported safetensors dtype \"{other}\" (only F32/F16/BF16 are read)")),
        }
    }

    fn element_size(self) -> usize {
        match self {
            SourceDtype::F32 => 4,
            SourceDtype::F16 | SourceDtype::Bf16 => 2,
        }
    }
}

#[allow(dead_code)]
pub struct TensorInfo {
    pub dtype: SourceDtype,
    pub shape: Vec<u32>,
    data_offsets: (usize, usize),
}

/// A parsed safetensors file: the header's tensor descriptors, in file
/// order, plus a reference to the raw data block they index into.
#[allow(dead_code)]
pub struct SafeTensors {
    order: Vec<String>,
    tensors: BTreeMap<String, TensorInfo>,
    data: Vec<u8>,
}

impl SafeTensors {
    pub fn read(path: &std::path::Path) -> Result<SafeTensors, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(bytes)
    }

    pub fn parse(bytes: Vec<u8>) -> Result<SafeTensors, String> {
        if bytes.len() < 8 {
            return Err("file too short to contain a safetensors header length".into());
        }
        let header_len = u64::from_le_bytes(bytes[0..8].try_into().unwrap()) as usize;
        let header_start: usize = 8;
        let header_end = header_start
            .checked_add(header_len)
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| "safetensors header length runs past end of file".to_string())?;
        let header_json = std::str::from_utf8(&bytes[header_start..header_end])
            .map_err(|e| format!("safetensors header is not valid UTF-8: {e}"))?;
        let header = otter_json::parse_str(header_json).map_err(|e| format!("safetensors header JSON: {e}"))?;
        let obj = header.as_object().ok_or("safetensors header is not a JSON object")?;

        let mut order = Vec::new();
        let mut tensors = BTreeMap::new();
        for (name, entry) in obj.iter() {
            if name == "__metadata__" {
                continue;
            }
            let dtype_str = entry.get("dtype").and_then(Value::as_str).ok_or_else(|| format!("tensor {name}: missing dtype"))?;
            let dtype = SourceDtype::from_str(dtype_str).map_err(|e| format!("tensor {name}: {e}"))?;
            let shape: Vec<u32> = entry
                .get("shape")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("tensor {name}: missing shape"))?
                .iter()
                .map(|v| v.as_u64().map(|n| n as u32).ok_or_else(|| format!("tensor {name}: non-integer shape entry")))
                .collect::<Result<_, _>>()?;
            let offsets = entry.get("data_offsets").and_then(Value::as_array).ok_or_else(|| format!("tensor {name}: missing data_offsets"))?;
            if offsets.len() != 2 {
                return Err(format!("tensor {name}: data_offsets must have exactly 2 entries"));
            }
            let start = offsets[0].as_u64().ok_or_else(|| format!("tensor {name}: bad data_offsets[0]"))? as usize;
            let end = offsets[1].as_u64().ok_or_else(|| format!("tensor {name}: bad data_offsets[1]"))? as usize;
            let expected_len = shape.iter().map(|&d| d as usize).product::<usize>() * dtype.element_size();
            if end < start || end - start != expected_len {
                return Err(format!(
                    "tensor {name}: data_offsets span {} bytes but shape*dtype needs {expected_len}",
                    end.saturating_sub(start)
                ));
            }
            order.push(name.to_string());
            tensors.insert(name.to_string(), TensorInfo { dtype, shape, data_offsets: (start, end) });
        }
        order.sort();

        let data = bytes[header_end..].to_vec();
        Ok(SafeTensors { order, tensors, data })
    }

    pub fn names(&self) -> &[String] {
        &self.order
    }

    #[allow(dead_code)]
    pub fn info(&self, name: &str) -> Option<&TensorInfo> {
        self.tensors.get(name)
    }

    /// Raw bytes for `name` in its original on-disk dtype (used by the
    /// bit-exact round-trip self-check, which re-derives the expected f32
    /// values straight from these bytes rather than trusting any
    /// intermediate cached copy).
    #[allow(dead_code)]
    pub fn raw_bytes(&self, name: &str) -> Option<&[u8]> {
        let info = self.tensors.get(name)?;
        Some(&self.data[info.data_offsets.0..info.data_offsets.1])
    }

    /// The tensor's values widened to f32 (BF16/F16 widening is always
    /// exact -- see `half.rs`).
    #[allow(dead_code)]
    pub fn to_f32(&self, name: &str) -> Option<Vec<f32>> {
        let info = self.tensors.get(name)?;
        let bytes = &self.data[info.data_offsets.0..info.data_offsets.1];
        Some(match info.dtype {
            SourceDtype::F32 => {
                let mut result = Vec::with_capacity(bytes.len() / 4);
                for chunk in bytes.chunks(4) {
                    if chunk.len() == 4 {
                        result.push(f32::from_le_bytes(chunk.try_into().unwrap()));
                    }
                }
                result
            }
            SourceDtype::F16 => {
                let mut result = Vec::with_capacity(bytes.len() / 2);
                for chunk in bytes.chunks(2) {
                    if chunk.len() == 2 {
                        result.push(f16_to_f32(u16::from_le_bytes(chunk.try_into().unwrap())));
                    }
                }
                result
            }
            SourceDtype::Bf16 => {
                let mut result = Vec::with_capacity(bytes.len() / 2);
                for chunk in bytes.chunks(2) {
                    if chunk.len() == 2 {
                        result.push(bf16_to_f32(u16::from_le_bytes(chunk.try_into().unwrap())));
                    }
                }
                result
            }
        })
    }
}

/// Writes a minimal, valid F32-only safetensors file (used by
/// `--tiny-random` to produce a checkpoint `transformers` can load
/// directly). `tensors` is `(name, shape, data)` in the order they should
/// appear in the header (safetensors itself doesn't require or guarantee
/// any particular order; sorted-by-name is simplest and deterministic).
#[allow(dead_code)]
pub fn write_f32(path: &std::path::Path, tensors: &[(String, Vec<u32>, Vec<f32>)]) -> Result<(), String> {
    let mut sorted: Vec<&(String, Vec<u32>, Vec<f32>)> = tensors.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut header = otter_json::Object::new();
    let mut offset = 0u64;
    let mut data = Vec::new();
    for (name, shape, values) in &sorted {
        let nbytes = (values.len() * 4) as u64;
        let mut entry = otter_json::Object::new();
        entry.insert("dtype".into(), Value::string("F32"));
        entry.insert("shape".into(), Value::Array(shape.iter().map(|&d| Value::from(u64::from(d))).collect()));
        entry.insert(
            "data_offsets".into(),
            Value::Array(vec![Value::from(offset), Value::from(offset + nbytes)]),
        );
        header.insert(name.to_string(), Value::Object(entry));
        for v in values.iter() {
            data.extend_from_slice(&v.to_le_bytes());
        }
        offset += nbytes;
    }
    let header_json = otter_json::to_string(&Value::Object(header));
    let header_bytes = header_json.as_bytes();

    let mut out = Vec::with_capacity(8 + header_bytes.len() + data.len());
    out.extend_from_slice(&(header_bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(header_bytes);
    out.extend_from_slice(&data);
    std::fs::write(path, out).map_err(|e| format!("writing {}: {e}", path.display()))
}
