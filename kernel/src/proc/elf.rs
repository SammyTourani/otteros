//! ELF64 loader (brief M2-T3 step 2): validates a static, non-PIE x86_64
//! executable and maps its `PT_LOAD` segments into a fresh user address
//! space. Parses bytes from the initramfs as if they were hostile (no
//! unchecked arithmetic on file-supplied offsets/sizes, no indexing past a
//! bounds check) even though today's only source is our own build --
//! every rejection is an `ElfError` value, never a panic.

use alloc::vec::Vec;

use crate::mm::addr::{FRAME_SIZE, VirtAddr};
use crate::mm::hhdm;
use crate::mm::paging::PageFlags;
use crate::mm::pmm;
use crate::mm::vmm::AddressSpace;

const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EV_CURRENT: u8 = 1;
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 0x3e;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_TLS: u32 = 7;

const PF_X: u32 = 1;
const PF_W: u32 = 2;

const EHDR_SIZE: usize = 64;
const PHDR_SIZE: usize = 56;

/// DECISIONS.md D18's user link base (`arch::x86_64::usermode::ENTRY_RIP`)
/// duplicated as a plain constant: no `PT_LOAD` segment may start below
/// it. A separate constant (not an import of `usermode::ENTRY_RIP`) since
/// that one is specifically "where the M2-T2 hand-assembled payload's
/// single page goes", not a general "lowest legal segment address" --
/// they happen to share the same value today because D18 fixes both to
/// the same link address.
const SEGMENT_LOW: u64 = 0x0000_0000_0040_0000;
/// Brief M2-T3's own bound ("vaddr within `[0x400000, 0x0000_7000_0000_0000)`"),
/// restored after a kernel-review finding: an earlier version of this
/// constant used `usermem::USER_SPACE_CEILING` (D15's `0x0000_8000_0000_0000`)
/// instead, which does reject anything aimed at the kernel half but does
/// *not* exclude a process's own 16 MiB reserved stack region
/// (`proc::process::STACK_RESERVED_BOTTOM..usermode::USER_STACK_TOP`, which
/// sits just below `0x0000_8000_0000_0000`) -- a hostile `PT_LOAD` could
/// still land there and collide with the stack `proc::exec::
/// build_initial_stack` maps afterward. `0x0000_7000_0000_0000` sits
/// comfortably below that whole reservation, so this one static bound
/// excludes it entirely without needing to duplicate the exact reservation
/// window here.
const SEGMENT_HIGH: u64 = 0x0000_7000_0000_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooShort,
    BadMagic,
    WrongClass,
    WrongEndian,
    WrongVersion,
    WrongMachine,
    WrongType,
    TruncatedProgramHeaders,
    SegmentOutOfRange,
    SegmentOverlap,
    WriteAndExecute,
    FilesizeExceedsMemsize,
    UnsupportedInterp,
    UnsupportedDynamic,
    UnsupportedTls,
    EntryNotExecutable,
    OutOfMemory,
}

#[derive(Debug)]
pub struct ElfImage {
    pub entry: u64,
}

fn u16_at(data: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(off..off + 2)?.try_into().ok()?))
}

fn u64_at(data: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(off..off + 8)?.try_into().ok()?))
}

struct ProgramHeader {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    /// `[page_start, page_end)`: `p_vaddr`/`p_vaddr + p_memsz` rounded
    /// outward to whole pages -- computed once, used both for the
    /// pairwise overlap check and `map_segment`'s own loop bounds, so the
    /// two can never disagree about what "this segment's pages" means
    /// (kernel-review-worthy: two segments whose *byte* ranges don't
    /// overlap can still round down/up onto the same page, which would
    /// otherwise make `map_segment` try to map that page twice and panic
    /// instead of this module cleanly rejecting it first).
    page_start: u64,
    page_end: u64,
}

