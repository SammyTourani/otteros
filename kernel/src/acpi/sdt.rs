//! The common ACPI System Description Table header (ACPI 6.x spec 5.2.6)
//! every table -- the MADT, the HPET, the FADT, ... -- starts with, plus
//! the XSDT/RSDT that list them (5.2.7/5.2.8).

use alloc::vec::Vec;
use core::str;

use super::checksum_is_zero;
use super::rsdp::Rsdp;
use crate::kprintln;
use crate::mm::addr::PhysAddr;
use crate::mm::hhdm;

/// Byte length of the common header every SDT starts with (ACPI 6.x
/// 5.2.6, table 5-29): signature(4) + length(4) + revision(1) +
/// checksum(1) + oem_id(6) + oem_table_id(8) + oem_revision(4) +
/// creator_id(4) + creator_revision(4).
pub const HEADER_LEN: usize = 36;

/// A validated ACPI table header: `base` plus the handful of fields
/// callers need, decoded once by `validate` (below) via unaligned byte
/// reads -- never a direct `*const SdtHeader` cast.
///
/// This matters in practice, not just in principle: real firmware does
/// **not** guarantee tables land on any particular byte boundary. QEMU's
/// legacy BIOS (SeaBIOS) path hands out an ACPI 1.0 (revision 0) RSDP
/// whose RSDT -- and therefore every SDT this module reads through it --
/// can start at an address that isn't even 2-byte aligned (confirmed
/// empirically: `gmake bios-test` hit exactly this and took a genuine
/// "misaligned pointer dereference" panic from an earlier version of
/// this module that dereferenced a `#[repr(C)] struct SdtHeader` -- whose
/// `u32` fields require 4-byte alignment -- directly through a pointer
/// cast). Reading every multi-byte field with `from_le_bytes` over a
/// `&[u8]` slice, as this module does throughout, has no such
/// requirement: a byte slice's element type (`u8`) is always alignment 1,
/// regardless of the base address's own alignment.
pub struct SdtHeader {
    base: *const u8,
    length: u32,
    revision: u8,
    signature: [u8; 4],
}

// SAFETY: `SdtHeader` is a read-only view over ACPI table memory that is
// HHDM-mapped for the kernel's entire lifetime and never written to or
// unmapped again after `validate` builds one -- sharing a `*const u8`
// into it is exactly as safe as sharing a `&'static [u8]` into it would
// be. `Send`/`Sync` aren't derived automatically only because of the raw
// pointer field.
unsafe impl Send for SdtHeader {}
unsafe impl Sync for SdtHeader {}

impl SdtHeader {
    /// The table's 4-character signature (e.g. `"APIC"` for the MADT),
    /// or `"????"` if it somehow isn't valid UTF-8 (ACPI signatures are
    /// always ASCII, so this is purely defensive).
    pub fn signature(&self) -> &str {
        str::from_utf8(&self.signature).unwrap_or("????")
    }

    /// The table's total length in bytes, header included.
    pub fn length(&self) -> u32 {
        self.length
    }

    pub fn revision(&self) -> u8 {
        self.revision
    }

    /// The table-specific bytes after this 36-byte header.
    pub fn body(&self) -> &'static [u8] {
        let body_len = (self.length as usize).saturating_sub(HEADER_LEN);
        // SAFETY: `self` was only ever constructed by `validate` below,
        // which confirms `self.length` bytes starting at `self.base` are
        // valid, checksummed, HHDM-mapped ACPI table memory that lives
        // for the kernel's whole lifetime -- so the `body_len` bytes
        // right after this 36-byte header are in bounds (`body_len <=
        // self.length - HEADER_LEN` by construction) and `'static` too.
        // A byte slice has no alignment requirement on its base pointer.
        unsafe { core::slice::from_raw_parts(self.base.add(HEADER_LEN), body_len) }
    }
}

/// Reads a little-endian `u32` out of `bytes` at `offset`, without
/// requiring `bytes.as_ptr() + offset` to be 4-byte aligned (unlike a
/// direct `*const u32` dereference would).
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// No real ACPI table this kernel parses is anywhere close to this size
/// (the MADT, HPET and FADT are all well under 1 KiB even on a large
/// machine); a `length` field above it is firmware corruption or a
/// malicious/fuzzed value, not a table this code should ever try to
/// checksum or slice (kernel-review, M1-T5 fix #1).
const MAX_TABLE_LENGTH: u64 = 16 * 1024 * 1024;

/// The lowest upper bound this kernel ever guarantees its own HHDM
/// reproduction covers, regardless of how little RAM the memory map
/// describes (`mm::vmm::map_hhdm` unconditionally fills every gap below
/// 4 GiB). `hhdm_bound` uses whichever of this or the memory map's own
/// highest address (`mm::hhdm::phys_end`) is larger.
const MIN_HHDM_GUARANTEED_END: u64 = 4 * 1024 * 1024 * 1024;

/// One past the highest physical address this kernel's own HHDM is
/// guaranteed to reach: the memory map's own highest mentioned address,
/// or 4 GiB, whichever is larger (kernel-review, M1-T5 fix #1). Every
/// table/XSDT-entry address `validate` is ever asked to read through must
/// fall (address *and* address + length) entirely below this, or it's
/// rejected before anything is read.
fn hhdm_bound() -> u64 {
    hhdm::phys_end().max(MIN_HHDM_GUARANTEED_END)
}

