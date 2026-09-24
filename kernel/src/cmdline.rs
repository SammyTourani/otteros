//! Access to the Limine-provided kernel command line. Used to pick the
//! kernel's mode (see src/test_main.rs and briefs/M0-T1.md step 3).

use limine::request::ExecutableCmdlineRequest;

#[used]
#[unsafe(link_section = ".requests")]
static CMDLINE_REQUEST: ExecutableCmdlineRequest = ExecutableCmdlineRequest::new();

/// Returns the raw kernel command line Limine passed us, or `""` if the
/// bootloader didn't answer the request at all.
pub fn get() -> &'static str {
    CMDLINE_REQUEST.response().map(|r| r.cmdline()).unwrap_or("")
}

/// Returns true if the kernel was booted in test mode (brief M2-T4, syscall 16).
/// Test mode is indicated by the `test` keyword in the kernel command line.
pub fn is_test_mode() -> bool {
    get().contains("test")
}
