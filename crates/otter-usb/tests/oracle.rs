//! Acceptance oracle for brief M10-T0a (otter-usb part 1: descriptors, control requests, HID boot
//! keyboard and mouse, mass-storage Bulk-Only Transport and SCSI), written by the orchestrator.
//! The crate must pass this file unchanged. Expected bytes come from the USB 2.0/3.2 specifications
//! (chapter 9), HID 1.11 (appendix B, the boot protocol), the HID usage tables, the Microsoft
//! keyboard scan code specification (USB usage to scan code set 1), USB Mass Storage Class
//! Bulk-Only Transport 1.0, and SBC-3/SPC-4.
//!
//! API this file relies on:
//!   otter_usb::desc:
//!     parse_device(&[u8]) -> Result<DeviceDescriptor, DescError>
//!       DeviceDescriptor { usb_version, class, subclass, protocol, max_packet_size0, vendor_id,
//!                          product_id, device_version, manufacturer_index, product_index,
//!                          serial_index, num_configurations } + fn ep0_max_packet(&self) -> u16
//!     parse_configuration(&[u8]) -> Result<Configuration, DescError>
//!       Configuration { value, attributes, max_power, interfaces: Vec<Interface> }
//!       Interface { number, alternate, class, subclass, protocol, endpoints: Vec<Endpoint>,
//!                   hid_report_length: Option<u16> }
//!       Endpoint { address, transfer: TransferType, max_packet_size, mult, interval, max_burst }
//!       TransferType { Control, Isochronous, Bulk, Interrupt }
//!     parse_string(&[u8]) -> Result<String, DescError>; parse_languages(&[u8]) -> Result<Vec<u16>, DescError>
//!     DescError { Truncated, BadLength, WrongType }
//!   otter_usb::setup::SetupPacket { request_type, request, value, index, length } with to_bytes() -> [u8; 8]
//!     and constructors get_descriptor, get_hid_report_descriptor, set_address, set_configuration,
//!     hid_set_protocol, hid_set_idle, hid_set_report_output, clear_endpoint_halt, bot_reset, bot_get_max_lun
//!   otter_usb::hid: BootKeyboard::new(), BootKeyboard::report(&mut self, &[u8], &mut Vec<u8>),
//!     leds(num, caps, scroll) -> u8, parse_boot_mouse(&[u8]) -> Option<MouseReport { buttons, dx, dy, wheel }>
//!   otter_usb::msc: Cbw { tag, data_length, direction_in, lun, cb, cb_len } with to_bytes() -> [u8; 31];
//!     test_unit_ready, request_sense, inquiry, read_capacity10, read_capacity16, synchronize_cache
//!     (tag, lun) -> Cbw; read / write (tag, lun, lba: u64, blocks: u32, block_size: u32) -> Option<Cbw>;
//!     parse_csw(&[u8], expected_tag, data_length) -> Result<Csw { tag, residue, status }, CswError>
//!     CswStatus { Passed, Failed, PhaseError }; CswError { WrongLength, BadSignature, TagMismatch,
//!     BadStatus, ResidueTooLarge }; parse_capacity10 / parse_capacity16 (&[u8]) -> Option<Capacity { last_lba, block_size }>;
//!     parse_sense(&[u8]) -> Option<Sense { key, asc, ascq }>;
//!     parse_inquiry(&[u8]) -> Option<Inquiry { qualifier, peripheral_type, removable, vendor, product, revision }>
//!
//! Semantics beyond the specifications:
//! - parse_configuration reads exactly wTotalLength bytes (more input is ignored, less is
//!   Truncated); every descriptor needs bLength >= 2 and must fit (else BadLength / Truncated);
//!   interface, endpoint, HID and SuperSpeed-companion descriptors shorter than their fixed size
//!   are BadLength; each (interface, alternate) pair is its own Interface in order; endpoints,
//!   the HID descriptor's report-descriptor length and a companion's bMaxBurst attach to the
//!   interface / endpoint before them; every other descriptor type is skipped. The first
//!   descriptor must be a configuration (type 2), a device descriptor type 1 (else WrongType).
//! - BootKeyboard::report takes an 8-byte boot report (shorter reports are ignored, bytes past 8
//!   are ignored) and appends scan code set 1 bytes: first the breaks (modifiers in bit order, then
//!   keys in the previous report's order), then the makes (modifiers in bit order, then keys in the
//!   new report's order). A break is the make with 0x80 set on its last byte (E0 prefixes kept).
//!   A report with any key slot 0x01..=0x03 (ErrorRollOver, POSTFail, ErrorUndefined) keeps the
//!   previous keys but still applies modifier changes. Duplicate usages count once; usages
//!   without a translation (and Pause) produce nothing. PrintScreen is E0 37 / E0 B7 (no fake
//!   shift). Mouse: buttons are bits 0-2, dx/dy/wheel are signed bytes (dy positive = down, as
//!   USB reports it); wheel is 0 when the report has 3 bytes; fewer than 3 bytes is None.
//! - msc::read/write use READ(10)/WRITE(10) when blocks <= 0xFFFF and the last block is at most
//!   LBA 0xFFFF_FFFF, else READ(16)/WRITE(16); None when blocks is 0 or blocks * block_size does
//!   not fit in u32. CSW checks follow BOT 6.3: 13 bytes, signature, tag (valid), then status 0/1
//!   with residue <= data_length, or status 2 (meaningful). Inquiry strings are trimmed of
//!   trailing spaces.

