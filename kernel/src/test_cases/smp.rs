//! Multicore bring-up tests for brief M6-T1, written by the orchestrator: the implementation must
//! pass these unchanged. The gate boots `-smp 4`. Application processors come up through the
//! Limine MP request (D25), each with its own GDT/TSS, per-CPU area (GS base) and LAPIC timer, and
//! wait in an idle loop that services cross-calls; scheduling threads on them is brief M6-T2.
//!
//! API: `smp::cpu_count() -> usize` (CPUs the firmware reported), `smp::online_count() -> usize`,
//! `smp::cpus() -> Vec<CpuInfo { index: usize, lapic_id: u32, is_bsp: bool }>`,
//! `smp::current_cpu() -> usize` (index of the calling CPU, from its per-CPU area),
//! `smp::run_on(index, f: fn(usize) -> usize, arg: usize) -> usize` (runs `f(arg)` on that CPU in
//! interrupt context through an IPI and returns its result; on the calling CPU it just calls `f`),
//! `smp::ticks_on(index) -> u64` (that CPU's own LAPIC timer interrupt count),
//! `arch::x86_64::lapic::id() -> u32` (the executing CPU's LAPIC ID).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use otteros_kernel::arch::x86_64::lapic;
use otteros_kernel::smp;
use otteros_kernel::time;

#[test_case]
fn smp_all_cpus_are_online() {
    assert_eq!(smp::cpu_count(), 4, "the gate boots -smp 4");
    assert_eq!(smp::online_count(), 4);
    let cpus = smp::cpus();
    let mut ids: Vec<u32> = cpus.iter().map(|c| c.lapic_id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 4, "distinct LAPIC IDs");
    assert_eq!(cpus.iter().filter(|c| c.is_bsp).count(), 1);
    let indexes: Vec<usize> = cpus.iter().map(|c| c.index).collect();
    assert_eq!(indexes, [0, 1, 2, 3], "indexes are dense and the BSP is index 0");
    assert!(cpus[0].is_bsp);
    assert_eq!(smp::current_cpu(), 0, "tests run on the BSP");
}

fn whoami(_arg: usize) -> usize {
    (smp::current_cpu() << 32) | lapic::id() as usize
}

#[test_case]
fn smp_each_cpu_has_its_own_per_cpu_area() {
    for cpu in smp::cpus() {
        let answer = smp::run_on(cpu.index, whoami, 0);
        assert_eq!(answer >> 32, cpu.index, "the per-CPU area on CPU {} says it is CPU {}", cpu.index, answer >> 32);
        assert_eq!((answer & 0xFFFF_FFFF) as u32, cpu.lapic_id, "and its LAPIC agrees");
    }
}

static HITS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

fn hit(arg: usize) -> usize {
    HITS[smp::current_cpu()].fetch_add(1, Ordering::SeqCst);
    arg.wrapping_mul(3)
}

#[test_case]
fn smp_cross_calls_reach_every_cpu_and_return() {
    for h in &HITS {
        h.store(0, Ordering::SeqCst);
    }
    for i in 0..1000usize {
        let cpu = i % 4;
        assert_eq!(smp::run_on(cpu, hit, i), i * 3, "call {i} on CPU {cpu}");
    }
    let counts: Vec<usize> = HITS.iter().map(|h| h.load(Ordering::SeqCst)).collect();
    assert_eq!(counts, [250, 250, 250, 250], "each CPU ran its own calls, exactly once each");
}

#[test_case]
fn smp_every_cpu_has_a_running_timer() {
    let before: Vec<u64> = (0..4).map(smp::ticks_on).collect();
    time::sleep_ms(200);
    let after: Vec<u64> = (0..4).map(smp::ticks_on).collect();
    for cpu in 0..4 {
        assert!(after[cpu] > before[cpu] + 20, "CPU {cpu}'s LAPIC timer ticked {} times in 200 ms", after[cpu] - before[cpu]);
    }
}
