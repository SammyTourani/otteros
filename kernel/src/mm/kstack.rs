//! Guard-paged kernel stacks (brief M1-T4 step 4): a bump allocator over
//! the kernel dynamic region (DECISIONS.md D15,
//! `0xffff_c000_0000_0000..0xffff_cfff_ffff_ffff`) that hands out
//! `KernelStack`s -- one deliberately unmapped 4 KiB guard page
//! immediately below `n` mapped, `RW+NX+GLOBAL` frames -- plus a small
//! registry so the double-fault path (`arch::x86_64::trap`) can recognise
//! "this CR2 is a guard page" and report a stack overflow instead of a
//! bare double fault.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use spin::Mutex;

use super::addr::{FRAME_SIZE, VirtAddr};
use super::paging::PageFlags;
use super::{pmm, vmm};
use crate::arch::x86_64::gdt;
use crate::kprintln;

/// DECISIONS.md D15: the kernel dynamic region set aside for stacks, MMIO
/// windows and a future vmalloc.
const DYNAMIC_REGION_START: u64 = 0xffff_c000_0000_0000;
/// One past D15's `0xffff_cfff_ffff_ffff`.
const DYNAMIC_REGION_END: u64 = 0xffff_d000_0000_0000;

/// A leading, always-unmapped gap between one stack's top and the next
/// one's guard page, on top of each stack's own guard page: pure defence
/// in depth, so a wildly out-of-bounds access landing exactly on the
/// *next* stack's top faults instead of silently corrupting it.
const INTER_STACK_GAP: u64 = FRAME_SIZE as u64;

static NEXT_STACK_BASE: AtomicU64 = AtomicU64::new(DYNAMIC_REGION_START);

/// One live kernel stack: `guard` is the single unmapped 4 KiB page right
/// below `bottom`; `[bottom, top)` is mapped and usable (`top` is the
/// initial `rsp` value -- the stack grows down from there).
#[derive(Clone, Copy, Debug)]
pub struct KernelStack {
    pub guard: VirtAddr,
    pub bottom: VirtAddr,
    pub top: VirtAddr,
}

/// Upper bound on how many kernel stacks can ever be registered at once.
/// Generous for M1 (there is exactly one: the boot stack); revisit once a
/// scheduler (M1-T7 onward) hands one to every thread.
const MAX_STACKS: usize = 64;

/// One registered guard page, as `[start, end)` (`start` = the guard
/// page's address, `end` = the stack's `bottom`, i.e. `start +
/// FRAME_SIZE`). A slot is "registered" exactly when `end != 0` -- no
/// real guard page ever ends at address 0 (frame 0 is never mapped; see
/// `pmm::free_frame`'s docs).
///
/// Kernel-review fix #1 (D14): this used to be a `spin::Mutex<..>`, but
/// `find_guard` is called from the double-fault handler, which must never
/// be able to block -- a fault landing while some *other* code path held
/// this same lock (a real, if currently only theoretical, scenario: any
/// future caller of `allocate` that isn't `init_boot_stack` at boot) would
/// deadlock forever instead of reporting the fault. Plain `AtomicU64`
/// pairs with `Relaxed` ordering are lock-free by construction; `Relaxed`
/// is enough because there is exactly one core and a fault on it always
/// observes its *own* prior writes in program order regardless of memory
/// ordering -- the only real concern here is a fault landing *during*
/// `register` below, which `Relaxed` still handles correctly (see there).
struct GuardSlot {
    start: AtomicU64,
    end: AtomicU64,
}

impl GuardSlot {
    const fn empty() -> Self {
        Self { start: AtomicU64::new(0), end: AtomicU64::new(0) }
    }
}

static GUARD_SLOTS: [GuardSlot; MAX_STACKS] = [const { GuardSlot::empty() }; MAX_STACKS];

/// Bump-allocates the next `GUARD_SLOTS` index for `register` to claim --
/// same pattern as `NEXT_STACK_BASE` above, just over slot indices instead
/// of addresses.
static NEXT_GUARD_SLOT: AtomicUsize = AtomicUsize::new(0);

/// What `find_guard` returns: just enough to report or verify a guard-page
/// hit. Unlike `KernelStack`, no `top` -- nothing that calls `find_guard`
/// (the double-fault handler, tests) ever needs it.
#[derive(Clone, Copy, Debug)]
pub struct GuardMatch {
    pub guard: VirtAddr,
    pub bottom: VirtAddr,
}

/// The stack `init_boot_stack` allocated -- the one the kernel is
/// actually running on from that point onward.
static BOOT_STACK: Mutex<Option<KernelStack>> = Mutex::new(None);