use otter_usb::desc::{self, DescError, TransferType};
use otter_usb::hid::{self, BootKeyboard};
use otter_usb::msc::{self, CswError, CswStatus};
use otter_usb::setup::SetupPacket;

// ---------------------------------------------------------------------------------------------
// Descriptors
// ---------------------------------------------------------------------------------------------

#[test]
fn device_descriptor() {
    let d = desc::parse_device(&[
        0x12, 0x01, 0x10, 0x03, 0x00, 0x00, 0x00, 0x09, 0x81, 0x07, 0x81, 0x55, 0x00, 0x01, 0x01, 0x02, 0x03, 0x01,
    ])
    .unwrap();
    assert_eq!(d.usb_version, 0x0310);
    assert_eq!((d.class, d.subclass, d.protocol), (0, 0, 0));
    assert_eq!(d.max_packet_size0, 9);
    assert_eq!(d.ep0_max_packet(), 512, "USB 3: bMaxPacketSize0 is an exponent");
    assert_eq!((d.vendor_id, d.product_id, d.device_version), (0x0781, 0x5581, 0x0100));
    assert_eq!((d.manufacturer_index, d.product_index, d.serial_index, d.num_configurations), (1, 2, 3, 1));

    let usb2 = desc::parse_device(&[
        0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, 0x27, 0x06, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x0B, 0x01,
    ])
    .unwrap();
    assert_eq!(usb2.ep0_max_packet(), 64, "USB 2: a byte count");
    assert_eq!(desc::parse_device(&[0x12, 0x01, 0x00, 0x02]).unwrap_err(), DescError::Truncated);
    let mut wrong = [0u8; 18];
    wrong[0] = 0x12;
    wrong[1] = 0x02;
    assert_eq!(desc::parse_device(&wrong).unwrap_err(), DescError::WrongType);
    let mut short = [0u8; 18];
    short[0] = 0x08;
    short[1] = 0x01;
    assert_eq!(desc::parse_device(&short).unwrap_err(), DescError::BadLength);
}

const KEYBOARD_CONFIG: [u8; 34] = [
    0x09, 0x02, 0x22, 0x00, 0x01, 0x01, 0x04, 0xA0, 0x32, // configuration: 34 bytes, 1 interface, value 1
    0x09, 0x04, 0x00, 0x00, 0x01, 0x03, 0x01, 0x01, 0x00, // interface 0: HID, boot, keyboard, 1 endpoint
    0x09, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, 0x3F, 0x00, // HID 1.11, one report descriptor of 63 bytes
    0x07, 0x05, 0x81, 0x03, 0x08, 0x00, 0x0A, // EP1 IN, interrupt, 8 bytes, interval 10
];

#[test]
fn keyboard_configuration() {
    let c = desc::parse_configuration(&KEYBOARD_CONFIG).unwrap();
    assert_eq!((c.value, c.attributes, c.max_power), (1, 0xA0, 0x32));
    assert_eq!(c.interfaces.len(), 1);
    let i = &c.interfaces[0];
    assert_eq!((i.number, i.alternate, i.class, i.subclass, i.protocol), (0, 0, 3, 1, 1));
    assert_eq!(i.hid_report_length, Some(63));
    assert_eq!(i.endpoints.len(), 1);
    let e = &i.endpoints[0];
    assert_eq!((e.address, e.transfer, e.max_packet_size, e.mult, e.interval, e.max_burst), (0x81, TransferType::Interrupt, 8, 0, 10, 0));

    let mut longer = KEYBOARD_CONFIG.to_vec();
    longer.extend_from_slice(&[0x07, 0x05, 0x82, 0x03, 0x08, 0x00, 0x0A]);
    assert_eq!(desc::parse_configuration(&longer).unwrap().interfaces[0].endpoints.len(), 1, "only wTotalLength bytes count");
}

