//! Acceptance oracle for brief M10-T0b (otter-xhci: the sans-I/O core of an xHCI driver), written
//! by the orchestrator. The crate must pass this file unchanged. Bit layouts are from the eXtensible
//! Host Controller Interface specification 1.2: TRBs (6.4), contexts (6.2), registers (5.3-5.6),
//! extended capabilities (7), interval rules (6.2.3.6).
//!
//! API this file relies on:
//!   trb:   Trb(pub [u32; 4]) (Debug, Clone, Copy, PartialEq, Eq) with trb_type() -> u8, cycle() -> bool;
//!          normal(buffer, length, ioc, chain, isp), setup_stage([u8; 8], DataStage::{None, Out, In}),
//!          data_stage(buffer, length, dir_in), status_stage(dir_in, ioc), link(target, toggle_cycle),
//!          enable_slot(slot_type), address_device(input_ctx, slot, block_set_address),
//!          configure_endpoint(input_ctx, slot), evaluate_context(input_ctx, slot), reset_endpoint(slot, dci),
//!          stop_endpoint(slot, dci), set_tr_dequeue(slot, dci, pointer, cycle), noop_command(),
//!          parse_event(&Trb) -> Event::{Transfer { pointer, residual, code, slot, dci, event_data },
//!          CommandCompletion { pointer, code, slot, parameter }, PortStatusChange { port, code }, Other { trb_type }}
//!          (every TRB builder leaves the cycle bit 0: the ring owns it)
//!   ring:  ProducerRing::new(base_phys, size) with push(&mut self, &[AtomicU32], Trb) -> u64,
//!          enqueue_pointer(), cycle(), free_slots(dequeue_phys) -> usize;
//!          EventRing::new(base_phys, size) with pop(&mut self, &[AtomicU32]) -> Option<Trb>,
//!          dequeue_pointer(), erdp() -> u64, erst_entry() -> [u32; 4]
//!   context: SlotContext { route_string: u32, speed: u8, mtt: bool, hub: bool, context_entries: u8,
//!          max_exit_latency: u16, root_hub_port: u8, num_ports: u8, parent_slot: u8, parent_port: u8,
//!          ttt: u8, interrupter: u16, device_address: u8, slot_state: u8 } and EndpointContext { state: u8,
//!          mult: u8, max_pstreams: u8, lsa: bool, interval: u8, max_esit_payload: u32, cerr: u8,
//!          ep_type: EpType, hid: bool, max_burst: u8, max_packet_size: u16, dequeue: u64 (16-byte aligned,
//!          without DCS), dcs: bool, average_trb_length: u16 } (Default, Debug, Clone, PartialEq, Eq) with
//!          to_dwords() -> [u32; 8] and from_dwords(&[u32]) -> Self; EpType (repr u8, 0..=7) with
//!          from_endpoint(bm_attributes, endpoint_address); Speed::{Full, Low, High, Super, SuperPlus}
//!          with from_psiv(u8) -> Option<Speed> and default_ep0_max_packet() -> u16;
//!          dci(endpoint_address) -> u8; interval(Speed, EpType, b_interval) -> u8;
//!          input_context_len(csz64) -> usize (dwords); write_input_control(&mut [u32], csz64, drop, add,
//!          configuration, interface, alternate); write_slot(&mut [u32], csz64, &SlotContext);
//!          write_endpoint(&mut [u32], csz64, dci, &EndpointContext); read_output_slot(&[u32], csz64);
//!          read_output_endpoint(&[u32], csz64, dci)
//!   regs:  PORTSC helpers portsc_neutral, portsc_reset, portsc_clear_changes (u32 -> u32) and
//!          parse_portsc(u32) -> PortStatus { connected, enabled, over_current, reset, link_state,
//!          powered, speed, connect_change, enable_change, warm_reset_change, over_current_change,
//!          reset_change, link_change, config_error_change }; parse_capabilities(&[u32; 8]) ->
//!          Capabilities { caplength, hci_version, max_slots, max_interrupters, max_ports, erst_max,
//!          max_scratchpad, ac64, context_64, xecp_offset, doorbell_offset, runtime_offset };
//!          doorbell_value(target, stream) -> u32; walk_extended_caps(read: impl Fn(u32) -> u32, first)
//!          -> Vec<ExtCap::{LegacySupport { offset, bios_owned, os_owned }, SupportedProtocol { offset,
//!          major, minor, port_offset, port_count, slot_type }, Other { id, offset }}> (at most 64 entries;
//!          a dword of all ones ends the walk);
//!          legacy_request_os(u32) -> u32
//!
//! Semantics beyond the specification:
//! - ProducerRing: `size` TRB slots at base_phys; the last slot holds a Link TRB back to base_phys
//!   with Toggle Cycle set. The producer cycle starts at 1. push() writes the TRB with the ring's
//!   cycle bit (dwords 0-2 first, dword 3 last with Release ordering) and returns its address; when
//!   the next slot is the link slot, push() immediately writes the Link TRB (cycle = the current
//!   producer cycle, chain = the chain bit of the TRB just pushed), toggles the producer cycle and
//!   moves the enqueue pointer to base_phys. free_slots(d) counts how many TRBs can be pushed
//!   before the enqueue slot would reach the consumer's dequeue slot d, keeping one slot empty.
//! - EventRing: one segment of `size` TRBs; the consumer cycle starts at 1; pop() returns the TRB at
//!   the dequeue slot when its cycle bit equals the consumer cycle (read dword 3 with Acquire first),
//!   advancing and toggling the consumer cycle at the end of the segment. erdp() is the dequeue
//!   pointer with EHB (bit 3) set; erst_entry() is [base lo, base hi, size, 0].
//! - Command completion pointers drop bits 0-3; transfer-event pointers are returned as they are.
//! - interval(): HS/SS interrupt and isochronous endpoints use bInterval - 1 with bInterval clamped
//!   to 1..=16; FS/LS interrupt endpoints use floor(log2(bInterval * 8)) with bInterval clamped to
//!   1..=255 (so 3..=10); FS isochronous endpoints use bInterval + 2 with bInterval clamped to
//!   1..=16; control and bulk endpoints use 0. Every result is capped at 15, the field's range.

