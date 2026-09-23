//! Tearing down a process's address space (brief M2-T2): freeing every
//! frame `proc::process::Process::create` (and later `map_anon`/stack
//! growth) ever mapped into the *user* half, and finally the PML4 itself.
//! Split out of `vmm`'s main module purely to keep that file under the
//! ~600-line guideline -- `AddressSpace`'s own type and every other method
//! still live there; this is simply a second `impl` block for it.

use super::AddressSpace;
use crate::mm::paging;
use crate::mm::pmm;

impl AddressSpace {
    /// Frees every mapped frame in this address space's *user* half (PML4
    /// entries 0..256, DECISIONS.md D15): both leaf pages (4 KiB or 2 MiB)
    /// and the intermediate PDPT/PD/PT frames themselves. Brief M2-T2's
    /// process teardown: unlike the kernel's own tables (STATUS.md's
    /// accepted "intermediate tables are never freed" TODO, and
    /// `paging::next_table_or_create`'s identical one), a *user* address
    /// space is fully and permanently torn down at process exit/kill, so
    /// nothing can still be relying on any of it staying mapped -- freeing
    /// the intermediate tables too, not just the leaves, is safe here in a
    /// way it isn't for the long-lived, ever-growing kernel space.
    ///
    /// Leaves every PML4 entry below 256 cleared (not present) but the
    /// PML4 frame itself untouched and still valid -- CR3 may still point
    /// at it (this runs from the exiting thread's own syscall/fault
    /// handler, still executing under this exact address space); see
    /// `destroy` for freeing that too, once nothing ever will again.
    /// Idempotent: calling this twice (`proc::kill` on an already-exited
    /// process) finds every entry already cleared and does nothing.
    pub fn free_user_space(&self) {
        let pml4 = self.pml4_table();
        for pml4_idx in 0..256 {
            let pml4_entry = pml4.entry_mut(pml4_idx);
            if !pml4_entry.is_present() {
                continue;
            }
            // SAFETY: every present PML4 entry below 256 was created by
            // `vmm`'s own `map_4k`/`map_2m` (via `next_table_or_create`),
            // so it points at a real, exclusively-owned PDPT frame; PML4
            // entries are never `HUGE` (Intel SDM Vol. 3A 4.5: there is no
            // such bit at this level).
            let pdpt = unsafe { paging::next_table(*pml4_entry).expect("just confirmed present") };
            for pdpt_idx in 0..paging::ENTRIES_PER_TABLE {
                let pdpt_entry = pdpt.entry_mut(pdpt_idx);
                if !pdpt_entry.is_present() {
                    continue;
                }
                debug_assert!(!pdpt_entry.is_huge(), "vmm::free_user_space: unexpected 1 GiB page (never created by this kernel)");
                // SAFETY: same reasoning as the PDPT lookup above, one
                // level down.
                let pd = unsafe { paging::next_table(*pdpt_entry).expect("just confirmed present") };
                for pd_idx in 0..paging::ENTRIES_PER_TABLE {
                    let pd_entry = pd.entry_mut(pd_idx);
                    if !pd_entry.is_present() {
                        continue;
                    }
                    if pd_entry.is_huge() {
                        pmm::free_frame(pd_entry.addr());
                        pd_entry.clear();
                        continue;
                    }
                    // SAFETY: `pd_entry` was just confirmed not huge; see
                    // `map_4k`.
                    let pt = unsafe { paging::next_table(*pd_entry).expect("just confirmed present") };
                    for pt_idx in 0..paging::ENTRIES_PER_TABLE {
                        let pt_entry = pt.entry_mut(pt_idx);
                        if pt_entry.is_present() {
                            pmm::free_frame(pt_entry.addr());
                            pt_entry.clear();
                        }
                    }
                    let pt_phys = pd_entry.addr();
                    pd_entry.clear();
                    pmm::free_frame(pt_phys);
                }
                let pd_phys = pdpt_entry.addr();
                pdpt_entry.clear();
                pmm::free_frame(pd_phys);
            }
            let pdpt_phys = pml4_entry.addr();
            pml4_entry.clear();
            pmm::free_frame(pdpt_phys);
        }
        // No `invlpg`/full flush here: every frame just freed belonged to
        // *this* address space's user half, and this call only ever runs
        // once nothing will ever again execute or dereference a pointer
        // into it (the owning process is exiting/killed) -- `activate()`,
        // the next time any CR3 reload happens at all, already flushes
        // every non-global TLB entry outright.
    }

    /// Frees this address space's own PML4 frame -- the one thing
    /// `free_user_space` deliberately leaves alone. `proc::wait` calls
    /// this once a zombie process is finally collected, well after its
    /// thread has been reaped and CR3 has long since moved on.
    ///
    /// # Safety
    /// No CPU may have this address space loaded in CR3, ever again, from
    /// this point on. `free_user_space` should already have run (or there
    /// was never anything mapped in the user half to begin with) -- this
    /// only frees the PML4 frame itself, so any *still-present* user
    /// mapping under it would otherwise leak.
    pub unsafe fn destroy(self) {
        pmm::free_frame(self.pml4());
    }
}
