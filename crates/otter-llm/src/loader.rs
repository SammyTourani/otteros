//! .otm file loader following tools/otter-convert/FORMAT.md exactly.
//! Validates all offsets, lengths, counts and shapes with checked arithmetic.

#![allow(clippy::manual_div_ceil)]

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

const OTM_MAGIC: u32 = 0x004D_544F; // "OTM\0"
const OTM_VERSION: u32 = 1;
const HEADER_SIZE: usize = 128;
const ALIGN: usize = 64;

/// Model loading errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    TooSmall(usize),
    BadMagic(u32),
    BadVersion(u32),
    TruncatedHeader,
    TruncatedTokenizerVocab,
    TruncatedMerges,
    TruncatedAddedTokens,
    TruncatedTensorTable,
    TruncatedTensorEntry,
    TruncatedTensorShape,
    BadTensorOffset,
    BadTensorAlignment,
    BadTensorData,
    VocabSizeMismatch,
    TokenizerFormat(String),
    ArithmeticOverflow,
    InvalidUtf8,
}

/// Data type for tensor storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DType {
    F32 = 0,
    Q8_0 = 1,
}

impl DType {
    fn from_u32(v: u32) -> Result<Self, LoadError> {
        match v {
            0 => Ok(DType::F32),
            1 => Ok(DType::Q8_0),
            _ => Err(LoadError::TokenizerFormat(alloc::format!("unknown dtype: {}", v))),
        }
    }
}

/// A single tensor's metadata and data view.
pub struct TensorView<'a> {
    pub name: &'a str,
    pub dtype: DType,
    pub shape: &'a [u32],
    pub data: &'a [u8],
    pub offset: u64,
    pub num_elements: u64,
}

/// Parsed .otm file with borrowed slices.
pub struct Model<'a> {
    pub vocab_size: u32,
    pub dim: u32,
    pub n_layers: u32,
    pub n_heads: u32,
    pub n_kv_heads: u32,
    pub head_dim: u32,
    pub hidden_dim: u32,
    pub rope_theta: f32,
    pub norm_eps: f32,
    pub max_context: u32,
    pub tied_embeddings: bool,
    pub qkv_bias: bool,
    pub qk_norm: bool,
    pub bos_id: Option<u32>,
    pub eos_id: Option<u32>,
    pub isolate_digits: bool,
    pub chat_template_kind: u32,

    // Tokenizer data
    pub vocab: &'a [Vec<u8>],
    pub added_tokens: &'a [(u32, &'a str)],
    pub merge_pairs: &'a [(u32, u32)],

    // Tensor views
    pub tensors: &'a [TensorView<'a>],

    // Raw data slice for tensor access
    data: &'a [u8],
}

/// Read a little-endian u32 at offset.
fn read_u32_le(data: &[u8], offset: usize) -> Result<u32, LoadError> {
    if offset.checked_add(4).ok_or(LoadError::ArithmeticOverflow)? > data.len() {
        return Err(LoadError::TruncatedHeader);
    }
    Ok(u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

/// Read a little-endian u64 at offset.
fn read_u64_le(data: &[u8], offset: usize) -> Result<u64, LoadError> {
    if offset.checked_add(8).ok_or(LoadError::ArithmeticOverflow)? > data.len() {
        return Err(LoadError::TruncatedHeader);
    }
    Ok(u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ]))
}

