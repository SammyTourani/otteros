//! The kernel's own 256-entry Interrupt Descriptor Table. Every vector
//! gets an interrupt gate pointing at its stub in `interrupts::STUBS`:
//! vectors 0-31 are the CPU-defined exceptions (brief M1-T1), 32-255
//! become IRQ handlers in a later task but already need a present gate so
//! a stray one doesn't fault instead of just landing in `trap_dispatch`.

use core::mem::size_of;

use crate::arch::x86_64::gdt::KERNEL_CODE_SELECTOR;
use crate::arch::x86_64::interrupts::STUBS;
use crate::kprintln;

const GATE_PRESENT: u8 = 0x80;
/// 64-bit interrupt gate (type `0b1110`); this codebase never uses trap
/// gates (`0b1111`, which leave `IF` alone) since interrupts stay off
/// (`cli`) for the whole of M1 regardless of gate type.
const GATE_TYPE_INTERRUPT: u8 = 0x0E;

/// One IDT gate (Intel SDM Vol. 3A, Figure 6-8): a 64-bit far pointer to
/// the handler (`offset_*`, split across three fields for historical
/// reasons), the code selector to run it in, an optional IST stack index,
/// and the present/DPL/type attribute byte.
#[repr(C)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const MISSING: IdtEntry = IdtEntry {
        offset_low: 0,
        selector: 0,
        ist: 0,
        type_attr: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn new(handler: u64, ist: u8, dpl: u8) -> Self {
        Self {
            offset_low: handler as u16,
            selector: KERNEL_CODE_SELECTOR,
            ist,
            type_attr: GATE_PRESENT | (dpl << 5) | GATE_TYPE_INTERRUPT,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }

    fn present(&self) -> bool {
        self.type_attr & GATE_PRESENT != 0
    }

    fn ist_index(&self) -> u8 {
        self.ist & 0x7
    }

    fn dpl(&self) -> u8 {
        (self.type_attr >> 5) & 0x3
    }
}

#[repr(C, packed)]
struct DtPointer {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; 256] = [IdtEntry::MISSING; 256];

/// The Interrupt Stack Table index a vector's gate should use, or 0 for
/// "don't switch stacks" (Intel SDM Vol. 3A 6.14.5). `#DF` (8) always
/// needs a known-good stack since it's often raised because the previous
/// stack was bad; NMI (2) and `#MC` (18) can land at arbitrary, possibly
/// stack-corrupted moments too.
fn ist_for(vector: usize) -> u8 {
    match vector {
        8 => 1,
        2 | 18 => 2,
        _ => 0,
    }
}

/// `#BP` (3) is DPL3 so userspace `int3` (breakpoints, later a debugger)
/// doesn't itself take a general-protection fault; everything else is
/// kernel-only for now.
fn dpl_for(vector: usize) -> u8 {
    if vector == 3 { 3 } else { 0 }
}

/// Installs the kernel's own IDT, replacing whatever (if anything) was
/// live before -- Limine leaves interrupts off and doesn't require a
/// specific IDT state, but nothing should rely on that.
pub fn init() {
    // SAFETY: boot-time, single-threaded, before `lidt` takes effect and
    // before interrupts are ever enabled; nothing else observes or
    // mutates `IDT` while this runs, and it's `'static` so the address
    // taken here stays valid forever.
    unsafe {
        let idt_ptr = &raw mut IDT;
        for (vector, &stub) in STUBS.iter().enumerate() {
            // `as usize as u64`, not a direct `as u64`: casting a function
            // pointer straight to a (potentially different-width) integer
            // type is the pattern `clippy::fn_to_numeric_cast` flags,
            // even though it's lossless here (both are 64-bit).
            let handler = stub as usize as u64;
            (*idt_ptr)[vector] = IdtEntry::new(handler, ist_for(vector), dpl_for(vector));
        }

        let dtp = DtPointer {
            limit: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: idt_ptr as u64,
        };
        core::arch::asm!("lidt [{}]", in(reg) &dtp, options(readonly, nostack, preserves_flags));
    }

    kprintln!("[idt] loaded (256 gates)");
}

/// Copies gate `vector` out of the `'static` `IDT` by raw-pointer
/// arithmetic (never forming a `&`/`&mut` to the `static mut` itself, only
/// to this local copy).
///
/// # Safety
/// By the time tests can call this, `init()` has already filled in and
/// installed `IDT`; there is no concurrent writer (single core, interrupts
/// off), and `vector` must be `< 256`.
unsafe fn gate(vector: usize) -> IdtEntry {
    // SAFETY: forwarded from this function's own contract; `IDT` has 256
    // entries and `vector < 256` is required by the caller.
    unsafe { (&raw const IDT).cast::<IdtEntry>().add(vector).read() }
}

/// Test-only introspection: is vector `v`'s gate marked present?
pub fn gate_present(vector: usize) -> bool {
    // SAFETY: `vector` is always a small literal (< 32) from test code.
    unsafe { gate(vector) }.present()
}

/// Test-only introspection: vector `v`'s gate's IST index.
pub fn gate_ist(vector: usize) -> u8 {
    // SAFETY: `vector` is always a small literal (< 32) from test code.
    unsafe { gate(vector) }.ist_index()
}

/// Test-only introspection: vector `v`'s gate's DPL.
pub fn gate_dpl(vector: usize) -> u8 {
    // SAFETY: `vector` is always a small literal (< 32) from test code.
    unsafe { gate(vector) }.dpl()
}
