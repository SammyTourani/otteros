//! One process (brief M2-T2, DECISIONS.md D17): an address space, exactly
//! one thread (for now), and the bookkeeping `syscall::table`'s
//! `map_anon`/stack-growth handlers need.

use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::usermode;
use crate::mm::addr::{FRAME_SIZE, VirtAddr};
use crate::mm::hhdm;
use crate::mm::paging::PageFlags;
use crate::mm::pmm;
use crate::mm::vmm::AddressSpace;
use crate::proc::usermem::USER_SPACE_CEILING;
use crate::proc::{elf, exec, SpawnError};
use crate::sched::{self, ThreadId};

pub type Pid = u64;

/// Why `Process::map_anon` refused a request -- `syscall::table`'s
/// `map_anon` handler maps these to `EINVAL`/`ENOMEM` respectively
/// (kernel-review round 2).
pub(crate) enum MapAnonError {
    /// `len` was zero, overflowed after page-rounding, exceeded the 1 GiB
    /// per-call cap, or the resulting range would reach the reserved
    /// stack region or `usermem::USER_SPACE_CEILING`.
    Invalid,
    OutOfMemory,
}

/// D17/brief step 4: a process's anonymous-mapping cursor (`map_anon`)
/// starts here and bumps upward.
const ANON_BASE: u64 = 0x0000_0000_1000_0000;
/// D18: 16 MiB of *reserved* address space for the stack, ending at
/// `usermode::USER_STACK_TOP`; only the top slice is actually mapped at
/// creation (`INITIAL_STACK_MAPPED`), the rest demand-grows.
const STACK_RESERVED: u64 = 16 * 1024 * 1024;
/// Brief M2-T2 step 4: "only the top 64 KiB mapped at start".
const INITIAL_STACK_MAPPED: u64 = 64 * 1024;
/// One past the lowest address the 16 MiB reservation covers -- a fault
/// below this is a real stack overflow (into the guard region), not
/// something `try_grow_stack` ever resolves.
const STACK_RESERVED_BOTTOM: u64 = usermode::USER_STACK_TOP - STACK_RESERVED;

