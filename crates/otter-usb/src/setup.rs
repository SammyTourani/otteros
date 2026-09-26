#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl SetupPacket {
    pub fn to_bytes(&self) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[0] = self.request_type;
        b[1] = self.request;
        b[2..4].copy_from_slice(&self.value.to_le_bytes());
        b[4..6].copy_from_slice(&self.index.to_le_bytes());
        b[6..8].copy_from_slice(&self.length.to_le_bytes());
        b
    }

    /// Get a descriptor by type and index.
    pub fn get_descriptor(desc_type: u8, index: u8, lang: u16, length: u16) -> SetupPacket {
        SetupPacket {
            request_type: 0x80,
            request: 0x06,
            value: ((desc_type as u16) << 8) | (index as u16),
            index: lang,
            length,
        }
    }

    /// Get a HID report descriptor.
    pub fn get_hid_report_descriptor(interface: u8, length: u16) -> SetupPacket {
        SetupPacket {
            request_type: 0x81,
            request: 0x06,
            value: 0x2200,
            index: interface as u16,
            length,
        }
    }

    /// Set device address.
    pub fn set_address(address: u8) -> SetupPacket {
        SetupPacket {
            request_type: 0x00,
            request: 0x05,
            value: address as u16,
            index: 0,
            length: 0,
        }
    }

    /// Set device configuration.
    pub fn set_configuration(value: u8) -> SetupPacket {
        SetupPacket {
            request_type: 0x00,
            request: 0x09,
            value: value as u16,
            index: 0,
            length: 0,
        }
    }

    /// Set HID protocol (boot or report).
    pub fn hid_set_protocol(interface: u8, boot: bool) -> SetupPacket {
        SetupPacket {
            request_type: 0x21,
            request: 0x0B,
            value: if boot { 0 } else { 1 },
            index: interface as u16,
            length: 0,
        }
    }

    /// Set HID idle rate.
    pub fn hid_set_idle(interface: u8, idle_ms: u8, report_id: u8) -> SetupPacket {
        SetupPacket {
            request_type: 0x21,
            request: 0x0A,
            value: ((idle_ms as u16) << 8) | (report_id as u16),
            index: interface as u16,
            length: 0,
        }
    }

    /// Set HID output report (LEDs, etc).
    pub fn hid_set_report_output(interface: u8, report_id: u8, length: u16) -> SetupPacket {
        SetupPacket {
            request_type: 0x21,
            request: 0x09,
            value: (0x02u16 << 8) | (report_id as u16),
            index: interface as u16,
            length,
        }
    }

    /// Clear endpoint halt.
    pub fn clear_endpoint_halt(endpoint: u8) -> SetupPacket {
        SetupPacket {
            request_type: 0x02,
            request: 0x01,
            value: 0,
            index: endpoint as u16,
            length: 0,
        }
    }

    /// BOT reset (Bulk-Only Transport).
    pub fn bot_reset(interface: u8) -> SetupPacket {
        SetupPacket {
            request_type: 0x21,
            request: 0xFF,
            value: 0,
            index: interface as u16,
            length: 0,
        }
    }

    /// BOT get maximum LUN.
    pub fn bot_get_max_lun(interface: u8) -> SetupPacket {
        SetupPacket {
            request_type: 0xA1,
            request: 0xFE,
            value: 0,
            index: interface as u16,
            length: 1,
        }
    }
}