/// Whether `[phys, phys + len)` lies entirely below `bound` -- `false`
/// (never a panic) if the addition itself would overflow a `u64`, which a
/// well-formed physical address/length pair can never do.
fn address_range_ok(phys: u64, len: u64, bound: u64) -> bool {
    match phys.checked_add(len) {
        Some(end) => end <= bound,
        None => false,
    }
}

/// Validates and wraps the SDT header at physical address `phys`. `None`
/// if `phys`'s header region (or, once `length` is known, the whole
/// table) falls outside this kernel's HHDM-guaranteed range, `length`
/// exceeds `MAX_TABLE_LENGTH`, the whole table (its full, self-reported
/// `length`, not just the 36-byte common header) doesn't sum to zero, or
/// it claims to be shorter than the header it must at least contain.
/// Every rejection is logged and skipped -- this never reads a single
/// byte outside a range already confirmed safe.
fn validate(phys: PhysAddr) -> Option<&'static SdtHeader> {
    let phys_u64 = phys.as_u64();
    let bound = hhdm_bound();

    // The header region itself must be addressable before anything is
    // read at all -- there is no signature to log yet (reading one would
    // be exactly the unsafe operation this check exists to prevent).
    if !address_range_ok(phys_u64, HEADER_LEN as u64, bound) {
        kprintln!("[acpi] rejected ???? (bad length/address) at 0x{phys_u64:x}");
        return None;
    }

    let base = hhdm::phys_to_virt(phys).as_u64() as *const u8;

    // SAFETY: reads just the 4-byte `length` field (offset 4, inside the
    // fixed 36-byte common header every SDT has) before trusting it for
    // anything else. `address_range_ok` just confirmed `[phys, phys +
    // HEADER_LEN)` lies inside this kernel's HHDM-guaranteed range. A
    // byte slice has no alignment requirement, regardless of how `base`
    // itself lands (see the struct's own docs on why that matters here).
    let header_bytes = unsafe { core::slice::from_raw_parts(base, HEADER_LEN) };
    let length = u64::from(read_u32(header_bytes, 4));

    let mut signature = [0u8; 4];
    signature.copy_from_slice(&header_bytes[0..4]);
    let sig_str = str::from_utf8(&signature).unwrap_or("????");

    if length < HEADER_LEN as u64 || length > MAX_TABLE_LENGTH || !address_range_ok(phys_u64, length, bound) {
        kprintln!("[acpi] rejected {sig_str} (bad length/address)");
        return None;
    }
    let length = length as usize;

    // SAFETY: `address_range_ok` just confirmed `[phys, phys + length)`
    // lies inside this kernel's HHDM-guaranteed range, and `length` is
    // capped at `MAX_TABLE_LENGTH` above.
    let full = unsafe { core::slice::from_raw_parts(base, length) };
    if !checksum_is_zero(full) {
        return None;
    }

    let revision = full[8];

    // `Box::leak`: every SDT this collects lives for the kernel's whole
    // lifetime anyway (identity-mapped ACPI memory, never freed), so
    // leaking the small `SdtHeader` handle itself (this struct's own
    // handful of fields, not the underlying table, which was never
    // heap-allocated to begin with) is exactly "make it live forever",
    // not an actual resource leak -- and it's what turns a value built
    // from a local, checksummed byte read into the `&'static SdtHeader`
    // `collect_headers` promises.
    Some(alloc::boxed::Box::leak(alloc::boxed::Box::new(SdtHeader {
        base,
        length: length as u32,
        revision,
        signature,
    })))
}

/// Walks the XSDT (preferred) or RSDT `rsdp` points at, validating and
/// collecting every SDT header it lists. A table whose checksum doesn't
/// validate is logged and skipped, not fatal -- one malformed table
/// (buggy firmware) shouldn't take the rest of ACPI discovery down with
/// it.
pub fn collect_headers(rsdp: &Rsdp) -> Vec<&'static SdtHeader> {
    let (root_phys, entry_size): (PhysAddr, usize) =
        match rsdp.xsdt_address {
            Some(xsdt) => (xsdt, 8),
            None => (rsdp.rsdt_address, 4),
        };

    let mut headers = Vec::new();
    let root_name = if entry_size == 8 { "XSDT" } else { "RSDT" };
    let Some(root) = validate(root_phys) else {
        kprintln!("[acpi] WARNING: {root_name} at 0x{:x} checksum invalid, no tables discovered", root_phys.as_u64());
        return headers;
    };

    for chunk in root.body().chunks_exact(entry_size) {
        let entry_phys =
            if entry_size == 8 { u64::from_le_bytes(chunk.try_into().unwrap()) } else { u64::from(read_u32(chunk, 0)) };

        match validate(PhysAddr::new(entry_phys)) {
            Some(header) => headers.push(header),
            None => kprintln!("[acpi] WARNING: SDT at 0x{entry_phys:x} failed checksum, skipped"),
        }
    }

    headers
}