/// `Process::name`'s storage (brief M2-T3, kernel-review-worthy): a
/// `spawn`ed process's name comes from a path a userspace caller supplied
/// at runtime, which can't be `'static` -- see `create_from_elf` -- so it
/// needs a genuine heap allocation. The M2-T2 payload path (`create`)
/// still only ever receives a `'static` string-literal name, the same as
/// before this type existed; giving that path its own `Static` variant
/// (rather than unconditionally allocating a `String` for every process
/// regardless of which path created it) keeps it costing exactly what it
/// always did -- found the hard way: every `test_cases::proc` test that
/// asserts an *exact* PMM free-frame count after a payload-spawned
/// process exits (several do) is sensitive to the kernel heap needing a
/// fresh slab page even once, anywhere earlier in the suite, for a
/// permanently-retained allocation (the same already-accepted class of
/// cost `sched::Thread`'s own boxed `FxsaveArea` has) -- an unconditional
/// `String` here would have added exactly that, to *every* process this
/// kernel ever creates, payloads included.
enum ProcName {
    Static(&'static str),
    Owned(String),
}

impl ProcName {
    fn as_str(&self) -> &str {
        match self {
            ProcName::Static(s) => s,
            ProcName::Owned(s) => s,
        }
    }
}

pub struct Process {
    pid: Pid,
    name: ProcName,
    address_space: AddressSpace,
    main_thread: Arc<sched::Thread>,
    /// D17: "an exit code and a parent" -- `None` for every M2-T2 process
    /// (nothing can create a child process yet: `spawn` is `-ENOSYS` until
    /// M2-T3, and every process that exists here was started directly by
    /// kernel test code via `proc::spawn_payload`, not by another
    /// process). Kept as real state (not just left out) so `wait`'s future
    /// "only my own children" enforcement has somewhere to read from.
    #[allow(dead_code)]
    parent: Option<Pid>,
    /// D17/brief step 4: the next `map_anon` call's return address.
    anon_cursor: AtomicU64,
    /// The lowest address currently mapped in the stack region --
    /// `proc::fault::handle_page_fault`'s demand-growth floor.
    stack_mapped_low: AtomicU64,
    /// Kernel-review round 3: whether *someone* has already claimed the
    /// right to tear down this process's address space (`begin_exit`).
    /// `false` -> `true` exactly once, ever, per process -- a concurrent
    /// second `kill`, or a `kill` racing the process's own fault/syscall
    /// self-exit, must never both call `free_user_space` on the same
    /// address space (each call is individually idempotent, but two
    /// *interleaved* calls -- one preempting the other mid-walk -- could
    /// still free the same frame twice).
    exiting: AtomicBool,
}

impl Process {
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Atomically claims the right to tear down this process's address
    /// space: `true` for the caller that wins (exactly one ever does),
    /// `false` for every other caller (kernel-review round 3) -- a
    /// concurrent second `kill`, or a `kill` racing the process's own
    /// fault/syscall self-exit, does nothing instead of double-freeing.
    pub(crate) fn begin_exit(&self) -> bool {
        self.exiting.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn parent(&self) -> Option<Pid> {
        self.parent
    }

    pub fn main_thread(&self) -> Arc<sched::Thread> {
        Arc::clone(&self.main_thread)
    }

    pub fn address_space(&self) -> AddressSpace {
        self.address_space
    }

    pub fn main_thread_id(&self) -> ThreadId {
        self.main_thread.id
    }

    /// Builds a fresh process: a new address space, `code` mapped
    /// executable+user at `usermode::ENTRY_RIP`, the initial slice of a
    /// demand-growable stack, and a `Ready` (not yet running) thread for
    /// it (`sched::spawn_user`).
    ///
    /// # Panics
    /// If `code` doesn't fit in one 4 KiB page (every brief M2-T2 payload
    /// is well under this), or if the PMM is out of memory.
    pub(crate) fn create(pid: Pid, name: &'static str, code: &[u8]) -> Arc<Self> {
        assert!(
            code.len() <= FRAME_SIZE,
            "proc::Process::create: payload is {} bytes, but only one page is ever mapped for code",
            code.len()
        );

        let space = AddressSpace::new_user();

        let code_frame = pmm::alloc_frame_zeroed().expect("proc::Process::create: out of memory for a payload's code page");
        let code_virt = hhdm::phys_to_virt(code_frame);
        // SAFETY: `code_frame` was just allocated and zeroed by this call
        // alone, so its HHDM alias is exclusively ours; `code.len() <=
        // FRAME_SIZE` (just asserted), so the copy stays inside it.
        // Writing the payload through the *kernel's* HHDM alias before
        // mapping the same frame executable+user (below) is the same
        // "build then map" ordering `mm::vmm::map_kernel_image` already
        // relies on for the kernel's own image.
        unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), code_virt.as_u64() as *mut u8, code.len()) };
        // `USER`, not `WRITABLE`: user code is read/execute only (W^X,
        // matching the kernel's own image policy) -- `map_4k` ORs in
        // `PRESENT` automatically, and never sets `NO_EXECUTE` on its own.
        space.map_4k(VirtAddr::new(usermode::ENTRY_RIP), code_frame, PageFlags::USER);

        let stack_low = usermode::USER_STACK_TOP - INITIAL_STACK_MAPPED;
        map_stack_range(&space, stack_low, usermode::USER_STACK_TOP);

        let main_thread_id = sched::spawn_user(name, space, usermode::ENTRY_RIP, usermode::USER_STACK_TOP);
        let main_thread = sched::find(main_thread_id).expect("proc::Process::create: just spawned this thread");

