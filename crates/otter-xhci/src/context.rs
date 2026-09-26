//! Device and endpoint contexts for xHCI 1.2.
//! Section 6.2: Device Context and Endpoint Context

/// Endpoint type (Section 6.2.2: Endpoint Context - Endpoint Type).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[derive(Default)]
pub enum EpType {
    #[default]
    NotValid = 0,
    IsochOut = 1,
    BulkOut = 2,
    InterruptOut = 3,
    Control = 4,
    IsochIn = 5,
    BulkIn = 6,
    InterruptIn = 7,
}


impl EpType {
    /// Determine endpoint type from USB descriptor fields.
    pub fn from_endpoint(bm_attributes: u8, endpoint_address: u8) -> Self {
        let transfer_type = bm_attributes & 0x03;
        let dir_in = endpoint_address & 0x80 != 0;

        match transfer_type {
            0x00 => EpType::Control,
            0x01 => if dir_in { EpType::IsochIn } else { EpType::IsochOut },
            0x02 => if dir_in { EpType::BulkIn } else { EpType::BulkOut },
            0x03 => if dir_in { EpType::InterruptIn } else { EpType::InterruptOut },
            _ => EpType::NotValid,
        }
    }
}

impl From<u8> for EpType {
    fn from(v: u8) -> Self {
        match v {
            0 => EpType::NotValid,
            1 => EpType::IsochOut,
            2 => EpType::BulkOut,
            3 => EpType::InterruptOut,
            4 => EpType::Control,
            5 => EpType::IsochIn,
            6 => EpType::BulkIn,
            7 => EpType::InterruptIn,
            _ => EpType::NotValid,
        }
    }
}

/// USB device speed (Section 5.4.3: Supported Protocol Capability).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    Full = 1,
    Low = 2,
    High = 3,
    Super = 4,
    SuperPlus = 5,
}

impl Speed {
    /// Determine speed from Protocol Speed ID (PSIV) value.
    pub fn from_psiv(psiv: u8) -> Option<Speed> {
        match psiv {
            1 => Some(Speed::Full),
            2 => Some(Speed::Low),
            3 => Some(Speed::High),
            4 => Some(Speed::Super),
            5 => Some(Speed::SuperPlus),
            _ => None,
        }
    }

    /// Get the default maximum packet size for endpoint 0 at this speed.
    pub fn default_ep0_max_packet(&self) -> u16 {
        match self {
            Speed::Full | Speed::Low => 8,
            Speed::High => 64,
            Speed::Super | Speed::SuperPlus => 512,
        }
    }
}

/// Device Context Index from endpoint address (Section 6.2.3.2).
pub fn dci(endpoint_address: u8) -> u8 {
    let ep_num = endpoint_address & 0x0F;
    if ep_num == 0 {
        1
    } else {
        let dir_in = (endpoint_address & 0x80) != 0;
        ep_num * 2 + (dir_in as u8)
    }
}

/// Calculate the Interval value from speed, endpoint type, and bInterval.
pub fn interval(speed: Speed, ep_type: EpType, b_interval: u8) -> u8 {
    use EpType::*;
    use Speed::*;

    let result = match (speed, ep_type) {
        // High-speed and SuperSpeed interrupt/isoch: bInterval - 1, input clamped to 1..=16
        (High | Super | SuperPlus, InterruptIn | InterruptOut | IsochIn | IsochOut) => {
            let bi = b_interval.clamp(1, 16);
            bi.saturating_sub(1)
        }
        // Full-speed/Low-speed interrupt: floor(log2(bInterval * 8)), input clamped to 1..=255
        (Full | Low, InterruptIn | InterruptOut) => {
            let bi = b_interval.max(1);
            let val = ((bi as u32) * 8).ilog2() as u8;
            val.max(3)
        }
        // Full-speed isoch: bInterval + 2, input clamped to 1..=16
        (Full, IsochIn | IsochOut) => {
            let bi = b_interval.clamp(1, 16);
            bi.saturating_add(2)
        }
        // Control and bulk endpoints: 0
        _ => 0,
    };

    result.min(15)
}

