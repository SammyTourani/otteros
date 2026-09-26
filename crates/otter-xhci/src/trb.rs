//! Transfer Request Block (TRB) encoding for xHCI 1.2.
//! Section 6.4: TRB types and layouts.

/// Transfer Request Block: 4 dwords, interpreted per TRB type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trb(pub [u32; 4]);

impl Trb {
    /// Extract the TRB type from dword 3, bits 10-15.
    pub fn trb_type(&self) -> u8 {
        ((self.0[3] >> 10) & 0x3F) as u8
    }

    /// Extract the cycle bit from dword 3, bit 0.
    pub fn cycle(&self) -> bool {
        (self.0[3] & 1) != 0
    }
}

/// Direction (TRT field) for Setup Stage TRB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataStage {
    None = 0,
    Out = 2,
    In = 3,
}

// TRB Type values (Section 6.4.1)
const TRB_TYPE_NORMAL: u8 = 1;
const TRB_TYPE_SETUP_STAGE: u8 = 2;
const TRB_TYPE_DATA_STAGE: u8 = 3;
const TRB_TYPE_STATUS_STAGE: u8 = 4;
const TRB_TYPE_LINK: u8 = 6;
const TRB_TYPE_ENABLE_SLOT: u8 = 9;
const TRB_TYPE_ADDRESS_DEVICE: u8 = 11;
const TRB_TYPE_CONFIGURE_ENDPOINT: u8 = 12;
const TRB_TYPE_EVALUATE_CONTEXT: u8 = 13;
const TRB_TYPE_RESET_ENDPOINT: u8 = 14;
const TRB_TYPE_STOP_ENDPOINT: u8 = 15;
const TRB_TYPE_SET_TR_DEQUEUE: u8 = 16;
const TRB_TYPE_NOOP_COMMAND: u8 = 23;

// Bit positions in dword 3 (Section 6.4)
const TRB_TYPE_SHIFT: u32 = 10;       // bits 10-15: TRB Type

// Transfer TRB flags
const NORMAL_ISP_SHIFT: u32 = 2;       // bit 2: Interrupt on Short Packet
const NORMAL_CHAIN_SHIFT: u32 = 4;     // bit 4: Chain
const NORMAL_IOC_SHIFT: u32 = 5;       // bit 5: Interrupt on Completion

// Setup Stage TRB flags
const SETUP_IDT_SHIFT: u32 = 6;        // bit 6: Immediate Data
const SETUP_TRT_SHIFT: u32 = 16;       // bits 16-17: Setup Transfer Type

// Data Stage TRB flags
const DATA_DIR_SHIFT: u32 = 16;        // bit 16: Direction

// Status Stage TRB flags
const STATUS_DIR_SHIFT: u32 = 16;      // bit 16: Direction

// Link TRB flags
const LINK_TOGGLE_SHIFT: u32 = 1;      // bit 1: Toggle Cycle

// Command TRB fields (Section 6.4.2.x)
const COMMAND_DCI_SHIFT: u32 = 16;      // bits 20:16: Endpoint ID (DCI)
const COMMAND_SLOT_SHIFT: u32 = 24;     // bits 31:24: Slot ID
const ADDRESS_BSA_SHIFT: u32 = 9;       // bit 9: Block Set Address

// ----- Transfer TRBs (Section 6.4.1)

/// Normal TRB for data transfer.
pub fn normal(buffer: u64, length: u32, ioc: bool, chain: bool, isp: bool) -> Trb {
    let mut dwords = [0u32; 4];
    dwords[0] = (buffer & 0xFFFF_FFFF) as u32;
    dwords[1] = (buffer >> 32) as u32;
    dwords[2] = length & 0x1F_FFFF;
    let mut d3 = (TRB_TYPE_NORMAL as u32) << TRB_TYPE_SHIFT;
    if ioc {
        d3 |= 1 << NORMAL_IOC_SHIFT;
    }
    if chain {
        d3 |= 1 << NORMAL_CHAIN_SHIFT;
    }
    if isp {
        d3 |= 1 << NORMAL_ISP_SHIFT;
    }
    dwords[3] = d3;
    Trb(dwords)
}

