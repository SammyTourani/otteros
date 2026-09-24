//! Building a fresh process's stack from a loaded ELF image (brief M2-T3
//! step 4): `proc::elf::load` maps the program's own segments; this module
//! maps the stack and writes the SysV-style argv/envp/auxv prologue onto
//! it (DECISIONS.md D18: "argv/envp copied onto the initial stack
//! SysV-style").

use alloc::vec::Vec;

use crate::arch::x86_64::usermode;
use crate::mm::addr::{FRAME_SIZE, VirtAddr};
use crate::mm::hhdm;
use crate::mm::paging::PageFlags;
use crate::mm::pmm;
use crate::mm::vmm::AddressSpace;
use crate::time;

/// D18: only the top slice of the 16 MiB stack reservation is mapped up
/// front, ending at `usermode::USER_STACK_TOP`; the rest demand-grows
/// (`proc::process::Process::try_grow_stack`, which owns the authoritative
/// copy of the full 16 MiB bound -- this module only ever needs the top
/// slice's own size).
const INITIAL_STACK_MAPPED: u64 = 64 * 1024;

/// The lowest address `Process::create`/`create_from_elf` map up front --
/// also `proc::process::Process`'s initial `stack_mapped_low`.
pub(crate) const STACK_LOW: u64 = usermode::USER_STACK_TOP - INITIAL_STACK_MAPPED;

/// AT_* auxv type numbers this kernel populates -- Linux's own numbering
/// (`syscall::errno`'s docs give the identical rationale for reusing real
/// Linux values: "a userspace helper library can reuse the same constants
/// a Linux program would").
const AT_NULL: u64 = 0;
const AT_PAGESZ: u64 = 6;
const AT_ENTRY: u64 = 9;
const AT_RANDOM: u64 = 25;
const RANDOM_BYTES: usize = 16;

/// Why `build_initial_stack` refused a request.
#[derive(Debug)]
pub(crate) enum StackError {
    /// Never actually reachable from a real `spawn` syscall
    /// (`syscall::table::sys_spawn` caps path to 256 bytes and argv to 32
    /// entries / 4 KiB total, both far below `INITIAL_STACK_MAPPED`), but
    /// checked anyway rather than trusting that arithmetic alone.
    ArgsTooLarge,
    /// The PMM ran out of frames while mapping the stack (kernel-review
    /// fix: this used to be an unconditional `.expect()` panic, which let
    /// a legitimate resource exhaustion -- reachable from ordinary use,
    /// not just a malformed input -- take the whole kernel down instead
    /// of just failing the one `spawn` call).
    OutOfMemory,
}

/// Maps `space`'s stack (the same shape every M2-T2 payload process
/// already got: the top `INITIAL_STACK_MAPPED` of a 16 MiB reservation)
/// and writes the SysV argv/envp/auxv prologue brief M2-T3 step 4
/// describes: `argc, argv[0..n], NULL, envp NULL, auxv (AT_PAGESZ,
/// AT_ENTRY, AT_RANDOM, AT_NULL), strings above`, with the returned `rsp`
/// 16-byte aligned and pointing at `argc`.
///
/// `entry` is only ever used as `AT_ENTRY`'s value here -- the new
/// thread's actual first `rip` is a separate, direct concern of whoever
/// spawns it (`proc::process::Process::create_from_elf`).
pub(crate) fn build_initial_stack(space: &AddressSpace, entry: u64, argv: &[&str]) -> Result<u64, StackError> {
    map_stack_top(space)?;

    let image = build_image(entry, argv)?;
    let base = usermode::USER_STACK_TOP - image.len() as u64;
    if base < STACK_LOW {
        return Err(StackError::ArgsTooLarge);
    }

    write_image(space, base, &image);
    Ok(base)
}

/// Maps the initial `INITIAL_STACK_MAPPED` slice of the stack region --
/// the same shape `proc::process::map_stack_range` gives every M2-T2
/// payload process, kept as a separate copy here (rather than sharing that
/// private helper) since a real `spawn` has nowhere to propagate a
/// mid-`Vec`-growth allocation failure *back to the caller* the way
/// `Process::map_anon` does; like `map_stack_range`, this is only ever
/// called once, for a brand-new address space that has never had anything
/// mapped in this region before.
///
/// Returns `Err(StackError::OutOfMemory)` (kernel-review fix: never
/// panics) if the PMM runs out of frames partway through -- whatever was
/// already mapped stays mapped; the caller (`Process::create_from_elf`)
/// tears down the whole partially-built address space on any error from
/// this function, so a partial stack is never left dangling.
fn map_stack_top(space: &AddressSpace) -> Result<(), StackError> {
    let mut addr = STACK_LOW;
    while addr < usermode::USER_STACK_TOP {
        let frame = pmm::alloc_frame_zeroed().ok_or(StackError::OutOfMemory)?;
        space.map_4k(VirtAddr::new(addr), frame, PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE);
        addr += FRAME_SIZE as u64;
    }
    Ok(())
}

