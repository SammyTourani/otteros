//! ustar initramfs (brief M2-T3 step 3): a Limine module named
//! `initramfs` (matched via `module_cmdline`, since revision-0 `Modules`
//! responses carry no other identifying name), parsed once at boot into a
//! static table. Limine hands the whole module over already resident
//! through the same HHDM the kernel's own page tables reproduce
//! (`mm::vmm`'s module docs).
//!
//! STATUS.md's Limine trap ("never declare a second static of an existing
//! Limine request type") doesn't apply here: `ModulesRequest` is a type
//! this crate has never requested before, so the one static below is the
//! first and only one.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use limine::request::ModulesRequest;
use spin::Mutex;

use crate::kprintln;

#[used]
#[unsafe(link_section = ".requests")]
static MODULES_REQUEST: ModulesRequest = ModulesRequest::new();

const BLOCK_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UstarError {
    BadChecksum,
    Truncated,
    /// Kernel-review fix: `prefix`/`name` are raw, attacker-controllable
    /// file data (this archive is trusted today, but parsed as if it
    /// weren't) -- a path that is absolute, or contains a `..` component,
    /// could otherwise be used with a future path-based lookup to escape
    /// wherever a caller expected `open`/`list` results to stay confined.
    /// Rejected at parse time, not at `open` time, so an unsafe path can
    /// never even enter `FILES`.
    UnsafePath,
}

/// `pub`, matching `parse`'s own visibility: `test_cases::initramfs` lives
/// in the *separate* `otteros-kernel-test` binary crate (like every other
/// kernel test), so anything it reaches through `otteros_kernel::` must be
/// genuinely public, not merely `pub(crate)` -- it reads both fields
/// directly to check `parse`'s output against a synthetic archive.
///
/// `data` borrows from whatever byte slice `parse` was given (`'a`) rather
/// than copying it onto the kernel heap: `init` calls `parse(module.data())`
/// with the Limine module's own, already-`'static`-resident memory (`mm::
/// vmm`'s module docs), so every file's content lives there for as long as
/// the kernel runs, at zero heap cost -- kernel-review-worthy, since this
/// archive's own binaries are tens of KiB each, comfortably past the
/// kernel heap's small-object classes: naively `.to_vec()`-ing each one
/// into a permanently-retained `Entry` would have quietly eaten several of
/// the heap's large-object slots for the rest of the kernel's life.
#[derive(Debug)]
pub struct Entry<'a> {
    pub path: String,
    pub data: &'a [u8],
}

/// Every regular file the archive contains, in archive order. Empty (not
/// an error) until `init` runs, or if it found nothing to parse -- callers
/// that need the initramfs check `is_present()` first (`kernel::init`,
/// `main.rs`, `tests.rs`). `'static`: always populated from the Limine
/// module's own permanently-resident memory (see `Entry`'s own docs), never
/// from a temporary buffer.
static FILES: Mutex<Vec<Entry<'static>>> = Mutex::new(Vec::new());
static PRESENT: AtomicBool = AtomicBool::new(false);

/// Whether `init` found and successfully parsed an `initramfs` module.
pub fn is_present() -> bool {
    PRESENT.load(Ordering::Acquire)
}

/// Parses the `initramfs` module Limine handed us, logging `[initramfs]
/// <n> files, <KiB> KiB` (brief M2-T3 step 3). A logged no-op, never a
/// panic, if Limine never answered the modules request, no module's own
/// cmdline is exactly `"initramfs"`, or the archive fails to parse --
/// every one of those just leaves `is_present()` `false`.
///
/// Must run after `mm::vmm::init_kernel_space` (the module's data is only
/// reachable through the kernel's own HHDM once that exists) and after
/// `mm::heap::init` (this allocates); `kernel::init` calls it at exactly
/// that point.
pub fn init() {
    let Some(resp) = MODULES_REQUEST.response() else {
        kprintln!("[initramfs] no modules response from Limine");
        return;
    };
    let Some(module) = resp.modules().iter().find(|m| m.cmdline() == "initramfs") else {
        kprintln!("[initramfs] no module named \"initramfs\"");
        return;
    };

    match parse(module.data()) {
        Ok(entries) => {
            let total_bytes: usize = entries.iter().map(|e| e.data.len()).sum();
            kprintln!("[initramfs] {} files, {} KiB", entries.len(), total_bytes / 1024);
            *FILES.lock() = entries;
            PRESENT.store(true, Ordering::Release);
        }
        Err(e) => kprintln!("[initramfs] FATAL: failed to parse module: {e:?}"),
    }
}

