//! ELF64 loader tests (brief M2-T3 step 8): a hand-built, otherwise-valid
//! minimal ELF64 image, mutated one field at a time to exercise every
//! rejection `proc::elf::load` documents, plus the real `/bin/hello` from
//! the initramfs. The "initial stack layout of a spawned process" test
//! brief step 8 also asks for lives in `test_cases::proc` instead (as
//! `zz_initial_stack_layout_of_spawned_process`) -- see that function's
//! own doc comment for why it has to run dead last in the suite rather
//! than here, alongside its siblings.

use alloc::vec::Vec;

use otteros_kernel::fs;
use otteros_kernel::mm::vmm::AddressSpace;
use otteros_kernel::proc::elf::{self, ElfError};

/// One `PT_LOAD` segment, RX, at `0x400000`, covering file offset
/// `0x1000..0x1010` (16 bytes) -- otherwise-valid so every test below
/// mutates exactly one thing away from "accepted".
fn minimal_elf() -> Vec<u8> {
    let mut buf = alloc::vec![0u8; 0x1010];

    buf[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    buf[4] = 2; // ELFCLASS64
    buf[5] = 1; // ELFDATA2LSB
    buf[6] = 1; // EV_CURRENT
    buf[16..18].copy_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    buf[18..20].copy_from_slice(&0x3eu16.to_le_bytes()); // e_machine = EM_X86_64
    buf[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
    buf[24..32].copy_from_slice(&0x400000u64.to_le_bytes()); // e_entry
    buf[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    buf[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
    buf[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    buf[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum

    buf[PH..PH + 4].copy_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    buf[PH + 4..PH + 8].copy_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    buf[PH + 8..PH + 16].copy_from_slice(&0x1000u64.to_le_bytes()); // p_offset
    buf[PH + 16..PH + 24].copy_from_slice(&0x400000u64.to_le_bytes()); // p_vaddr
    buf[PH + 24..PH + 32].copy_from_slice(&0x400000u64.to_le_bytes()); // p_paddr
    buf[PH + 32..PH + 40].copy_from_slice(&16u64.to_le_bytes()); // p_filesz
    buf[PH + 40..PH + 48].copy_from_slice(&16u64.to_le_bytes()); // p_memsz
    buf[PH + 48..PH + 56].copy_from_slice(&0x1000u64.to_le_bytes()); // p_align

    buf
}

const PH: usize = 64;

/// Loads `data` into a throwaway address space and tears it down again --
/// every test here only cares about `elf::load`'s `Result`, never about
/// the address space surviving afterward.
fn load_and_discard(data: &[u8]) -> Result<elf::ElfImage, ElfError> {
    let space = AddressSpace::new_user();
    let result = elf::load(&space, data);
    space.free_user_space();
    // SAFETY: `space` was created by this function alone, was never
    // registered with `proc`, activated, or given to any thread -- nothing
    // can be relying on it.
    unsafe { space.destroy() };
    result
}

#[test_case]
fn elf_rejects_bad_magic() {
    let mut data = minimal_elf();
    data[0] = 0x00;
    assert_eq!(load_and_discard(&data).unwrap_err(), ElfError::BadMagic);
}

#[test_case]
fn elf_rejects_32_bit_class() {
    let mut data = minimal_elf();
    data[4] = 1; // ELFCLASS32
    assert_eq!(load_and_discard(&data).unwrap_err(), ElfError::WrongClass);
}

#[test_case]
fn elf_rejects_wrong_machine() {
    let mut data = minimal_elf();
    data[18..20].copy_from_slice(&0x03u16.to_le_bytes()); // EM_386
    assert_eq!(load_and_discard(&data).unwrap_err(), ElfError::WrongMachine);
}

#[test_case]
fn elf_rejects_truncated_program_headers() {
    let mut data = minimal_elf();
    // The header still claims one 56-byte program header at offset 64,
    // but the file itself ends only 10 bytes into it.
    data.truncate(64 + 10);
    assert_eq!(load_and_discard(&data).unwrap_err(), ElfError::TruncatedProgramHeaders);
}

#[test_case]
fn elf_rejects_segment_outside_user_range() {
    let mut data = minimal_elf();
    data[PH + 16..PH + 24].copy_from_slice(&0x1000u64.to_le_bytes()); // p_vaddr below D18's link base
    assert_eq!(load_and_discard(&data).unwrap_err(), ElfError::SegmentOutOfRange);
}

#[test_case]
fn elf_rejects_write_and_execute_segment() {
    let mut data = minimal_elf();
    data[PH + 4..PH + 8].copy_from_slice(&7u32.to_le_bytes()); // PF_R | PF_W | PF_X
    assert_eq!(load_and_discard(&data).unwrap_err(), ElfError::WriteAndExecute);
}

#[test_case]
fn elf_rejects_filesize_exceeds_memsize() {
    let mut data = minimal_elf();
    data[PH + 32..PH + 40].copy_from_slice(&32u64.to_le_bytes()); // p_filesz > p_memsz (16)
    assert_eq!(load_and_discard(&data).unwrap_err(), ElfError::FilesizeExceedsMemsize);
}

/// Kernel-review M2-T3 fix: a `PT_LOAD` segment landing inside the 16 MiB
/// stack reservation (`proc::process::STACK_RESERVED_BOTTOM..usermode::
/// USER_STACK_TOP`) must be rejected -- `SEGMENT_HIGH` used to be D15's
/// full user-space ceiling, which doesn't exclude that region at all.
/// `syscall::table::sys_spawn` maps any `ElfError` other than
/// `OutOfMemory` to `-ENOEXEC`.
#[test_case]
fn elf_rejects_segment_inside_stack_region() {
    let mut data = minimal_elf();
    // Comfortably inside the reservation (16 MiB below `USER_STACK_TOP`,
    // `0x0000_7fff_ffff_f000`) and, not incidentally, well above the new,
    // lowered `SEGMENT_HIGH` (`0x0000_7000_0000_0000`) too.
    data[PH + 16..PH + 24].copy_from_slice(&0x0000_7fff_ffff_e000u64.to_le_bytes());
    assert_eq!(load_and_discard(&data).unwrap_err(), ElfError::SegmentOutOfRange);
}

/// Kernel-review M2-T3 fix: `p_vaddr` near `u64::MAX` (with a small
/// `p_memsz`) makes `p_vaddr + p_memsz` succeed via `checked_add` but land
/// too close to `u64::MAX` for rounding up to the next page to succeed
/// without itself overflowing -- `read_phdr` must reject this with a
/// value (`checked_next_multiple_of`), never panic or wrap.
#[test_case]
fn elf_rejects_vaddr_near_u64_max() {
    let mut data = minimal_elf();
    data[PH + 16..PH + 24].copy_from_slice(&(u64::MAX - 100).to_le_bytes()); // p_vaddr
    data[PH + 40..PH + 48].copy_from_slice(&50u64.to_le_bytes()); // p_memsz: sum is still <= u64::MAX
    assert_eq!(load_and_discard(&data).unwrap_err(), ElfError::SegmentOutOfRange);
}

/// Kernel-review M2-T3 fix: `e_phnum` claiming the maximum 65535 program
/// headers on a file barely 4 KiB long must be rejected by the upfront
/// `phoff + phnum * phentsize <= data.len()` check -- *before* `load`
/// ever allocates a `Vec` sized to hold 65535 headers -- not merely
/// eventually, after paying for that allocation regardless.
#[test_case]
fn elf_rejects_huge_phnum_on_tiny_file() {
    let mut data = minimal_elf();
    data[56..58].copy_from_slice(&0xffffu16.to_le_bytes()); // e_phnum = 65535
    assert_eq!(load_and_discard(&data).unwrap_err(), ElfError::TruncatedProgramHeaders);
}

/// The real `/bin/hello` the userspace build produced, straight out of the
/// initramfs, loads cleanly.
#[test_case]
fn elf_accepts_real_hello() {
    let data = fs::initramfs::open("/bin/hello").expect("initramfs should contain /bin/hello");
    let space = AddressSpace::new_user();
    let image = elf::load(&space, data).expect("/bin/hello should be a valid static x86_64 ELF64 executable");
    assert!(image.entry >= 0x400000, "entry point should land inside the user link region");
    space.free_user_space();
    // SAFETY: see `load_and_discard`.
    unsafe { space.destroy() };
}
