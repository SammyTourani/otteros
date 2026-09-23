//! Guard-paged kernel stacks (brief M1-T4 step 4; brief M2-T3 step: fixed-
//! stride reusable slots), carved out of the kernel dynamic region
//! (DECISIONS.md D15, `0xffff_c000_0000_0000..0xffff_cfff_ffff_ffff`).
//!
//! The region is laid out as `NUM_SLOTS` fixed-size slots, each one
//! deliberately unmapped 4 KiB guard page immediately followed by
//! `STACK_PAGES` mapped, `RW+NX+GLOBAL` frames -- no gap between one
//! slot's stack top and the next slot's guard page, so an overrun past the
//! top still lands on an unmapped page one instruction later. A slot's
//! *virtual range* (and its guard page) is handed out by index: a small
//! free list (`SLOTS`) reuses a freed index before ever bumping past the
//! highest index yet touched, so the number of stacks that can *ever* be
//! created across a boot is bounded only by how many are *concurrently
//! live* (`NUM_SLOTS`), not by lifetime spawn count -- physical frames are
//! still returned to the PMM on `free` exactly as before; only the
//! virtual-address/guard-page bookkeeping is reused.
//!
//! `find_guard` (the double-fault path, `arch::x86_64::trap`, needs this)
//! no longer consults any runtime registry at all: because every slot has
//! the same fixed size and starts at a fixed offset from
//! `DYNAMIC_REGION_START`, "is this address a guard page" is answered by
//! two divisions and a comparison, true regardless of whether that slot
//! has ever actually been allocated. This keeps the check lock-free *and*
//! allocation-free (stricter than the old fixed-array-of-atomics registry
//! even managed: there is no shared state to read at all).

use alloc::vec::Vec;

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

/// Every kernel stack is exactly this many 4 KiB pages (64 KiB), matching
/// the pre-scheduler boot stack (`BOOT_STACK_PAGES`, lib.rs) and every
/// spawned thread's stack (`THREAD_STACK_PAGES`, `sched::thread`) -- a
/// fixed-stride slot layout requires one uniform size, so `allocate`
/// rejects any other `pages` value outright (see its docs).
pub const STACK_PAGES: usize = 64 * 1024 / FRAME_SIZE;
const STACK_BYTES: u64 = STACK_PAGES as u64 * FRAME_SIZE as u64;

/// One slot = one guard page + `STACK_PAGES` mapped pages, back to back.
const SLOT_SIZE: u64 = FRAME_SIZE as u64 + STACK_BYTES;

/// Brief M2-T3: at least 4096 stacks may be live at once (was a hard,
/// non-reclaimable ceiling of 64 *lifetime* allocations -- see the module
/// docs above for why a live/reuse bound is a wholly different, far more
/// generous thing). `NUM_SLOTS * SLOT_SIZE` (~272 MiB) is a tiny sliver of
/// D15's ~16 TiB dynamic region; the `const` assertion below keeps that
/// true even if either constant changes later.
pub const NUM_SLOTS: usize = 4096;

const _: () = assert!(
    NUM_SLOTS as u64 * SLOT_SIZE <= DYNAMIC_REGION_END - DYNAMIC_REGION_START,
    "kstack: NUM_SLOTS * SLOT_SIZE must fit inside the D15 kernel dynamic region"
);

/// One live kernel stack: `guard` is the single unmapped 4 KiB page right
/// below `bottom`; `[bottom, top)` is mapped and usable (`top` is the
/// initial `rsp` value -- the stack grows down from there).
#[derive(Clone, Copy, Debug)]
pub struct KernelStack {
    pub guard: VirtAddr,
    pub bottom: VirtAddr,
    pub top: VirtAddr,
}

/// What `find_guard` returns: just enough to report or verify a guard-page
/// hit. Unlike `KernelStack`, no `top` -- nothing that calls `find_guard`
/// (the double-fault handler, tests) ever needs it.
#[derive(Clone, Copy, Debug)]
pub struct GuardMatch {
    pub guard: VirtAddr,
    pub bottom: VirtAddr,
}

/// The addresses that belong to slot `index`, computed purely from its
/// position -- the one function both `allocate` and `find_guard` build
/// their answers from, so the two can never disagree about the layout.
fn slot_addrs(index: usize) -> KernelStack {
    let base = DYNAMIC_REGION_START + index as u64 * SLOT_SIZE;
    let guard = VirtAddr::new(base);
    let bottom = VirtAddr::new(base + FRAME_SIZE as u64);
    let top = VirtAddr::new(bottom.as_u64() + STACK_BYTES);
    KernelStack { guard, bottom, top }
}

/// Slot-index bookkeeping: `next_fresh` is a bump allocator over indices
/// that have never been touched; `free` holds indices whose previous
/// occupant has been `free`d and is available for immediate reuse,
/// checked first so a long-running kernel settles into recycling a small
/// working set of slots rather than forever bumping `next_fresh`.
///
/// Not required to be lock-free (unlike `find_guard` below): `allocate`/
/// `free` already call into `vmm`/`pmm`, which take locks of their own, so
/// a plain `spin::Mutex` here adds no new deadlock risk -- only
/// `find_guard` is reachable from the double-fault handler (D14).
struct SlotAllocator {
    next_fresh: usize,
    free: Vec<usize>,
}

impl SlotAllocator {
    const fn new() -> Self {
        Self { next_fresh: 0, free: Vec::new() }
    }

    fn claim(&mut self) -> usize {
        if let Some(index) = self.free.pop() {
            return index;
        }
        let index = self.next_fresh;
        assert!(index < NUM_SLOTS, "kstack: {NUM_SLOTS} stack slots are already live at once");
        self.next_fresh += 1;
        index
    }

