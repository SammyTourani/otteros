//! The legacy 8259 Programmable Interrupt Controller pair (brief M1-T5
//! step 3): remapped off the CPU-exception vectors it defaults to, then
//! immediately and fully masked. From this point on the I/O APIC (not
//! this) owns every external interrupt line, but a stray legacy IRQ must
//! never be able to land on vector 0-31 and be mistaken for a CPU
//! exception -- remapping first, even though everything is masked a few
//! instructions later, is what makes that true for the brief instant
//! between "PIC exists" and "PIC is masked" too.

use crate::arch::x86_64::port::{inb, outb};
use crate::kprintln;

const MASTER_COMMAND: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_COMMAND: u16 = 0xA0;
const SLAVE_DATA: u16 = 0xA1;

/// ICW1: edge-triggered, cascade mode, ICW4 will follow (Intel 8259A
/// datasheet).
const ICW1_INIT: u8 = 0x11;
/// ICW4: 8086/88 mode.
const ICW4_8086: u8 = 0x01;

/// Where this kernel remaps the master/slave PIC's vectors: past every
/// CPU exception (0-31), and -- deliberately -- exactly where the I/O
/// APIC's own vectors for the same legacy IRQ lines will also land, so
/// nothing downstream needs to know or care that a legacy PIC exists at
/// all once this returns.
const MASTER_OFFSET: u8 = 32;
const SLAVE_OFFSET: u8 = 40;

/// A dummy write real 8259 hardware needs time to process between ICW
/// bytes. Port 0x80 is an unused POST-code diagnostic port on every
/// PC-compatible platform, conventionally used for exactly this delay
/// (harmless, if unnecessary, under QEMU's instant emulated PIC).
fn io_wait() {
    // SAFETY: nothing on any PC-compatible platform this kernel targets
    // (D1) relies on port 0x80 for anything the kernel itself cares
    // about; writing to it is the standard, side-effect-free delay trick.
    unsafe { outb(0x80, 0) };
}

/// Remaps the master/slave 8259 pair to vectors `MASTER_OFFSET` (32..=39)
/// / `SLAVE_OFFSET` (40..=47), then masks every line on both (brief step
/// 3): the full four-ICW initialisation sequence (Intel 8259A datasheet),
/// run with interrupts already off -- this always runs before
/// `arch::x86_64::irq::enable`'s `sti`.
pub fn remap_and_mask() {
    // SAFETY: ports 0x20/0x21/0xA0/0xA1 are the master/slave 8259's fixed
    // command/data ports on every PC-compatible platform (QEMU's `q35`
    // machine included, D1). This is exactly the documented four-ICW
    // initialisation sequence, ending in "mask everything" -- always a
    // valid, safe thing to write to a PIC regardless of whatever state it
    // was already in.
    unsafe {
        outb(MASTER_COMMAND, ICW1_INIT);
        io_wait();
        outb(SLAVE_COMMAND, ICW1_INIT);
        io_wait();

        outb(MASTER_DATA, MASTER_OFFSET); // ICW2: master vector offset.
        io_wait();
        outb(SLAVE_DATA, SLAVE_OFFSET); // ICW2: slave vector offset.
        io_wait();

        outb(MASTER_DATA, 0b0000_0100); // ICW3: slave attached on IRQ2.
        io_wait();
        outb(SLAVE_DATA, 0b0000_0010); // ICW3: this slave's own cascade identity (2).
        io_wait();

        outb(MASTER_DATA, ICW4_8086);
        io_wait();
        outb(SLAVE_DATA, ICW4_8086);
        io_wait();

        outb(MASTER_DATA, 0xFF); // mask every line.
        outb(SLAVE_DATA, 0xFF);
    }

    kprintln!("[pic] remapped to {MASTER_OFFSET}..={}, fully masked", SLAVE_OFFSET + 7);
}

/// Test-only introspection: `(master_mask, slave_mask)`. Both must read
/// back `0xFF` after `remap_and_mask`.
pub fn masks() -> (u8, u8) {
    // SAFETY: reading a PIC's interrupt-mask register has no side
    // effects.
    unsafe { (inb(MASTER_DATA), inb(SLAVE_DATA)) }
}