/// Allocates a new guard-paged stack with `pages` mapped 4 KiB frames
/// (`pages * 4 KiB` usable bytes), carved out of the kernel dynamic
/// region, and registers it so `find_guard` can recognise its guard page.
///
/// # Panics
/// If `pages` is 0, if the kernel dynamic region is exhausted, if the PMM
/// is out of memory, or if the registry is full (`MAX_STACKS`) -- all
/// kernel bugs or genuine resource exhaustion this early in boot, not
/// recoverable conditions.
pub fn allocate(pages: usize) -> KernelStack {
    assert!(pages > 0, "kstack::allocate: pages must be at least 1");

    let stack_bytes = pages as u64 * FRAME_SIZE as u64;
    let region_len = INTER_STACK_GAP + FRAME_SIZE as u64 + stack_bytes;
    let base = NEXT_STACK_BASE.fetch_add(region_len, Ordering::SeqCst);
    assert!(
        base + region_len <= DYNAMIC_REGION_END,
        "kstack::allocate: exhausted the kernel dynamic region"
    );

    let guard = VirtAddr::new(base + INTER_STACK_GAP);
    let bottom = VirtAddr::new(guard.as_u64() + FRAME_SIZE as u64);
    let top = VirtAddr::new(bottom.as_u64() + stack_bytes);

    let space = vmm::kernel_address_space();
    let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE | PageFlags::GLOBAL;
    for i in 0..pages as u64 {
        let frame = pmm::alloc_frame_zeroed().expect("kstack::allocate: out of memory for a kernel stack");
        space.map_4k(VirtAddr::new(bottom.as_u64() + i * FRAME_SIZE as u64), frame, flags);
    }
    // `guard` is deliberately left unmapped -- that's the entire point.

    let stack = KernelStack { guard, bottom, top };
    register(stack);
    stack
}

fn register(stack: KernelStack) {
    let idx = NEXT_GUARD_SLOT.fetch_add(1, Ordering::Relaxed);
    assert!(idx < MAX_STACKS, "kstack: guard registry is full ({MAX_STACKS} stacks)");
    let slot = &GUARD_SLOTS[idx];
    // `start` before `end`, never the other way around: `end` is what
    // marks a slot "registered" (see `GuardSlot`'s docs), so if a fault
    // interrupts this function between the two stores below, `find_guard`
    // still sees `end == 0` and correctly treats the slot as not-yet-
    // registered instead of reading a stale (zero) `start` paired with a
    // real `end`.
    slot.start.store(stack.guard.as_u64(), Ordering::Relaxed);
    slot.end.store(stack.bottom.as_u64(), Ordering::Relaxed);
}

/// Whether `addr` lies inside some registered stack's guard page --
/// equivalently (brief M1-T4 step 5), "within 4 KiB below that stack's
/// bottom": the guard page *is* exactly that range. Used by the
/// double-fault handler to tell a stack overflow apart from any other
/// double fault -- so, per D14 and kernel-review fix #1, this must never
/// block: it only ever does `Relaxed` atomic loads over a fixed array,
/// never a lock.
pub fn find_guard(addr: VirtAddr) -> Option<GuardMatch> {
    let a = addr.as_u64();
    for slot in &GUARD_SLOTS {
        let end = slot.end.load(Ordering::Relaxed);
        if end == 0 {
            continue; // never registered (or a fault caught `register` mid-write; see there).
        }
        let start = slot.start.load(Ordering::Relaxed);
        if a >= start && a < end {
            return Some(GuardMatch { guard: VirtAddr::new(start), bottom: VirtAddr::new(end) });
        }
    }
    None
}

/// Allocates the boot stack (called once, right after
/// `vmm::init_kernel_space`), points the TSS's `RSP0` at its top, and
/// returns it so the caller can actually switch `rsp` there
/// (`switch_stack_and_call`).
pub fn init_boot_stack(pages: usize) -> KernelStack {
    let stack = allocate(pages);
    *BOOT_STACK.lock() = Some(stack);
    gdt::set_rsp0(stack.top.as_u64());
    kprintln!(
        "[kstack] boot stack: guard=0x{:x} bottom=0x{:x} top=0x{:x}",
        stack.guard.as_u64(),
        stack.bottom.as_u64(),
        stack.top.as_u64()
    );
    stack
}

/// Test-only introspection: the boot stack `init_boot_stack` allocated --
/// the one every test in `test_cases::vmm` is, at that point, actually
/// running on.
///
/// # Panics
/// If `init_boot_stack` hasn't run yet.
pub fn boot_stack() -> KernelStack {
    BOOT_STACK.lock().expect("kstack::init_boot_stack was never called")
}

/// Switches to a brand new stack and calls `f`, never returning to
/// whatever frame called this (brief M1-T4 step 4). After
/// `vmm::init_kernel_space` switches CR3, the kernel is still running on
/// the stack Limine handed it at entry; this is how execution moves onto
/// its own guard-paged one instead, permanently. `f` is `-> !`, so there
/// is no `ret` back into the old frame -- the old (Limine) stack is
/// simply never touched again.
///
/// # Safety
/// `top` must be the 16-byte-aligned top of a mapped, exclusively-owned
/// stack, large enough for `f`'s entire call chain; `f` must genuinely
/// never return.
pub unsafe fn switch_stack_and_call(top: u64, f: extern "C" fn() -> !) -> ! {
    // Cast through `usize` first (`clippy::fn_to_numeric_cast`'s preferred
    // form -- see `arch::x86_64::idt::init`'s identical pattern), then
    // load it into a register: `sym` operands need a compile-time-known
    // item path, which a runtime function-pointer *parameter* like `f`
    // isn't, so this calls through a register instead of `sym`.
    let f_addr = f as usize as u64;
    // SAFETY: forwarded from this function's own contract. `noreturn` is
    // accurate, not just documentation: `rsp` has already been replaced
    // wholesale before `call`, so even if `f` somehow returned there is no
    // valid target for that `ret` to reach -- and `f`'s own type (`-> !`)
    // guarantees it never tries.
    unsafe {
        core::arch::asm!(
            "mov rsp, {top}",
            "call {f}",
            top = in(reg) top,
            f = in(reg) f_addr,
            options(noreturn),
        );
    }
}
