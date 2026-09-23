//! The vector 32..=255 dispatch table (brief M1-T5 step 7): one optional
//! handler slot per IRQ vector, `register`/`unregister`, and the
//! "no handler -> spurious" bookkeeping `arch::x86_64::trap::
//! trap_dispatch` falls back to. Lock-free (one `AtomicUsize` per slot,
//! storing the handler's address, 0 = empty) -- like `mm::kstack`'s guard
//! registry (see its docs for the identical reasoning): `dispatch` runs
//! from inside an interrupt handler and must never be able to block, and
//! `register`/`unregister` (normal context) must never be able to
//! deadlock against an interrupt that lands on this core while they run.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::acpi;
use crate::arch::x86_64::lapic;
use crate::arch::x86_64::trap::TrapFrame;
use crate::kprintln;

/// First IRQ vector (brief M1-T1: 0-31 are CPU exceptions, never routed
/// through this table).
pub const FIRST_IRQ_VECTOR: usize = 32;
/// One slot per vector in `FIRST_IRQ_VECTOR..=255`.
const IRQ_VECTOR_COUNT: usize = 256 - FIRST_IRQ_VECTOR;

/// An IRQ handler: takes the interrupted context, mutably (nothing
/// registers one that needs to adjust it yet, but the signature -- brief
/// M1-T5's own design -- leaves room for a future one, e.g. a scheduler
/// tick or a syscall-like trap, that does).
pub type Handler = fn(&mut TrapFrame);

/// `HANDLERS[v - FIRST_IRQ_VECTOR]` holds `Some(handler)`'s address (as a
/// `usize`), or 0 if vector `v` has none registered. Handlers are plain
/// `fn` pointers with no captured state, so a `usize` round-trips one
/// exactly -- see `register`/`dispatch`.
static HANDLERS: [AtomicUsize; IRQ_VECTOR_COUNT] = [const { AtomicUsize::new(0) }; IRQ_VECTOR_COUNT];

/// Interrupts that reached `dispatch` with no handler registered for
/// their vector -- includes the LAPIC's own hardware spurious vector
/// (0xFF, once `lapic::init` registers a counting-only handler for it,
/// brief step 4) and any other vector nothing has ever claimed.
static SPURIOUS_COUNT: AtomicUsize = AtomicUsize::new(0);

fn slot(vector: u8) -> Option<&'static AtomicUsize> {
    let v = vector as usize;
    (v >= FIRST_IRQ_VECTOR).then(|| &HANDLERS[v - FIRST_IRQ_VECTOR])
}

/// Registers `handler` for `vector` (32..=255), replacing any previous
/// one. `trap_dispatch` starts calling it -- and stops counting `vector`
/// as spurious -- the instant this store completes.
///
/// # Panics
/// If `vector < FIRST_IRQ_VECTOR` (32): exception vectors are never
/// routed through this table.
pub fn register(vector: u8, handler: Handler) {
    let slot = slot(vector).unwrap_or_else(|| panic!("irq::register: vector {vector} is a CPU exception, not an IRQ"));
    slot.store(handler as usize, Ordering::Release);
}

/// Clears whatever handler `vector` had (a no-op if it had none, or if
/// `vector` isn't a valid IRQ vector).
pub fn unregister(vector: u8) {
    if let Some(slot) = slot(vector) {
        slot.store(0, Ordering::Release);
    }
}

/// How many interrupts have reached `dispatch` with no handler
/// registered for their vector (see `SPURIOUS_COUNT`'s docs).
pub fn spurious_count() -> usize {
    SPURIOUS_COUNT.load(Ordering::Relaxed)
}

/// Called by `trap::trap_dispatch` for every vector `>= FIRST_IRQ_VECTOR`:
/// looks up and calls `vector`'s handler (or counts it spurious if none
/// is registered), then always sends the local APIC an EOI -- even for a
/// vector nothing claimed, so a real external interrupt that arrives
/// before its driver has registered (or after a driver bug forgets to)
/// doesn't wedge the LAPIC by leaving its priority bit permanently
/// in-service.
pub(crate) fn dispatch(vector: u8, frame: &mut TrapFrame) {
    let handler_addr = slot(vector).map(|s| s.load(Ordering::Acquire)).unwrap_or(0);
    if handler_addr == 0 {
        SPURIOUS_COUNT.fetch_add(1, Ordering::Relaxed);
    } else {
        // SAFETY: the only non-zero value ever stored in a `HANDLERS`
        // slot is a real `Handler` (`fn(&mut TrapFrame)`) cast to `usize`
        // by `register` above; function pointers round-trip through a
        // same-width integer losslessly, and `Handler` has no captured
        // state to go stale.
        let handler: Handler = unsafe { core::mem::transmute::<usize, Handler>(handler_addr) };
        handler(frame);
    }
    lapic::eoi();

    // Brief M2-T1's design cautions: a thread switch triggered from the
    // timer IRQ path must happen *after* EOI (so the LAPIC's in-service
    // bit for this vector is already cleared before this core potentially
    // goes on to run a completely different thread for a while) and with
    // interrupts still disabled -- both true here: this whole function
    // runs inside the interrupt gate that dispatched `vector`, which
    // cleared IF on entry, and EOI was just sent above.
    // `sched::preempt_if_needed` is a no-op unless the timer tick just
    // set `NEED_RESCHED`, so calling it unconditionally after every IRQ
    // (not just the timer's) costs nothing on the common path.
    crate::sched::preempt_if_needed();
}

/// Ties an ISA IRQ number (0-15) to the Global System Interrupt the I/O
/// APIC actually redirects it through (brief M1-T5 step 6): applies the
/// MADT's Interrupt Source Override list if `irq` has one, otherwise
/// falls back to the ACPI-default identity mapping (GSI == ISA IRQ
/// number).
pub fn isa_to_gsi(irq: u8) -> u32 {
    acpi::with_info(|info| {
        info.madt.overrides.iter().find(|o| o.isa_irq == irq).map(|o| o.gsi).unwrap_or(u32::from(irq))
    })
}

/// Enables interrupts for good (brief M1-T5 step 7). From this point on,
/// the kernel never `cli`s again except inside a critical section a
/// `sync::IrqMutex` guard (or `arch::x86_64::interrupts::
/// without_interrupts`) already owns.
pub fn enable() {
    // SAFETY: every vector 0-255 already has a present IDT gate
    // (`idt::init`, brief M1-T1), and every one that can plausibly fire
    // before any driver-specific `register` call has a safe fallback --
    // `trap_dispatch`'s own exception arms for 0-31, and this module's
    // spurious-counting default (still always EOI'd) for 32-255 -- so
    // nothing turning interrupts on here can land on an absent or unsafe
    // handler.
    unsafe { crate::arch::x86_64::interrupts::sti() };
    kprintln!("[irq] interrupts enabled");
}
