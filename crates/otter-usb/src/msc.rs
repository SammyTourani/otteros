use alloc::string::String;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CswStatus {
    Passed,
    Failed,
    PhaseError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CswError {
    WrongLength,
    BadSignature,
    TagMismatch,
    BadStatus,
    ResidueTooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Csw {
    pub tag: u32,
    pub residue: u32,
    pub status: CswStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capacity {
    pub last_lba: u64,
    pub block_size: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sense {
    pub key: u8,
    pub asc: u8,
    pub ascq: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inquiry {
    pub qualifier: u8,
    pub peripheral_type: u8,
    pub removable: bool,
    pub vendor: String,
    pub product: String,
    pub revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cbw {
    pub tag: u32,
    pub data_length: u32,
    pub direction_in: bool,
    pub lun: u8,
    pub cb: [u8; 16],
    pub cb_len: u8,
}

impl Cbw {
    pub fn to_bytes(&self) -> [u8; 31] {
        let mut b = [0u8; 31];
        b[0..4].copy_from_slice(b"USBC");
        b[4..8].copy_from_slice(&self.tag.to_le_bytes());
        b[8..12].copy_from_slice(&self.data_length.to_le_bytes());
        b[12] = if self.direction_in { 0x80 } else { 0x00 };
        b[13] = self.lun;
        b[14] = self.cb_len;
        b[15..31].copy_from_slice(&self.cb);
        b
    }
}

pub fn test_unit_ready(tag: u32, lun: u8) -> Cbw {
    Cbw {
        tag,
        data_length: 0,
        direction_in: false,
        lun,
        cb: [0u8; 16],
        cb_len: 6,
    }
}

pub fn request_sense(tag: u32, lun: u8) -> Cbw {
    let mut cb = [0u8; 16];
    cb[0] = 0x03;
    cb[4] = 18;
    Cbw {
        tag,
        data_length: 18,
        direction_in: true,
        lun,
        cb,
        cb_len: 6,
    }
}

pub fn inquiry(tag: u32, lun: u8) -> Cbw {
    let mut cb = [0u8; 16];
    cb[0] = 0x12;
    cb[4] = 36;
    Cbw {
        tag,
        data_length: 36,
        direction_in: true,
        lun,
        cb,
        cb_len: 6,
    }
}

pub fn read_capacity10(tag: u32, lun: u8) -> Cbw {
    let mut cb = [0u8; 16];
    cb[0] = 0x25;
    Cbw {
        tag,
        data_length: 8,
        direction_in: true,
        lun,
        cb,
        cb_len: 10,
    }
}

pub fn read_capacity16(tag: u32, lun: u8) -> Cbw {
    let mut cb = [0u8; 16];
    cb[0] = 0x9E;
    cb[1] = 0x10;
    cb[13] = 32;
    Cbw {
        tag,
        data_length: 32,
        direction_in: true,
        lun,
        cb,
        cb_len: 16,
    }
}

pub fn synchronize_cache(tag: u32, lun: u8) -> Cbw {
    let mut cb = [0u8; 16];
    cb[0] = 0x35;
    Cbw {
        tag,
        data_length: 0,
        direction_in: false,
        lun,
        cb,
        cb_len: 10,
    }
}

pub fn read(tag: u32, lun: u8, lba: u64, blocks: u32, block_size: u32) -> Option<Cbw> {
    if blocks == 0 {
        return None;
    }

    let data_length = (blocks as u64).checked_mul(block_size as u64)?;
    if data_length > u32::MAX as u64 {
        return None;
    }

    if blocks <= 0xFFFF && lba + blocks as u64 <= 0x100000000 {
        // READ(10)
        let mut cb = [0u8; 16];
        cb[0] = 0x28;
        cb[2..6].copy_from_slice(&(lba as u32).to_be_bytes());
        cb[7..9].copy_from_slice(&(blocks as u16).to_be_bytes());
        Some(Cbw {
            tag,
            data_length: data_length as u32,
            direction_in: true,
            lun,
            cb,
            cb_len: 10,
        })
    } else {
        // READ(16)
        let mut cb = [0u8; 16];
        cb[0] = 0x88;
        cb[2..10].copy_from_slice(&lba.to_be_bytes());
        cb[10..14].copy_from_slice(&blocks.to_be_bytes());
        Some(Cbw {
            tag,
            data_length: data_length as u32,
            direction_in: true,
            lun,
            cb,
            cb_len: 16,
        })
    }
}

pub fn write(tag: u32, lun: u8, lba: u64, blocks: u32, block_size: u32) -> Option<Cbw> {
    if blocks == 0 {
        return None;
    }

    let data_length = (blocks as u64).checked_mul(block_size as u64)?;
    if data_length > u32::MAX as u64 {
        return None;
    }

    if blocks <= 0xFFFF && lba + blocks as u64 <= 0x100000000 {
        // WRITE(10)
        let mut cb = [0u8; 16];
        cb[0] = 0x2A;
        cb[2..6].copy_from_slice(&(lba as u32).to_be_bytes());
        cb[7..9].copy_from_slice(&(blocks as u16).to_be_bytes());
        Some(Cbw {
            tag,
            data_length: data_length as u32,
            direction_in: false,
            lun,
            cb,
            cb_len: 10,
        })
    } else {
        // WRITE(16)
        let mut cb = [0u8; 16];
        cb[0] = 0x8A;
        cb[2..10].copy_from_slice(&lba.to_be_bytes());
        cb[10..14].copy_from_slice(&blocks.to_be_bytes());
        Some(Cbw {
            tag,
            data_length: data_length as u32,
            direction_in: false,
            lun,
            cb,
            cb_len: 16,
        })
    }
}

pub fn parse_csw(data: &[u8], expected_tag: u32, data_length: u32) -> Result<Csw, CswError> {
    if data.len() != 13 {
        return Err(CswError::WrongLength);
    }

    // Check signature
    if data[0..4] != *b"USBS" {
        return Err(CswError::BadSignature);
    }

    let tag = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    if tag != expected_tag {
        return Err(CswError::TagMismatch);
    }

    let residue = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    let status_byte = data[12];

    let status = match status_byte {
        0 => CswStatus::Passed,
        1 => CswStatus::Failed,
        2 => CswStatus::PhaseError,
        _ => return Err(CswError::BadStatus),
    };

    // Check residue validity
    if status != CswStatus::PhaseError && residue > data_length {
        return Err(CswError::ResidueTooLarge);
    }

    Ok(Csw {
        tag,
        residue,
        status,
    })
}

pub fn parse_capacity10(data: &[u8]) -> Option<Capacity> {
    if data.len() < 8 {
        return None;
    }

    let last_lba = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64;
    let block_size = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);

    Some(Capacity {
        last_lba,
        block_size,
    })
}

pub fn parse_capacity16(data: &[u8]) -> Option<Capacity> {
    if data.len() < 12 {
        return None;
    }

    let last_lba = u64::from_be_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]);
    let block_size = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

    Some(Capacity {
        last_lba,
        block_size,
    })
}

pub fn parse_sense(data: &[u8]) -> Option<Sense> {
    if data.is_empty() {
        return None;
    }

    let response_code = data[0] & 0x7F;

    if response_code == 0x70 || response_code == 0x71 {
        // Fixed format
        if data.len() < 18 {
            return None;
        }
        let key = data[2] & 0x0F;
        let asc = data[12];
        let ascq = data[13];
        Some(Sense { key, asc, ascq })
    } else if response_code == 0x72 || response_code == 0x73 {
        // Descriptor format
        if data.len() < 4 {
            return None;
        }
        let key = data[1] & 0x0F;
        let asc = data[2];
        let ascq = data[3];
        Some(Sense { key, asc, ascq })
    } else {
        None
    }
}

fn trim_string(s: &str) -> String {
    String::from(s.trim_end())
}

pub fn parse_inquiry(data: &[u8]) -> Option<Inquiry> {
    if data.len() < 36 {
        return None;
    }

    let qualifier = (data[0] >> 5) & 0x07;
    let peripheral_type = data[0] & 0x1F;
    let removable = (data[1] & 0x80) != 0;

    let vendor_bytes = &data[8..16];
    let product_bytes = &data[16..32];
    let revision_bytes = &data[32..36];

    let vendor = trim_string(
        core::str::from_utf8(vendor_bytes).unwrap_or(""),
    );
    let product = trim_string(
        core::str::from_utf8(product_bytes).unwrap_or(""),
    );
    let revision = trim_string(
        core::str::from_utf8(revision_bytes).unwrap_or(""),
    );

    Some(Inquiry {
        qualifier,
        peripheral_type,
        removable,
        vendor,
        product,
        revision,
    })
}
