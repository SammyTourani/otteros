//! Brief M2-T3 step 8: "initial stack layout of a spawned process (read
//! back argc/argv/auxv through its address space)".
//!
//! Lives in its own, deliberately `zz_`-named module -- not inside
//! `test_cases::proc`, where it would naturally belong thematically --
//! purely so it runs dead last in the suite (`custom_test_frameworks`
//! sorts `#[test_case]`s by fully-qualified `module::function` name, brief
//! M2-T2b, so a module name starting with `zz_` sorts after every other
//! one in this crate). Found the hard way (kernel-review-worthy): this is
//! the only kernel test that `kill`s a process *before it has ever
//! actually run* -- `sched::force_exit` on a still-`Ready` target defers
//! reaping (and, since kernel-review "make memory use flat", `sched::
//! retire`, which is what actually frees its `Thread`/boxed `FxsaveArea`)
//! to whenever `schedule`'s own pick-next loop next happens to discover
//! it sitting `Exited` in the ready queue -- not deterministically before
//! `proc::kill`/`wait` below return. Several *other* tests in
//! `test_cases::proc` (and one in `test_cases::sched`) assert
//! `pmm::stats().free` returns to *exactly* a freshly-captured baseline
//! after spawning their own process/thread; running this one after every
//! such exact-equality assertion has already run avoids that reaping's
//! timing ever being able to invalidate one of them.

use otteros_kernel::arch::x86_64::usermode;
use otteros_kernel::mm::addr::{FRAME_SIZE, VirtAddr};
use otteros_kernel::mm::hhdm;
use otteros_kernel::mm::pmm;
use otteros_kernel::mm::vmm::AddressSpace;
use otteros_kernel::proc;
use otteros_kernel::sched;

/// Reads `size_of::<u64>()` bytes at `addr` in `space` through the
/// kernel's own HHDM alias of whichever physical frame backs it --
/// `usermem::copy_from_user` isn't usable here (brief M2-T3): it assumes
/// `space` is the *currently active* CR3, which a spawned-but-not-yet-run
/// process's address space never is from this test's own kernel thread.
fn read_u64(space: &AddressSpace, addr: u64) -> u64 {
    let page = addr & !(FRAME_SIZE as u64 - 1);
    let offset = (addr - page) as usize;
    let (phys, _, _) = space.translate(VirtAddr::new(page)).expect("expected this stack page to be mapped");
    let src = hhdm::phys_to_virt(phys).as_u64() as *const u8;
    let mut bytes = [0u8; 8];
    // SAFETY: `translate` just confirmed `page` is mapped in `space`, so
    // its HHDM alias is a valid, readable 4 KiB frame; `offset + 8 <=
    // FRAME_SIZE` is guaranteed by every call site below (every field this
    // test reads is 8-byte aligned and never the last word of a page).
    unsafe { core::ptr::copy_nonoverlapping(src.add(offset), bytes.as_mut_ptr(), 8) };
    u64::from_le_bytes(bytes)
}

/// Reads a NUL-terminated string at `addr` in `space`, byte by byte,
/// through the same HHDM-alias technique as `read_u64`.
fn read_cstr(space: &AddressSpace, addr: u64) -> alloc::string::String {
    let mut out = alloc::vec::Vec::new();
    let mut offset = 0u64;
    loop {
        let a = addr + offset;
        let page = a & !(FRAME_SIZE as u64 - 1);
        let page_offset = (a - page) as usize;
        let (phys, _, _) = space.translate(VirtAddr::new(page)).expect("expected this string's page to be mapped");
        let src = hhdm::phys_to_virt(phys).as_u64() as *const u8;
        // SAFETY: `translate` just confirmed `page` is mapped; `page_offset < FRAME_SIZE`.
        let byte = unsafe { *src.add(page_offset) };
        if byte == 0 {
            break;
        }
        out.push(byte);
        offset += 1;
    }
    alloc::string::String::from_utf8(out).expect("test-written argv strings are ASCII")
}

