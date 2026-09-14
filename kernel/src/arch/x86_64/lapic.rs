//! The local APIC (xAPIC MMIO mode; x2APIC support is a later task --
//! brief M1-T5 step 4). Every register is a 32-bit MMIO word, reached
//! through the HHDM at the physical base the MADT reported (Intel SDM
//! Vol. 3A Chapter 10).

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::irq;
use crate::arch::x86_64::trap::TrapFrame;
use crate::mm::hhdm;
use crate::{acpi, kprintln};

// Register byte offsets from the LAPIC's MMIO base (Intel SDM Vol. 3A
// Table 10-1).
const REG_ID: u64 = 0x20;
const REG_EOI: u64 = 0xB0;
const REG_SPURIOUS: u64 = 0xF0;
const REG_TPR: u64 = 0x80;
const REG_LVT_TIMER: u64 = 0x320;
const REG_TIMER_INITIAL_COUNT: u64 = 0x380;
const REG_TIMER_CURRENT_COUNT: u64 = 0x390;
const REG_TIMER_DIVIDE: u64 = 0x3E0;

/// The LAPIC's own hardware spurious-vector interrupt (Intel SDM Vol. 3A
/// 10.9): a benign race between masking a line and the LAPIC already
/// having decided to deliver it. Fixed at 0xFF (every bit of the vector
/// set) rather than a low, driver-assignable number, matching every real
/// firmware/OS convention for it.
const SPURIOUS_VECTOR: u8 = 0xFF;
/// Spurious Interrupt Vector Register bit 8: the actual "local APIC
/// enabled" switch in xAPIC mode, separate from (and required in addition
/// to) `IA32_APIC_BASE`'s global enable bit, which firmware/Limine
/// already leaves set.
const SVR_APIC_ENABLE: u32 = 1 << 8;
/// LVT entry bit 16: masked.
const LVT_MASKED: u32 = 1 << 16;
/// LVT Timer register bit 17: periodic mode (0 would be one-shot).
const LVT_TIMER_PERIODIC: u32 = 1 << 17;
/// Divide Configuration Register encoding for "divide by 16" (Intel SDM
/// Vol. 3A Table 10-10: bits 3,1,0 read `0b0_01_1`, bit 2 always
/// reserved-0).
const DIVIDE_BY_16: u32 = 0b0011;

static BASE: AtomicU64 = AtomicU64::new(0);

fn base() -> u64 {
    let value = BASE.load(Ordering::Acquire);
    debug_assert_ne!(value, 0, "lapic: init was never called");
    value
}

fn read(offset: u64) -> u32 {
    let addr = (base() + offset) as *const u32;
    // SAFETY: `base()` is the LAPIC's MMIO base, reached through the HHDM
    // (`mm::vmm::map_hhdm` unconditionally covers every physical address
    // below 4 GiB, which the LAPIC's fixed-by-firmware window always is);
    // every offset this module reads is a real, 4-byte-aligned, readable
    // LAPIC register (Intel SDM Vol. 3A Table 10-1). `read_volatile`
    // because this is MMIO: the value can change independently of
    // anything this core wrote, and a plain load could be reordered or
    // elided by the compiler.
    unsafe { core::ptr::read_volatile(addr) }
}

fn write(offset: u64, value: u32) {
    let addr = (base() + offset) as *mut u32;
    // SAFETY: same reasoning as `read`, for a register this module only
    // ever writes a value that register's own documented format expects;
    // `write_volatile` so the compiler can't reorder, coalesce or elide a
    // write the hardware needs to actually observe.
    unsafe { core::ptr::write_volatile(addr, value) };
}

/// This CPU's local APIC ID (Intel SDM Vol. 3A 10.4.6): bits 24-31 of the
/// ID register in xAPIC mode.
pub fn id() -> u8 {
    (read(REG_ID) >> 24) as u8
}

/// Signals end-of-interrupt for whichever vector is currently in
/// service. A documented no-op if nothing is (Intel SDM Vol. 3A 10.8.5).
pub fn eoi() {
    write(REG_EOI, 0);
}

/// Counts hardware spurious-vector (0xFF) interrupts since `init` (Intel
/// SDM Vol. 3A 10.9): a benign hardware race that needs no other
/// handling.
static SPURIOUS_INTERRUPTS: AtomicU64 = AtomicU64::new(0);

fn spurious_handler(_frame: &mut TrapFrame) {
    SPURIOUS_INTERRUPTS.fetch_add(1, Ordering::Relaxed);
}

/// How many hardware spurious-vector interrupts this LAPIC has raised
/// since `init`.
pub fn spurious_interrupts() -> u64 {
    SPURIOUS_INTERRUPTS.load(Ordering::Relaxed)
}

/// Programs the LVT timer register: `vector`, periodic or one-shot, and
/// masked or not. Shared by calibration (`time::init`: one-shot, masked)
/// and normal operation (periodic, unmasked).
pub(crate) fn set_timer_lvt(vector: u8, periodic: bool, masked: bool) {
    let mut value = u32::from(vector);
    if periodic {
        value |= LVT_TIMER_PERIODIC;
    }
    if masked {
        value |= LVT_MASKED;
    }
    write(REG_LVT_TIMER, value);
}

/// Sets the timer's divide configuration to "divide by 16". `time::init`
/// always calls this once, before calibrating, and never changes it
/// afterward, so the ticks-per-millisecond calibration measures stays
/// valid once periodic mode starts using the same divisor.
pub(crate) fn set_timer_divide_by_16() {
    write(REG_TIMER_DIVIDE, DIVIDE_BY_16);
}

/// Starts (or restarts) the timer counting down from `count`, at whatever
/// divide/mode `set_timer_divide_by_16`/`set_timer_lvt` last configured.
pub(crate) fn set_timer_initial_count(count: u32) {
    write(REG_TIMER_INITIAL_COUNT, count);
}

/// The timer's current countdown value (Intel SDM Vol. 3A 10.5.4): counts
/// down from the last initial count written.
pub(crate) fn timer_current_count() -> u32 {
    read(REG_TIMER_CURRENT_COUNT)
}

/// Test-only introspection: the timer's currently *programmed* reload
/// value (Intel SDM Vol. 3A 10.5.4: the initial-count register is
/// read/write and always reads back exactly what was last written to it,
/// independent of the separate, ever-decrementing current-count register)
/// -- lets a test confirm the hardware itself holds what `time::init`
/// computed, not just that the kernel's own bookkeeping says so
/// (kernel-review, M1-T5 fix #4).
pub fn timer_initial_count_register() -> u32 {
    read(REG_TIMER_INITIAL_COUNT)
}

/// Brings up the local APIC (brief M1-T5 step 4): finds its MMIO base
/// from the parsed MADT (`acpi::with_info`) -- already reachable through
/// the HHDM's unconditional first-4-GiB coverage, `mm::vmm::map_hhdm`, so
/// nothing new needs mapping here -- sets TPR to 0 (accept every
/// priority), installs a counting-only handler for the hardware spurious
/// vector, then enables the APIC via the spurious-interrupt vector
/// register.
pub fn init() {
    let lapic_address = acpi::with_info(|info| info.madt.lapic_address);
    let virt = hhdm::phys_to_virt(lapic_address);
    BASE.store(virt.as_u64(), Ordering::Release);

    write(REG_TPR, 0);

    irq::register(SPURIOUS_VECTOR, spurious_handler);
    write(REG_SPURIOUS, SVR_APIC_ENABLE | u32::from(SPURIOUS_VECTOR));

    kprintln!("[lapic] id={} base=0x{:x}", id(), lapic_address.as_u64());
}