#[test]
fn superspeed_storage_configuration() {
    let bytes = [
        0x09, 0x02, 0x2C, 0x00, 0x01, 0x01, 0x00, 0x80, 0x70, //
        0x09, 0x04, 0x00, 0x00, 0x02, 0x08, 0x06, 0x50, 0x00, // mass storage, SCSI, bulk-only
        0x07, 0x05, 0x81, 0x02, 0x00, 0x04, 0x00, // EP1 IN bulk 1024
        0x06, 0x30, 0x0F, 0x00, 0x00, 0x00, // companion: burst 15
        0x07, 0x05, 0x02, 0x02, 0x00, 0x04, 0x00, // EP2 OUT bulk 1024
        0x06, 0x30, 0x03, 0x00, 0x00, 0x00, // companion: burst 3
    ];
    let c = desc::parse_configuration(&bytes).unwrap();
    let i = &c.interfaces[0];
    assert_eq!((i.class, i.subclass, i.protocol, i.hid_report_length), (8, 6, 0x50, None));
    let summary: Vec<_> = i.endpoints.iter().map(|e| (e.address, e.transfer, e.max_packet_size, e.max_burst)).collect();
    assert_eq!(summary, [(0x81, TransferType::Bulk, 1024, 15), (0x02, TransferType::Bulk, 1024, 3)]);
}

#[test]
fn composite_configuration_with_alternates_and_class_descriptors() {
    let bytes = [
        0x09, 0x02, 0x58, 0x00, 0x02, 0x01, 0x00, 0x80, 0xFA, //
        0x08, 0x0B, 0x00, 0x02, 0x02, 0x06, 0x00, 0x00, // interface association
        0x09, 0x04, 0x00, 0x00, 0x01, 0x02, 0x06, 0x00, 0x00, // CDC ECM control
        0x05, 0x24, 0x00, 0x10, 0x01, // CDC header
        0x05, 0x24, 0x06, 0x00, 0x01, // CDC union
        0x0D, 0x24, 0x0F, 0x04, 0x00, 0x00, 0x00, 0x00, 0xEA, 0x05, 0x00, 0x00, 0x00, // CDC Ethernet
        0x07, 0x05, 0x83, 0x03, 0x10, 0x00, 0x08, // EP3 IN interrupt 16
        0x09, 0x04, 0x01, 0x00, 0x00, 0x0A, 0x00, 0x00, 0x00, // data interface, alt 0, no endpoints
        0x09, 0x04, 0x01, 0x01, 0x02, 0x0A, 0x00, 0x00, 0x00, // data interface, alt 1
        0x07, 0x05, 0x81, 0x02, 0x00, 0x02, 0x00, // EP1 IN bulk 512
        0x07, 0x05, 0x02, 0x02, 0x00, 0x02, 0x00, // EP2 OUT bulk 512
    ];
    let c = desc::parse_configuration(&bytes).unwrap();
    let shape: Vec<_> = c.interfaces.iter().map(|i| (i.number, i.alternate, i.class, i.endpoints.len())).collect();
    assert_eq!(shape, [(0, 0, 2, 1), (1, 0, 0x0A, 0), (1, 1, 0x0A, 2)]);
    assert_eq!(c.interfaces[0].endpoints[0].address, 0x83);
    assert_eq!(c.interfaces[2].endpoints[1].address, 0x02);
}

#[test]
fn high_bandwidth_isochronous_endpoint() {
    let bytes = [
        0x09, 0x02, 0x19, 0x00, 0x01, 0x01, 0x00, 0x80, 0x32, //
        0x09, 0x04, 0x00, 0x01, 0x01, 0x0E, 0x02, 0x00, 0x00, //
        0x07, 0x05, 0x81, 0x05, 0x00, 0x14, 0x01, // EP1 IN isochronous, 1024 bytes x 3 transactions
    ];
    let e = desc::parse_configuration(&bytes).unwrap().interfaces[0].endpoints[0].clone();
    assert_eq!((e.transfer, e.max_packet_size, e.mult, e.interval), (TransferType::Isochronous, 1024, 2, 1));
}

