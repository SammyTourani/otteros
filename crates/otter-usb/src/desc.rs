use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescError {
    Truncated,
    BadLength,
    WrongType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferType {
    Control,
    Isochronous,
    Bulk,
    Interrupt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub usb_version: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub max_packet_size0: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub device_version: u16,
    pub manufacturer_index: u8,
    pub product_index: u8,
    pub serial_index: u8,
    pub num_configurations: u8,
}

impl DeviceDescriptor {
    pub fn ep0_max_packet(&self) -> u16 {
        if self.usb_version >= 0x0300 {
            1u16 << self.max_packet_size0
        } else {
            self.max_packet_size0 as u16
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub address: u8,
    pub transfer: TransferType,
    pub max_packet_size: u16,
    pub mult: u8,
    pub interval: u8,
    pub max_burst: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub number: u8,
    pub alternate: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: Vec<Endpoint>,
    pub hid_report_length: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub attributes: u8,
    pub max_power: u8,
    pub interfaces: Vec<Interface>,
}

pub fn parse_device(data: &[u8]) -> Result<DeviceDescriptor, DescError> {
    if data.len() < 18 {
        return Err(DescError::Truncated);
    }
    if data[0] < 18 {
        return Err(DescError::BadLength);
    }
    if data[1] != 0x01 {
        return Err(DescError::WrongType);
    }

    Ok(DeviceDescriptor {
        usb_version: u16::from_le_bytes([data[2], data[3]]),
        class: data[4],
        subclass: data[5],
        protocol: data[6],
        max_packet_size0: data[7],
        vendor_id: u16::from_le_bytes([data[8], data[9]]),
        product_id: u16::from_le_bytes([data[10], data[11]]),
        device_version: u16::from_le_bytes([data[12], data[13]]),
        manufacturer_index: data[14],
        product_index: data[15],
        serial_index: data[16],
        num_configurations: data[17],
    })
}

pub fn parse_configuration(data: &[u8]) -> Result<Configuration, DescError> {
    if data.len() < 9 {
        return Err(DescError::Truncated);
    }
    if data[0] < 2 {
        return Err(DescError::BadLength);
    }
    if data[1] != 0x02 {
        return Err(DescError::WrongType);
    }

    let total_len = u16::from_le_bytes([data[2], data[3]]) as usize;
    let actual_len = data.len().min(total_len);

    if actual_len < 9 {
        return Err(DescError::Truncated);
    }

    let mut config = Configuration {
        value: data[5],
        attributes: data[7],
        max_power: data[8],
        interfaces: Vec::new(),
    };

    let mut pos = 9;
    let mut current_interface: Option<Interface> = None;

    while pos < actual_len {
        if pos >= data.len() {
            return Err(DescError::Truncated);
        }

        let blen = data[pos] as usize;
        if blen < 2 {
            return Err(DescError::BadLength);
        }
        if pos + blen > actual_len {
            return Err(DescError::Truncated);
        }

        let btype = data[pos + 1];

        match btype {
            0x04 => {
                // Interface descriptor
                if blen < 9 {
                    return Err(DescError::BadLength);
                }
                if let Some(iface) = current_interface.take() {
                    config.interfaces.push(iface);
                }

                current_interface = Some(Interface {
                    number: data[pos + 2],
                    alternate: data[pos + 3],
                    class: data[pos + 5],
                    subclass: data[pos + 6],
                    protocol: data[pos + 7],
                    endpoints: Vec::new(),
                    hid_report_length: None,
                });
            }
            0x05 => {
                // Endpoint descriptor
                if blen < 7 {
                    return Err(DescError::BadLength);
                }
                if let Some(ref mut iface) = current_interface {
                    let address = data[pos + 2];
                    let attr = data[pos + 3];
                    let transfer_type = match attr & 0x03 {
                        0x00 => TransferType::Control,
                        0x01 => TransferType::Isochronous,
                        0x02 => TransferType::Bulk,
                        0x03 => TransferType::Interrupt,
                        _ => unreachable!(),
                    };
                    let max_packet_size = u16::from_le_bytes([data[pos + 4], data[pos + 5]]);
                    let base_size = max_packet_size & 0x7FF;
                    let mult = ((max_packet_size >> 11) & 0x03) as u8;
                    let interval = data[pos + 6];

                    iface.endpoints.push(Endpoint {
                        address,
                        transfer: transfer_type,
                        max_packet_size: base_size,
                        mult,
                        interval,
                        max_burst: 0,
                    });
                }
            }
            0x21 => {
                // HID descriptor
                if blen < 9 {
                    return Err(DescError::BadLength);
                }
                if pos + 9 <= actual_len {
                    let report_desc_len = u16::from_le_bytes([data[pos + 7], data[pos + 8]]);
                    if let Some(ref mut iface) = current_interface {
                        iface.hid_report_length = Some(report_desc_len);
                    }
                }
            }
            0x30 => {
                // SuperSpeed Endpoint Companion descriptor
                if blen < 6 {
                    return Err(DescError::BadLength);
                }
                if let Some(ref mut iface) = current_interface
                    && let Some(endpoint) = iface.endpoints.last_mut()
                {
                    endpoint.max_burst = data[pos + 2];
                }
            }
            _ => {
                // Skip unknown descriptor types
            }
        }

        pos += blen;
    }

    if let Some(iface) = current_interface {
        config.interfaces.push(iface);
    }

    Ok(config)
}

pub fn parse_string(data: &[u8]) -> Result<String, DescError> {
    if data.len() < 2 {
        return Err(DescError::Truncated);
    }
    let blen = data[0] as usize;
    if blen < 2 {
        return Err(DescError::BadLength);
    }
    if data[1] != 0x03 {
        return Err(DescError::WrongType);
    }
    if data.len() < blen {
        return Err(DescError::Truncated);
    }

    let str_data = &data[2..blen];
    if !str_data.len().is_multiple_of(2) {
        return Err(DescError::BadLength);
    }

    let mut result = String::new();
    let mut i = 0;
    while i < str_data.len() {
        let code_unit = u16::from_le_bytes([str_data[i], str_data[i + 1]]);
        i += 2;

        if !(0xD800..=0xDFFF).contains(&code_unit) {
            // Regular BMP character or outside surrogate range
            if let Some(ch) = char::from_u32(code_unit as u32) {
                result.push(ch);
            } else {
                result.push('\u{FFFD}');
            }
        } else if (0xD800..=0xDBFF).contains(&code_unit) {
            // High surrogate; expect low surrogate
            if i < str_data.len() {
                let low = u16::from_le_bytes([str_data[i], str_data[i + 1]]);
                if (0xDC00..=0xDFFF).contains(&low) {
                    let high = ((code_unit - 0xD800) as u32) << 10;
                    let low_bits = (low - 0xDC00) as u32;
                    let code_point = 0x10000 + high + low_bits;
                    if let Some(ch) = char::from_u32(code_point) {
                        result.push(ch);
                    } else {
                        result.push('\u{FFFD}');
                    }
                    i += 2;
                } else {
                    // Unpaired high surrogate
                    result.push('\u{FFFD}');
                }
            } else {
                // Unpaired high surrogate at end
                result.push('\u{FFFD}');
            }
        } else {
            // Low surrogate without high; replacement character
            result.push('\u{FFFD}');
        }
    }

    Ok(result)
}

pub fn parse_languages(data: &[u8]) -> Result<Vec<u16>, DescError> {
    if data.len() < 2 {
        return Err(DescError::Truncated);
    }
    let blen = data[0] as usize;
    if blen < 2 {
        return Err(DescError::BadLength);
    }
    if data[1] != 0x03 {
        return Err(DescError::WrongType);
    }
    if data.len() < blen {
        return Err(DescError::Truncated);
    }

    let mut result = Vec::new();
    let mut i = 2;
    while i + 1 < blen {
        result.push(u16::from_le_bytes([data[i], data[i + 1]]));
        i += 2;
    }

    Ok(result)
}