fn read_phdr(data: &[u8], off: usize) -> Result<ProgramHeader, ElfError> {
    let get_u32 = |o: usize| -> Result<u32, ElfError> {
        Ok(u32::from_le_bytes(data.get(off + o..off + o + 4).ok_or(ElfError::TruncatedProgramHeaders)?.try_into().unwrap()))
    };
    let get_u64 = |o: usize| -> Result<u64, ElfError> { u64_at(data, off + o).ok_or(ElfError::TruncatedProgramHeaders) };

    let p_vaddr = get_u64(16)?;
    let p_memsz = get_u64(40)?;
    let page_start = p_vaddr & !(FRAME_SIZE as u64 - 1);
    // Kernel-review fix: both `p_vaddr` and `p_memsz` are raw, unchecked
    // file data -- a hostile header with `p_vaddr` near `u64::MAX` can
    // make `p_vaddr + p_memsz` succeed (via `checked_add`) yet still sit
    // too close to `u64::MAX` for `next_multiple_of` to round up without
    // itself overflowing. `checked_next_multiple_of` rejects that instead
    // of overflowing (a plain `next_multiple_of` panics on overflow in a
    // debug build, or silently wraps to a bogus, too-small `page_end` in
    // release -- either way, reachable from nothing but a malformed file).
    let seg_end = p_vaddr.checked_add(p_memsz).ok_or(ElfError::SegmentOutOfRange)?;
    let page_end = seg_end.checked_next_multiple_of(FRAME_SIZE as u64).ok_or(ElfError::SegmentOutOfRange)?;

    Ok(ProgramHeader {
        p_type: get_u32(0)?,
        p_flags: get_u32(4)?,
        p_offset: get_u64(8)?,
        p_vaddr,
        p_filesz: get_u64(32)?,
        p_memsz,
        page_start,
        page_end,
    })
}

/// Validates `data` as a static, non-PIE, x86_64 ELF64 executable and maps
/// every `PT_LOAD` segment into `space` (brief M2-T3 step 2). Returns the
/// entry point on success. Every validation pass runs to completion before
/// the first `map_4k` call, so `space` is left exactly as it was if this
/// returns `Err`.
pub fn load(space: &AddressSpace, data: &[u8]) -> Result<ElfImage, ElfError> {
    validate_header(data)?;

    let entry = u64_at(data, 24).ok_or(ElfError::TooShort)?;
    let phoff = u64_at(data, 32).ok_or(ElfError::TooShort)? as usize;
    let phentsize = u16_at(data, 54).ok_or(ElfError::TooShort)? as usize;
    let phnum = u16_at(data, 56).ok_or(ElfError::TooShort)? as usize;
    if phentsize < PHDR_SIZE {
        return Err(ElfError::TruncatedProgramHeaders);
    }

    // Kernel-review fix: validate the *entire* program header table's
    // extent against the file length once, up front, with checked
    // arithmetic -- before ever allocating a `Vec` sized to `phnum`.
    // `phnum` is a `u16` straight from the file (so at most 65535), but a
    // tiny hostile file can still claim the maximum and, without this
    // check, force a `with_capacity` sized for all of them regardless of
    // whether the file could possibly contain that many; this bounds the
    // allocation to what the file's own length can actually support.
    let phdr_table_len = phnum.checked_mul(phentsize).ok_or(ElfError::TruncatedProgramHeaders)?;
    let phdr_table_end = phoff.checked_add(phdr_table_len).ok_or(ElfError::TruncatedProgramHeaders)?;
    if phdr_table_end > data.len() {
        return Err(ElfError::TruncatedProgramHeaders);
    }

    let mut headers = Vec::with_capacity(phnum);
    for i in 0..phnum {
        let off = phoff.checked_add(i * phentsize).ok_or(ElfError::TruncatedProgramHeaders)?;
        if off.checked_add(PHDR_SIZE).ok_or(ElfError::TruncatedProgramHeaders)? > data.len() {
            return Err(ElfError::TruncatedProgramHeaders);
        }
        headers.push(read_phdr(data, off)?);
    }

    let mut load_segments = Vec::new();
    for ph in &headers {
        match ph.p_type {
            PT_INTERP => return Err(ElfError::UnsupportedInterp),
            PT_DYNAMIC => return Err(ElfError::UnsupportedDynamic),
            PT_TLS => return Err(ElfError::UnsupportedTls),
            PT_LOAD => load_segments.push(ph),
            _ => {} // PT_GNU_STACK and anything else this loader doesn't care about: ignored.
        }
    }

    for ph in &load_segments {
        validate_segment(data, ph)?;
    }
    for (i, a) in load_segments.iter().enumerate() {
        for b in &load_segments[i + 1..] {
            if a.page_start < b.page_end && b.page_start < a.page_end {
                return Err(ElfError::SegmentOverlap);
            }
        }
    }

    let entry_ok = load_segments
        .iter()
        .any(|ph| ph.p_flags & PF_X != 0 && entry >= ph.p_vaddr && entry < ph.p_vaddr.saturating_add(ph.p_memsz));
    if !entry_ok {
        return Err(ElfError::EntryNotExecutable);
    }

    for ph in &load_segments {
        map_segment(space, data, ph)?;
    }

    Ok(ElfImage { entry })
}