#[test]
fn malformed_configurations() {
    let mut short_total = KEYBOARD_CONFIG.to_vec();
    short_total.truncate(30);
    assert_eq!(desc::parse_configuration(&short_total).unwrap_err(), DescError::Truncated, "wTotalLength beyond the input");

    let mut zero_len = KEYBOARD_CONFIG;
    zero_len[18] = 0x00; // the HID descriptor's bLength
    assert_eq!(desc::parse_configuration(&zero_len).unwrap_err(), DescError::BadLength, "bLength 0 would never advance");

    let mut past_end = KEYBOARD_CONFIG;
    past_end[27] = 0x09; // the endpoint claims 9 bytes; only 7 remain
    assert_eq!(desc::parse_configuration(&past_end).unwrap_err(), DescError::Truncated);

    let mut short_interface = KEYBOARD_CONFIG.to_vec();
    short_interface[9] = 0x05;
    short_interface.drain(14..18);
    short_interface[2] = 30;
    assert_eq!(desc::parse_configuration(&short_interface).unwrap_err(), DescError::BadLength, "an interface needs 9 bytes");

    let mut not_config = KEYBOARD_CONFIG;
    not_config[1] = 0x01;
    assert_eq!(desc::parse_configuration(&not_config).unwrap_err(), DescError::WrongType);
    assert_eq!(desc::parse_configuration(&[0x09, 0x02, 0x09]).unwrap_err(), DescError::Truncated);
}

#[test]
fn string_descriptors() {
    assert_eq!(desc::parse_languages(&[0x04, 0x03, 0x09, 0x04]).unwrap(), [0x0409]);
    assert_eq!(desc::parse_languages(&[0x06, 0x03, 0x09, 0x04, 0x07, 0x04]).unwrap(), [0x0409, 0x0407]);
    let otter = [0x0C, 0x03, b'O', 0, b't', 0, b't', 0, b'e', 0, b'r', 0];
    assert_eq!(desc::parse_string(&otter).unwrap(), "Otter");
    let accented = [0x06, 0x03, 0xE9, 0x00, 0x3D, 0xD8]; // "é" then an unpaired high surrogate
    assert_eq!(desc::parse_string(&accented).unwrap(), "é\u{FFFD}");
    let pair = [0x06, 0x03, 0x3D, 0xD8, 0x26, 0xDC]; // U+1F426 bird as a surrogate pair
    assert_eq!(desc::parse_string(&pair).unwrap(), "\u{1F426}");
    assert_eq!(desc::parse_string(&[0x0C, 0x03, b'O', 0]).unwrap_err(), DescError::Truncated);
    assert_eq!(desc::parse_string(&[0x05, 0x03, b'O', 0, 0]).unwrap_err(), DescError::BadLength, "odd length");
    assert_eq!(desc::parse_string(&[0x04, 0x02, b'O', 0]).unwrap_err(), DescError::WrongType);
}

// ---------------------------------------------------------------------------------------------
// Control requests
// ---------------------------------------------------------------------------------------------

