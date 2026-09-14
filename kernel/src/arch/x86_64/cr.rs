//! Control-register (CR0/CR2/CR3/CR4), EFER MSR, `invlpg` and TLB-flush
//! access (brief M1-T4). DECISIONS.md D2 forbids the `x86_64` crate for
//! exactly this: page-table entries, CR0/CR3/CR4/EFER access and `invlpg`
//! are ours to write.
//!
//! Reads are plain safe functions (nothing about *reading* a control
//! register can violate memory safety); writes are `unsafe fn` with a
//! `# Safety` doc, the same convention `arch::x86_64::port::inb`/`outb`
//! use for raw hardware access -- getting one of these wrong doesn't just
//! misbehave, it can invalidate every pointer the kernel holds.

use crate::mm::addr::PhysAddr;

/// CR0 bit 16 (Intel SDM Vol. 3A 2.5): once set, write-protect applies to
/// supervisor-mode (ring 0) accesses too, so a read-only mapping actually
/// stops the *kernel* from writing through it, not just ring 3.
pub const CR0_WP: u64 = 1 << 16;

/// CR4 bit 7 (Intel SDM Vol. 3A 2.5): global (`PageFlags::GLOBAL`) pages'
/// TLB entries survive a CR3 reload instead of being flushed with
/// everything else.
pub const CR4_PGE: u64 = 1 << 7;

/// The Extended Feature Enable Register MSR (Intel SDM Vol. 4, 2.1).
pub const EFER_MSR: u32 = 0xC000_0080;
/// EFER bit 11: once set (and only then), a page table entry's
/// `PageFlags::NO_EXECUTE` bit is honoured; before that, bit 63 of every
/// entry is simply reserved (must be zero).
pub const EFER_NXE_BIT: u64 = 1 << 11;

/// Reads CR0.
#[inline]
pub fn read_cr0() -> u64 {
    let value: u64;
    // SAFETY: a single register read with no memory access or side effect.
    unsafe { core::arch::asm!("mov {}, cr0", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Writes CR0.
///
/// # Safety
/// The caller must ensure `value` keeps every invariant the running kernel
/// depends on intact -- in particular, never clear `CR0_WP` once anything
/// relies on write-protected pages actually being enforced for ring 0, and
/// never clear the paging-enable bit (31) at all (Limine already enabled
/// paging; this kernel has no code path that expects it off).
#[inline]
pub unsafe fn write_cr0(value: u64) {
    // SAFETY: forwarded from this function's own contract.
    unsafe { core::arch::asm!("mov cr0, {}", in(reg) value, options(nostack, preserves_flags)) };
}

/// Reads CR2: the faulting linear address the CPU latches on a page fault.
/// Only meaningful immediately after taking a `#PF` (vector 14) or a `#DF`
/// (vector 8) caused by one -- see `arch::x86_64::trap::trap_dispatch`.
#[inline]
pub fn read_cr2() -> u64 {
    let value: u64;
    // SAFETY: reading CR2 has no side effects and is valid from ring 0 at
    // any time; this is a single register read with no memory access.
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Reads CR3 (the current page-table root; bits 12..51 -- we never use
/// `PCID`, so the low 12 bits are always zero).
#[inline]
pub fn read_cr3() -> u64 {
    let value: u64;
    // SAFETY: a single register read with no memory access or side effect.
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Switches the active page-table root to `pml4`, flushing every
/// non-global TLB entry.
///
/// # Safety
/// `pml4` must be the physical, 4 KiB-aligned address of a fully-built,
/// valid PML4: every pointer the kernel currently holds (HHDM aliases,
/// the framebuffer, Limine responses, the heap's frames, the PMM bitmap,
/// the code currently executing and its stack) must remain mapped with
/// the same meaning after this call, or the very next instruction fetch
/// or memory access can fault or read/write the wrong physical memory.
#[inline]
pub unsafe fn write_cr3(pml4: PhysAddr) {
    // SAFETY: forwarded from this function's own contract. Deliberately
    // not `nomem`/`nostack`: this changes the meaning of every memory
    // access that follows, so the compiler must not reorder memory
    // operations across it.
    unsafe { core::arch::asm!("mov cr3, {}", in(reg) pml4.as_u64(), options(preserves_flags)) };
}

/// Reads CR4.
#[inline]
pub fn read_cr4() -> u64 {
    let value: u64;
    // SAFETY: a single register read with no memory access or side effect.
    unsafe { core::arch::asm!("mov {}, cr4", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Writes CR4.
///
/// # Safety
/// The caller must ensure `value` keeps every invariant the running
/// kernel depends on intact (in particular, this kernel assumes 4-level
/// paging throughout `mm::paging`/`mm::vmm`, so bit 12, `LA57`, must never
/// be set).
#[inline]
pub unsafe fn write_cr4(value: u64) {
    // SAFETY: forwarded from this function's own contract.
    unsafe { core::arch::asm!("mov cr4, {}", in(reg) value, options(nostack, preserves_flags)) };
}

/// Reads a Model-Specific Register.
///
/// # Safety
/// `msr` must name an MSR that exists on this CPU and is safe to read
/// here; an unimplemented MSR raises `#GP` instead of returning a value.
#[inline]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    (u64::from(hi) << 32) | u64::from(lo)
}

/// Writes a Model-Specific Register.
///
/// # Safety
/// `msr` must name an MSR that exists on this CPU and is safe to write
/// here, and `value` must be meaningful for it -- both are entirely
/// MSR-specific and unchecked.
#[inline]
pub unsafe fn wrmsr(msr: u32, value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") lo,
            in("edx") hi,
            options(nostack, preserves_flags),
        );
    }
}

/// Invalidates the TLB entry (if any) for the page containing `virt`.
///
/// # Safety
/// `virt` should be a canonical address; invalidating the wrong (or no)
/// entry is always memory-safe by itself, but the caller relies on the
/// *matching* stale translation actually being gone afterwards (used
/// after every `mm::vmm::AddressSpace` map/unmap).
#[inline]
pub unsafe fn invlpg(virt: u64) {
    // SAFETY: forwarded from this function's own contract. Not `nomem`:
    // this changes which physical memory subsequent accesses to `virt`
    // observe, so it must act as a compiler memory barrier too.
    unsafe { core::arch::asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
}

/// Flushes every non-global TLB entry by reloading CR3 with its own
/// current value (Intel SDM Vol. 3A 4.10.4.1).
///
/// # Safety
/// Same contract as `write_cr3` (this *is* a `write_cr3`, just with the
/// address it already holds) -- sound as long as the currently-active
/// page tables are still fully valid, which is always true when this is
/// simply "flush stale translations", not "switch address spaces".
#[inline]
pub unsafe fn flush_tlb_full() {
    // SAFETY: forwarded from this function's own contract.
    unsafe { write_cr3(PhysAddr::new(read_cr3())) };
}