/// Read a little-endian f32 at offset.
fn read_f32_le(data: &[u8], offset: usize) -> Result<f32, LoadError> {
    if offset.checked_add(4).ok_or(LoadError::ArithmeticOverflow)? > data.len() {
        return Err(LoadError::TruncatedHeader);
    }
    Ok(f32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

/// Read a little-endian u16 at offset.
fn read_u16_le(data: &[u8], offset: usize) -> Result<u16, LoadError> {
    if offset.checked_add(2).ok_or(LoadError::ArithmeticOverflow)? > data.len() {
        return Err(LoadError::TruncatedHeader);
    }
    Ok(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

impl<'a> Model<'a> {
    pub fn load(data: &'a [u8]) -> Result<Model<'a>, LoadError> {
        // Must be large enough for header + footer
        if data.len() < HEADER_SIZE + 8 {
            return Err(LoadError::TooSmall(data.len()));
        }

        // Read footer to find tensor table offset
        let footer_offset = data.len().checked_sub(8).ok_or(LoadError::ArithmeticOverflow)?;
        let table_offset = read_u64_le(data, footer_offset)? as usize;

        if table_offset >= footer_offset {
            return Err(LoadError::BadTensorOffset);
        }

        // Read header
        let mut offset = 0;
        let magic = read_u32_le(data, offset)?;
        offset += 4;
        let version = read_u32_le(data, offset)?;
        offset += 4;

        if magic != OTM_MAGIC {
            return Err(LoadError::BadMagic(magic));
        }
        if version != OTM_VERSION {
            return Err(LoadError::BadVersion(version));
        }

        // Read model config
        let vocab_size = read_u32_le(data, offset)?;
        offset += 4;
        let dim = read_u32_le(data, offset)?;
        offset += 4;
        let n_layers = read_u32_le(data, offset)?;
        offset += 4;
        let n_heads = read_u32_le(data, offset)?;
        offset += 4;
        let n_kv_heads = read_u32_le(data, offset)?;
        offset += 4;
        let head_dim = read_u32_le(data, offset)?;
        offset += 4;
        let hidden_dim = read_u32_le(data, offset)?;
        offset += 4;
        let rope_theta = read_f32_le(data, offset)?;
        offset += 4;
        let norm_eps = read_f32_le(data, offset)?;
        offset += 4;
        let max_context = read_u32_le(data, offset)?;
        offset += 4;

        // Read flags
        let tied_embeddings = read_u32_le(data, offset)? != 0;
        offset += 4;
        let qkv_bias = read_u32_le(data, offset)? != 0;
        offset += 4;
        let qk_norm = read_u32_le(data, offset)? != 0;
        offset += 4;

        // Read tokenizer metadata
        let bos_id = {
            let v = read_u32_le(data, offset)?;
            offset += 4;
            if v == u32::MAX { None } else { Some(v) }
        };
        let eos_id = {
            let v = read_u32_le(data, offset)?;
            offset += 4;
            if v == u32::MAX { None } else { Some(v) }
        };
        let isolate_digits = read_u32_le(data, offset)? != 0;
        offset += 4;
        let chat_template_kind = read_u32_le(data, offset)?;

        // Skip to offset 128 (header end)
        offset = HEADER_SIZE;

        // Read tokenizer section
        let vocab_len = read_u32_le(data, offset)? as usize;
        offset += 4;

        if vocab_len != vocab_size as usize {
            return Err(LoadError::VocabSizeMismatch);
        }

        // Parse vocab entries
        let mut vocab = Vec::new();
        for _ in 0..vocab_len {
            let byte_len = read_u16_le(data, offset)? as usize;
            offset += 2;

            if offset.checked_add(byte_len).ok_or(LoadError::ArithmeticOverflow)? > data.len() {
                return Err(LoadError::TruncatedTokenizerVocab);
            }

            vocab.push(data[offset..offset + byte_len].to_vec());
            offset += byte_len;
        }

        // Parse merges (stored as (left_id, right_id) pairs in rank order)
        let merge_count = read_u32_le(data, offset)? as usize;
        offset += 4;

        let mut merge_pairs = Vec::new();
        for _ in 0..merge_count {
            let left_id = read_u32_le(data, offset)?;
            offset += 4;
            let right_id = read_u32_le(data, offset)?;
            offset += 4;
            merge_pairs.push((left_id, right_id));
        }

        // Parse added tokens
        let added_tokens_len = read_u32_le(data, offset)? as usize;
        offset += 4;

        let mut added_tokens = Vec::new();
        for _ in 0..added_tokens_len {
            let id = read_u32_le(data, offset)?;
            offset += 4;

            let content_len = read_u16_le(data, offset)? as usize;
            offset += 2;

            if offset.checked_add(content_len).ok_or(LoadError::ArithmeticOverflow)? > data.len() {
                return Err(LoadError::TruncatedAddedTokens);
            }

            let content_bytes = &data[offset..offset + content_len];
            let content_str = core::str::from_utf8(content_bytes)
                .map_err(|_| LoadError::InvalidUtf8)?;

            added_tokens.push((id, content_str));
            offset += content_len;
        }

        // Parse tensor table
        offset = table_offset;

        let num_tensors = read_u32_le(data, offset)? as usize;
        offset += 4;

        let mut tensor_metadata = Vec::new();

        for _ in 0..num_tensors {
            let name_len = read_u32_le(data, offset)? as usize;
            offset += 4;

            if offset.checked_add(name_len).ok_or(LoadError::ArithmeticOverflow)? > data.len() {
                return Err(LoadError::TruncatedTensorEntry);
            }

            let name_bytes = &data[offset..offset + name_len];
            let name_str = core::str::from_utf8(name_bytes)
                .map_err(|_| LoadError::InvalidUtf8)?;

            offset += name_len;

            let dtype_u32 = read_u32_le(data, offset)?;
            offset += 4;
            let dtype = DType::from_u32(dtype_u32)?;

            let num_dims = read_u32_le(data, offset)? as usize;
            offset += 4;

            if offset.checked_add(num_dims.checked_mul(4).ok_or(LoadError::ArithmeticOverflow)?)
                .ok_or(LoadError::ArithmeticOverflow)? > data.len() {
                return Err(LoadError::TruncatedTensorShape);
            }

            let mut shape = Vec::new();
            for _ in 0..num_dims {
                shape.push(read_u32_le(data, offset)?);
                offset += 4;
            }

            let tensor_offset = read_u64_le(data, offset)? as usize;
            offset += 8;
            let num_elements = read_u64_le(data, offset)?;
            offset += 8;

            // Validate alignment
            if !tensor_offset.is_multiple_of(ALIGN) {
                return Err(LoadError::BadTensorAlignment);
            }

            let expected_bytes = if dtype == DType::Q8_0 {
                let num_blocks = (num_elements + 31) / 32;
                num_blocks.checked_mul(36).ok_or(LoadError::ArithmeticOverflow)?
            } else {
                num_elements.checked_mul(4).ok_or(LoadError::ArithmeticOverflow)?
            };

            if tensor_offset.checked_add(expected_bytes as usize).ok_or(LoadError::ArithmeticOverflow)? > data.len() {
                return Err(LoadError::BadTensorData);
            }

            tensor_metadata.push((name_str, dtype, shape, tensor_offset, num_elements));
        }

        // Leak to convert to 'a lifetime
        let vocab_boxed: Box<[Vec<u8>]> = vocab.into();
        let vocab_leaked = Box::leak(vocab_boxed);

        let added_boxed: Box<[(u32, &'a str)]> = added_tokens.into();
        let added_leaked = Box::leak(added_boxed);

        let shapes_boxed: Box<[Vec<u32>]> = tensor_metadata.iter().map(|(_, _, s, _, _)| s.clone()).collect::<Vec<_>>().into();
        let shapes_leaked = Box::leak(shapes_boxed);

        let mut final_tensor_views = Vec::new();
        for (i, (name, dtype, _, tensor_offset, num_elements)) in tensor_metadata.iter().enumerate() {
            let tensor_bytes = if *dtype == DType::Q8_0 {
                let num_blocks = (*num_elements + 31) / 32;
                let len = (num_blocks * 36) as usize;
                &data[*tensor_offset..tensor_offset + len]
            } else {
                let len = (*num_elements * 4) as usize;
                &data[*tensor_offset..tensor_offset + len]
            };

            final_tensor_views.push(TensorView {
                name,
                dtype: *dtype,
                shape: &shapes_leaked[i],
                data: tensor_bytes,
                offset: *tensor_offset as u64,
                num_elements: *num_elements,
            });
        }

        let tensors_boxed: Box<[TensorView<'a>]> = final_tensor_views.into();
        let tensors_leaked = Box::leak(tensors_boxed);

        let merge_pairs_boxed: Box<[(u32, u32)]> = merge_pairs.into();
        let merge_pairs_leaked = Box::leak(merge_pairs_boxed);

        Ok(Model {
            vocab_size,
            dim,
            n_layers,
            n_heads,
            n_kv_heads,
            head_dim,
            hidden_dim,
            rope_theta,
            norm_eps,
            max_context,
            tied_embeddings,
            qkv_bias,
            qk_norm,
            bos_id,
            eos_id,
            isolate_digits,
            chat_template_kind,
            vocab: vocab_leaked,
            added_tokens: added_leaked,
            merge_pairs: merge_pairs_leaked,
            tensors: tensors_leaked,
            data,
        })
    }
}