/// Setup Stage TRB for control transfers.
pub fn setup_stage(request: [u8; 8], direction: DataStage) -> Trb {
    let mut dwords = [0u32; 4];
    dwords[0] = u32::from_le_bytes([request[0], request[1], request[2], request[3]]);
    dwords[1] = u32::from_le_bytes([request[4], request[5], request[6], request[7]]);
    dwords[2] = 8;
    let d3 = (TRB_TYPE_SETUP_STAGE as u32) << TRB_TYPE_SHIFT
        | (1 << SETUP_IDT_SHIFT)
        | ((direction as u32) << SETUP_TRT_SHIFT);
    dwords[3] = d3;
    Trb(dwords)
}

/// Data Stage TRB for control transfers.
pub fn data_stage(buffer: u64, length: u32, dir_in: bool) -> Trb {
    let mut dwords = [0u32; 4];
    dwords[0] = (buffer & 0xFFFF_FFFF) as u32;
    dwords[1] = (buffer >> 32) as u32;
    dwords[2] = length & 0x1F_FFFF;
    let mut d3 = (TRB_TYPE_DATA_STAGE as u32) << TRB_TYPE_SHIFT;
    if dir_in {
        d3 |= 1 << DATA_DIR_SHIFT;
    }
    dwords[3] = d3;
    Trb(dwords)
}

/// Status Stage TRB for control transfers.
pub fn status_stage(dir_in: bool, ioc: bool) -> Trb {
    let mut d3 = (TRB_TYPE_STATUS_STAGE as u32) << TRB_TYPE_SHIFT;
    if dir_in {
        d3 |= 1 << STATUS_DIR_SHIFT;
    }
    if ioc {
        d3 |= 1 << NORMAL_IOC_SHIFT;
    }
    Trb([0, 0, 0, d3])
}

/// Link TRB for ring wrapping.
pub fn link(target: u64, toggle_cycle: bool) -> Trb {
    let mut dwords = [0u32; 4];
    dwords[0] = (target & 0xFFFF_FFFF) as u32;
    dwords[1] = (target >> 32) as u32;
    let mut d3 = (TRB_TYPE_LINK as u32) << TRB_TYPE_SHIFT;
    if toggle_cycle {
        d3 |= 1 << LINK_TOGGLE_SHIFT;
    }
    dwords[3] = d3;
    Trb(dwords)
}

// ----- Command TRBs (Section 6.4.2)

/// Enable Slot Command TRB.
pub fn enable_slot(slot_type: u8) -> Trb {
    let d3 = ((TRB_TYPE_ENABLE_SLOT as u32) << TRB_TYPE_SHIFT) | ((slot_type as u32) << 16);
    Trb([0, 0, 0, d3])
}

/// Address Device Command TRB.
pub fn address_device(input_ctx: u64, slot: u8, block_set_address: bool) -> Trb {
    let mut dwords = [0u32; 4];
    dwords[0] = (input_ctx & 0xFFFF_FFFF) as u32;
    dwords[1] = (input_ctx >> 32) as u32;
    let mut d3 = (TRB_TYPE_ADDRESS_DEVICE as u32) << TRB_TYPE_SHIFT
        | ((slot as u32) << COMMAND_SLOT_SHIFT);
    if block_set_address {
        d3 |= 1 << ADDRESS_BSA_SHIFT;
    }
    dwords[3] = d3;
    Trb(dwords)
}

/// Configure Endpoint Command TRB.
pub fn configure_endpoint(input_ctx: u64, slot: u8) -> Trb {
    let mut dwords = [0u32; 4];
    dwords[0] = (input_ctx & 0xFFFF_FFFF) as u32;
    dwords[1] = (input_ctx >> 32) as u32;
    let d3 = (TRB_TYPE_CONFIGURE_ENDPOINT as u32) << TRB_TYPE_SHIFT
        | ((slot as u32) << COMMAND_SLOT_SHIFT);
    dwords[3] = d3;
    Trb(dwords)
}

