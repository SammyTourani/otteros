//! The panic handler (brief M2-T3 step 5): `panic in <prog>: <msg>` to fd
//! 2, then exit(101).

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::{eprintln, process};

/// A `'static` string's raw parts (`AtomicPtr` needs a `Sized`, `Copy`-ish
/// payload -- a fat `&'static str` reference doesn't fit in one pointer,
/// so this stores the two halves of one instead). Set once by
/// `set_program_name` (every program's own `main` calls it as its first
/// statement); read by `panic` if it ever fires. `(null, 0)` -- the
/// default -- reads back as `"<unknown>"`.
static NAME_PTR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static NAME_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Every program calls this once, as the first statement in its own
/// `main`, so a later panic can report which program it came from
/// (`libotter` itself has no other way to know: `spawn`'s ABI keeps a
/// process's own path separate from its `argv`, so nothing here can just
/// read `args()[0]`).
pub fn set_program_name(name: &'static str) {
    NAME_PTR.store(name.as_ptr().cast_mut(), Ordering::Relaxed);
    NAME_LEN.store(name.len(), Ordering::Relaxed);
}

fn program_name() -> &'static str {
    let ptr = NAME_PTR.load(Ordering::Relaxed);
    let len = NAME_LEN.load(Ordering::Relaxed);
    if ptr.is_null() {
        return "<unknown>";
    }
    // SAFETY: `ptr`/`len` were set together, once, by `set_program_name`,
    // from a genuine `&'static str` (so `ptr` is valid for `len` bytes of
    // UTF-8 for the rest of the program's life) -- single-threaded (D17),
    // so there's no concurrent writer to race.
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    core::str::from_utf8(bytes).unwrap_or("<unknown>")
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    eprintln!("panic in {}: {}", program_name(), info.message());
    process::exit(101);
}