fn validate_header(data: &[u8]) -> Result<(), ElfError> {
    if data.len() < EHDR_SIZE {
        return Err(ElfError::TooShort);
    }
    if data[0..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(ElfError::BadMagic);
    }
    if data[4] != ELFCLASS64 {
        return Err(ElfError::WrongClass);
    }
    if data[5] != ELFDATA2LSB {
        return Err(ElfError::WrongEndian);
    }
    if data[6] != EV_CURRENT {
        return Err(ElfError::WrongVersion);
    }
    if u16_at(data, 16).ok_or(ElfError::TooShort)? != ET_EXEC {
        return Err(ElfError::WrongType);
    }
    if u16_at(data, 18).ok_or(ElfError::TooShort)? != EM_X86_64 {
        return Err(ElfError::WrongMachine);
    }
    Ok(())
}

fn validate_segment(data: &[u8], ph: &ProgramHeader) -> Result<(), ElfError> {
    if ph.p_filesz > ph.p_memsz {
        return Err(ElfError::FilesizeExceedsMemsize);
    }
    if ph.p_flags & PF_W != 0 && ph.p_flags & PF_X != 0 {
        return Err(ElfError::WriteAndExecute);
    }
    let seg_end = ph.p_vaddr.checked_add(ph.p_memsz).ok_or(ElfError::SegmentOutOfRange)?;
    if ph.p_vaddr < SEGMENT_LOW || seg_end > SEGMENT_HIGH {
        return Err(ElfError::SegmentOutOfRange);
    }
    let file_end = ph.p_offset.checked_add(ph.p_filesz).ok_or(ElfError::SegmentOutOfRange)?;
    if file_end > data.len() as u64 {
        return Err(ElfError::SegmentOutOfRange);
    }
    Ok(())
}

fn map_segment(space: &AddressSpace, data: &[u8], ph: &ProgramHeader) -> Result<(), ElfError> {
    let mut flags = PageFlags::USER;
    if ph.p_flags & PF_W != 0 {
        flags |= PageFlags::WRITABLE;
    }
    if ph.p_flags & PF_X == 0 {
        flags |= PageFlags::NO_EXECUTE;
    }

    let mut page = ph.page_start;
    while page < ph.page_end {
        let frame = pmm::alloc_frame_zeroed().ok_or(ElfError::OutOfMemory)?;
        let virt = hhdm::phys_to_virt(frame).as_u64() as *mut u8;

        // Copy whichever bytes of this page fall inside [p_vaddr, p_vaddr
        // + p_filesz) from the file -- anything before p_vaddr (only
        // possible on the first page, if p_vaddr isn't itself page-
        // aligned) or at/after p_filesz (the bss tail) stays zero (the
        // frame is already zeroed by `alloc_frame_zeroed`).
        let page_file_start = page.max(ph.p_vaddr);
        let page_file_end = (page + FRAME_SIZE as u64).min(ph.p_vaddr + ph.p_filesz);
        if page_file_end > page_file_start {
            let src_off = (page_file_start - ph.p_vaddr + ph.p_offset) as usize;
            let dst_off = (page_file_start - page) as usize;
            let len = (page_file_end - page_file_start) as usize;
            // SAFETY: `validate_segment` already confirmed `p_offset +
            // p_filesz <= data.len()`, and `[page_file_start,
            // page_file_end)` is always inside `[p_vaddr, p_vaddr +
            // p_filesz)`, so `src_off..src_off+len` is inside `data`.
            // `virt` is a fresh, exclusively-owned, zeroed frame this call
            // alone just allocated, and `dst_off + len <= FRAME_SIZE`
            // because `page_file_end <= page + FRAME_SIZE`.
            unsafe { core::ptr::copy_nonoverlapping(data[src_off..src_off + len].as_ptr(), virt.add(dst_off), len) };
        }

        space.map_4k(VirtAddr::new(page), frame, flags);
        page += FRAME_SIZE as u64;
    }
    Ok(())
}
