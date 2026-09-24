//! PCI configuration space header parsing and BAR decoding.

use super::ecam;
use super::Bdf;
use crate::mm::addr::PhysAddr;

/// The raw header from PCI config space (first 64 bytes).
pub struct ConfigHeader {
    pub vendor_id: u16,
    pub device_id: u16,
    pub command: u16,
    pub status: u16,
    pub revision_id: u8,
    pub prog_if: u8,
    pub subclass_code: u8,
    pub class_code: u8,
    pub cache_line_size: u8,
    pub latency_timer: u8,
    pub header_type: u8,
    pub bist: u8,
    pub bars: [Option<u64>; 6],
    pub bar_is_io: [bool; 6],
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
}

/// Reads and decodes the PCI config header for a device.
pub fn read_header(base_addr: PhysAddr, bdf: Bdf) -> ConfigHeader {
    let ecam_base = ecam::map_ecam_window(base_addr, 0x1000);
    let offset = bdf.ecam_offset();

    // Offsets into PCI config space (from the header definition):
    // 0x00: vendor ID (u16)
    // 0x02: device ID (u16)
    // 0x04: command (u16)
    // 0x06: status (u16)
    // 0x08: revision ID (u8)
    // 0x09: prog IF (u8)
    // 0x0A: subclass (u8)
    // 0x0B: class (u8)
    // 0x0C: cache line size (u8)
    // 0x0D: latency timer (u8)
    // 0x0E: header type (u8)
    // 0x0F: BIST (u8)
    // 0x10-0x27: BARs (6 × u32, or 4 × u64 for type 0)
    // 0x3C: interrupt line (u8)
    // 0x3D: interrupt pin (u8)

    // SAFETY: ecam_base is a valid ECAM window mapped by map_ecam_window,
    // and offsets are within bounds (first 16 bytes of config space).
    let (vendor_id, device_id, command, status, revision_id, prog_if, subclass_code, class_code,
         cache_line_size, latency_timer, header_type, bist) = unsafe {
        (
            ecam::read_u16(ecam_base, offset),
            ecam::read_u16(ecam_base, offset + 0x02),
            ecam::read_u16(ecam_base, offset + 0x04),
            ecam::read_u16(ecam_base, offset + 0x06),
            ecam::read_u8(ecam_base, offset + 0x08),
            ecam::read_u8(ecam_base, offset + 0x09),
            ecam::read_u8(ecam_base, offset + 0x0A),
            ecam::read_u8(ecam_base, offset + 0x0B),
            ecam::read_u8(ecam_base, offset + 0x0C),
            ecam::read_u8(ecam_base, offset + 0x0D),
            ecam::read_u8(ecam_base, offset + 0x0E),
            ecam::read_u8(ecam_base, offset + 0x0F),
        )
    };

    // Decode BARs (handles both 32-bit and 64-bit BARs).
    // SAFETY: All offsets are within the PCI config space (0x10-0x27 for BARs).
    let mut bars = [None; 6];
    let mut bar_is_io = [false; 6];
    let mut i = 0;
    while i < 6 {
        let bar_offset = 0x10 + (i as u32 * 4);
        let bar_val = unsafe { ecam::read_u32(ecam_base, offset + bar_offset) };
        if bar_val == 0 {
            i += 1;
            continue;
        }

        if (bar_val & 1) != 0 {
            // I/O space BAR
            bar_is_io[i] = true;
            bars[i] = Some((bar_val & 0xfffffffc) as u64);
            i += 1;
        } else {
            // Memory space BAR
            let bar_type = (bar_val >> 1) & 0x3;
            if bar_type == 0 {
                // 32-bit BAR
                bars[i] = Some((bar_val & 0xfffffff0) as u64);
                i += 1;
            } else if bar_type == 2 {
                // 64-bit BAR
                if i + 1 < 6 {
                    let bar_val_hi = unsafe { ecam::read_u32(ecam_base, offset + bar_offset + 4) };
                    let full_addr = ((bar_val_hi as u64) << 32) | ((bar_val & 0xfffffff0) as u64);
                    bars[i] = Some(full_addr);
                    bars[i + 1] = None; // Upper half of 64-bit BAR
                    i += 2;
                } else {
                    i += 1;
                }
            } else {
                // Reserved type
                i += 1;
            }
        }
    }

    // SAFETY: Offsets 0x3C-0x3D are within the PCI config space.
    let (interrupt_line, interrupt_pin) = unsafe {
        (
            ecam::read_u8(ecam_base, offset + 0x3C),
            ecam::read_u8(ecam_base, offset + 0x3D),
        )
    };

    ConfigHeader {
        vendor_id,
        device_id,
        command,
        status,
        revision_id,
        prog_if,
        subclass_code,
        class_code,
        cache_line_size,
        latency_timer,
        header_type,
        bist,
        bars,
        bar_is_io,
        interrupt_line,
        interrupt_pin,
    }
}