/// `path`'s contents (leading `/` optional -- archive entries are stored
/// without one), or `None` if it isn't a regular file this archive
/// contains. Borrowed straight out of the Limine module's own memory (see
/// `Entry`'s docs) -- no copy.
pub fn open(path: &str) -> Option<&'static [u8]> {
    let path = path.trim_start_matches('/');
    FILES.lock().iter().find(|e| e.path == path).map(|e| e.data)
}

/// Every regular file's path, in archive order (test/introspection use).
pub fn list() -> Vec<String> {
    FILES.lock().iter().map(|e| e.path.clone()).collect()
}

/// Parses a ustar byte stream, borrowing every file's content directly out
/// of `data` (see `Entry`'s docs). `pub` (not just an internal helper) so
/// `test_cases::initramfs` -- a separate binary crate, like every other
/// kernel test -- can feed it synthetic archives directly, without needing
/// a real Limine module.
pub fn parse(data: &[u8]) -> Result<Vec<Entry<'_>>, UstarError> {
    let mut entries = Vec::new();
    let mut offset = 0usize;

    while offset + BLOCK_SIZE <= data.len() {
        let header = &data[offset..offset + BLOCK_SIZE];
        if header.iter().all(|&b| b == 0) {
            break; // The end-of-archive marker (two, conventionally) starts here.
        }

        let stored_checksum = parse_octal(&header[148..156]).ok_or(UstarError::BadChecksum)?;
        if checksum(header) != stored_checksum {
            return Err(UstarError::BadChecksum);
        }

        let name = cstr(&header[0..100]);
        let prefix = cstr(&header[345..500]);
        let typeflag = header[156];
        let size = parse_octal(&header[124..136]).ok_or(UstarError::Truncated)? as usize;

        let content_start = offset + BLOCK_SIZE;
        let content_end = content_start.checked_add(size).ok_or(UstarError::Truncated)?;
        if content_end > data.len() {
            return Err(UstarError::Truncated);
        }

        // '0' is POSIX ustar's "regular file"; '\0' is the older pre-
        // POSIX convention some writers still emit. Brief step 3:
        // "regular files and directories only" -- directories ('5') carry
        // no content and are simply not files `open` can ever return, so
        // there is nothing further to do for one beyond skipping it here.
        if typeflag == b'0' || typeflag == 0 {
            let path = if prefix.is_empty() { String::from(name) } else { alloc::format!("{prefix}/{name}") };
            if !is_safe_path(&path) {
                return Err(UstarError::UnsafePath);
            }
            entries.push(Entry { path, data: &data[content_start..content_end] });
        }

        let padded_size = size.next_multiple_of(BLOCK_SIZE);
        offset = content_start.checked_add(padded_size).ok_or(UstarError::Truncated)?;
    }

    Ok(entries)
}

/// The checksum ustar's own header field describes: the unsigned sum of
/// every byte in the 512-byte header, treating the 8-byte checksum field
/// itself as if it were all ASCII spaces (POSIX 1003.1-2001).
fn checksum(header: &[u8]) -> u64 {
    header.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { u64::from(b' ') } else { u64::from(b) }).sum()
}

/// Parses a ustar numeric field: ASCII octal digits, optionally
/// space-padded on the left and NUL/space-terminated on the right (POSIX
/// allows either terminator). `None` only for a genuinely non-octal byte
/// before any digit has been seen -- an all-zero/empty field parses as
/// `Some(0)`, which is a legitimate size.
fn parse_octal(field: &[u8]) -> Option<u64> {
    let mut value = 0u64;
    let mut seen_digit = false;
    for &b in field {
        match b {
            b'0'..=b'7' => {
                value = value.checked_mul(8)?.checked_add(u64::from(b - b'0'))?;
                seen_digit = true;
            }
            0 | b' ' if !seen_digit => {} // leading padding.
            0 | b' ' => break,            // trailing terminator.
            _ => return None,
        }
    }
    Some(value)
}

/// Whether `path` (the already-joined `prefix`/`name`) is safe to store
/// and later look up by: not absolute, and containing no `..` path
/// component anywhere (kernel-review fix). Checked as real path
/// components split on `/`, not a bare substring search, so an entirely
/// ordinary filename like `a..b` (no `/` anywhere near it) is never
/// rejected -- only a genuine `..` segment is.
fn is_safe_path(path: &str) -> bool {
    !path.starts_with('/') && !path.split('/').any(|component| component == "..")
}

/// A NUL-terminated (or, if never NUL, whole-field) byte range as `&str`,
/// or `""` if it isn't valid UTF-8.
fn cstr(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..end]).unwrap_or("")
}