    fn release(&mut self, index: usize) {
        self.free.push(index);
    }
}

static SLOTS: Mutex<SlotAllocator> = Mutex::new(SlotAllocator::new());

/// The stack `init_boot_stack` allocated -- the one the kernel is
/// actually running on from that point onward.
static BOOT_STACK: Mutex<Option<KernelStack>> = Mutex::new(None);

/// Allocates a new guard-paged stack: claims a slot (reusing a freed one
/// if any exist, else the next never-touched index) and maps its
/// `STACK_PAGES` frames.
///
/// # Panics
/// If `pages != STACK_PAGES` (fixed-stride slots require the one uniform
/// size every caller in this kernel already uses), if `NUM_SLOTS` stacks
/// are already live at once, or if the PMM is out of memory.
pub fn allocate(pages: usize) -> KernelStack {
    assert_eq!(
        pages, STACK_PAGES,
        "kstack::allocate: fixed-stride slots are always {STACK_PAGES} pages; got {pages}"
    );

    let index = SLOTS.lock().claim();
    let stack = slot_addrs(index);

    let space = vmm::kernel_address_space();
    let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE | PageFlags::GLOBAL;
    for i in 0..STACK_PAGES as u64 {
        let frame = pmm::alloc_frame_zeroed().expect("kstack::allocate: out of memory for a kernel stack");
        space.map_4k(VirtAddr::new(stack.bottom.as_u64() + i * FRAME_SIZE as u64), frame, flags);
    }
    // `stack.guard` is deliberately left unmapped -- that's the entire point.

    stack
}

/// Whether `addr` lies inside some slot's guard page -- purely arithmetic
/// (brief M2-T3): true iff `addr` falls inside the stack-slot region *and*
/// its offset within its slot is less than `FRAME_SIZE`. This says nothing
/// about whether that slot is currently allocated, freed, or never
/// touched -- by construction every slot's leading page is always left
/// unmapped regardless (`allocate` only ever maps `[bottom, top)`), so the
/// answer is correct in every one of those cases without needing to know
/// which applies.
///
/// Used by the double-fault handler to tell a stack overflow apart from
/// any other double fault -- so, per D14, this must never block: no lock,
/// no shared mutable state, nothing but arithmetic on constants and `addr`
/// itself.
pub fn find_guard(addr: VirtAddr) -> Option<GuardMatch> {
    let a = addr.as_u64();
    let rel = a.checked_sub(DYNAMIC_REGION_START)?;
    let slot_region_len = NUM_SLOTS as u64 * SLOT_SIZE;
    if rel >= slot_region_len {
        return None; // beyond the last slot (MMIO/vmalloc territory, or genuinely out of range).
    }
    let offset_in_slot = rel % SLOT_SIZE;
    if offset_in_slot >= FRAME_SIZE as u64 {
        return None; // inside a slot's mapped stack pages, not its guard page.
    }
    let index = (rel / SLOT_SIZE) as usize;
    let stack = slot_addrs(index);
    Some(GuardMatch { guard: stack.guard, bottom: stack.bottom })
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

/// Reclaims a stack's mapped frames (brief M2-T1: a reaped kernel
/// thread's stack) and, per brief M2-T3, returns its slot index to the
/// free list so a *later* `allocate` reuses this exact virtual range and
/// guard page instead of touching a fresh one. Unmaps and frees every
/// page in `[stack.bottom, stack.top)` back to the PMM; the guard page
/// itself was never mapped, so there is nothing to do for it there.
///
/// `pub(crate)`, not `pub` (kernel-review, M2-T1): the only legitimate
/// caller is `sched::schedule`'s reaper, which never frees its own
/// `s.current` (see the module docs there) -- keeping this out of the
/// public API means nothing outside this crate's own scheduler can even
/// attempt to call it on a live stack.
///
/// `owner_exited`/`owner_is_current` are passed explicitly, rather than
/// the owning `Thread` itself, so this lower-level `mm` module doesn't
/// need to depend on `sched`'s types -- but the two invariants that
/// matter (brief M2-T1: "a thread must never free its own stack") are
/// still asserted *here*, not just trusted from the call site, exactly
/// as if this took the `Thread` directly.
///
/// # Panics
/// If `owner_is_current` is `true` (this would be freeing the stack the
/// caller itself is currently running on), or if `owner_exited` is
/// `false` (the owning thread hasn't actually exited yet). Otherwise,
/// relies on `mm::vmm::AddressSpace::unmap`/`mm::pmm::free_frame`'s own
/// panics for genuine misuse (a `stack` this module didn't itself hand
/// out, or one already freed).
pub(crate) fn free(stack: KernelStack, owner_exited: bool, owner_is_current: bool) {
    assert!(!owner_is_current, "kstack::free: refusing to free the currently running thread's own stack");
    assert!(owner_exited, "kstack::free: refusing to free a stack whose owning thread hasn't exited");

    let space = vmm::kernel_address_space();
    let mut addr = stack.bottom.as_u64();
    while addr < stack.top.as_u64() {
        if let Some(phys) = space.unmap(VirtAddr::new(addr)) {
            pmm::free_frame(phys);
        }
        addr += FRAME_SIZE as u64;
    }

    // `stack.guard` is exactly `DYNAMIC_REGION_START + index * SLOT_SIZE`
    // (see `slot_addrs`), so this recovers `index` without needing to
    // store it anywhere else.
    let index = ((stack.guard.as_u64() - DYNAMIC_REGION_START) / SLOT_SIZE) as usize;
    SLOTS.lock().release(index);
}
