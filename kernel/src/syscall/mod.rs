//! Syscall dispatch (brief M2-T2, DECISIONS.md D16): the glue between
//! `arch::x86_64::syscall_entry`'s raw register frame and `table::dispatch`'s
//! actual syscall table. Numbers/ABI documented in `SYSCALLS.md` at the
//! repo root.

pub mod errno;
pub mod table;

use crate::arch::x86_64::syscall_entry::SyscallFrame;

/// Called by `arch::x86_64::syscall_entry`'s naked asm for every `syscall`
/// instruction (after it has built a full `SyscallFrame` on the calling
/// thread's kernel stack): dispatches through `table::dispatch` and writes
/// the result back into `frame.rax`, exactly where the asm's own register-
/// restore sequence expects to find it. Diverges instead of returning
/// here for `exit` (syscall 0, `table::sys_exit`) -- see that function's
/// own docs; the asm's `call` site simply never gets control back in that
/// case, which is exactly "never return to user mode" (brief M2-T2 step
/// 3).
///
/// # Safety
/// `frame` must point at a live, fully-initialized `SyscallFrame` -- only
/// `arch::x86_64::syscall_entry::syscall_entry`'s asm ever calls this,
/// immediately after building exactly that.
pub(crate) unsafe extern "C" fn syscall_dispatch(frame: *mut SyscallFrame) {
    // SAFETY: forwarded from this function's own contract.
    let frame = unsafe { &mut *frame };
    let result = table::dispatch(frame);
    frame.rax = result as u64;

    // Kernel-review round 2 (supersedes brief M2-T2's original "return via
    // iretq in that case"): `frame.rcx` is the `rip` `sysretq` will jump
    // to. Nothing in `table::dispatch` ever touches it -- it's always
    // exactly the (necessarily canonical, since the CPU was already
    // executing there) `rip` the user's own `syscall` instruction had --
    // so this can only ever fire on a genuine kernel bug, but the
    // consequence of ever getting it wrong (the classic CVE-2012-0217
    // `sysret` privilege-escalation shape: `sysret` validates `rcx` *after*
    // committing `CPL=3`) is severe enough to check explicitly rather than
    // trust the invariant. On failure, this kills the process outright --
    // never attempts *any* return to it, by `sysretq` or `iretq` -- and
    // never returns to `syscall_entry`'s own `call` site, exactly like
    // `sys_exit`.
    if !crate::arch::x86_64::syscall_entry::is_canonical(frame.rcx) {
        crate::proc::exit_current_process(128 + 11);
    }
}