/// Evaluate Context Command TRB.
pub fn evaluate_context(input_ctx: u64, slot: u8) -> Trb {
    let mut dwords = [0u32; 4];
    dwords[0] = (input_ctx & 0xFFFF_FFFF) as u32;
    dwords[1] = (input_ctx >> 32) as u32;
    let d3 = (TRB_TYPE_EVALUATE_CONTEXT as u32) << TRB_TYPE_SHIFT
        | ((slot as u32) << COMMAND_SLOT_SHIFT);
    dwords[3] = d3;
    Trb(dwords)
}

/// Reset Endpoint Command TRB.
pub fn reset_endpoint(slot: u8, dci: u8) -> Trb {
    let d3 = (TRB_TYPE_RESET_ENDPOINT as u32) << TRB_TYPE_SHIFT
        | ((slot as u32) << COMMAND_SLOT_SHIFT)
        | ((dci as u32) << COMMAND_DCI_SHIFT);
    Trb([0, 0, 0, d3])
}

/// Stop Endpoint Command TRB.
pub fn stop_endpoint(slot: u8, dci: u8) -> Trb {
    let d3 = (TRB_TYPE_STOP_ENDPOINT as u32) << TRB_TYPE_SHIFT
        | ((slot as u32) << COMMAND_SLOT_SHIFT)
        | ((dci as u32) << COMMAND_DCI_SHIFT);
    Trb([0, 0, 0, d3])
}

/// Set TR Dequeue Pointer Command TRB.
pub fn set_tr_dequeue(slot: u8, dci: u8, pointer: u64, dcs: bool) -> Trb {
    let mut dwords = [0u32; 4];
    let addr_with_dcs = (pointer & 0xFFFF_FFFF_FFFF_FFF0) | if dcs { 1 } else { 0 };
    dwords[0] = (addr_with_dcs & 0xFFFF_FFFF) as u32;
    dwords[1] = (addr_with_dcs >> 32) as u32;
    let d3 = (TRB_TYPE_SET_TR_DEQUEUE as u32) << TRB_TYPE_SHIFT
        | ((slot as u32) << COMMAND_SLOT_SHIFT)
        | ((dci as u32) << COMMAND_DCI_SHIFT);
    dwords[3] = d3;
    Trb(dwords)
}

/// No-Op Command TRB.
pub fn noop_command() -> Trb {
    let d3 = (TRB_TYPE_NOOP_COMMAND as u32) << TRB_TYPE_SHIFT;
    Trb([0, 0, 0, d3])
}

// ----- Event TRBs (Section 6.4.3)

/// Parsed event TRB variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Transfer {
        pointer: u64,
        residual: u32,
        code: u8,
        slot: u8,
        dci: u8,
        event_data: bool,
    },
    CommandCompletion {
        pointer: u32,
        code: u8,
        slot: u8,
        parameter: u32,
    },
    PortStatusChange {
        port: u8,
        code: u8,
    },
    Other {
        trb_type: u8,
    },
}

/// Parse an event TRB.
pub fn parse_event(trb: &Trb) -> Event {
    let trb_type = trb.trb_type();
    let d0 = trb.0[0];
    let d1 = trb.0[1];
    let d2 = trb.0[2];
    let d3 = trb.0[3];
    let slot = ((d3 >> 24) & 0xFF) as u8;
    let code = ((d2 >> 24) & 0xFF) as u8;

    match trb_type {
        32 => {
            // Transfer Event (Section 6.4.3.1)
            let dci = ((d3 >> 16) & 0x1F) as u8;
            let residual = d2 & 0x00FF_FFFF;
            let event_data = (d3 >> 2) & 1 != 0;
            let pointer = u64::from(d1) << 32 | (d0 as u64);
            Event::Transfer {
                pointer,
                residual,
                code,
                slot,
                dci,
                event_data,
            }
        }
        33 => {
            // Command Completion Event (Section 6.4.3.4)
            let parameter = d2 & 0x00FF_FFFF;
            let pointer = d0 & 0xFFFF_FFF0;
            Event::CommandCompletion {
                pointer,
                code,
                slot,
                parameter,
            }
        }
        34 => {
            // Port Status Change Event (Section 6.4.3.8)
            let port = ((d0 >> 24) & 0xFF) as u8;
            Event::PortStatusChange { port, code }
        }
        _ => Event::Other { trb_type },
    }
}
