use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseReport {
    pub buttons: u8,
    pub dx: i16,
    pub dy: i16,
    pub wheel: i8,
}

pub struct BootKeyboard {
    previous_report: [u8; 8],
}

impl Default for BootKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl BootKeyboard {
    pub fn new() -> Self {
        BootKeyboard {
            previous_report: [0u8; 8],
        }
    }

    pub fn report(&mut self, report: &[u8], out: &mut Vec<u8>) {
        // Ignore reports shorter than 8 bytes
        if report.len() < 8 {
            return;
        }

        // Only use the first 8 bytes
        let report = &report[..8];

        // Check for rollover error codes in the key array
        let mut has_rollover = false;
        for &key in &report[2..8] {
            if (0x01..=0x03).contains(&key) {
                has_rollover = true;
                break;
            }
        }

        // Extract previous and current modifiers and keys
        let prev_mod = self.previous_report[0];
        let curr_mod = report[0];

        // Collect previous keys (deduplicated)
        let mut prev_keys = Vec::new();
        for &key in &self.previous_report[2..8] {
            if key != 0 && !prev_keys.contains(&key) {
                prev_keys.push(key);
            }
        }

        // Collect current keys
        let mut curr_keys = Vec::new();
        if !has_rollover {
            for &key in &report[2..8] {
                if key != 0 && !curr_keys.contains(&key) {
                    curr_keys.push(key);
                }
            }
        } else {
            // On rollover error, keep the previous keys unchanged
            curr_keys = prev_keys.clone();
        }

        // Generate breaks for modifier changes
        for bit in 0..8 {
            let prev_bit = (prev_mod >> bit) & 1;
            let curr_bit = (curr_mod >> bit) & 1;
            if prev_bit != 0 && curr_bit == 0 {
                // Modifier released
                let codes = get_modifier_codes(bit);
                for &code in codes {
                    out.push(code | 0x80);
                }
            }
        }

        // Generate breaks for released keys
        for &key in &prev_keys {
            if !curr_keys.contains(&key) && key != 0x48 {
                // Key was released (not Pause)
                if let Some(codes) = get_key_codes(key) {
                    let break_code = codes[codes.len() - 1] | 0x80;
                    for &code in &codes[..codes.len() - 1] {
                        out.push(code);
                    }
                    out.push(break_code);
                }
            }
        }

        // Generate makes for modifier changes
        for bit in 0..8 {
            let prev_bit = (prev_mod >> bit) & 1;
            let curr_bit = (curr_mod >> bit) & 1;
            if prev_bit == 0 && curr_bit != 0 {
                // Modifier pressed
                let codes = get_modifier_codes(bit);
                for &code in codes {
                    out.push(code);
                }
            }
        }

        // Generate makes for pressed keys
        for &key in &curr_keys {
            if !prev_keys.contains(&key) && key != 0x48 {
                // Key was pressed (not Pause)
                if let Some(codes) = get_key_codes(key) {
                    for &code in codes {
                        out.push(code);
                    }
                }
            }
        }

        // Update previous report
        if !has_rollover {
            self.previous_report.copy_from_slice(&report[..8]);
        } else {
            // Keep keys unchanged but update modifiers
            self.previous_report[0] = curr_mod;
        }
    }
}

fn get_modifier_codes(bit: u8) -> &'static [u8] {
    match bit {
        0 => &[0x1D],          // LCtrl
        1 => &[0x2A],          // LShift
        2 => &[0x38],          // LAlt
        3 => &[0xE0, 0x5B],    // LGUI
        4 => &[0xE0, 0x1D],    // RCtrl
        5 => &[0x36],          // RShift
        6 => &[0xE0, 0x38],    // RAlt
        7 => &[0xE0, 0x5C],    // RGUI
        _ => &[],
    }
}