#[test]
fn setup_packets() {
    let cases: [(SetupPacket, [u8; 8]); 12] = [
        (SetupPacket::get_descriptor(1, 0, 0, 18), [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00]),
        (SetupPacket::get_descriptor(2, 0, 0, 9), [0x80, 0x06, 0x00, 0x02, 0x00, 0x00, 0x09, 0x00]),
        (SetupPacket::get_descriptor(3, 2, 0x0409, 255), [0x80, 0x06, 0x02, 0x03, 0x09, 0x04, 0xFF, 0x00]),
        (SetupPacket::get_hid_report_descriptor(1, 63), [0x81, 0x06, 0x00, 0x22, 0x01, 0x00, 0x3F, 0x00]),
        (SetupPacket::set_address(5), [0x00, 0x05, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (SetupPacket::set_configuration(1), [0x00, 0x09, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (SetupPacket::hid_set_protocol(0, true), [0x21, 0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (SetupPacket::hid_set_protocol(2, false), [0x21, 0x0B, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00]),
        (SetupPacket::hid_set_idle(0, 125, 0), [0x21, 0x0A, 0x00, 0x7D, 0x00, 0x00, 0x00, 0x00]),
        (SetupPacket::hid_set_report_output(0, 0, 1), [0x21, 0x09, 0x00, 0x02, 0x00, 0x00, 0x01, 0x00]),
        (SetupPacket::clear_endpoint_halt(0x81), [0x02, 0x01, 0x00, 0x00, 0x81, 0x00, 0x00, 0x00]),
        (SetupPacket::bot_reset(0), [0x21, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
    ];
    for (i, (packet, bytes)) in cases.iter().enumerate() {
        assert_eq!(&packet.to_bytes(), bytes, "case {i}");
    }
    let lun = SetupPacket::bot_get_max_lun(1);
    assert_eq!(lun.to_bytes(), [0xA1, 0xFE, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00]);
    assert_eq!((lun.request_type, lun.request, lun.value, lun.index, lun.length), (0xA1, 0xFE, 0, 1, 1));
}

// ---------------------------------------------------------------------------------------------
// HID boot keyboard -> scan code set 1
// ---------------------------------------------------------------------------------------------

fn kb_report(modifiers: u8, keys: &[u8]) -> [u8; 8] {
    let mut r = [0u8; 8];
    r[0] = modifiers;
    r[2..2 + keys.len()].copy_from_slice(keys);
    r
}

fn feed(kb: &mut BootKeyboard, modifiers: u8, keys: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    kb.report(&kb_report(modifiers, keys), &mut out);
    out
}

/// (usage, make bytes) from the Microsoft USB-to-set-1 table.
const TABLE: &[(u8, &[u8])] = &[
    (0x04, &[0x1E]), (0x05, &[0x30]), (0x06, &[0x2E]), (0x07, &[0x20]), (0x08, &[0x12]), (0x09, &[0x21]),
    (0x0A, &[0x22]), (0x0B, &[0x23]), (0x0C, &[0x17]), (0x0D, &[0x24]), (0x0E, &[0x25]), (0x0F, &[0x26]),
    (0x10, &[0x32]), (0x11, &[0x31]), (0x12, &[0x18]), (0x13, &[0x19]), (0x14, &[0x10]), (0x15, &[0x13]),
    (0x16, &[0x1F]), (0x17, &[0x14]), (0x18, &[0x16]), (0x19, &[0x2F]), (0x1A, &[0x11]), (0x1B, &[0x2D]),
    (0x1C, &[0x15]), (0x1D, &[0x2C]),
    (0x1E, &[0x02]), (0x1F, &[0x03]), (0x20, &[0x04]), (0x21, &[0x05]), (0x22, &[0x06]), (0x23, &[0x07]),
    (0x24, &[0x08]), (0x25, &[0x09]), (0x26, &[0x0A]), (0x27, &[0x0B]),
    (0x28, &[0x1C]), (0x29, &[0x01]), (0x2A, &[0x0E]), (0x2B, &[0x0F]), (0x2C, &[0x39]), (0x2D, &[0x0C]),
    (0x2E, &[0x0D]), (0x2F, &[0x1A]), (0x30, &[0x1B]), (0x31, &[0x2B]), (0x32, &[0x2B]), (0x33, &[0x27]),
    (0x34, &[0x28]), (0x35, &[0x29]), (0x36, &[0x33]), (0x37, &[0x34]), (0x38, &[0x35]), (0x39, &[0x3A]),
    (0x3A, &[0x3B]), (0x3B, &[0x3C]), (0x3C, &[0x3D]), (0x3D, &[0x3E]), (0x3E, &[0x3F]), (0x3F, &[0x40]),
    (0x40, &[0x41]), (0x41, &[0x42]), (0x42, &[0x43]), (0x43, &[0x44]), (0x44, &[0x57]), (0x45, &[0x58]),
    (0x46, &[0xE0, 0x37]), (0x47, &[0x46]),
    (0x49, &[0xE0, 0x52]), (0x4A, &[0xE0, 0x47]), (0x4B, &[0xE0, 0x49]), (0x4C, &[0xE0, 0x53]),
    (0x4D, &[0xE0, 0x4F]), (0x4E, &[0xE0, 0x51]), (0x4F, &[0xE0, 0x4D]), (0x50, &[0xE0, 0x4B]),
    (0x51, &[0xE0, 0x50]), (0x52, &[0xE0, 0x48]), (0x53, &[0x45]), (0x54, &[0xE0, 0x35]), (0x55, &[0x37]),
    (0x56, &[0x4A]), (0x57, &[0x4E]), (0x58, &[0xE0, 0x1C]), (0x59, &[0x4F]), (0x5A, &[0x50]),
    (0x5B, &[0x51]), (0x5C, &[0x4B]), (0x5D, &[0x4C]), (0x5E, &[0x4D]), (0x5F, &[0x47]), (0x60, &[0x48]),
    (0x61, &[0x49]), (0x62, &[0x52]), (0x63, &[0x53]), (0x64, &[0x56]), (0x65, &[0xE0, 0x5D]),
];

fn brk(make: &[u8]) -> Vec<u8> {
    let mut b = make.to_vec();
    *b.last_mut().unwrap() |= 0x80;
    b
}

#[test]
fn every_key_in_the_table() {
    let mut kb = BootKeyboard::new();
    for &(usage, make) in TABLE {
        assert_eq!(feed(&mut kb, 0, &[usage]), make, "make of usage {usage:#04x}");
        assert_eq!(feed(&mut kb, 0, &[]), brk(make), "break of usage {usage:#04x}");
    }
}

#[test]
fn modifiers_in_bit_order() {
    let mut kb = BootKeyboard::new();
    let makes: [&[u8]; 8] = [&[0x1D], &[0x2A], &[0x38], &[0xE0, 0x5B], &[0xE0, 0x1D], &[0x36], &[0xE0, 0x38], &[0xE0, 0x5C]];
    for (bit, make) in makes.iter().enumerate() {
        assert_eq!(feed(&mut kb, 1 << bit, &[]), make.to_vec(), "modifier bit {bit}");
        assert_eq!(feed(&mut kb, 0, &[]), brk(make), "modifier bit {bit} released");
    }
    assert_eq!(feed(&mut kb, 0xD0, &[]), [0xE0, 0x1D, 0xE0, 0x38, 0xE0, 0x5C], "RCtrl, RAlt, RGUI together");
    assert_eq!(feed(&mut kb, 0x00, &[]), [0xE0, 0x9D, 0xE0, 0xB8, 0xE0, 0xDC]);
}

#[test]
fn breaks_before_makes() {
    let mut kb = BootKeyboard::new();
    assert_eq!(feed(&mut kb, 0x02, &[0x04]), [0x2A, 0x1E], "shift then a: 'A'");
    assert_eq!(feed(&mut kb, 0x00, &[0x04, 0x05]), [0xAA, 0x30], "shift released, b pressed, a still held");
    assert_eq!(feed(&mut kb, 0x01, &[0x05, 0x06]), [0x9E, 0x1D, 0x2E], "a released; then ctrl and c pressed");
    assert_eq!(feed(&mut kb, 0x01, &[0x06, 0x05]), [], "same keys in another order: nothing");
    assert_eq!(feed(&mut kb, 0x00, &[]), [0x9D, 0xAE, 0xB0], "modifier breaks, then keys in the previous report's order (c, b)");
}

#[test]
fn rollover_keeps_keys_but_applies_modifiers() {
    let mut kb = BootKeyboard::new();
    assert_eq!(feed(&mut kb, 0, &[0x04]), [0x1E]);
    assert_eq!(feed(&mut kb, 0x02, &[1, 1, 1, 1, 1, 1]), [0x2A], "ErrorRollOver: a stays held, shift changes");
    assert_eq!(feed(&mut kb, 0x02, &[0x04, 0x05]), [0x30]);
    assert_eq!(feed(&mut kb, 0x02, &[0x04, 0x02]), [], "POSTFail in one slot: keys unchanged");
    assert_eq!(feed(&mut kb, 0x00, &[]), [0xAA, 0x9E, 0xB0]);
}

#[test]
fn odd_reports() {
    let mut kb = BootKeyboard::new();
    assert_eq!(feed(&mut kb, 0, &[0x04, 0x04]), [0x1E], "a duplicate usage counts once");
    assert_eq!(feed(&mut kb, 0, &[]), [0x9E]);
    assert_eq!(feed(&mut kb, 0, &[0x48, 0x66, 0xE8]), [], "Pause and untranslated usages produce nothing");
    assert_eq!(feed(&mut kb, 0, &[]), []);
    let mut out = Vec::new();
    kb.report(&[0x00, 0x00, 0x04], &mut out);
    assert!(out.is_empty(), "a report shorter than 8 bytes is ignored");
    kb.report(&[0x00, 0x00, 0x04, 0, 0, 0, 0, 0, 0x05], &mut out);
    assert_eq!(out, [0x1E], "bytes past 8 are ignored");
    let six = [0x04, 0x05, 0x06, 0x07, 0x08, 0x09];
    let mut kb = BootKeyboard::new();
    assert_eq!(feed(&mut kb, 0, &six), [0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21]);
}

#[test]
fn keyboard_leds_and_mouse() {
    assert_eq!(hid::leds(false, false, false), 0);
    assert_eq!(hid::leds(true, false, false), 0b001);
    assert_eq!(hid::leds(false, true, false), 0b010);
    assert_eq!(hid::leds(true, true, true), 0b111);

    let m = hid::parse_boot_mouse(&[0x01, 0x05, 0xFB]).unwrap();
    assert_eq!((m.buttons, m.dx, m.dy, m.wheel), (1, 5, -5, 0));
    let m = hid::parse_boot_mouse(&[0xFF, 0x80, 0x7F, 0xFF]).unwrap();
    assert_eq!((m.buttons, m.dx, m.dy, m.wheel), (0x07, -128, 127, -1));
    assert!(hid::parse_boot_mouse(&[0x01, 0x02]).is_none());
}

// ---------------------------------------------------------------------------------------------
// Mass storage: Bulk-Only Transport and SCSI
// ---------------------------------------------------------------------------------------------

fn cbw_bytes(tag: u32, len: u32, flags: u8, lun: u8, cb: &[u8]) -> [u8; 31] {
    let mut b = [0u8; 31];
    b[0..4].copy_from_slice(b"USBC");
    b[4..8].copy_from_slice(&tag.to_le_bytes());
    b[8..12].copy_from_slice(&len.to_le_bytes());
    b[12] = flags;
    b[13] = lun;
    b[14] = cb.len() as u8;
    b[15..15 + cb.len()].copy_from_slice(cb);
    b
}

#[test]
fn scsi_commands_in_cbws() {
    let t = 0x1234_5678;
    assert_eq!(msc::inquiry(t, 0).to_bytes(), cbw_bytes(t, 36, 0x80, 0, &[0x12, 0, 0, 0, 36, 0]));
    assert_eq!(msc::test_unit_ready(t, 0).to_bytes(), cbw_bytes(t, 0, 0x00, 0, &[0; 6]));
    assert_eq!(msc::request_sense(t, 2).to_bytes(), cbw_bytes(t, 18, 0x80, 2, &[0x03, 0, 0, 0, 18, 0]));
    assert_eq!(msc::read_capacity10(t, 0).to_bytes(), cbw_bytes(t, 8, 0x80, 0, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
    assert_eq!(
        msc::read_capacity16(t, 0).to_bytes(),
        cbw_bytes(t, 32, 0x80, 0, &[0x9E, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0])
    );
    assert_eq!(msc::synchronize_cache(t, 0).to_bytes(), cbw_bytes(t, 0, 0x00, 0, &[0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
    let c = msc::inquiry(7, 3);
    assert_eq!((c.tag, c.data_length, c.direction_in, c.lun, c.cb_len), (7, 36, true, 3, 6));
}

#[test]
fn read_and_write_pick_the_command_size() {
    let r = msc::read(1, 0, 0x0102_0304, 8, 512).unwrap();
    assert_eq!(r.to_bytes(), cbw_bytes(1, 4096, 0x80, 0, &[0x28, 0, 0x01, 0x02, 0x03, 0x04, 0, 0x00, 0x08, 0]));
    let w = msc::write(2, 1, 7, 1, 4096).unwrap();
    assert_eq!(w.to_bytes(), cbw_bytes(2, 4096, 0x00, 1, &[0x2A, 0, 0, 0, 0, 7, 0, 0, 1, 0]));
    let last32 = msc::read(3, 0, 0xFFFF_FFF0, 16, 512).unwrap();
    assert_eq!(last32.cb[0], 0x28, "ends exactly at LBA 0xFFFF_FFFF: still READ(10)");
    let over = msc::read(4, 0, 0xFFFF_FFF0, 0x20, 512).unwrap();
    assert_eq!(
        over.to_bytes(),
        cbw_bytes(4, 16384, 0x80, 0, &[0x88, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xF0, 0, 0, 0, 0x20, 0, 0])
    );
    let many = msc::write(5, 0, 5, 70_000, 512).unwrap();
    assert_eq!(
        many.to_bytes(),
        cbw_bytes(5, 35_840_000, 0x00, 0, &[0x8A, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0x00, 0x01, 0x11, 0x70, 0, 0])
    );
    let big_lba = msc::read(6, 0, 0x0000_0012_3456_789A, 1, 4096).unwrap();
    assert_eq!(&big_lba.cb[..10], &[0x88, 0, 0x00, 0x00, 0x00, 0x12, 0x34, 0x56, 0x78, 0x9A]);
    assert!(msc::read(7, 0, 0, 0, 512).is_none(), "zero blocks");
    assert!(msc::read(8, 0, 0, 0x0100_0000, 512).is_none(), "8 GiB does not fit a CBW length");
}

fn csw(tag: u32, residue: u32, status: u8) -> Vec<u8> {
    let mut b = b"USBS".to_vec();
    b.extend_from_slice(&tag.to_le_bytes());
    b.extend_from_slice(&residue.to_le_bytes());
    b.push(status);
    b
}

#[test]
fn csw_validity_and_meaning() {
    let ok = msc::parse_csw(&csw(9, 0, 0), 9, 512).unwrap();
    assert_eq!((ok.tag, ok.residue, ok.status), (9, 0, CswStatus::Passed));
    assert_eq!(msc::parse_csw(&csw(9, 512, 1), 9, 512).unwrap().status, CswStatus::Failed);
    assert_eq!(msc::parse_csw(&csw(9, 9999, 2), 9, 512).unwrap().status, CswStatus::PhaseError, "phase error is meaningful at any residue");
    assert_eq!(msc::parse_csw(&csw(9, 513, 0), 9, 512).unwrap_err(), CswError::ResidueTooLarge);
    assert_eq!(msc::parse_csw(&csw(9, 0, 3), 9, 512).unwrap_err(), CswError::BadStatus);
    assert_eq!(msc::parse_csw(&csw(8, 0, 0), 9, 512).unwrap_err(), CswError::TagMismatch);
    let mut sig = csw(9, 0, 0);
    sig[3] = b'C';
    assert_eq!(msc::parse_csw(&sig, 9, 512).unwrap_err(), CswError::BadSignature);
    assert_eq!(msc::parse_csw(&csw(9, 0, 0)[..12], 9, 512).unwrap_err(), CswError::WrongLength);
    let mut long = csw(9, 0, 0);
    long.push(0);
    assert_eq!(msc::parse_csw(&long, 9, 512).unwrap_err(), CswError::WrongLength);
}

#[test]
fn capacity_sense_and_inquiry_data() {
    let c = msc::parse_capacity10(&[0x00, 0x01, 0xFF, 0xFF, 0x00, 0x00, 0x02, 0x00]).unwrap();
    assert_eq!((c.last_lba, c.block_size), (0x0001_FFFF, 512));
    assert!(msc::parse_capacity10(&[0; 7]).is_none());
    let mut c16 = [0u8; 32];
    c16[..12].copy_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0x10, 0]);
    let c = msc::parse_capacity16(&c16).unwrap();
    assert_eq!((c.last_lba, c.block_size), (0x1_0000_0000, 4096));
    assert!(msc::parse_capacity16(&c16[..11]).is_none());

    let fixed = [0x70, 0, 0x05, 0, 0, 0, 0, 0x0A, 0, 0, 0, 0, 0x24, 0x00, 0, 0, 0, 0];
    let s = msc::parse_sense(&fixed).unwrap();
    assert_eq!((s.key, s.asc, s.ascq), (5, 0x24, 0));
    let mut deferred = fixed;
    deferred[0] = 0xF1; // valid bit + deferred error
    deferred[2] = 0xE2; // filemark/EOM/ILI flags above the key
    let s = msc::parse_sense(&deferred).unwrap();
    assert_eq!((s.key, s.asc, s.ascq), (2, 0x24, 0));
    let s = msc::parse_sense(&[0x72, 0x06, 0x28, 0x00, 0, 0, 0, 0]).unwrap();
    assert_eq!((s.key, s.asc, s.ascq), (6, 0x28, 0), "descriptor format");
    assert!(msc::parse_sense(&fixed[..13]).is_none(), "fixed format needs 14 bytes");
    assert!(msc::parse_sense(&[0x00; 18]).is_none(), "unknown response code");

    let mut inq = vec![0x00, 0x80, 0x06, 0x02, 0x1F, 0x00, 0x00, 0x00];
    inq.extend_from_slice(b"Otter   Boot Stick      1.00");
    let i = msc::parse_inquiry(&inq).unwrap();
    assert_eq!((i.qualifier, i.peripheral_type, i.removable), (0, 0, true));
    assert_eq!((i.vendor.as_str(), i.product.as_str(), i.revision.as_str()), ("Otter", "Boot Stick", "1.00"));
    let mut none = inq.clone();
    none[0] = 0x7F;
    none[1] = 0x00;
    let i = msc::parse_inquiry(&none).unwrap();
    assert_eq!((i.qualifier, i.peripheral_type, i.removable), (3, 0x1F, false));
    assert!(msc::parse_inquiry(&inq[..35]).is_none());
}

#[test]
fn random_descriptors_never_panic() {
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let bases: [&[u8]; 1] = [&KEYBOARD_CONFIG];
    for _ in 0..20_000 {
        let mut m = bases[0].to_vec();
        for _ in 0..1 + next() % 4 {
            let i = (next() as usize) % m.len();
            m[i] = next() as u8;
        }
        let len = (next() as usize) % (m.len() + 1);
        let _ = desc::parse_configuration(&m[..len]);
        let _ = desc::parse_configuration(&m);
        let _ = desc::parse_device(&m);
        let _ = desc::parse_string(&m);
        let _ = msc::parse_sense(&m);
        let _ = msc::parse_inquiry(&m);
        let _ = hid::parse_boot_mouse(&m);
        let mut kb = BootKeyboard::new();
        let mut out = Vec::new();
        kb.report(&m, &mut out);
    }
}