/// Builds the whole argv/envp/auxv prologue as one contiguous byte buffer,
/// as if it already sat at its final address: every pointer inside it is
/// the real, absolute user virtual address it will occupy once
/// `write_image` copies it there. Works out the final length first (from
/// which the final base address follows), then fills in the pointer area
/// (low addresses) and the strings area (high addresses) against that
/// known-in-advance layout -- `argc`, then each `argv` pointer, then a
/// NULL terminator, envp's own (immediate, since there are no environment
/// variables yet) NULL, then the four auxv pairs, then every argv string
/// and the `AT_RANDOM` bytes.
fn build_image(entry: u64, argv: &[&str]) -> Result<Vec<u8>, StackError> {
    let pointer_words = 1 + (argv.len() + 1) + 1 + 8; // argc, argv[..]+NULL, envp NULL, 4 auxv pairs (incl. AT_NULL).
    let pointer_area_len = pointer_words * 8;
    let strings_len: usize = argv.iter().map(|a| a.len() + 1).sum::<usize>() + RANDOM_BYTES;
    let image_len = pointer_area_len + strings_len;

    if image_len > INITIAL_STACK_MAPPED as usize {
        return Err(StackError::ArgsTooLarge);
    }
    // `image_len` itself is rarely a multiple of 16 (it depends on argv's
    // own byte lengths), but the brief's own ABI requires "rsp 16-byte
    // aligned pointing at argc" -- `USER_STACK_TOP` is page- (so 16-byte-)
    // aligned, so padding the *buffer itself* out to a 16-byte multiple is
    // what keeps `base_addr` (== the eventual initial `rsp`, computed from
    // it below) aligned too. The extra bytes go at the very end of the
    // buffer -- i.e. the *highest* addresses, just below `USER_STACK_TOP`,
    // above every string -- so `argc` still lands at offset `0` (`base_addr`
    // itself) exactly as the SysV convention (and every other offset below)
    // assumes.
    let padded_len = image_len.next_multiple_of(16);
    let base_addr = usermode::USER_STACK_TOP - padded_len as u64;

    let mut image = alloc::vec![0u8; padded_len];
    let mut strings_offset = pointer_area_len;
    let mut argv_ptrs: Vec<u64> = Vec::with_capacity(argv.len());

    for arg in argv {
        let addr = base_addr + strings_offset as u64;
        image[strings_offset..strings_offset + arg.len()].copy_from_slice(arg.as_bytes());
        image[strings_offset + arg.len()] = 0;
        argv_ptrs.push(addr);
        strings_offset += arg.len() + 1;
    }

    let random_addr = base_addr + strings_offset as u64;
    image[strings_offset..strings_offset + RANDOM_BYTES].copy_from_slice(&pseudo_random_bytes());
    strings_offset += RANDOM_BYTES;
    debug_assert_eq!(strings_offset, image_len, "proc::exec::build_image: strings area didn't exactly fill the image");

    let mut w = 0usize;
    let mut put = |image: &mut Vec<u8>, value: u64| {
        image[w..w + 8].copy_from_slice(&value.to_le_bytes());
        w += 8;
    };
    put(&mut image, argv.len() as u64); // argc
    for ptr in &argv_ptrs {
        put(&mut image, *ptr);
    }
    put(&mut image, 0); // argv NULL terminator
    put(&mut image, 0); // envp: no environment variables yet, just NULL
    put(&mut image, AT_PAGESZ);
    put(&mut image, FRAME_SIZE as u64);
    put(&mut image, AT_ENTRY);
    put(&mut image, entry);
    put(&mut image, AT_RANDOM);
    put(&mut image, random_addr);
    put(&mut image, AT_NULL);
    put(&mut image, 0);
    debug_assert_eq!(w, pointer_area_len, "proc::exec::build_image: pointer area didn't exactly fill its reserved space");

    Ok(image)
}

/// "16 bytes from the TSC for now" (brief step 4) -- not a real CSPRNG,
/// just entropy-shaped bytes for `AT_RANDOM`, which nothing in this
/// userspace (no libc, no stack-protector canaries, no ASLR) actually
/// consumes yet either.
fn pseudo_random_bytes() -> [u8; RANDOM_BYTES] {
    let lo: u32;
    let hi: u32;
    // SAFETY: `rdtsc` is unprivileged and always safe to execute; this
    // just reads the two 32-bit halves it defines into `eax`/`edx`.
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)) };
    let tsc = (u64::from(hi) << 32) | u64::from(lo);

    let mut out = [0u8; RANDOM_BYTES];
    out[..8].copy_from_slice(&tsc.to_le_bytes());
    out[8..].copy_from_slice(&time::uptime_ms().to_le_bytes());
    out
}

/// Copies `image` into `space`'s stack starting at `base`, one physical
/// frame at a time (`space.translate` -> `hhdm::phys_to_virt`) -- the same
/// "reach a just-mapped user frame through the kernel's own HHDM alias"
/// pattern `proc::process::Process::create` already uses for a payload's
/// code page.
fn write_image(space: &AddressSpace, base: u64, image: &[u8]) {
    let mut done = 0usize;
    while done < image.len() {
        let addr = base + done as u64;
        let page = addr & !(FRAME_SIZE as u64 - 1);
        let page_offset = (addr - page) as usize;
        let (phys, _, _) = space.translate(VirtAddr::new(page)).expect("proc::exec::write_image: stack page not mapped");
        let dst = hhdm::phys_to_virt(phys).as_u64() as *mut u8;
        let chunk = (FRAME_SIZE - page_offset).min(image.len() - done);
        // SAFETY: `dst + page_offset` is the HHDM alias of a frame
        // `map_stack_top` just mapped, exclusively, as this process's own
        // stack (`translate` confirms it's actually mapped there); `chunk`
        // never crosses into the next frame (`FRAME_SIZE - page_offset`
        // caps it); nothing else can be reading or writing this frame
        // concurrently -- the process this stack belongs to hasn't been
        // scheduled yet.
        unsafe { core::ptr::copy_nonoverlapping(image[done..done + chunk].as_ptr(), dst.add(page_offset), chunk) };
        done += chunk;
    }
}
