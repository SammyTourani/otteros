//! The kernel's own GDT and TSS (Limine's are only valid until this runs).
//!
//! Selector layout (fixed on purpose -- see module docs on `USER_DATA_SELECTOR`
//! for why the user descriptors sit exactly where they do):
//! null (0x00), kernel code (0x08), kernel data (0x10), user data (0x18,
//! DPL3), user code (0x20, DPL3), TSS (0x28, a 16-byte system descriptor).

use core::cell::UnsafeCell;
use core::mem::size_of;

use crate::kprintln;

pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
/// Sits at `KERNEL_DATA_SELECTOR + 8` and `USER_CODE_SELECTOR` at
/// `KERNEL_DATA_SELECTOR + 16` on purpose: the `syscall`/`sysret`
/// instructions (wired up once userspace exists) derive both user
/// selectors from a single GDT "base" value in the `STAR` MSR this way, so
/// the order of these four descriptors isn't just cosmetic.
pub const USER_DATA_SELECTOR: u16 = 0x18;
pub const USER_CODE_SELECTOR: u16 = 0x20;
pub const TSS_SELECTOR: u16 = 0x28;

const KERNEL_STACK_SIZE: usize = 64 * 1024;
const IST_STACK_SIZE: usize = 32 * 1024;

/// A statically-allocated, 16-byte-aligned stack. Only ever read from Rust
/// for its address (the top, for `RSPn`/`ISTn`, or the base, for the
/// `tss_ist1_valid` test's bounds check) -- the CPU writes into it directly
/// once loaded into the TSS.
///
/// The array is wrapped in `UnsafeCell` on purpose: a plain `static FOO:
/// [u8; N]` that Rust code never writes through a `&mut` is exactly the
/// kind of "provably never mutated in safe Rust" data LLVM is free to fold
/// into a read-only section -- and did, silently, the first time this was
/// a bare array (`nm` showed it with symbol type `r`; the CPU then took a
/// write-protection page fault the first time it tried to push an
/// exception frame onto it). `UnsafeCell` is the documented signal that
/// tells the compiler "this may be mutated through a raw pointer", which
/// keeps it in writable `.bss` regardless of the fact that no *Rust*
/// reference ever mutates it either.
#[repr(C, align(16))]
struct Stack<const N: usize>(UnsafeCell<[u8; N]>);

// SAFETY: a `static` needs `Sync`, and `UnsafeCell` opts out of it by
// default; the only "writer" is the CPU itself pushing an exception frame
// (never Rust code), and that only ever happens from single-threaded,
// interrupts-off kernel code, so there is no data race for `Sync` to guard
// against here.
unsafe impl<const N: usize> Sync for Stack<N> {}

impl<const N: usize> Stack<N> {
    fn base(&self) -> u64 {
        self.0.get() as u64
    }

    fn top(&self) -> u64 {
        self.base() + N as u64
    }
}

static KERNEL_STACK: Stack<KERNEL_STACK_SIZE> = Stack(UnsafeCell::new([0; KERNEL_STACK_SIZE]));
static DF_STACK: Stack<IST_STACK_SIZE> = Stack(UnsafeCell::new([0; IST_STACK_SIZE]));
static NMI_STACK: Stack<IST_STACK_SIZE> = Stack(UnsafeCell::new([0; IST_STACK_SIZE]));

/// The x86_64 Task State Segment (Intel SDM Vol. 3A, Figure 8-11). We only
/// use it for `RSP0` (ring 3 -> ring 0 transitions, from M2 onward) and the
/// IST stacks; there is no hardware task-switching and no I/O permission
/// bitmap (`iomap_base` points past the end of the segment limit).
///
/// **Never take `&`/`&mut` to any field of this struct.** It's
/// `repr(C, packed)` because the hardware layout is not naturally aligned
/// (`rsp0` sits at byte offset 4, `ist1` at offset 36, ...); `rustc` denies
/// forming a reference to most of these fields outright (E0793: "reference
/// to packed field is unaligned"), and for the couple it wouldn't catch, a
/// misaligned reference is UB even if never dereferenced. Reading a field
/// by *copying* it out (`let x = tss.rsp0;`) is fine -- only references are
/// the problem -- but writing goes through `set_rsp0`/`set_ist` below,
/// which use `write_unaligned` explicitly so the hazard can't be
/// reintroduced by a future refactor that casually writes `tss.rsp0 = x`
/// again from somewhere a reference is easy to accidentally take instead.
#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist1: u64,
    ist2: u64,
    ist3: u64,
    ist4: u64,
    ist5: u64,
    ist6: u64,
    ist7: u64,
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