/// Device Context (Slot Context) per Section 6.2.2.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct SlotContext {
    pub route_string: u32,
    pub speed: u8,
    pub mtt: bool,
    pub hub: bool,
    pub context_entries: u8,
    pub max_exit_latency: u16,
    pub root_hub_port: u8,
    pub num_ports: u8,
    pub parent_slot: u8,
    pub parent_port: u8,
    pub ttt: u8,
    pub interrupter: u16,
    pub device_address: u8,
    pub slot_state: u8,
}

impl SlotContext {
    /// Encode to 8 dwords (32 bytes).
    pub fn to_dwords(&self) -> [u32; 8] {
        let mut d = [0u32; 8];

        // Dword 0: Route String (19:0), Speed (23:20), MTT (25), Hub (26), Context Entries (31:27)
        d[0] = (self.route_string & 0xFFFFF)
            | (((self.speed & 0x0F) as u32) << 20)
            | ((self.mtt as u32) << 25)
            | ((self.hub as u32) << 26)
            | (((self.context_entries & 0x1F) as u32) << 27);

        // Dword 1: Max Exit Latency (15:0), Root Hub Port Number (23:16), Number of Ports (31:24)
        d[1] = ((self.max_exit_latency as u32) & 0xFFFF)
            | (((self.root_hub_port as u32) & 0xFF) << 16)
            | (((self.num_ports as u32) & 0xFF) << 24);

        // Dword 2: Parent Hub Slot ID (7:0), Parent Port Number (15:8), TTT (17:16), Interrupter Target (31:22)
        d[2] = ((self.parent_slot as u32) & 0xFF)
            | (((self.parent_port as u32) & 0xFF) << 8)
            | (((self.ttt as u32) & 0x03) << 16)
            | (((self.interrupter as u32) & 0x3FF) << 22);

        // Dword 3: USB Device Address (7:0), Slot State (31:27)
        d[3] = ((self.device_address as u32) & 0xFF)
            | (((self.slot_state as u32) & 0x1F) << 27);

        d
    }

    /// Decode from 8 dwords.
    pub fn from_dwords(d: &[u32]) -> Self {
        let d0 = d[0];
        let d1 = d[1];
        let d2 = d[2];
        let d3 = d[3];

        SlotContext {
            route_string: d0 & 0xFFFFF,
            speed: ((d0 >> 20) & 0x0F) as u8,
            mtt: (d0 >> 25) & 1 != 0,
            hub: (d0 >> 26) & 1 != 0,
            context_entries: ((d0 >> 27) & 0x1F) as u8,
            max_exit_latency: (d1 & 0xFFFF) as u16,
            root_hub_port: ((d1 >> 16) & 0xFF) as u8,
            num_ports: ((d1 >> 24) & 0xFF) as u8,
            parent_slot: (d2 & 0xFF) as u8,
            parent_port: ((d2 >> 8) & 0xFF) as u8,
            ttt: ((d2 >> 16) & 0x03) as u8,
            interrupter: ((d2 >> 22) & 0x3FF) as u16,
            device_address: (d3 & 0xFF) as u8,
            slot_state: ((d3 >> 27) & 0x1F) as u8,
        }
    }
}

/// Endpoint Context per Section 6.2.3.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct EndpointContext {
    pub state: u8,
    pub mult: u8,
    pub max_pstreams: u8,
    pub lsa: bool,
    pub interval: u8,
    pub max_esit_payload: u32,
    pub cerr: u8,
    pub ep_type: EpType,
    pub hid: bool,
    pub max_burst: u8,
    pub max_packet_size: u16,
    pub dequeue: u64,
    pub dcs: bool,
    pub average_trb_length: u16,
}

