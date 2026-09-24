//! ustar parser tests (brief M2-T3 step 8): synthetic single-file archives
//! (this kernel's own `fs::initramfs::parse` both writes and reads these
//! test fixtures, so a uniform "N-1 octal digits + NUL" numeric-field
//! encoding is exactly as valid as any other POSIX-permitted variant --
//! see `write_octal`) exercising checksum validation and the `prefix`
//! field POSIX ustar uses for names too long for the 100-byte `name`
//! field alone.

use alloc::vec::Vec;

use otteros_kernel::fs::initramfs::{self, UstarError};

const BLOCK_SIZE: usize = 512;

/// Writes `value` as zero-padded octal digits filling every byte of
/// `field` except the last, which is always NUL-terminated -- POSIX allows
/// this (as well as a trailing space instead of the second NUL, which real
/// tar writers use for the checksum field specifically); `fs::initramfs`'s
/// own `parse_octal` accepts either, so picking one uniform encoding here
/// for every numeric field keeps this test fixture builder simple.
fn write_octal(field: &mut [u8], value: u64) {
    let width = field.len() - 1;
    let digits = alloc::format!("{value:0width$o}");
    field[..width].copy_from_slice(digits.as_bytes());
    field[width] = 0;
}

/// The same checksum `fs::initramfs::parse` itself computes (POSIX
/// 1003.1-2001: sum every byte, treating the checksum field as all
/// spaces) -- reimplemented here since that helper is a private
/// implementation detail, not part of the crate's public surface.
fn checksum(header: &[u8; BLOCK_SIZE]) -> u64 {
    header.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { u64::from(b' ') } else { u64::from(b) }).sum()
}

/// Builds one complete ustar record (a 512-byte header plus `content`,
/// padded to the next 512-byte boundary) for a single regular file named
/// `prefix`/`name` (or just `name`, if `prefix` is empty), with a correct
/// checksum.
fn build_archive(prefix: &str, name: &str, content: &[u8]) -> Vec<u8> {
    let mut header = [0u8; BLOCK_SIZE];
    header[0..name.len()].copy_from_slice(name.as_bytes());
    write_octal(&mut header[124..136], content.len() as u64); // size
    header[156] = b'0'; // typeflag: regular file
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let sum = checksum(&header); // must be last: covers every other field.
    write_octal(&mut header[148..156], sum);

    let mut archive = header.to_vec();
    archive.extend_from_slice(content);
    let padding = content.len().next_multiple_of(BLOCK_SIZE) - content.len();
    archive.extend(core::iter::repeat_n(0u8, padding));
    archive
}

/// A corrupted checksum is rejected outright, never silently accepted with
/// whatever garbage the size/name fields happen to hold.
#[test_case]
fn ustar_checksum_failure_is_rejected() {
    let mut archive = build_archive("", "hello.txt", b"hi");
    archive[10] ^= 0xff; // flip a byte inside `name`, invalidating the checksum computed over it.
    assert_eq!(initramfs::parse(&archive).unwrap_err(), UstarError::BadChecksum);
}

/// A name longer than the 100-byte `name` field alone splits across
/// `prefix` (POSIX ustar): `fs::initramfs::parse` must join them back with
/// a `/`, and return the right content alongside it.
#[test_case]
fn ustar_prefix_field_is_joined_with_name() {
    let archive = build_archive("a/long/directory/path", "file.bin", b"payload");
    let entries = initramfs::parse(&archive).expect("a correctly checksummed archive should parse");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "a/long/directory/path/file.bin");
    assert_eq!(entries[0].data, b"payload");
}

/// A directory entry (typeflag `'5'`) contributes no `open`-able file --
/// brief step 3: "regular files and directories only" means directories
/// are recognised (not rejected outright) but never returned as content.
#[test_case]
fn ustar_directory_entries_are_skipped() {
    let mut archive = build_archive("", "somedir", b"");
    archive[156] = b'5'; // typeflag: directory
    let mut header = [0u8; BLOCK_SIZE];
    header.copy_from_slice(&archive[..BLOCK_SIZE]);
    write_octal(&mut archive[148..156], checksum(&header)); // recompute: typeflag changed above.

    let entries = initramfs::parse(&archive).expect("a directory-only archive should still parse");
    assert_eq!(entries.len(), 0, "a directory entry should never appear as an openable file");
}

/// A trailing NUL name for `path.contains('/')`-free convenience: not part
/// of the brief's own list, but a cheap extra confirming `String::from`
/// (the `prefix.is_empty()` branch) round-trips a plain short name too.
#[test_case]
fn ustar_plain_short_name_has_no_prefix() {
    let archive = build_archive("", "init", b"\x7fELF");
    let entries = initramfs::parse(&archive).expect("should parse");
    assert_eq!(entries[0].path, "init");
}

/// Kernel-review fix: a `..` path component -- whether it comes from
/// `name` alone or only appears after joining `prefix` and `name` -- is
/// rejected at parse time, never stored where a later `open` lookup could
/// be tricked into walking outside the archive's own intended tree.
#[test_case]
fn ustar_dotdot_in_name_is_rejected() {
    let archive = build_archive("", "../etc/passwd", b"evil");
    assert_eq!(initramfs::parse(&archive).unwrap_err(), UstarError::UnsafePath);
}

#[test_case]
fn ustar_dotdot_split_across_prefix_and_name_is_rejected() {
    // Neither `prefix` ("a") nor `name` ("..") alone looks unsafe -- only
    // the *joined* path ("a/..") reveals the traversal component, which is
    // exactly why the check has to run after joining, not on each half.
    let archive = build_archive("a", "..", b"evil");
    assert_eq!(initramfs::parse(&archive).unwrap_err(), UstarError::UnsafePath);
}

/// An absolute path (a leading `/` once `prefix`/`name` are joined) is
/// rejected the same way -- ustar entries are always relative to the
/// archive's own root.
#[test_case]
fn ustar_absolute_path_is_rejected() {
    let archive = build_archive("", "/etc/passwd", b"evil");
    assert_eq!(initramfs::parse(&archive).unwrap_err(), UstarError::UnsafePath);
}