impl Tss {
    /// Sets `RSP0` (the ring 0 stack pointer loaded on a ring 3 -> ring 0
    /// transition) via an explicit unaligned write.
    fn set_rsp0(&mut self, value: u64) {
        // SAFETY: `core::ptr::addr_of_mut!` forms a raw pointer without
        // ever creating an intermediate `&mut` to the misaligned field;
        // `self` is a valid, live `&mut Tss`, so that raw pointer is valid
        // for a `write_unaligned`.
        unsafe { core::ptr::addr_of_mut!(self.rsp0).write_unaligned(value) };
    }

    /// Sets `ISTn` (`n` in `1..=7`) via an explicit unaligned write.
    ///
    /// # Panics
    /// If `n` is not in `1..=7` -- a programmer error (there is no IST0),
    /// never a runtime/input condition.
    fn set_ist(&mut self, n: u8, value: u64) {
        // `addr_of_mut!` only forms a raw pointer (never an intermediate
        // `&mut` to the misaligned field), so no `unsafe` is needed yet.
        let field = match n {
            1 => core::ptr::addr_of_mut!(self.ist1),
            2 => core::ptr::addr_of_mut!(self.ist2),
            3 => core::ptr::addr_of_mut!(self.ist3),
            4 => core::ptr::addr_of_mut!(self.ist4),
            5 => core::ptr::addr_of_mut!(self.ist5),
            6 => core::ptr::addr_of_mut!(self.ist6),
            7 => core::ptr::addr_of_mut!(self.ist7),
            _ => panic!("invalid IST index {n} (must be 1..=7)"),
        };
        // SAFETY: `field` was just derived from `self`, a valid, live
        // `&mut Tss`, so it's valid for a `write_unaligned` even though
        // it's misaligned.
        unsafe { field.write_unaligned(value) };
    }
}

const TSS_ZERO: Tss = Tss {
    reserved0: 0,
    rsp0: 0,
    rsp1: 0,
    rsp2: 0,
    reserved1: 0,
    ist1: 0,
    ist2: 0,
    ist3: 0,
    ist4: 0,
    ist5: 0,
    ist6: 0,
    ist7: 0,
    reserved2: 0,
    reserved3: 0,
    iomap_base: 0,
};

static mut TSS: Tss = TSS_ZERO;

#[repr(C, packed)]
struct DtPointer {
    limit: u16,
    base: u64,
}

/// Builds a flat (base 0, limit 0xFFFFF) code/data segment descriptor.
/// Long mode ignores base/limit for such segments once `L=1`/`DPL` are set
/// correctly, but filling in the conventional "flat" values keeps this
/// readable next to any reference GDT dump.
const fn flat_descriptor(access: u8, flags: u8) -> u64 {
    let limit: u64 = 0xFFFF;
    limit | ((access as u64) << 40) | (0xF << 48) | ((flags as u64) << 52)
}

/// Access byte bits: P(7) DPL(6:5) S(4) Type(3:0). Code type 0b1010
/// (execute/read), data type 0b0010 (read/write).
const KERNEL_CODE: u64 = flat_descriptor(0x9A, 0xA); // P DPL0 S=1 code; G=1 L=1
const KERNEL_DATA: u64 = flat_descriptor(0x92, 0xC); // P DPL0 S=1 data; G=1 D=1
const USER_DATA: u64 = flat_descriptor(0xF2, 0xC); // P DPL3 S=1 data; G=1 D=1
const USER_CODE: u64 = flat_descriptor(0xFA, 0xA); // P DPL3 S=1 code; G=1 L=1

/// 7 `u64`s: null, kernel code/data, user data/code, and the 16-byte TSS
/// descriptor spanning the last two slots (filled in by `init()`, since it
/// embeds the TSS's runtime address).
static mut GDT: [u64; 7] = [0, KERNEL_CODE, KERNEL_DATA, USER_DATA, USER_CODE, 0, 0];

