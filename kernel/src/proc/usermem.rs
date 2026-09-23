//! Validated copies between the kernel and a user process's own address
//! space (brief M2-T2 step 6, hardened per kernel-review round 2).
//!
//! Two layers, deliberately redundant: `validate_range` walks the page
//! table up front (cheap, and gives a precise `-EFAULT` without ever
//! touching memory for the common "obviously bad pointer" case), and the
//! actual copy runs through `arch::x86_64::usercopy`'s fault-safe routines
//! (the real safety net -- see that module's docs for why the page-table
//! check alone is only a time-of-check guarantee, not time-of-use). No
//! SMAP on this kernel yet (TODO: `stac`/`clac` around the actual copy
//! once `CR4.SMAP` is ever set) -- neither layer needs it: the calling
//! thread's own `AddressSpace` is, by construction, the one currently
//! active in CR3 (a syscall or a ring-3 fault never changes CR3 itself --
//! only `sched::schedule`'s own context switch does, and that never runs
//! concurrently with the syscall/fault handler that's mid-copy).

use crate::arch::x86_64::usercopy;
use crate::mm::addr::{FRAME_SIZE, VirtAddr};
use crate::mm::paging::PageFlags;
use crate::mm::vmm::AddressSpace;
use crate::syscall::errno::EFAULT;

/// DECISIONS.md D15: user space ends at `0x0000_7fff_ffff_ffff`; nothing at
/// or above the next power-of-two boundary is ever a legitimate user
/// pointer, checked *before* walking a single page table entry.
pub const USER_SPACE_CEILING: u64 = 0x0000_8000_0000_0000;

/// Whether a range needs to be writable (a destination) or merely present
/// (a source) -- kernel-review round 2: `copy_to_user` must refuse a
/// read-only destination (e.g. a process's own code page) instead of
/// silently... doing nothing useful with it, since the underlying copy
/// would just fault anyway, but *which* fault (and diagnosing it) is much
/// clearer caught here, at the same place every other validation failure
/// is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
}

/// Confirms every page in `[ptr, ptr + len)` is present and `USER`-
/// accessible in `space` (and, for `Access::Write`, also `WRITABLE`) --
/// the fast/common-case check every function below shares. The actual
/// copy (`arch::x86_64::usercopy`) is what makes this safe even if it
/// races with a concurrent unmap on some future multi-core build; this is
/// purely a precise, early `-EFAULT` for the ordinary case.
fn validate_range(space: &AddressSpace, ptr: u64, len: usize, access: Access) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    let end = ptr.checked_add(len as u64).ok_or(EFAULT as i64)?;
    if end > USER_SPACE_CEILING {
        return Err(EFAULT as i64);
    }

    let mut page = ptr & !(FRAME_SIZE as u64 - 1);
    while page < end {
        match space.translate(VirtAddr::new(page)) {
            Some((_, flags, _)) if flags.contains(PageFlags::USER) && (access == Access::Read || flags.contains(PageFlags::WRITABLE)) => {}
            _ => return Err(EFAULT as i64),
        }
        page += FRAME_SIZE as u64;
    }
    Ok(())
}

/// Confirms `[ptr, ptr + len)` is present, `USER`-accessible and
/// `WRITABLE` in `space`, without copying anything -- for a caller that
/// needs to fail fast, *before* doing something slow/blocking, rather
/// than discover the destination was always bad only after that wait
/// (`syscall::table`'s `read` does exactly this in front of its blocking
/// keyboard read).
pub fn validate_writable(space: &AddressSpace, ptr: u64, len: usize) -> Result<(), i64> {
    validate_range(space, ptr, len, Access::Write)
}

/// Copies `dst.len()` bytes from `user_ptr` (in `space`) into `dst`.
/// `Err(-EFAULT)` if any page in range isn't mapped+`USER`-accessible, or
/// if the actual copy faults regardless (`arch::x86_64::usercopy`'s
/// fixup-table safety net) -- nothing meaningful is left in `dst` either
/// way.
pub fn copy_from_user(space: &AddressSpace, dst: &mut [u8], user_ptr: u64) -> Result<(), i64> {
    validate_range(space, user_ptr, dst.len(), Access::Read)?;
    if dst.is_empty() {
        return Ok(());
    }
    // SAFETY: `dst` is a valid, exclusively-owned slice the caller gave
    // us, at least `dst.len()` bytes of real kernel memory; `user_ptr` is
    // whatever the caller claims, exactly what `usercopy_from_user_raw`
    // itself determines is safe to read (see `arch::x86_64::usercopy`'s
    // module docs) rather than something this call site vouches for.
    unsafe { usercopy::copy_from_user_checked(dst.as_mut_ptr(), user_ptr as *const u8, dst.len()) }.map_err(|()| EFAULT as i64)
}

/// Copies `src` into `user_ptr` (in `space`). `Err(-EFAULT)` if any page in
/// range isn't mapped+`USER`+`WRITABLE`, or if the actual copy faults
/// regardless -- see `copy_from_user`.
pub fn copy_to_user(space: &AddressSpace, user_ptr: u64, src: &[u8]) -> Result<(), i64> {
    validate_range(space, user_ptr, src.len(), Access::Write)?;
    if src.is_empty() {
        return Ok(());
    }
    // SAFETY: `src` is a valid, exclusively-owned-for-reading slice the
    // caller gave us, at least `src.len()` bytes of real kernel memory;
    // symmetric to `copy_from_user` otherwise.
    unsafe { usercopy::copy_to_user_checked(user_ptr as *mut u8, src.as_ptr(), src.len()) }.map_err(|()| EFAULT as i64)
}

/// Reads a NUL-terminated string of at most `max` bytes starting at `ptr`
/// (in `space`), validating one byte at a time as the scan reaches it
/// (rather than needing to know the final length up front). `Err(-EFAULT)`
/// if the scan reaches an unmapped/non-`USER` page before a NUL byte, or
/// `max` bytes, whichever comes first, without ever finding one.
///
/// Not yet called by any M2-T2 syscall (none of them take a path/string
/// argument -- `spawn`, M2-T3, will be the first), but implemented now per
/// the brief so `spawn`'s own implementation doesn't have to invent it
/// later under time pressure.
pub fn user_str(space: &AddressSpace, ptr: u64, max: usize) -> Result<alloc::string::String, i64> {
    let mut out = alloc::vec::Vec::new();
    let mut offset: usize = 0;
    while offset < max {
        let addr = ptr.checked_add(offset as u64).ok_or(EFAULT as i64)?;
        let mut byte = [0u8; 1];
        copy_from_user(space, &mut byte, addr)?;
        if byte[0] == 0 {
            return alloc::string::String::from_utf8(out).map_err(|_| EFAULT as i64);
        }
        out.push(byte[0]);
        offset += 1;
    }
    Err(EFAULT as i64)
}
