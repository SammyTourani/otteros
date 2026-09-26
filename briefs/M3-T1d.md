# Brief M3-T1d — PCI configuration access without ECAM (port I/O fallback) and `gmake pc-test`

## Goal
Close PLAN M3's PCI box: configuration space through ECAM when ACPI has an MCFG table, else
through the legacy mechanism #1 (ports 0xCF8/0xCFC), with every PCI user (enumeration,
capabilities, BAR probing, the virtio transports) going through one accessor. Prove it on `-M pc`
(i440FX, no MCFG) with a new gate target. Acceptance tests: `kernel/src/test_cases/pci_access.rs`
(orchestrator, unchanged; its header is the API) plus the existing virtio-blk and net tests.

## Applies
CLAUDE.md conventions (`// SAFETY:` on every `unsafe`, zero warnings, `[pci]` log prefix). Never
run git checkout/restore/reset/stash/clean. Do not edit `pci_access.rs` or any other existing test.

## Design
1. `drivers::pci`: `enum AccessMethod { Ecam, PortIo }` (Debug, Clone, Copy, PartialEq, Eq), chosen
   once in `init()` and logged: `[pci] config access: ecam (MCFG)` or
   `[pci] config access: port-io (no MCFG)`. `config_read{8,16,32}` / `config_write{8,16,32}(bdf,
   offset)` dispatch on it; `port_io_read32(bdf, offset: u8)` always uses 0xCF8/0xCFC (address =
   0x8000_0000 | bus << 16 | device << 11 | function << 8 | (offset & 0xFC); narrower accesses read
   the dword and shift; writes of 8/16 bits read-modify-write only where the register allows —
   prefer 32-bit accesses in callers). Port I/O is serialised by a lock with IRQs disabled (the
   address/data pair is not atomic). Offsets >= 256 under port I/O read 0xFFFF_FFFF and ignore
   writes.
2. Replace every direct ECAM use (`config.rs`, `mod.rs` enumeration, capability walk, BAR sizing,
   `drivers/virtio/pci.rs`) with the accessor; ECAM mapping stays for the Ecam case.
   `acpi::has_mcfg() -> bool`.
3. Test runner filter: when the kernel cmdline contains `testfilter=a,b,c`, run only tests whose
   full name contains one of the substrings (log `[test] filter: a,b,c`); the `[ok] tests passed (N)`
   line counts the tests that ran.
4. `gmake pc-test`: a test ISO whose Limine cmdline is `test testfilter=pci_access,virtio_blk,net`
   (same mechanism as the other `build/limine-test-*.conf` files), booted with a new
   `scripts/qemu.py --machine pc` option (default stays q35) with the same disks and NIC, expecting
   success and `--expect-serial 'config access: port-io'`. Add `pc-test` to `scripts/check.sh`
   (the gate then has 14 targets) and to CLAUDE.md's command list.

## Acceptance
- `source scripts/env.sh && gmake check` → 14/14 PASS (the four pci_access tests pass on q35 in
  `test`/`bios-test` and on i440FX in `pc-test`, together with the virtio_blk and net tests).
- `grep -rn "ecam::" kernel/src --include=*.rs | grep -v drivers/pci/` → nothing (one accessor).

## Report
<=15 lines: files, the [check] table, the `[pci] config access` lines of both machines, deviations.
Do not commit.