/// Builds a 64-bit TSS descriptor (16 bytes = two `u64`s; Intel SDM
/// Vol. 3A 8.2.3). Access byte `0x89`: present, DPL0, S=0 (system),
/// type `0b1001` (available 64-bit TSS).
fn tss_descriptor(base: u64, limit: u32) -> (u64, u64) {
    let limit = u64::from(limit);
    let low = (limit & 0xFFFF)
        | ((base & 0xFF_FFFF) << 16)
        | (0x89 << 40)
        | (((limit >> 16) & 0xF) << 48)
        | (((base >> 24) & 0xFF) << 56);
    let high = base >> 32;
    (low, high)
}

/// Installs the kernel's own GDT and TSS and reloads every segment
/// register from it, replacing Limine's (which are only guaranteed valid
/// until the first thing that touches the GDT, such as this).
pub fn init() {
    // SAFETY: boot-time, single-threaded, before interrupts are enabled;
    // nothing else can observe or mutate `TSS`/`GDT` while this runs, and
    // both are `'static` so the addresses taken here stay valid forever.
    unsafe {
        let tss_ptr = &raw mut TSS;
        (*tss_ptr).set_rsp0(KERNEL_STACK.top());
        (*tss_ptr).set_ist(1, DF_STACK.top());
        (*tss_ptr).set_ist(2, NMI_STACK.top());
        (*tss_ptr).iomap_base = size_of::<Tss>() as u16;

        let (low, high) = tss_descriptor(tss_ptr as u64, (size_of::<Tss>() - 1) as u32);
        let gdt_ptr = &raw mut GDT;
        (*gdt_ptr)[5] = low;
        (*gdt_ptr)[6] = high;

        let dtp = DtPointer {
            limit: (size_of::<[u64; 7]>() - 1) as u16,
            base: gdt_ptr as u64,
        };
        // SAFETY (nested): `dtp` is a valid, live `DtPointer` on this
        // function's stack for the duration of the instruction.
        core::arch::asm!("lgdt [{}]", in(reg) &dtp, options(readonly, nostack, preserves_flags));
    }

    // SAFETY: the standard far-return CS reload -- push a code selector
    // and a target address, then a far return pops both into CS:RIP. The
    // target is the very next instruction after this block, so this is
    // stack- and control-flow-neutral other than actually updating CS to
    // the kernel code descriptor `lgdt` above just installed.
    unsafe {
        core::arch::asm!(
            "push {sel}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            sel = const KERNEL_CODE_SELECTOR as u64,
            tmp = lateout(reg) _,
            options(preserves_flags),
        );
    }

    // SAFETY: loading DS/ES/SS with the kernel data selector and FS/GS
    // with a null selector matches exactly what the GDT just installed
    // defines for ring 0; segment-register loads have no memory side
    // effects beyond updating the CPU's hidden descriptor cache.
    unsafe {
        core::arch::asm!(
            "mov ds, {data:x}",
            "mov es, {data:x}",
            "mov ss, {data:x}",
            "mov fs, {zero:x}",
            "mov gs, {zero:x}",
            data = in(reg) KERNEL_DATA_SELECTOR,
            zero = in(reg) 0u16,
            options(nostack, preserves_flags),
        );
    }

    // SAFETY: `ltr` loads the Task Register from the TSS descriptor this
    // function just installed at `TSS_SELECTOR`, which points at the
    // `'static` `TSS` above with valid `RSP0`/`IST1`/`IST2` fields already
    // written.
    unsafe {
        core::arch::asm!("ltr {0:x}", in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
    }

    kprintln!("[gdt] loaded");
}

/// Test-only introspection: the TSS's current `IST1` value.
pub fn ist1() -> u64 {
    // SAFETY: read-only access to a single field of the `'static` `TSS`,
    // which by the time this is callable (after `init()`) is never
    // mutated again.
    unsafe { TSS.ist1 }
}

/// Test-only introspection: `(base, top)` of the double-fault (IST1) stack.
pub fn df_stack_bounds() -> (u64, u64) {
    (DF_STACK.base(), DF_STACK.top())
}
