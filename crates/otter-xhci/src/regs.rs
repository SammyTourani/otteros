//! xHCI Register support (PORTSC, capability registers, extended capabilities).
//! Section 5.3-5.6: Register Definitions

extern crate alloc;
use alloc::vec::Vec;

// ----- PORTSC (Port Status and Control Register) Section 5.4.2

/// Port Connected bit (bit 0)
const PORTSC_CCS: u32 = 1 << 0;

/// Port Enabled/Disabled bit (bit 1)
const PORTSC_PED: u32 = 1 << 1;

/// Over-current Active bit (bit 3)
const PORTSC_OCA: u32 = 1 << 3;

/// Port Reset bit (bit 4)
const PORTSC_PR: u32 = 1 << 4;

/// Port Link State (bits 8-5)
const PORTSC_PLS_SHIFT: u32 = 5;
const PORTSC_PLS_MASK: u32 = 0x0F;

/// Port Power bit (bit 9)
const PORTSC_PP: u32 = 1 << 9;

/// Port Speed (bits 13-10)
const PORTSC_SPEED_SHIFT: u32 = 10;
const PORTSC_SPEED_MASK: u32 = 0x0F;

/// Port Connection Change bit (bit 17)
const PORTSC_CSC: u32 = 1 << 17;

/// Port Enable/Disable Change bit (bit 18)
const PORTSC_PEC: u32 = 1 << 18;

/// Port Warm Reset Change bit (bit 19)
const PORTSC_WRC: u32 = 1 << 19;

/// Over-current Change bit (bit 20)
const PORTSC_OCC: u32 = 1 << 20;

/// Port Reset Change bit (bit 21)
const PORTSC_PRC: u32 = 1 << 21;

/// Port Link State Change bit (bit 22)
const PORTSC_PLC: u32 = 1 << 22;

/// Port Config Error Change bit (bit 23)
const PORTSC_CEC: u32 = 1 << 23;

const PORTSC_CHANGE_MASK: u32 = PORTSC_CSC | PORTSC_PEC | PORTSC_WRC | PORTSC_OCC | PORTSC_PRC | PORTSC_PLC | PORTSC_CEC;

/// Parsed port status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortStatus {
    pub connected: bool,
    pub enabled: bool,
    pub over_current: bool,
    pub reset: bool,
    pub link_state: u8,
    pub powered: bool,
    pub speed: u8,
    pub connect_change: bool,
    pub enable_change: bool,
    pub warm_reset_change: bool,
    pub over_current_change: bool,
    pub reset_change: bool,
    pub link_change: bool,
    pub config_error_change: bool,
}

/// Parse a PORTSC register value.
pub fn parse_portsc(portsc: u32) -> PortStatus {
    PortStatus {
        connected: (portsc & PORTSC_CCS) != 0,
        enabled: (portsc & PORTSC_PED) != 0,
        over_current: (portsc & PORTSC_OCA) != 0,
        reset: (portsc & PORTSC_PR) != 0,
        link_state: ((portsc >> PORTSC_PLS_SHIFT) & PORTSC_PLS_MASK) as u8,
        powered: (portsc & PORTSC_PP) != 0,
        speed: ((portsc >> PORTSC_SPEED_SHIFT) & PORTSC_SPEED_MASK) as u8,
        connect_change: (portsc & PORTSC_CSC) != 0,
        enable_change: (portsc & PORTSC_PEC) != 0,
        warm_reset_change: (portsc & PORTSC_WRC) != 0,
        over_current_change: (portsc & PORTSC_OCC) != 0,
        reset_change: (portsc & PORTSC_PRC) != 0,
        link_change: (portsc & PORTSC_PLC) != 0,
        config_error_change: (portsc & PORTSC_CEC) != 0,
    }
}

/// Neutralize PORTSC: preserve status bits, clear write-to-clear bits and reserved bits.
pub fn portsc_neutral(portsc: u32) -> u32 {
    // Clear all change bits (write-to-clear), PR, reserved bits
    portsc & 0x4E00_FFE9
}

/// Prepare PORTSC value to reset the port.
pub fn portsc_reset(portsc: u32) -> u32 {
    (portsc_neutral(portsc) & !PORTSC_PED) | PORTSC_PR
}

/// Prepare PORTSC value to clear change bits that are set: the neutral value (4.19.1, never
/// writing 1 to PED, PR or another RW1C bit by accident) plus exactly the change bits (RW1C)
/// that `portsc` reports.
pub fn portsc_clear_changes(portsc: u32) -> u32 {
    portsc_neutral(portsc) | (portsc & PORTSC_CHANGE_MASK)
}

// ----- Capability Registers Section 5.3

/// Parsed capability registers (5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub caplength: u8,
    pub hci_version: u16,
    pub max_slots: u8,
    pub max_interrupters: u16,
    pub max_ports: u8,
    pub erst_max: u8,
    pub max_scratchpad: u16,
    pub ac64: bool,
    pub context_64: bool,
    /// Byte offset of the first extended capability from the MMIO base (0 = none).
    pub xecp_offset: u32,
    /// Byte offset of the doorbell array from the MMIO base.
    pub doorbell_offset: u32,
    /// Byte offset of the runtime registers from the MMIO base.
    pub runtime_offset: u32,
}