fn get_key_codes(usage: u8) -> Option<&'static [u8]> {
    match usage {
        0x04 => Some(&[0x1E]),
        0x05 => Some(&[0x30]),
        0x06 => Some(&[0x2E]),
        0x07 => Some(&[0x20]),
        0x08 => Some(&[0x12]),
        0x09 => Some(&[0x21]),
        0x0A => Some(&[0x22]),
        0x0B => Some(&[0x23]),
        0x0C => Some(&[0x17]),
        0x0D => Some(&[0x24]),
        0x0E => Some(&[0x25]),
        0x0F => Some(&[0x26]),
        0x10 => Some(&[0x32]),
        0x11 => Some(&[0x31]),
        0x12 => Some(&[0x18]),
        0x13 => Some(&[0x19]),
        0x14 => Some(&[0x10]),
        0x15 => Some(&[0x13]),
        0x16 => Some(&[0x1F]),
        0x17 => Some(&[0x14]),
        0x18 => Some(&[0x16]),
        0x19 => Some(&[0x2F]),
        0x1A => Some(&[0x11]),
        0x1B => Some(&[0x2D]),
        0x1C => Some(&[0x15]),
        0x1D => Some(&[0x2C]),
        0x1E => Some(&[0x02]),
        0x1F => Some(&[0x03]),
        0x20 => Some(&[0x04]),
        0x21 => Some(&[0x05]),
        0x22 => Some(&[0x06]),
        0x23 => Some(&[0x07]),
        0x24 => Some(&[0x08]),
        0x25 => Some(&[0x09]),
        0x26 => Some(&[0x0A]),
        0x27 => Some(&[0x0B]),
        0x28 => Some(&[0x1C]),
        0x29 => Some(&[0x01]),
        0x2A => Some(&[0x0E]),
        0x2B => Some(&[0x0F]),
        0x2C => Some(&[0x39]),
        0x2D => Some(&[0x0C]),
        0x2E => Some(&[0x0D]),
        0x2F => Some(&[0x1A]),
        0x30 => Some(&[0x1B]),
        0x31 => Some(&[0x2B]),
        0x32 => Some(&[0x2B]),
        0x33 => Some(&[0x27]),
        0x34 => Some(&[0x28]),
        0x35 => Some(&[0x29]),
        0x36 => Some(&[0x33]),
        0x37 => Some(&[0x34]),
        0x38 => Some(&[0x35]),
        0x39 => Some(&[0x3A]),
        0x3A => Some(&[0x3B]),
        0x3B => Some(&[0x3C]),
        0x3C => Some(&[0x3D]),
        0x3D => Some(&[0x3E]),
        0x3E => Some(&[0x3F]),
        0x3F => Some(&[0x40]),
        0x40 => Some(&[0x41]),
        0x41 => Some(&[0x42]),
        0x42 => Some(&[0x43]),
        0x43 => Some(&[0x44]),
        0x44 => Some(&[0x57]),
        0x45 => Some(&[0x58]),
        0x46 => Some(&[0xE0, 0x37]),  // PrintScreen
        0x47 => Some(&[0x46]),
        0x49 => Some(&[0xE0, 0x52]),  // Insert
        0x4A => Some(&[0xE0, 0x47]),  // Home
        0x4B => Some(&[0xE0, 0x49]),  // PgUp
        0x4C => Some(&[0xE0, 0x53]),  // Delete
        0x4D => Some(&[0xE0, 0x4F]),  // End
        0x4E => Some(&[0xE0, 0x51]),  // PgDn
        0x4F => Some(&[0xE0, 0x4D]),  // Right
        0x50 => Some(&[0xE0, 0x4B]),  // Left
        0x51 => Some(&[0xE0, 0x50]),  // Down
        0x52 => Some(&[0xE0, 0x48]),  // Up
        0x53 => Some(&[0x45]),        // NumLock
        0x54 => Some(&[0xE0, 0x35]),  // Keypad /
        0x55 => Some(&[0x37]),        // Keypad *
        0x56 => Some(&[0x4A]),        // Keypad -
        0x57 => Some(&[0x4E]),        // Keypad +
        0x58 => Some(&[0xE0, 0x1C]),  // Keypad Enter
        0x59 => Some(&[0x4F]),        // Keypad 1
        0x5A => Some(&[0x50]),        // Keypad 2
        0x5B => Some(&[0x51]),        // Keypad 3
        0x5C => Some(&[0x4B]),        // Keypad 4
        0x5D => Some(&[0x4C]),        // Keypad 5
        0x5E => Some(&[0x4D]),        // Keypad 6
        0x5F => Some(&[0x47]),        // Keypad 7
        0x60 => Some(&[0x48]),        // Keypad 8
        0x61 => Some(&[0x49]),        // Keypad 9
        0x62 => Some(&[0x52]),        // Keypad 0
        0x63 => Some(&[0x53]),        // Keypad .
        0x64 => Some(&[0x56]),        // Keyboard <> |
        0x65 => Some(&[0xE0, 0x5D]),  // Keyboard App
        _ => None,
    }
}

pub fn leds(num: bool, caps: bool, scroll: bool) -> u8 {
    let mut result = 0u8;
    if num {
        result |= 0x01;
    }
    if caps {
        result |= 0x02;
    }
    if scroll {
        result |= 0x04;
    }
    result
}

pub fn parse_boot_mouse(data: &[u8]) -> Option<MouseReport> {
    if data.len() < 3 {
        return None;
    }

    let buttons = data[0] & 0x07;
    let dx = data[1] as i8 as i16;
    let dy = data[2] as i8 as i16;
    let wheel = if data.len() >= 4 {
        data[3] as i8
    } else {
        0
    };

    Some(MouseReport {
        buttons,
        dx,
        dy,
        wheel,
    })
}