        Arc::new(Self {
            pid,
            name: ProcName::Static(name),
            address_space: space,
            main_thread,
            parent: None,
            anon_cursor: AtomicU64::new(ANON_BASE),
            stack_mapped_low: AtomicU64::new(stack_low),
            exiting: AtomicBool::new(false),
        })
    }

    /// Like `create`, but for a real ELF binary (brief M2-T3's `spawn`
    /// syscall, via `proc::spawn`): loads `elf_data` (`proc::elf::load`)
    /// into a fresh address space instead of mapping one fixed hand-
    /// assembled page, builds a SysV-style initial stack for `argv`
    /// (`proc::exec::build_initial_stack`) instead of an empty one, and
    /// starts the new thread at the ELF's own entry point instead of the
    /// M2-T2 payloads' shared `usermode::ENTRY_RIP` constant.
    ///
    /// Never leaks the address space it creates: if either step fails,
    /// `space` (which nothing else has seen yet -- no thread was spawned
    /// for it, it was never registered anywhere, never activated) is torn
    /// down before returning `Err`.
    pub(crate) fn create_from_elf(pid: Pid, name: &str, elf_data: &[u8], argv: &[&str], parent_pid: Option<Pid>) -> Result<Arc<Self>, SpawnError> {
        let space = AddressSpace::new_user();

        // Kernel-review fix: a genuine resource exhaustion (`ElfError::
        // OutOfMemory`/`exec::StackError::OutOfMemory`) is a distinct
        // failure from "this file/request is malformed" -- it must map to
        // `-ENOMEM`, not `-ENOEXEC`/`-E2BIG`, so a caller can tell "try
        // again later" apart from "this will never work". Either way,
        // `destroy_unused` frees every frame and mapping already created
        // for `space` before this returns -- `AddressSpace::
        // free_user_space` walks and frees whatever's *actually* mapped,
        // regardless of how far loading got before it failed, so a
        // mid-load failure never leaks the segments/stack pages that were
        // already in place.
        let image = match elf::load(&space, elf_data) {
            Ok(image) => image,
            Err(elf::ElfError::OutOfMemory) => {
                destroy_unused(space);
                return Err(SpawnError::OutOfMemory);
            }
            Err(e) => {
                destroy_unused(space);
                return Err(SpawnError::BadElf(e));
            }
        };
        let initial_rsp = match exec::build_initial_stack(&space, image.entry, argv) {
            Ok(rsp) => rsp,
            Err(exec::StackError::OutOfMemory) => {
                destroy_unused(space);
                return Err(SpawnError::OutOfMemory);
            }
            Err(exec::StackError::ArgsTooLarge) => {
                destroy_unused(space);
                return Err(SpawnError::BadStack);
            }
        };

        // A real ELF process's thread isn't given `name` itself (`Thread`
        // still only ever stores a `'static str`, unlike `Process` now --
        // see the `name` field's own docs) -- nothing reads a user
        // thread's name for anything but kernel-internal debugging, so a
        // constant is enough; the *process*, which callers actually
        // observe (`proc::fault`'s kill log, `getpid`/`wait`), keeps the
        // real one.
        let main_thread_id = sched::spawn_user("user", space, image.entry, initial_rsp);
        let main_thread = sched::find(main_thread_id).expect("proc::Process::create_from_elf: just spawned this thread");

        // Extract basename from path for the process name (e.g., "/bin/init" -> "init")
        let basename = name.split('/').next_back().unwrap_or(name);

        Ok(Arc::new(Self {
            pid,
            name: ProcName::Owned(String::from(basename)),
            address_space: space,
            main_thread,
            parent: parent_pid,
            anon_cursor: AtomicU64::new(ANON_BASE),
            stack_mapped_low: AtomicU64::new(exec::STACK_LOW),
            exiting: AtomicBool::new(false),
        }))
    }

    /// Bumps the anonymous-mapping cursor by `len` (rounded up to whole
    /// pages) and maps that many fresh zeroed, writable, user, non-
    /// executable frames there (`syscall::table`'s `map_anon`). Returns
    /// the *start* of the new mapping.
    ///
    /// Kernel-review round 2: rejects (`Err`, never a panic/assert) a
    /// zero, overflowing, or larger-than-`MAX_MAP_ANON_LEN`-per-call `len`,
    /// and a request whose resulting range would reach the reserved stack
    /// region or `usermem::USER_SPACE_CEILING` -- checked *before*
    /// committing the cursor, so a rejected call never advances it (unlike
    /// a real failure partway through the actual mapping loop below, which
    /// -- same as before -- leaves whatever was already mapped in place
    /// rather than unwinding it).
    pub(crate) fn map_anon(&self, len: usize) -> Result<VirtAddr, MapAnonError> {
        /// Brief kernel-review round 2: "max 1 GiB per call".
        const MAX_MAP_ANON_LEN: usize = 1024 * 1024 * 1024;

        if len == 0 || len > MAX_MAP_ANON_LEN {
            return Err(MapAnonError::Invalid);
        }
        let pages = len.div_ceil(FRAME_SIZE);
        let mapped_len = (pages as u64).checked_mul(FRAME_SIZE as u64).ok_or(MapAnonError::Invalid)?;

        // Single-threaded per process today (D17: one thread per process),
        // so a plain load-check-store (not a true compare-and-swap) can't
        // race with itself; a second thread sharing this cursor is a
        // future milestone's problem to make atomic for real.
        let base = self.anon_cursor.load(Ordering::Relaxed);
        let end = base.checked_add(mapped_len).ok_or(MapAnonError::Invalid)?;
        if end > STACK_RESERVED_BOTTOM || end > USER_SPACE_CEILING {
            return Err(MapAnonError::Invalid);
        }
        self.anon_cursor.store(end, Ordering::Relaxed);

        for i in 0..pages as u64 {
            let frame = pmm::alloc_frame_zeroed().ok_or(MapAnonError::OutOfMemory)?;
            self.address_space.map_4k(
                VirtAddr::new(base + i * FRAME_SIZE as u64),
                frame,
                PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE,
            );
        }
        Ok(VirtAddr::new(base))
    }

    /// Unmaps and frees `[addr, addr + len)` (`syscall::table`'s `unmap`).
    ///
    /// Kernel-review round 2: `Err` (never a panic/assert, and never
    /// touching a single page table entry) unless `addr`/`len` are both
    /// 4 KiB aligned, `len` is non-zero, the *whole* range fits (checked
    /// arithmetic) inside the process's own user half `[usermode::
    /// ENTRY_RIP, usermem::USER_SPACE_CEILING)` -- so this can never reach
    /// a kernel PML4 entry (256..511, D15), which sits at and above
    /// `USER_SPACE_CEILING` by construction -- and every page in the
    /// range is *actually mapped*: checked in a first pass before this
    /// unmaps anything, so a bad range is rejected atomically rather than
    /// partially unmapped.
    pub(crate) fn unmap(&self, addr: u64, len: u64) -> Result<(), ()> {
        if len == 0 || !addr.is_multiple_of(FRAME_SIZE as u64) || !len.is_multiple_of(FRAME_SIZE as u64) {
            return Err(());
        }
        let end = addr.checked_add(len).ok_or(())?;
        if addr < usermode::ENTRY_RIP || end > USER_SPACE_CEILING {
            return Err(());
        }

        let mut a = addr;
        while a < end {
            if self.address_space.translate(VirtAddr::new(a)).is_none() {
                return Err(());
            }
            a += FRAME_SIZE as u64;
        }

        let mut a = addr;
        while a < end {
            if let Some(phys) = self.address_space.unmap(VirtAddr::new(a)) {
                pmm::free_frame(phys);
            }
            a += FRAME_SIZE as u64;
        }
        Ok(())
    }

    /// Resolves a ring-3 page fault at `fault_addr` by demand-growing the
    /// stack, if it falls inside the reserved-but-not-yet-mapped region
    /// below the current floor (brief M2-T2 step 8). `false` if
    /// `fault_addr` isn't a stack-growth case at all (already mapped, at
    /// or above the current floor, or below the entire 16 MiB reservation
    /// -- the guard region) or the PMM is out of memory partway through;
    /// `proc::fault::handle_page_fault` kills the process either way.
    pub(crate) fn try_grow_stack(&self, fault_addr: VirtAddr) -> bool {
        let target = fault_addr.align_down(FRAME_SIZE as u64).as_u64();
        if target < STACK_RESERVED_BOTTOM {
            return false;
        }
        let old_low = self.stack_mapped_low.load(Ordering::Acquire);
        if target >= old_low {
            return false;
        }

        let mut addr = target;
        while addr < old_low {
            let Some(frame) = pmm::alloc_frame_zeroed() else {
                // Whatever was mapped in this call so far stays mapped
                // (there is no partial rollback) -- the caller kills the
                // process regardless of `false` here, which frees it all
                // again via `free_user_space` immediately afterward.
                self.stack_mapped_low.store(addr + FRAME_SIZE as u64, Ordering::Release);
                return false;
            };
            self.address_space.map_4k(
                VirtAddr::new(addr),
                frame,
                PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE,
            );
            addr += FRAME_SIZE as u64;
        }
        self.stack_mapped_low.store(target, Ordering::Release);
        true
    }
}