/// Parses the first eight capability dwords: 0x00 CAPLENGTH (7:0) and HCIVERSION (31:16),
/// 0x04 HCSPARAMS1, 0x08 HCSPARAMS2, 0x0C HCSPARAMS3, 0x10 HCCPARAMS1, 0x14 DBOFF,
/// 0x18 RTSOFF, 0x1C HCCPARAMS2 (5.3.1-5.3.9).
pub fn parse_capabilities(caps: &[u32; 8]) -> Capabilities {
    let hcsparams1 = caps[1];
    let hcsparams2 = caps[2];
    let hccparams1 = caps[4];
    // HCSPARAMS2 (5.3.4): IST 3:0, ERST Max 7:4, Max Scratchpad Buffers Hi 25:21, SPR 26,
    // Max Scratchpad Buffers Lo 31:27.
    let max_scratchpad_hi = ((hcsparams2 >> 21) & 0x1F) as u16;
    let max_scratchpad_lo = ((hcsparams2 >> 27) & 0x1F) as u16;
    Capabilities {
        caplength: (caps[0] & 0xFF) as u8,
        hci_version: (caps[0] >> 16) as u16,
        // HCSPARAMS1 (5.3.3): MaxSlots 7:0, MaxIntrs 18:8, MaxPorts 31:24.
        max_slots: (hcsparams1 & 0xFF) as u8,
        max_interrupters: ((hcsparams1 >> 8) & 0x7FF) as u16,
        max_ports: (hcsparams1 >> 24) as u8,
        erst_max: ((hcsparams2 >> 4) & 0x0F) as u8,
        max_scratchpad: (max_scratchpad_hi << 5) | max_scratchpad_lo,
        // HCCPARAMS1 (5.3.6): AC64 bit 0, CSZ bit 2, xECP 31:16 in dwords.
        ac64: hccparams1 & 1 != 0,
        context_64: (hccparams1 >> 2) & 1 != 0,
        xecp_offset: (hccparams1 >> 16) * 4,
        // DBOFF (5.3.7): bits 31:2, dword aligned. RTSOFF (5.3.8): bits 31:5, 32-byte aligned.
        doorbell_offset: caps[5] & !0x3,
        runtime_offset: caps[6] & !0x1F,
    }
}

/// Doorbell register value (Section 5.6).
pub fn doorbell_value(target: u8, stream: u32) -> u32 {
    ((stream & 0xFFFF) << 16) | (target as u32)
}

// ----- Extended Capabilities Section 7

/// Extended Capability variants (7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtCap {
    LegacySupport {
        offset: u32,
        bios_owned: bool,
        os_owned: bool,
    },
    SupportedProtocol {
        offset: u32,
        major: u8,
        minor: u8,
        port_offset: u8,
        port_count: u8,
        slot_type: u8,
    },
    Other {
        id: u8,
        offset: u32,
    },
}

/// Most capabilities a walk returns; a corrupt list cannot make it run forever.
pub const MAX_EXT_CAPS: usize = 64;

/// Walks the extended capability list starting at byte offset `first` (0 = no list). `read`
/// returns the dword at a byte offset from the MMIO base. Every capability's dword 0 holds its
/// ID (7:0) and the next pointer (15:8, in dwords relative to this capability, 0 = last) (7.0).
/// A dword of all ones ends the walk: the controller no longer answers (surprise removal).
pub fn walk_extended_caps(read: impl Fn(u32) -> u32, first: u32) -> Vec<ExtCap> {
    let mut caps = Vec::new();
    let mut current = first;
    while current != 0 && caps.len() < MAX_EXT_CAPS {
        let dword0 = read(current);
        if dword0 == 0xFFFF_FFFF {
            break;
        }
        let id = (dword0 & 0xFF) as u8;
        let next = (dword0 >> 8) & 0xFF;
        caps.push(match id {
            // USBLEGSUP (7.1.1): HC BIOS Owned Semaphore bit 16, HC OS Owned Semaphore bit 24.
            1 => ExtCap::LegacySupport {
                offset: current,
                bios_owned: dword0 & (1 << 16) != 0,
                os_owned: dword0 & (1 << 24) != 0,
            },
            // Supported Protocol (7.2): dword 0 Minor 23:16, Major 31:24; dword 2 Compatible
            // Port Offset 7:0, Count 15:8; dword 3 Protocol Slot Type 4:0.
            2 => {
                let ports = read(current.wrapping_add(8));
                ExtCap::SupportedProtocol {
                    offset: current,
                    major: (dword0 >> 24) as u8,
                    minor: (dword0 >> 16) as u8,
                    port_offset: (ports & 0xFF) as u8,
                    port_count: ((ports >> 8) & 0xFF) as u8,
                    slot_type: (read(current.wrapping_add(12)) & 0x1F) as u8,
                }
            }
            _ => ExtCap::Other { id, offset: current },
        });
        if next == 0 {
            break;
        }
        current = current.wrapping_add(next * 4);
    }
    caps
}

/// Set the OS ownership bit in a Legacy Support Capability.
pub fn legacy_request_os(legacy_cap: u32) -> u32 {
    legacy_cap | (1 << 24)
}