use core::sync::atomic::{AtomicU32, Ordering};
use otter_xhci::context::{self, EndpointContext, EpType, SlotContext, Speed};
use otter_xhci::regs::{self, ExtCap};
use otter_xhci::ring::{EventRing, ProducerRing};
use otter_xhci::trb::{self, DataStage, Event, Trb};

fn memory(trbs: usize) -> Vec<AtomicU32> {
    (0..trbs * 4).map(|_| AtomicU32::new(0)).collect()
}

fn slot(mem: &[AtomicU32], index: usize) -> [u32; 4] {
    core::array::from_fn(|i| mem[index * 4 + i].load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------------------------
// TRBs
// ---------------------------------------------------------------------------------------------

#[test]
fn transfer_trbs() {
    assert_eq!(trb::normal(0x1234_5678_9ABC_DEF0, 512, true, false, true).0, [0x9ABC_DEF0, 0x1234_5678, 512, 0x0000_0424]);
    assert_eq!(trb::normal(0x2000, 4096, false, true, false).0, [0x2000, 0, 4096, 0x0000_0410], "chain");
    let get_device = [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00];
    assert_eq!(trb::setup_stage(get_device, DataStage::In).0, [0x0100_0680, 0x0012_0000, 8, 0x0003_0840]);
    assert_eq!(trb::setup_stage(get_device, DataStage::Out).0[3], 0x0002_0840);
    assert_eq!(trb::setup_stage(get_device, DataStage::None).0[3], 0x0000_0840);
    assert_eq!(trb::data_stage(0xDEAD_B000, 18, true).0, [0xDEAD_B000, 0, 18, 0x0001_0C00]);
    assert_eq!(trb::data_stage(0xDEAD_B000, 18, false).0[3], 0x0000_0C00);
    assert_eq!(trb::status_stage(false, true).0, [0, 0, 0, 0x0000_1020]);
    assert_eq!(trb::status_stage(true, true).0, [0, 0, 0, 0x0001_1020]);
    assert_eq!(trb::link(0x0000_0001_2345_6780, true).0, [0x2345_6780, 0x0000_0001, 0, 0x0000_1802]);
    let t = trb::normal(0x2000, 8, true, false, false);
    assert_eq!((t.trb_type(), t.cycle()), (1, false), "builders leave the cycle bit to the ring");
}

#[test]
fn command_trbs() {
    assert_eq!(trb::enable_slot(0).0, [0, 0, 0, 0x0000_2400]);
    assert_eq!(trb::enable_slot(3).0, [0, 0, 0, 0x0003_2400]);
    assert_eq!(trb::address_device(0x0010_0040, 5, false).0, [0x0010_0040, 0, 0, 0x0500_2C00]);
    assert_eq!(trb::address_device(0x0010_0040, 5, true).0, [0x0010_0040, 0, 0, 0x0500_2E00]);
    assert_eq!(trb::configure_endpoint(0x0020_0000, 1).0, [0x0020_0000, 0, 0, 0x0100_3000]);
    assert_eq!(trb::evaluate_context(0x0020_0000, 2).0, [0x0020_0000, 0, 0, 0x0200_3400]);
    assert_eq!(trb::reset_endpoint(1, 3).0, [0, 0, 0, 0x0103_3800]);
    assert_eq!(trb::stop_endpoint(2, 5).0, [0, 0, 0, 0x0205_3C00]);
    assert_eq!(trb::set_tr_dequeue(1, 3, 0x0000_0002_0030_0010, true).0, [0x0030_0011, 0x0000_0002, 0, 0x0103_4000]);
    assert_eq!(trb::set_tr_dequeue(1, 3, 0x0030_0010, false).0[0], 0x0030_0010);
    assert_eq!(trb::noop_command().0, [0, 0, 0, 0x0000_5C00]);
    assert_eq!(trb::noop_command().trb_type(), 23);
}

#[test]
fn events() {
    let transfer = Trb([0x0010_0020, 0x0000_0001, (13 << 24) | 496, 0x0503_8001]);
    assert_eq!(
        trb::parse_event(&transfer),
        Event::Transfer { pointer: 0x1_0010_0020, residual: 496, code: 13, slot: 5, dci: 3, event_data: false }
    );
    let event_data = Trb([0xCAFE_F00D, 0x1234_5678, 1 << 24, 0x0503_8005]);
    assert_eq!(
        trb::parse_event(&event_data),
        Event::Transfer { pointer: 0x1234_5678_CAFE_F00D, residual: 0, code: 1, slot: 5, dci: 3, event_data: true }
    );
    let command = Trb([0x0000_4035, 0, (1 << 24) | 0x42, 0x0700_8401]);
    assert_eq!(trb::parse_event(&command), Event::CommandCompletion { pointer: 0x4030, code: 1, slot: 7, parameter: 0x42 });
    let port = Trb([3 << 24, 0, 1 << 24, 0x0000_8801]);
    assert_eq!(trb::parse_event(&port), Event::PortStatusChange { port: 3, code: 1 });
    let host = Trb([0, 0, 21 << 24, (37 << 10) | 1]);
    assert_eq!(trb::parse_event(&host), Event::Other { trb_type: 37 });
}

// ---------------------------------------------------------------------------------------------
// Rings
// ---------------------------------------------------------------------------------------------

#[test]
fn producer_ring_wraps_through_its_link_trb() {
    let mem = memory(4);
    let mut ring = ProducerRing::new(0x1000, 4);
    assert_eq!((ring.enqueue_pointer(), ring.cycle()), (0x1000, true));
    assert_eq!(ring.push(&mem, trb::noop_command()), 0x1000);
    assert_eq!(slot(&mem, 0), [0, 0, 0, 0x5C01], "written with cycle 1");
    assert_eq!(ring.push(&mem, trb::enable_slot(0)), 0x1010);
    assert_eq!(slot(&mem, 3), [0, 0, 0, 0], "the link slot is untouched until the enqueue reaches it");
    let chained = trb::normal(0x9000, 64, false, true, false);
    assert_eq!(ring.push(&mem, chained), 0x1020);
    assert_eq!(slot(&mem, 2), [0x9000, 0, 64, 0x0411]);
    assert_eq!(slot(&mem, 3), [0x1000, 0, 0, 0x1813], "link: back to base, toggle cycle, chain copied, cycle 1");
    assert_eq!((ring.enqueue_pointer(), ring.cycle()), (0x1000, false), "wrapped with the cycle toggled");
    let mut with_cycle = trb::noop_command();
    with_cycle.0[3] |= 1;
    assert_eq!(ring.push(&mem, with_cycle), 0x1000);
    assert_eq!(slot(&mem, 0), [0, 0, 0, 0x5C00], "the ring's cycle (now 0) wins over the caller's bit");
    ring.push(&mem, trb::noop_command());
    ring.push(&mem, trb::noop_command());
    assert_eq!(slot(&mem, 3), [0x1000, 0, 0, 0x1802], "second lap: link written with cycle 0, no chain");
    assert_eq!((ring.enqueue_pointer(), ring.cycle()), (0x1000, true));
}

#[test]
fn producer_ring_free_slots() {
    let mem = memory(4);
    let mut ring = ProducerRing::new(0x1000, 4);
    assert_eq!(ring.free_slots(0x1000), 2, "3 usable slots, one kept empty");
    ring.push(&mem, trb::noop_command());
    assert_eq!(ring.free_slots(0x1000), 1);
    ring.push(&mem, trb::noop_command());
    assert_eq!(ring.free_slots(0x1000), 0, "full");
    assert_eq!(ring.free_slots(0x1020), 2, "the consumer caught up");
    let big = ProducerRing::new(0x40_0000, 256);
    assert_eq!(big.free_slots(0x40_0000), 254);
}

#[test]
fn event_ring_consumes_by_cycle() {
    let mem = memory(4);
    let mut ring = EventRing::new(0x8000, 4);
    assert_eq!(ring.erst_entry(), [0x8000, 0, 4, 0]);
    assert_eq!(ring.pop(&mem), None, "zeroed memory has cycle 0; the consumer expects 1");
    let write = |i: usize, dwords: [u32; 4]| {
        for (k, d) in dwords.iter().enumerate() {
            mem[i * 4 + k].store(*d, Ordering::Relaxed);
        }
    };
    write(0, [0x11, 0, 1 << 24, (33 << 10) | 1]);
    assert_eq!(ring.pop(&mem), Some(Trb([0x11, 0, 1 << 24, (33 << 10) | 1])));
    assert_eq!((ring.dequeue_pointer(), ring.erdp()), (0x8010, 0x8018));
    assert_eq!(ring.pop(&mem), None);
    for i in 1..4 {
        write(i, [i as u32, 0, 1 << 24, (34 << 10) | 1]);
    }
    for i in 1..4 {
        assert_eq!(ring.pop(&mem).map(|t| t.0[0]), Some(i as u32));
    }
    assert_eq!(ring.dequeue_pointer(), 0x8000, "wrapped");
    assert_eq!(ring.pop(&mem), None, "slot 0 still holds the old lap's cycle 1; the consumer now expects 0");
    write(0, [0x99, 0, 1 << 24, 34 << 10]);
    assert_eq!(ring.pop(&mem).map(|t| t.0[0]), Some(0x99));
    let wide = EventRing::new(0x0000_0001_0000_0040, 256);
    assert_eq!(wide.erst_entry(), [0x40, 1, 256, 0]);
}

// ---------------------------------------------------------------------------------------------
// Contexts
// ---------------------------------------------------------------------------------------------

#[test]
fn slot_contexts() {
    let root = SlotContext { speed: 3, context_entries: 1, root_hub_port: 2, ..Default::default() };
    assert_eq!(root.to_dwords(), [0x0830_0000, 0x0002_0000, 0, 0, 0, 0, 0, 0]);
    let hub = SlotContext {
        route_string: 0x12345,
        speed: 4,
        mtt: true,
        hub: true,
        context_entries: 3,
        max_exit_latency: 100,
        root_hub_port: 1,
        num_ports: 4,
        parent_slot: 1,
        parent_port: 2,
        ttt: 1,
        interrupter: 5,
        device_address: 7,
        slot_state: 2,
    };
    let d = hub.to_dwords();
    assert_eq!(d, [0x1E41_2345, 0x0401_0064, 0x0141_0201, 0x1000_0007, 0, 0, 0, 0]);
    assert_eq!(SlotContext::from_dwords(&d), hub);
}

#[test]
fn endpoint_contexts() {
    let bulk = EndpointContext {
        cerr: 3,
        ep_type: EpType::BulkIn,
        max_burst: 15,
        max_packet_size: 1024,
        dequeue: 0x0012_3450,
        dcs: true,
        average_trb_length: 3072,
        ..Default::default()
    };
    let d = bulk.to_dwords();
    assert_eq!(d, [0, 0x0400_0F36, 0x0012_3451, 0, 0x0000_0C00, 0, 0, 0]);
    assert_eq!(EndpointContext::from_dwords(&d), bulk);
    let interrupt = EndpointContext {
        state: 1,
        interval: 6,
        max_esit_payload: 0x1_0008,
        cerr: 3,
        ep_type: EpType::InterruptIn,
        max_packet_size: 8,
        dequeue: 0x0000_0002_0000_0000,
        dcs: true,
        average_trb_length: 8,
        ..Default::default()
    };
    let d = interrupt.to_dwords();
    assert_eq!(d, [0x0106_0001, 0x0008_003E, 0x0000_0001, 0x0000_0002, 0x0008_0008, 0, 0, 0]);
    assert_eq!(EndpointContext::from_dwords(&d), interrupt);
    let streams = EndpointContext { mult: 2, max_pstreams: 5, lsa: true, hid: true, ep_type: EpType::IsochOut, ..Default::default() };
    assert_eq!(streams.to_dwords()[0], (2 << 8) | (5 << 10) | (1 << 15));
    assert_eq!(streams.to_dwords()[1], (1 << 3) | (1 << 7));
    assert_eq!(EndpointContext::from_dwords(&streams.to_dwords()), streams);
}

#[test]
fn input_and_output_context_layout() {
    for csz64 in [false, true] {
        let stride = if csz64 { 16 } else { 8 };
        assert_eq!(context::input_context_len(csz64), 33 * stride);
        let mut input = vec![0u32; context::input_context_len(csz64)];
        context::write_input_control(&mut input, csz64, 0b100, 0b1011, 1, 2, 3);
        assert_eq!(&input[..8], &[0b100, 0b1011, 0, 0, 0, 0, 0, 0x0003_0201]);
        let s = SlotContext { speed: 3, context_entries: 3, root_hub_port: 1, ..Default::default() };
        context::write_slot(&mut input, csz64, &s);
        assert_eq!(&input[stride..stride + 8], &s.to_dwords());
        let ep0 = EndpointContext { cerr: 3, ep_type: EpType::Control, max_packet_size: 64, dequeue: 0x5000, dcs: true, average_trb_length: 8, ..Default::default() };
        context::write_endpoint(&mut input, csz64, 1, &ep0);
        assert_eq!(&input[2 * stride..2 * stride + 8], &ep0.to_dwords(), "DCI 1 is input context entry 2");
        let ep3 = EndpointContext { ep_type: EpType::BulkIn, max_packet_size: 512, ..Default::default() };
        context::write_endpoint(&mut input, csz64, 3, &ep3);
        assert_eq!(&input[4 * stride..4 * stride + 8], &ep3.to_dwords());
        assert!(input[stride + 8..2 * stride].iter().all(|&w| w == 0), "the upper half of a 64-byte context stays zero");

        let mut output = vec![0u32; 32 * stride];
        output[..8].copy_from_slice(&s.to_dwords());
        output[3 * stride..3 * stride + 8].copy_from_slice(&ep3.to_dwords());
        assert_eq!(context::read_output_slot(&output, csz64), s);
        assert_eq!(context::read_output_endpoint(&output, csz64, 3), ep3, "device context entry n is DCI n");
    }
}

#[test]
fn endpoint_numbering_and_types() {
    let cases = [(0x00u8, 1u8), (0x80, 1), (0x81, 3), (0x01, 2), (0x02, 4), (0x82, 5), (0x0F, 30), (0x8F, 31)];
    for (address, dci) in cases {
        assert_eq!(context::dci(address), dci, "endpoint address {address:#04x}");
    }
    let types = [
        ((0x02, 0x81), EpType::BulkIn, 6u8),
        ((0x02, 0x02), EpType::BulkOut, 2),
        ((0x03, 0x83), EpType::InterruptIn, 7),
        ((0x03, 0x03), EpType::InterruptOut, 3),
        ((0x01, 0x81), EpType::IsochIn, 5),
        ((0x05, 0x01), EpType::IsochOut, 1),
        ((0x00, 0x00), EpType::Control, 4),
    ];
    for ((attributes, address), ty, raw) in types {
        assert_eq!(EpType::from_endpoint(attributes, address), ty);
        assert_eq!(ty as u8, raw);
    }
    assert_eq!(EpType::NotValid as u8, 0);
}

#[test]
fn intervals_and_speeds() {
    use EpType::*;
    use Speed::*;
    let cases = [
        (High, InterruptIn, 4u8, 3u8),
        (Super, InterruptIn, 1, 0),
        (High, IsochIn, 1, 0),
        (High, InterruptIn, 0, 0),
        (High, InterruptIn, 17, 15),
        (Full, InterruptIn, 10, 6),
        (Full, InterruptIn, 1, 3),
        (Full, InterruptIn, 0, 3),
        (Full, InterruptIn, 32, 8),
        (Low, InterruptIn, 255, 10),
        (Low, InterruptOut, 128, 10),
        (Full, IsochIn, 1, 3),
        (Full, IsochOut, 4, 6),
        (Full, IsochOut, 12, 14),
        (Full, IsochOut, 20, 15),
        (High, BulkIn, 5, 0),
        (Full, Control, 9, 0),
    ];
    for (speed, ty, b_interval, expected) in cases {
        assert_eq!(context::interval(speed, ty, b_interval), expected, "{speed:?} {ty:?} bInterval {b_interval}");
    }
    let psiv: Vec<_> = (0..=6).map(Speed::from_psiv).collect();
    assert_eq!(psiv, [None, Some(Full), Some(Low), Some(High), Some(Super), Some(SuperPlus), None]);
    let ep0: Vec<_> = [Low, Full, High, Super, SuperPlus].iter().map(|s| s.default_ep0_max_packet()).collect();
    assert_eq!(ep0, [8, 8, 64, 512, 512]);
}

// ---------------------------------------------------------------------------------------------
// Registers
// ---------------------------------------------------------------------------------------------

#[test]
fn port_status_and_control() {
    assert_eq!(regs::portsc_neutral(0xFFFF_FFFF), 0x4E00_FFE9, "never write 1 to PED, a change bit, PR or LWS by accident");
    let v = 0x0022_0E03; // connected, enabled, powered, high speed, connect + reset changes pending
    let p = regs::parse_portsc(v);
    assert!(p.connected && p.enabled && p.powered && !p.reset && !p.over_current);
    assert_eq!((p.link_state, p.speed), (0, 3));
    assert!(p.connect_change && p.reset_change);
    assert!(!p.enable_change && !p.warm_reset_change && !p.over_current_change && !p.link_change && !p.config_error_change);
    assert_eq!(regs::portsc_reset(v), 0x0000_0E11);
    assert_eq!(regs::portsc_clear_changes(v), 0x0022_0E01);
    let resetting = regs::parse_portsc(0x0000_0211 | (5 << 5));
    assert!(resetting.reset && resetting.connected && !resetting.enabled);
    assert_eq!(resetting.link_state, 5);
    let all_changes = regs::parse_portsc(0x00FE_0000);
    assert!(all_changes.connect_change && all_changes.enable_change && all_changes.warm_reset_change);
    assert!(all_changes.over_current_change && all_changes.reset_change && all_changes.link_change && all_changes.config_error_change);
}

#[test]
fn capability_registers() {
    let caps = regs::parse_capabilities(&[0x0100_0020, 0x0800_0840, 0x1020_0041, 0, 0x0200_0005, 0x2003, 0x101F, 0]);
    assert_eq!((caps.caplength, caps.hci_version), (0x20, 0x0100));
    assert_eq!((caps.max_slots, caps.max_interrupters, caps.max_ports), (64, 8, 8));
    assert_eq!((caps.erst_max, caps.max_scratchpad), (4, 34));
    assert!(caps.ac64 && caps.context_64);
    assert_eq!((caps.xecp_offset, caps.doorbell_offset, caps.runtime_offset), (0x800, 0x2000, 0x1000));
    let plain = regs::parse_capabilities(&[0x0110_0040, 0x2000_0120, 0, 0, 0x0000_0000, 0x800, 0x600, 0]);
    assert_eq!((plain.caplength, plain.hci_version, plain.max_slots, plain.max_interrupters, plain.max_ports), (0x40, 0x0110, 0x20, 1, 0x20));
    assert!(!plain.ac64 && !plain.context_64);
    assert_eq!((plain.max_scratchpad, plain.xecp_offset), (0, 0));
    assert_eq!(regs::doorbell_value(0, 0), 0);
    assert_eq!(regs::doorbell_value(3, 0), 3);
    assert_eq!(regs::doorbell_value(5, 0x1234), 0x1234_0005);
}

#[test]
fn extended_capabilities() {
    let mut space = vec![0u32; 0x1000 / 4];
    space[0x800 / 4] = 0x0001_0401; // legacy support, BIOS owned, next +4 dwords
    space[0x810 / 4] = 0x0200_0402; // supported protocol USB 2.0, next +4 dwords
    space[0x814 / 4] = 0x2042_5355; // "USB "
    space[0x818 / 4] = 0x0000_0401; // ports 1..=4
    space[0x820 / 4] = 0x0300_0002; // supported protocol USB 3.0, last
    space[0x824 / 4] = 0x2042_5355;
    space[0x828 / 4] = 0x0000_0405; // ports 5..=8
    space[0x82C / 4] = 0x0000_0000;
    let read = |offset: u32| space[(offset / 4) as usize];
    let caps = regs::walk_extended_caps(read, 0x800);
    assert_eq!(
        caps,
        [
            ExtCap::LegacySupport { offset: 0x800, bios_owned: true, os_owned: false },
            ExtCap::SupportedProtocol { offset: 0x810, major: 2, minor: 0, port_offset: 1, port_count: 4, slot_type: 0 },
            ExtCap::SupportedProtocol { offset: 0x820, major: 3, minor: 0, port_offset: 5, port_count: 4, slot_type: 0 },
        ]
    );
    assert_eq!(regs::legacy_request_os(0x0001_0401), 0x0101_0401);
    assert!(regs::walk_extended_caps(read, 0).is_empty(), "no extended capabilities");

    let dead = |_offset: u32| 0xFFFF_FFFFu32; // a controller that fell off the bus reads all ones
    assert!(regs::walk_extended_caps(dead, 0x800).is_empty(), "all ones ends the walk");
    let endless = |_offset: u32| 0x0000_01C0u32; // ID 0xC0, next +1 dword, forever
    let caps = regs::walk_extended_caps(endless, 0x800);
    assert_eq!(caps.len(), 64, "the walk is bounded");
    assert_eq!(caps[63], ExtCap::Other { id: 0xC0, offset: 0x800 + 63 * 4 });
}