/// Scans the PCI capability list and returns (capability_id, offset) pairs.
pub fn scan_capabilities(base_addr: PhysAddr, bdf: Bdf) -> alloc::vec::Vec<(u8, u8)> {
    let ecam_base = ecam::map_ecam_window(base_addr, 0x1000);
    let offset = bdf.ecam_offset();

    let mut caps = alloc::vec::Vec::new();

    // Read the capability pointer (offset 0x34).
    // SAFETY: offset 0x34 is within PCI config space.
    let mut cap_ptr = unsafe { ecam::read_u8(ecam_base, offset + 0x34) };
    let mut seen = [false; 256];

    while (0x40..0xff).contains(&cap_ptr) {
        if seen[cap_ptr as usize] {
            break; // Avoid infinite loops on corrupted capability lists.
        }
        seen[cap_ptr as usize] = true;

        // Each capability entry starts with:
        // +0: capability ID (u8)
        // +1: next capability pointer (u8)
        // SAFETY: cap_ptr is in range [0x40, 0xff), so offset + cap_ptr is within config space.
        let (cap_id, next_ptr) = unsafe {
            (
                ecam::read_u8(ecam_base, offset + cap_ptr as u32),
                ecam::read_u8(ecam_base, offset + cap_ptr as u32 + 1),
            )
        };

        caps.push((cap_id, cap_ptr));
        cap_ptr = next_ptr;
    }

    caps
}

/// Reads the secondary bus number from a PCI-to-PCI bridge.
pub fn read_bridge_secondary_bus(base_addr: PhysAddr, bdf: Bdf) -> u8 {
    let ecam_base = ecam::map_ecam_window(base_addr, 0x1000);
    let offset = bdf.ecam_offset();
    // Secondary bus is at offset 0x19 in the bridge header.
    // SAFETY: offset 0x19 is within the PCI config space.
    unsafe { ecam::read_u8(ecam_base, offset + 0x19) }
}

/// Probes the size of a BAR by writing all-ones and reading back.
/// This temporarily disables I/O and memory access decoding.
pub fn probe_bar_size(base_addr: PhysAddr, bdf: Bdf, bar_index: u8) -> u64 {
    assert!(bar_index < 6, "BAR index must be < 6");

    let ecam_base = ecam::map_ecam_window(base_addr, 0x1000);
    let offset = bdf.ecam_offset();

    let bar_offset = 0x10 + (bar_index as u32 * 4);

    // SAFETY: All offsets are within PCI config space.
    let (orig_cmd, orig_bar) = unsafe {
        (
            ecam::read_u16(ecam_base, offset + 0x04),
            ecam::read_u32(ecam_base, offset + bar_offset),
        )
    };

    // Save original command (disable I/O and memory access).
    unsafe {
        ecam::write_u16(ecam_base, offset + 0x04, orig_cmd & !0x0003);
    }

    // Write all-ones and read back (size encoding).
    unsafe {
        ecam::write_u32(ecam_base, offset + bar_offset, 0xffffffff);
    }
    let size_bits = unsafe { ecam::read_u32(ecam_base, offset + bar_offset) };

    // Restore original BAR.
    unsafe {
        ecam::write_u32(ecam_base, offset + bar_offset, orig_bar);
    }

    // Restore original command.
    unsafe {
        ecam::write_u16(ecam_base, offset + 0x04, orig_cmd);
    }

    // Decode the size.
    let size = if size_bits == 0 {
        0
    } else if (size_bits & 1) != 0 {
        // I/O space BAR: bits 1-31 are the size mask
        !((size_bits & 0xfffffffc).saturating_sub(1)) as u64
    } else {
        // Memory space BAR: bits 4-31 are the size mask
        !((size_bits & 0xfffffff0).saturating_sub(1)) as u64
    };
    size + 1
}