impl EndpointContext {
    /// Encode to 8 dwords.
    pub fn to_dwords(&self) -> [u32; 8] {
        let mut d = [0u32; 8];

        // Dword 0: State (2:0), Mult (9:8), MaxPStreams (14:10), LSA (15), Interval (23:16), Max ESIT Payload Hi (31:24)
        d[0] = ((self.state as u32) & 0x07)
            | (((self.mult as u32) & 0x03) << 8)
            | (((self.max_pstreams as u32) & 0x1F) << 10)
            | ((self.lsa as u32) << 15)
            | (((self.interval as u32) & 0xFF) << 16)
            | ((self.max_esit_payload >> 16) & 0xFF) << 24;

        // Dword 1: CErr (2:1), EP Type (5:3), HID (7), Max Burst Size (15:8), Max Packet Size (31:16)
        d[1] = (((self.cerr as u32) & 0x03) << 1)
            | (((self.ep_type as u32) & 0x07) << 3)
            | ((self.hid as u32) << 7)
            | (((self.max_burst as u32) & 0xFF) << 8)
            | (((self.max_packet_size as u32) & 0xFFFF) << 16);

        // Dwords 2-3: TR Dequeue Pointer (dword2 bits 31:0, dword3 bits 31:0) with DCS in dword2 bit 0
        let dequeue_addr = self.dequeue | if self.dcs { 1 } else { 0 };
        d[2] = (dequeue_addr & 0xFFFF_FFFF) as u32;
        d[3] = (dequeue_addr >> 32) as u32;

        // Dword 4: Average TRB Length (15:0), Max ESIT Payload Lo (31:16)
        d[4] = ((self.average_trb_length as u32) & 0xFFFF)
            | ((self.max_esit_payload & 0xFFFF) << 16);

        d
    }

    /// Decode from 8 dwords.
    pub fn from_dwords(d: &[u32]) -> Self {
        let d0 = d[0];
        let d1 = d[1];
        let d2 = d[2];
        let d3 = d[3];
        let d4 = d[4];

        let dequeue_raw = (d3 as u64) << 32 | (d2 as u64);
        let dcs = (dequeue_raw & 1) != 0;

        let max_esit_hi = (d0 >> 24) & 0xFF;
        let max_esit_lo = (d4 >> 16) & 0xFFFF;
        let max_esit_payload = (max_esit_hi << 16) | max_esit_lo;

        EndpointContext {
            state: (d0 & 0x07) as u8,
            mult: ((d0 >> 8) & 0x03) as u8,
            max_pstreams: ((d0 >> 10) & 0x1F) as u8,
            lsa: (d0 >> 15) & 1 != 0,
            interval: ((d0 >> 16) & 0xFF) as u8,
            max_esit_payload,
            cerr: ((d1 >> 1) & 0x03) as u8,
            ep_type: EpType::from(((d1 >> 3) & 0x07) as u8),
            hid: (d1 >> 7) & 1 != 0,
            max_burst: ((d1 >> 8) & 0xFF) as u8,
            max_packet_size: ((d1 >> 16) & 0xFFFF) as u16,
            dequeue: dequeue_raw & !1u64,
            dcs,
            average_trb_length: (d4 & 0xFFFF) as u16,
        }
    }
}

/// Get the input context length in dwords.
pub fn input_context_len(csz64: bool) -> usize {
    let stride = if csz64 { 16 } else { 8 };
    stride * 33
}

/// Write the Input Control Context.
pub fn write_input_control(
    ctx: &mut [u32],
    _csz64: bool,
    drop: u32,
    add: u32,
    configuration: u8,
    interface: u8,
    alternate: u8,
) {
    ctx[0] = drop;
    ctx[1] = add;
    ctx[2..7].fill(0);
    ctx[7] = ((alternate as u32) << 16) | ((interface as u32) << 8) | (configuration as u32);
}

/// Write the Slot Context to the input context.
pub fn write_slot(ctx: &mut [u32], csz64: bool, slot: &SlotContext) {
    let stride = if csz64 { 16 } else { 8 };
    ctx[stride..stride + 8].copy_from_slice(&slot.to_dwords());
}

/// Write an Endpoint Context to the input context.
pub fn write_endpoint(ctx: &mut [u32], csz64: bool, dci: u8, ep: &EndpointContext) {
    let stride = if csz64 { 16 } else { 8 };
    let entry = (dci as usize) + 1;
    let offset = entry * stride;
    ctx[offset..offset + 8].copy_from_slice(&ep.to_dwords());
}

/// Read the Slot Context from the output device context.
pub fn read_output_slot(ctx: &[u32], _csz64: bool) -> SlotContext {
    SlotContext::from_dwords(&ctx[0..8])
}

/// Read an Endpoint Context from the output device context.
pub fn read_output_endpoint(ctx: &[u32], csz64: bool, dci: u8) -> EndpointContext {
    let stride = if csz64 { 16 } else { 8 };
    let entry = dci as usize;
    let offset = entry * stride;
    EndpointContext::from_dwords(&ctx[offset..offset + 8])
}
