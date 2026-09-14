//! ACPI table discovery (brief M1-T5): the RSDP Limine hands us, then the
//! (X)SDT and every table it lists, far enough to find the MADT (local/IO
//! APIC layout) and the HPET's base address. DECISIONS.md D2: no `acpi`
//! crate -- every byte here is parsed by hand against the ACPI 6.x spec
//! this module's doc comments cite.

pub mod fadt;
pub mod hpet;
pub mod madt;
pub mod rsdp;
pub mod sdt;

use limine::request::RsdpRequest;

use crate::mm::addr::PhysAddr;
use crate::sync::IrqMutex;
use crate::{kprintln, qemu};

#[used]
#[unsafe(link_section = ".requests")]
static RSDP_REQUEST: RsdpRequest = RsdpRequest::new();

/// Everything later subsystems need out of the ACPI tables, gathered once
/// at boot by `init` and never mutated again.
pub struct AcpiInfo {
    pub rsdp_phys: PhysAddr,
    pub rsdp_revision: u8,
    /// How many SDTs (of any signature) the (X)RSDT listed with a valid
    /// checksum -- always includes the MADT itself, so this is never 0
    /// once `init` has returned successfully.
    pub sdt_count: usize,
    pub madt: madt::Madt,
    pub hpet_address: Option<PhysAddr>,
    /// FADT ("FACP") revision, kept for a later power-off task; `None` if
    /// no FADT was present (never expected on a real machine, but nothing
    /// here depends on it yet).
    pub facp_revision: Option<u8>,
    /// The FADT's IA-PC Boot Architecture Flags (brief M1-T6,
    /// `drivers::ps2`), or `None` if no FADT was present, its revision
    /// predates that field, or the table was too short -- see
    /// `fadt::boot_arch_flags`'s docs for what each of those means to a
    /// caller.
    pub iapc_boot_arch_flags: Option<u16>,
}

static ACPI_INFO: IrqMutex<Option<AcpiInfo>> = IrqMutex::new(None);

/// Sums `bytes` mod 256: zero iff the structure's own checksum byte was
/// chosen to make it so (ACPI 6.x 5.2.5.3, and every SDT after it via
/// 5.2.6). Shared by `rsdp` and `sdt`; `pub` (not `pub(crate)`) so the
/// `otteros-kernel-test` binary -- a separate crate -- can exercise the
/// arithmetic directly against synthetic bytes (`test_cases::acpi`),
/// independent of whatever real ACPI tables QEMU happens to provide.
pub fn checksum_is_zero(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b)) == 0
}

/// Discovers and parses the ACPI tables (brief M1-T5 step 2): validates
/// the RSDP, walks the XSDT (falling back to the RSDT), validates and
/// logs every SDT header it finds, then parses the MADT and (if present)
/// the HPET and FADT. Must run after `mm::vmm::init_kernel_space` --
/// every table is reached through the HHDM, which only covers ACPI's
/// physical addresses (reclaimable/NVS/reserved memory included) once the
/// kernel's own page tables are active (see `mm::vmm::map_hhdm`).
///
/// Fatal (`qemu::exit(false)`, matching every other "Limine/firmware
/// didn't give us what we need" path in this kernel -- `mm::init`,
/// `mm::vmm::map_kernel_image`) if Limine never answered the RSDP
/// request, the RSDP's checksum doesn't validate, or no MADT ("APIC")
/// table is present: nothing past this point (the LAPIC, the I/O APIC,
/// the scheduler tick) can exist without one.
pub fn init() {
    let response = RSDP_REQUEST.response().unwrap_or_else(|| {
        kprintln!("[acpi] FATAL: no RSDP response from Limine");
        qemu::exit(false);
    });
    // WARNING (the `limine` crate's own docs on `RsdpRequest`): the RSDP
    // address is only a *physical* address for base revision 3 -- earlier
    // revisions hand back a virtual one instead. DECISIONS.md D3 pins
    // this kernel to base revision 3 (`lib.rs`'s `BASE_REVISION`), so it
    // always is here.
    let rsdp_phys = PhysAddr::new(response.address as u64);

    let parsed = rsdp::Rsdp::parse(rsdp_phys).unwrap_or_else(|| {
        kprintln!("[acpi] FATAL: RSDP checksum invalid at 0x{:x}", rsdp_phys.as_u64());
        qemu::exit(false);
    });
    kprintln!(
        "[acpi] RSDP revision {} at 0x{:x} ({})",
        parsed.revision,
        rsdp_phys.as_u64(),
        if parsed.xsdt_address.is_some() { "XSDT" } else { "RSDT" }
    );

    let headers = sdt::collect_headers(&parsed);

    let mut madt_result = None;
    let mut hpet_address = None;
    let mut facp_revision = None;
    let mut iapc_boot_arch_flags = None;
    for header in &headers {
        kprintln!("[acpi] {} len={} rev={}", header.signature(), header.length(), header.revision());
        match header.signature() {
            "APIC" => madt_result = madt::parse(header),
            "HPET" => hpet_address = hpet::parse(header),
            "FACP" => {
                facp_revision = Some(header.revision());
                iapc_boot_arch_flags = fadt::boot_arch_flags(header);
            }
            _ => {}
        }
    }

    let madt = madt_result.unwrap_or_else(|| {
        kprintln!("[acpi] FATAL: no MADT (APIC) table present");
        qemu::exit(false);
    });

    kprintln!(
        "[acpi] MADT: {} cpus, ioapic at 0x{:x}, {} overrides",
        madt.cpus.iter().filter(|c| c.enabled).count(),
        madt.ioapics.first().map(|i| i.address.as_u64()).unwrap_or(0),
        madt.overrides.len()
    );

    *ACPI_INFO.lock() = Some(AcpiInfo {
        rsdp_phys,
        rsdp_revision: parsed.revision,
        sdt_count: headers.len(),
        madt,
        hpet_address,
        facp_revision,
        iapc_boot_arch_flags,
    });
}

/// Runs `f` against the parsed ACPI info.
///
/// # Panics
/// If `init` hasn't run yet.
pub fn with_info<R>(f: impl FnOnce(&AcpiInfo) -> R) -> R {
    let guard = ACPI_INFO.lock();
    f(guard.as_ref().expect("acpi::init was never called"))
}
