//! Entering ring 3 for the very first time on a brand-new thread (brief
//! M2-T2 step 5) -- as opposed to *returning* to ring 3 from a syscall
//! (`arch::x86_64::syscall_entry`) or from an interrupt/exception that
//! interrupted it (`arch::x86_64::interrupts::common_stub`).

use super::gdt;

/// Where every M2-T2 user payload's code is mapped and where execution
/// starts (DECISIONS.md D18: user binaries link at `0x0000_0000_0040_0000`).
/// A fixed constant for now because every payload *is* the whole (tiny,
/// hand-assembled) image with no ELF header to read an entry point out of;
/// M2-T3's ELF loader will replace this with the value it actually parses.
pub const ENTRY_RIP: u64 = 0x0000_0000_0040_0000;

/// DECISIONS.md D18: the top of a process's 16 MiB user stack (it grows
/// down from here). `proc::process` maps the top 64 KiB up front and
/// demand-grows the rest (`proc::fault::handle_page_fault`).
pub const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_f000;

/// Builds an `iretq` frame for `(rip, rsp)` on the *caller's own* current
/// (kernel) stack and jumps into ring 3 through it. Never returns.
/// `sched::context::user_trampoline` is the only caller: the very first
/// time a freshly spawned user thread is ever scheduled, after `sched::
/// schedule` has already activated its process's `AddressSpace` (see that
/// function's own CR3-switch-on-difference logic) and `sti`'d.
///
/// # Safety
/// `rip` must be a mapped, `USER`+executable address in the *currently
/// active* address space, and `rsp` must point into a mapped, writable,
/// `USER`-accessible stack in that same space -- `enter_ring3` does not
/// itself validate either. The caller must not be holding any lock or
/// other resource that needs releasing on this exact call stack: this
/// never returns, by any means, to whatever called it.
pub unsafe fn enter_ring3(rip: u64, rsp: u64) -> ! {
    let user_ss16 = gdt::USER_DATA_SELECTOR | 3;
    let user_cs = u64::from(gdt::USER_CODE_SELECTOR | 3);
    let user_ss = u64::from(user_ss16);
    // RFLAGS: bit 1 is always-1 (Intel SDM Vol. 1 3.4.3); bit 9 is IF --
    // ring 3 code runs with interrupts enabled, like every other context
    // in this kernel.
    let rflags: u64 = (1 << 1) | (1 << 9);

    // SAFETY: forwarded from this function's own contract. `swapgs`
    // brings `GS_BASE` from "this is ring 0" (the per-CPU area,
    // `arch::x86_64::percpu`) to "this is ring 3" (0 -- see that module's
    // invariant) exactly once, immediately before the privilege change it
    // describes actually happens, mirroring `syscall_entry`'s and
    // `interrupts::common_stub`'s identical pairing on every other ring
    // 3 <-> ring 0 crossing. Loading `ds`/`es` with the ring-3 data
    // selector first is required, not cosmetic: without it, both would
    // still hold the kernel's ring-0 selector (DPL 0) once CPL becomes 3,
    // making them "unusable" the moment any ring-3 instruction implicitly
    // references them (Intel SDM Vol. 3A 5.8.2) -- an instant `#GP`. `gs`
    // is deliberately left untouched (seeindex `percpu`'s module docs):
    // reloading it here the same way would reset its *hidden base* to the
    // (zero) descriptor-table value, destroying the `swapgs` above.
    unsafe {
        core::arch::asm!(
            "mov ds, {user_ss16:x}",
            "mov es, {user_ss16:x}",
            "swapgs",
            "push {user_ss}",
            "push {rsp}",
            "push {rflags}",
            "push {user_cs}",
            "push {rip}",
            "iretq",
            user_ss16 = in(reg) user_ss16,
            user_ss = in(reg) user_ss,
            rsp = in(reg) rsp,
            rflags = in(reg) rflags,
            user_cs = in(reg) user_cs,
            rip = in(reg) rip,
            options(noreturn),
        );
    }
}
