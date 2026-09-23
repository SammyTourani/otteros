//! Fault-safe user<->kernel byte copies (kernel-review round 2): the real
//! safety net behind `proc::usermem`'s page-table pre-check
//! (`AddressSpace::translate`). That pre-check is necessarily a
//! time-of-check, not a time-of-use guarantee -- it's still correct
//! single-core (nothing else can change the mapping between the check and
//! the copy, since both run with this same thread as the only one able to
//! touch its own address space), but it stops being sufficient the moment
//! D25's SMP work lets a second core unmap the same page in between. This
//! module makes that irrelevant: the actual copy runs through a tiny,
//! hand-written loop whose *entire* instruction range is registered in
//! `FIXUP_TABLE`, and `arch::x86_64::trap::trap_dispatch` -- for a `#PF`/`#GP`
//! taken at ring 0 (`proc::usermem` always calls this via a normal,
//! ring-0-executing syscall/fault handler, never from ring 3 directly) --
//! checks that table before ever reaching its normal "an unexpected fault
//! took the kernel down" handling. A hit rewrites the trap frame's `rip` to
//! the matching fixup, which simply loads `-1` into `rax` and `ret`s,
//! exactly as if the copy routine itself had detected the problem and
//! returned an error -- so the caller (still `proc::usermem`) sees a clean
//! `-1` return, never a fault, no matter *why* the access failed.
//!
//! Two directions, not a shared "give me a fault-safe pointer deref"
//! primitive: `usercopy_from_user` reads through `src` (assumed to be the
//! possibly-untrustworthy one) and writes through `dst` (assumed to
//! always be valid kernel memory); `usercopy_to_user` is the mirror image.
//! In fact *either* side faulting anywhere inside the registered range is
//! treated as a failure either way -- simpler, and strictly safer, than
//! trying to pin down which one specific instruction can fault in each
//! direction.

unsafe extern "C" {
    fn usercopy_from_user_raw(dst: *mut u8, src: *const u8, len: usize) -> i64;
    fn usercopy_to_user_raw(dst: *mut u8, src: *const u8, len: usize) -> i64;

    static usercopy_from_user_fault_start: u8;
    static usercopy_from_user_fault_end: u8;
    static usercopy_from_user_fixup: u8;
    static usercopy_to_user_fault_start: u8;
    static usercopy_to_user_fault_end: u8;
    static usercopy_to_user_fixup: u8;
}

core::arch::global_asm!(
    r#"
    .global usercopy_from_user_raw
    .global usercopy_from_user_fault_start
    .global usercopy_from_user_fault_end
    .global usercopy_from_user_fixup
usercopy_from_user_raw:
    xor eax, eax
    test rdx, rdx
    jz 2f
usercopy_from_user_fault_start:
1:
    mov cl, [rsi]
    mov [rdi], cl
    inc rsi
    inc rdi
    dec rdx
    jnz 1b
usercopy_from_user_fault_end:
2:
    ret
usercopy_from_user_fixup:
    mov rax, -1
    ret

    .global usercopy_to_user_raw
    .global usercopy_to_user_fault_start
    .global usercopy_to_user_fault_end
    .global usercopy_to_user_fixup
usercopy_to_user_raw:
    xor eax, eax
    test rdx, rdx
    jz 4f
usercopy_to_user_fault_start:
3:
    mov cl, [rsi]
    mov [rdi], cl
    inc rsi
    inc rdi
    dec rdx
    jnz 3b
usercopy_to_user_fault_end:
4:
    ret
usercopy_to_user_fixup:
    mov rax, -1
    ret
    "#
);

/// Copies `len` bytes from `src` (untrusted) to `dst` (trusted kernel
/// memory). `Ok(())` on success; `Err(())` if a fault occurred anywhere
/// during the copy (handled by `trap_dispatch`'s fixup lookup, never a
/// kernel panic).
///
/// # Safety
/// `dst` must be valid, writable kernel memory for `len` bytes. `src` may
/// be *anything* -- validity is exactly what this function itself
/// determines (that's its purpose); the caller is expected to have already
/// done a page-table pre-check (`proc::usermem::validate_range`) as the
/// fast/common path, with this as the actual safety guarantee.
pub(crate) unsafe fn copy_from_user_checked(dst: *mut u8, src: *const u8, len: usize) -> Result<(), ()> {
    // SAFETY: forwarded from this function's own contract; `usercopy_from_user_raw`
    // never dereferences `src`/`dst` outside `[.., .. + len)`, and any fault
    // it takes is caught by `trap_dispatch` and turned into an ordinary
    // `-1` return instead of propagating.
    if unsafe { usercopy_from_user_raw(dst, src, len) } < 0 { Err(()) } else { Ok(()) }
}

/// Copies `len` bytes from `src` (trusted kernel memory) to `dst`
/// (untrusted). See `copy_from_user_checked`; the safety contract is
/// symmetric (here, `dst`'s validity is what this function determines).
///
/// # Safety
/// `src` must be valid, readable kernel memory for `len` bytes.
pub(crate) unsafe fn copy_to_user_checked(dst: *mut u8, src: *const u8, len: usize) -> Result<(), ()> {
    // SAFETY: see `copy_from_user_checked`.
    if unsafe { usercopy_to_user_raw(dst, src, len) } < 0 { Err(()) } else { Ok(()) }
}

/// If `rip` falls inside one of the registered copy routines' faultable
/// instruction ranges, returns the address to resume at instead (its
/// fixup, which simply reports failure and returns) -- `None` for every
/// other address, which is by far the common case for every fault this
/// kernel ever takes.
///
/// Called from `arch::x86_64::trap::trap_dispatch` for a `#PF`/`#GP` taken
/// at ring 0 (`frame.cs & 3 == 0`), *before* that function decides between
/// its normal "ring 3 fault" / "kernel panic" handling.
pub(crate) fn find_fixup(rip: u64) -> Option<u64> {
    // SAFETY: reading the *addresses* of these `extern "C"` linker symbols
    // (never their "contents" -- they're zero-sized markers, exactly like
    // `mm::vmm::map_kernel_image`'s `__text_start`/`__text_end`) is always
    // sound; `&raw const` never dereferences them.
    let ranges = [
        (
            &raw const usercopy_from_user_fault_start as u64,
            &raw const usercopy_from_user_fault_end as u64,
            &raw const usercopy_from_user_fixup as u64,
        ),
        (
            &raw const usercopy_to_user_fault_start as u64,
            &raw const usercopy_to_user_fault_end as u64,
            &raw const usercopy_to_user_fixup as u64,
        ),
    ];
    ranges.into_iter().find(|&(start, end, _)| rip >= start && rip < end).map(|(_, _, fixup)| fixup)
}
