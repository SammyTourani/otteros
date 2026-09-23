//! New-thread stack seeding and the trampoline every brand-new thread's
//! first `ret` (off `arch::x86_64::switch::switch_to`) lands in (brief
//! M2-T1).

use crate::arch::x86_64::interrupts::sti;
use crate::mm::addr::VirtAddr;

/// How many callee-saved GPRs `switch::switch_to` pushes/pops: rbp, rbx,
/// r12, r13, r14, r15.
const CALLEE_SAVED_SLOTS: u64 = 6;

/// Writes the initial contents of a brand-new thread's stack so that
/// `switch::switch_to`'s `ret` lands in `trampoline` below, and returns
/// the initial `rsp` value to store in the new thread's control block.
///
/// Layout built (addresses increasing upward, matching what `switch_to`'s
/// six `pop`s followed by `ret` expect to find, in order): six zeroed
/// callee-saved-register slots, then `trampoline`'s address as the "return
/// address" the final `ret` pops, at `top - 16` (not `top - 8`) -- one
/// extra 8-byte gap below `top`. That gap is not padding; it fixes a
/// stack-alignment mismatch a naive `top - 8` placement would introduce:
/// `switch_to`'s 6 pushes/pops move `rsp` by 48 bytes (a multiple of 16),
/// so they never change its residue mod 16, and for a *previously
/// switched-away* thread, `switch_to` was itself reached via an ordinary
/// `call` from `sched::schedule` -- which leaves `rsp === 8 (mod 16)` at
/// `switch_to`'s own entry, and therefore also at the point its 6 pushes
/// finish and it saves `rsp` (48 is `=== 0 mod 16`). Loading that same
/// `=== 8 mod 16` value back and running the 6 pops *and* the final `ret`
/// (+56 total, `=== 8 mod 16`) lands at `=== 0 mod 16` -- correct for
/// resuming *inside* `schedule`, right after its own call to `switch_to`
/// returns. But `trampoline` is a genuine function *entry*, not a
/// mid-function resumption point, and a function entry reached via a
/// real `call` instead sees `rsp === 8 (mod 16)` (the `call` itself
/// pushes 8 bytes from a 16-aligned call site). So the saved `rsp` a
/// brand-new thread starts with must itself be `=== 0 (mod 16)` --
/// `top - 64` (7 slots at `top-64..top-16`, plus this 8-byte gap) --
/// not `top - 56 === 8 (mod 16)`, which would land `trampoline` 8 bytes
/// off from where its compiler-generated prologue expects `rsp` to be.
pub(crate) fn build_initial_stack(top: VirtAddr) -> u64 {
    build_initial_stack_with(top, trampoline)
}

/// Like `build_initial_stack`, but for a brand-new *user* thread (brief
/// M2-T2): the first `switch::switch_to` into it lands in `user_trampoline`
/// instead, which enters ring 3 rather than calling an ordinary Rust
/// `entry(arg)`.
pub(crate) fn build_initial_user_stack(top: VirtAddr) -> u64 {
    build_initial_stack_with(top, user_trampoline)
}

/// Shared by `build_initial_stack`/`build_initial_user_stack` -- identical
/// layout (see `build_initial_stack`'s own doc comment above for the full
/// alignment reasoning), just with the trampoline address parameterised.
fn build_initial_stack_with(top: VirtAddr, tramp: extern "C" fn() -> !) -> u64 {
    let top = top.as_u64();
    assert!(top.is_multiple_of(16), "sched::context::build_initial_stack: {top:#x} isn't 16-byte aligned");

    let return_addr_slot = top - 16;
    // `tramp as *const ()` first (not straight to an integer type):
    // casting a function *item* (as opposed to an already-typed function
    // pointer variable, see `mm::kstack::switch_stack_and_call`'s
    // identical reasoning for `f`) directly to an integer is what
    // `function_casts_as_integer` warns about; going through a pointer
    // type first is the same bit pattern with no warning.
    let trampoline_addr = tramp as *const () as u64;
    // SAFETY: `top` is the top of a freshly mapped, exclusively-owned
    // kernel stack (`mm::kstack::allocate`, at least `THREAD_STACK_PAGES`
    // * 4 KiB deep, far more than the 56 bytes written here); nothing
    // else has a pointer into it yet, so writing these initial words is
    // sound.
    unsafe { (return_addr_slot as *mut u64).write(trampoline_addr) };

    let mut slot = return_addr_slot;
    for _ in 0..CALLEE_SAVED_SLOTS {
        slot -= 8;
        // SAFETY: see above.
        unsafe { (slot as *mut u64).write(0) };
    }
    slot
}

/// Where a brand-new thread's very first `ret` (see `build_initial_stack`)
/// lands. An ordinary (not naked) `extern "C" fn`: `switch::switch_to`'s
/// `ret` hands control here exactly as if this were returning from a
/// normal call, so its compiler-generated prologue is exactly what's
/// needed -- no register shuffling required, since `entry`/`arg` are read
/// back out of `sched::current()` rather than smuggled through registers.
extern "C" fn trampoline() -> ! {
    // SAFETY: this only ever runs as the first instructions of a thread
    // `sched::schedule` just finished switching to, with interrupts
    // disabled the whole way through that switch (brief M2-T1's design
    // cautions); enabling them here is what makes this thread
    // preemptible like every other one from this point on.
    unsafe { sti() };

    let thread = super::current();
    let code = (thread.entry())(thread.arg());
    thread.set_exit_code(code);
    super::exit_current();
}

/// Where a brand-new *user* thread's very first `ret` (see
/// `build_initial_user_stack`) lands (brief M2-T2): enables interrupts
/// exactly like `trampoline`, then jumps into ring 3 through
/// `usermode::enter_ring3` -- never returns, since that function doesn't
/// either.
///
/// `sched::schedule` has already `activate()`d this thread's process
/// address space by the time this runs (its CR3-switch-on-difference logic
/// runs on *every* switch, including a brand-new thread's very first one),
/// and every M2-T2 process maps its code executable+user at
/// `usermode::ENTRY_RIP` and its stack writable+user ending at
/// `usermode::USER_STACK_TOP` (`proc::process::Process::create`) --
/// exactly `enter_ring3`'s own preconditions.
extern "C" fn user_trampoline() -> ! {
    // SAFETY: see `trampoline`'s identical reasoning -- this is a brand-
    // new thread's first-ever instructions, reached with interrupts
    // disabled the whole way through the switch that landed here.
    unsafe { sti() };
    // SAFETY: see this function's own doc comment above.
    unsafe { crate::arch::x86_64::usermode::enter_ring3(crate::arch::x86_64::usermode::ENTRY_RIP, crate::arch::x86_64::usermode::USER_STACK_TOP) };
}