/// Tears down a brand-new address space that `create_from_elf` decided not
/// to use after all (a validation failure partway through building it).
/// Safe specifically because nothing has ever seen `space` yet: no thread
/// was spawned for it (a failure here always happens *before*
/// `sched::spawn_user`), it was never registered in `proc::REGISTRY`, and
/// it was never `activate()`d -- the exact same "fresh, private,
/// exclusively ours" precondition `free_user_space`/`destroy` themselves
/// document.
fn destroy_unused(space: AddressSpace) {
    space.free_user_space();
    // SAFETY: `space` was created moments ago by this exact call chain and
    // has not been given to anything else (see this function's own docs)
    // -- no CPU has it (or will ever have it) loaded in CR3.
    unsafe { space.destroy() };
}

/// Maps `[low, high)` as a writable, user, non-executable stack slice,
/// one zeroed frame at a time. Shared by `Process::create` (the initial 64
/// KiB) and, conceptually, `try_grow_stack`'s own loop (kept separate
/// there since it needs to update `stack_mapped_low` incrementally on an
/// out-of-memory partway through, which this simpler helper doesn't need
/// to care about).
fn map_stack_range(space: &AddressSpace, low: u64, high: u64) {
    let mut addr = low;
    while addr < high {
        let frame = pmm::alloc_frame_zeroed().expect("proc::process::map_stack_range: out of memory for a payload's stack");
        space.map_4k(VirtAddr::new(addr), frame, PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE);
        addr += FRAME_SIZE as u64;
    }
}
