//! `map_anon`/`unmap` wrappers (`SYSCALLS.md` 6/7): anonymous, writable,
//! non-executable memory. Used by `alloc_impl`'s global allocator and
//! directly by programs that want their own mapping (e.g. `utest`'s
//! round-trip check).

use crate::syscall::{self, num};

/// Maps `len` bytes of fresh, zeroed, writable, non-executable memory.
/// `len` is rounded up to a page by the kernel; the returned pointer is
/// always page-aligned.
pub fn map_anon(len: usize) -> Result<*mut u8, syscall::Errno> {
    // SAFETY: `MAP_ANON` takes one argument (a length) and returns an
    // address or a negative errno -- exactly `call1`'s contract.
    let addr = unsafe { syscall::call1(num::MAP_ANON, len as u64) }?;
    Ok(addr as *mut u8)
}

/// Unmaps `[addr, addr + len)`. Both must be 4 KiB aligned and the whole
/// range must already be mapped (`SYSCALLS.md` 7).
///
/// # Safety
/// Nothing may still be reading or writing through `addr` after this call
/// returns `Ok`.
pub unsafe fn unmap(addr: *mut u8, len: usize) -> Result<(), syscall::Errno> {
    // SAFETY: `UNMAP` takes two arguments (address, length) and returns
    // `0` or a negative errno -- exactly `call2`'s contract; the rest is
    // forwarded from this function's own contract.
    unsafe { syscall::call2(num::UNMAP, addr as u64, len as u64) }?;
    Ok(())
}
