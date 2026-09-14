//! Physical/virtual address newtypes shared by every `mm` submodule (brief
//! M1-T2 step 1). Both wrap a bare `u64` so they interoperate freely with
//! raw Limine data (`Entry::base`, `HhdmRespData::offset`, ...) via an
//! explicit `::new`, while staying genuinely distinct types to the
//! compiler: a `PhysAddr` can never be silently passed where a `VirtAddr`
//! (or a plain integer) was expected, or vice versa.

use core::fmt;

/// Size in bytes of one physical page frame -- the unit the whole `mm`
/// subsystem hands out and reclaims memory in.
pub const FRAME_SIZE: usize = 4096;

/// A physical memory address. Not directly dereferenceable -- go through
/// `mm::hhdm::phys_to_virt` first.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysAddr(u64);

/// A virtual (CPU-visible) memory address.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtAddr(u64);

/// Generates the identical `new`/`as_u64`/alignment/`frame_index` API for
/// both newtypes above. A macro rather than a shared trait: there is no
/// caller that's generic over "some address type", every call site knows
/// exactly which kind it has, so a trait would only add an indirection
/// nothing here needs.
macro_rules! impl_addr {
    ($name:ident) => {
        impl $name {
            /// Wraps a raw address. No validation: callers building one
            /// from Limine data or arithmetic are trusted to know what
            /// they're doing, same as the `u64` this replaces.
            #[inline]
            pub const fn new(addr: u64) -> Self {
                Self(addr)
            }

            #[inline]
            pub const fn as_u64(self) -> u64 {
                self.0
            }

            /// Rounds down to the previous multiple of `align`.
            ///
            /// # Panics
            /// If `align` is not a power of two (including 0).
            #[inline]
            pub const fn align_down(self, align: u64) -> Self {
                assert!(align.is_power_of_two(), "align must be a power of two");
                Self(self.0 & !(align - 1))
            }

            /// Rounds up to the next multiple of `align` (a
            /// already-aligned address is returned unchanged).
            ///
            /// # Panics
            /// If `align` is not a power of two (including 0).
            #[inline]
            pub const fn align_up(self, align: u64) -> Self {
                assert!(align.is_power_of_two(), "align must be a power of two");
                Self((self.0 + (align - 1)) & !(align - 1))
            }

            /// Whether this address is already a multiple of `align`.
            ///
            /// # Panics
            /// If `align` is not a power of two (including 0).
            #[inline]
            pub const fn is_aligned(self, align: u64) -> bool {
                assert!(align.is_power_of_two(), "align must be a power of two");
                self.0 & (align - 1) == 0
            }

            /// This address's 4 KiB frame number (`addr / FRAME_SIZE`).
            #[inline]
            pub const fn frame_index(self) -> u64 {
                self.0 / FRAME_SIZE as u64
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}(0x{:x})", stringify!($name), self.0)
            }
        }
    };
}

impl_addr!(PhysAddr);
impl_addr!(VirtAddr);