/// Spawns `/bin/hello` with two arguments and inspects its initial stack
/// *before* letting it run at all (no `yield_now` between `spawn` and the
/// reads below -- the same "inspect immediately, clean up after" pattern
/// `test_cases::proc::two_processes_same_vaddr_have_different_physical_
/// frames` already relies on) -- reading is non-destructive either way
/// (nothing here writes), so even a stray preemption landing in between
/// couldn't corrupt what's being checked, only risk the process having
/// already exited (and freed its own stack) first, which `kill`+`wait`
/// below tolerate regardless of which happened.
#[test_case]
fn zz_initial_stack_layout_of_spawned_process() {
    const AT_PAGESZ: u64 = 6;
    const AT_ENTRY: u64 = 9;
    const AT_RANDOM: u64 = 25;
    const AT_NULL: u64 = 0;

    let pid = proc::spawn("/bin/hello", &["argA", "argB"]).expect("/bin/hello should spawn");
    let process = proc::find(pid).expect("just spawned");
    let space = process.address_space();

    // The exact layout `proc::exec::build_initial_stack` documents:
    // rsp itself isn't observable from here (nothing exposes a not-yet-run
    // thread's saved `rsp` outside `sched`), so this walks forward from
    // the *known* top-of-stack constant instead, computing where `argc`
    // must sit from the same lengths `build_image` itself would have used.
    let top = usermode::USER_STACK_TOP;
    let argv = ["argA", "argB"];
    let pointer_area_len = (1 + (argv.len() + 1) + 1 + 8) * 8;
    let strings_len: usize = argv.iter().map(|a| a.len() + 1).sum::<usize>() + 16;
    // `proc::exec::build_image` pads its buffer out to a 16-byte multiple
    // (trailing, unused bytes above the strings) so the resulting `rsp` is
    // itself 16-byte aligned -- see that function's own docs.
    let image_len = (pointer_area_len + strings_len).next_multiple_of(16);
    let rsp = top - image_len as u64;
    assert!(rsp.is_multiple_of(16), "rsp should be 16-byte aligned");

    assert_eq!(read_u64(&space, rsp), 2, "argc should be 2");
    let argv0_ptr = read_u64(&space, rsp + 8);
    let argv1_ptr = read_u64(&space, rsp + 16);
    assert_eq!(read_u64(&space, rsp + 24), 0, "argv must be NULL-terminated");
    assert_eq!(read_u64(&space, rsp + 32), 0, "envp should be an immediate NULL (no env vars yet)");
    assert_eq!(read_u64(&space, rsp + 40), AT_PAGESZ);
    assert_eq!(read_u64(&space, rsp + 48), FRAME_SIZE as u64);
    assert_eq!(read_u64(&space, rsp + 56), AT_ENTRY);
    assert_eq!(read_u64(&space, rsp + 72), AT_RANDOM);
    let random_ptr = read_u64(&space, rsp + 80);
    assert_eq!(read_u64(&space, rsp + 88), AT_NULL);
    assert_eq!(read_u64(&space, rsp + 96), 0);

    assert_eq!(read_cstr(&space, argv0_ptr), "argA");
    assert_eq!(read_cstr(&space, argv1_ptr), "argB");
    assert!(random_ptr > argv1_ptr, "AT_RANDOM's 16 bytes should sit above the argv strings");

    proc::kill(pid, 0);
    proc::wait(pid);
}

/// Kernel-review M2-T3 fix: a frame allocation failing partway through an
/// otherwise-perfectly-valid `/bin/hello` load must surface as
/// `SpawnError::OutOfMemory` (`-ENOMEM` at the syscall boundary) -- never
/// `BadElf`/`-ENOEXEC`, since nothing about the file itself is wrong -- and
/// must leak nothing: `Process::create_from_elf` calls `destroy_unused` on
/// every failure path, which walks back whatever the failed attempt had
/// already mapped and frees it.
#[test_case]
fn spawn_out_of_memory_returns_enomem_and_leaks_nothing() {
    // A real spawn+wait first, settling any one-time cost the *bookkeeping*
    // around a spawn attempt itself needs (process-table growth, and so
    // on) -- the allocation this test goes on to fail deliberately never
    // gets far enough to pay for anything workload-shaped, so a single
    // warm-up (rather than `test_cases::proc::assert_workload_leaks_no_
    // frames`'s full two-round dance) is enough here.
    let warm_up = proc::spawn("/bin/hello", &[]).expect("a normal spawn should succeed");
    proc::wait(warm_up);
    for _ in 0..20 {
        sched::yield_now();
    }

    let baseline = pmm::stats().free;

    // Skip past `AddressSpace::new_user`'s own, single PML4 allocation --
    // an unrelated, pre-existing call this fix doesn't change, and one
    // whose own failure is an `.expect()` this fix is explicitly not
    // about -- then fail every allocation for a while: comfortably more
    // than loading `/bin/hello`'s few segments could ever need, so this
    // reliably fails partway through `elf::load` rather than not at all.
    pmm::inject_alloc_failures_after(1, 64);
    let result = proc::spawn("/bin/hello", &[]);
    pmm::inject_alloc_failures_after(0, 0); // never let a leftover budget bleed into a later test.

    assert!(matches!(result, Err(proc::SpawnError::OutOfMemory)), "expected Err(OutOfMemory), got {result:?}");

    let mut free = pmm::stats().free;
    for _ in 0..50 {
        sched::yield_now();
        free = pmm::stats().free;
        if free >= baseline {
            break;
        }
    }
    assert_eq!(free, baseline, "a failed spawn must leak no frames at all");
}
