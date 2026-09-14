//! The Intel 8042 PS/2 controller (brief M1-T6): ports 0x60 (data) and
//! 0x64 (status on read, command on write). `init` brings up the
//! controller and its port-1 (keyboard) device entirely by polling, with
//! every wait bounded (`wait_for`) -- "real laptops may not have one"
//! (brief), so nothing here may ever spin forever. All of `init`'s own
//! reads of 0x60 are safe to do synchronously because they run before
//! `drivers::ps2::init` unmasks IRQ1's I/O APIC redirection entry (its
//! step 3, after this module's `init` returns): nothing else can be
//! reading port 0x60 out from under it. `set_leds`, which is also called
//! later once IRQ1 is live, is the one function here that has to account
//! for that -- see its own docs.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::x86_64::port::{inb, outb};
use crate::kprintln;
use crate::time;

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const COMMAND_PORT: u16 = 0x64;

/// Status register bit 0: a byte is waiting at the data port.
const STATUS_OUTPUT_FULL: u8 = 1 << 0;
/// Status register bit 1: the controller is not ready to accept a byte
/// written to the data or command port.
const STATUS_INPUT_FULL: u8 = 1 << 1;

const CMD_READ_CONFIG: u8 = 0x20;
const CMD_WRITE_CONFIG: u8 = 0x60;
const CMD_DISABLE_PORT2: u8 = 0xA7;
const CMD_DISABLE_PORT1: u8 = 0xAD;
const CMD_ENABLE_PORT1: u8 = 0xAE;
const CMD_SELF_TEST: u8 = 0xAA;
const CMD_TEST_PORT1: u8 = 0xAB;

const SELF_TEST_PASS: u8 = 0x55;
const PORT_TEST_PASS: u8 = 0x00;

/// Configuration byte bit 0: first PS/2 port interrupt (IRQ1) enabled.
const CONFIG_PORT1_IRQ: u8 = 1 << 0;
/// Configuration byte bit 4: first PS/2 port clock *disabled*: this bit
/// must be clear for the port to run at all.
const CONFIG_PORT1_CLOCK_DISABLE: u8 = 1 << 4;
/// Configuration byte bit 6: first PS/2 port scancode translation --
/// translates the device's native scancode set 2 into set 1 before a
/// byte ever reaches the data port, which is the set `scancode` decodes.
const CONFIG_PORT1_TRANSLATION: u8 = 1 << 6;

const DEV_RESET: u8 = 0xFF;
const DEV_ACK: u8 = 0xFA;
const DEV_SELF_TEST_PASS: u8 = 0xAA;
const DEV_SET_LEDS: u8 = 0xED;

/// Set-LEDs data byte bit 2: Caps Lock.
const LED_CAPS_LOCK: u8 = 1 << 2;

/// Bounded-wait budget for one ordinary controller/device handshake
/// (accepting a written byte, or an immediate response like the
/// self-test/port-test result bytes), in `time::ticks()` milliseconds.
/// Generous for real hardware, negligible next to `gmake test`'s 90 s
/// overall timeout; a machine with no 8042 at all always gives up within
/// this instead of hanging the boot.
const HANDSHAKE_BUDGET_MS: u64 = 100;
/// Bounded-wait budget for the keyboard device's post-reset ACK and BAT
/// (self-test) response (kernel-review, M1-T6 fix 2): the PS/2 spec
/// allows a device up to ~500-750 ms to complete its power-on/reset self
/// test, well beyond `HANDSHAKE_BUDGET_MS` -- a real keyboard taking that
/// long must not be misread as "no i8042 present".
const RESET_BUDGET_MS: u64 = 1000;

/// `true` once `init` has run every step of the bring-up sequence
/// successfully. `false` if ACPI already said no 8042 exists, or any
/// bounded wait timed out -- either way, `drivers::ps2::init` treats this
/// the same: never register the IRQ handler or touch the I/O APIC.
static PRESENT: AtomicBool = AtomicBool::new(false);

/// Whether the i8042 bring-up sequence completed successfully. Tests and
/// `drivers::ps2::init` use this to skip everything downstream (IRQ
/// registration, the end-to-end typing test) gracefully on a machine that
/// genuinely has no PS/2 controller.
pub fn is_present() -> bool {
    PRESENT.load(Ordering::Relaxed)
}

fn read_status() -> u8 {
    // SAFETY: port 0x64 is the 8042's fixed status-register port on
    // every PC-compatible platform (DECISIONS.md D1); reading it has no
    // side effects.
    unsafe { inb(STATUS_PORT) }
}

/// Polls `condition` until it returns `true` or `budget_ms` (by
/// `time::ticks()`, live from `time::init` onward -- `drivers::ps2::init`
/// only ever runs after `start_interrupts` has already called that and
/// enabled interrupts) elapses, `hlt`ing between checks. Never spins
/// forever: real hardware (or a QEMU machine with no i8042) that simply
/// never responds must not hang the boot.
fn wait_for(budget_ms: u64, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = time::ticks().saturating_add(budget_ms);
    loop {
        if condition() {
            return true;
        }
        if time::ticks() >= deadline {
            return false;
        }
        // SAFETY: `hlt` halts until the next interrupt and is always
        // valid to execute from ring 0; every caller of `wait_for` (this
        // module's own `init`/`set_leds`, the only callers) only ever
        // runs after `time::init`'s periodic timer is already live, so
        // this always wakes again well within `budget_ms`.
        unsafe { core::arch::asm!("hlt", options(nomem, nostack, preserves_flags)) };
    }
}

fn wait_output_full(budget_ms: u64) -> bool {
    wait_for(budget_ms, || read_status() & STATUS_OUTPUT_FULL != 0)
}

fn wait_input_clear() -> bool {
    wait_for(HANDSHAKE_BUDGET_MS, || read_status() & STATUS_INPUT_FULL == 0)
}

/// Reads one byte from the data port, first waiting up to `budget_ms` for
/// the controller to report one is actually available. `None` on
/// timeout. `budget_ms` is `HANDSHAKE_BUDGET_MS` for every ordinary
/// response, and the longer `RESET_BUDGET_MS` for the two reads right
/// after a device reset (kernel-review, M1-T6 fix 2).
fn read_data(budget_ms: u64) -> Option<u8> {
    if !wait_output_full(budget_ms) {
        return None;
    }
    // SAFETY: port 0x60 is the 8042's fixed data-register port;
    // `wait_output_full` just confirmed a byte is waiting there.
    Some(unsafe { inb(DATA_PORT) })
}

/// Writes one byte to the data port, first waiting (bounded) for the
/// controller to report it's ready to accept one. `false` on timeout
/// (nothing is written).
fn write_data(byte: u8) -> bool {
    if !wait_input_clear() {
        return false;
    }
    // SAFETY: port 0x60 is the 8042's fixed data-register port;
    // `wait_input_clear` just confirmed the controller is ready.
    unsafe { outb(DATA_PORT, byte) };
    true
}

/// Writes one byte to the command port, first waiting (bounded) for the
/// controller to report it's ready. `false` on timeout.
fn write_command(byte: u8) -> bool {
    if !wait_input_clear() {
        return false;
    }
    // SAFETY: port 0x64 is the 8042's fixed command-register port (write
    // side); `wait_input_clear` just confirmed the controller is ready.
    unsafe { outb(COMMAND_PORT, byte) };
    true
}

/// Reads and discards bytes from the data port while the controller
/// reports one waiting, up to a small fixed iteration bound -- never a
/// `time::ticks()` wait: an empty output buffer must return immediately,
/// not wait on a byte that was never coming.
fn flush_output_buffer() {
    const MAX_FLUSH_BYTES: u32 = 32;
    for _ in 0..MAX_FLUSH_BYTES {
        if read_status() & STATUS_OUTPUT_FULL == 0 {
            return;
        }
        // SAFETY: status just confirmed a byte is waiting at 0x60.
        unsafe { inb(DATA_PORT) };
    }
}

/// Reads one byte directly from the data port with no wait at all: valid
/// only from `drivers::ps2`'s IRQ1 handler, where the interrupt itself
/// *is* the controller's signal that a byte is already sitting at 0x60
/// (an i8042 only ever raises IRQ1 because `STATUS_OUTPUT_FULL` just
/// became true for exactly this byte).
pub(super) fn read_scancode_byte() -> u8 {
    // SAFETY: see the doc comment above -- only called from the IRQ1
    // handler, for which a byte being ready at 0x60 is precisely why it
    // is running at all.
    unsafe { inb(DATA_PORT) }
}

/// Sets (or clears) the Caps Lock LED. Fire-and-forget: waits (bounded)
/// for the controller to accept each of the two command bytes, but --
/// unlike `set_leds_during_init` below -- never waits for the device's
/// ACK response. That asymmetry is deliberate: `init` (via
/// `set_leds_during_init`) is the only caller that can still safely read
/// 0x60 synchronously (IRQ1's I/O APIC redirection entry is unmasked only
/// after `init` returns -- `drivers::ps2::init`'s step 3). Every other
/// caller (`keyboard::poll_event`, whenever the decoder toggles caps
/// lock) runs with IRQ1 already live, so the device's ACK arrives through
/// the interrupt handler into the ring instead of anywhere this function
/// could read it -- `scancode::Decoder::feed` silently swallows it there
/// (it special-cases 0xFA/0xFE, which are never valid scancodes).
pub fn set_leds(caps_lock: bool) {
    let led_byte = if caps_lock { LED_CAPS_LOCK } else { 0 };
    write_data(DEV_SET_LEDS);
    write_data(led_byte);
}

/// Like `set_leds`, but also waits (bounded, `HANDSHAKE_BUDGET_MS`) for
/// and discards the device's ACK after each of the two command bytes
/// (kernel-review, M1-T6 fix 4). Reading 0x60 synchronously right after
/// writing a command is exactly what `set_leds`'s own docs say becomes
/// unsafe once IRQ1 is live -- this is a separate function, callable only
/// from `init` (before IRQ1 is unmasked), so nothing outside `init` can
/// ever reach the ACK-waiting version by mistake. Any ACK that never
/// arrives (a resend, or a slow device) is simply left for `init`'s own
/// final `flush_output_buffer` call to clean up, not treated as a
/// bring-up failure -- LED state is cosmetic.
fn set_leds_during_init(caps_lock: bool) {
    let led_byte = if caps_lock { LED_CAPS_LOCK } else { 0 };
    if write_data(DEV_SET_LEDS) {
        read_data(HANDSHAKE_BUDGET_MS);
    }
    if write_data(led_byte) {
        read_data(HANDSHAKE_BUDGET_MS);
    }
}

/// Brings up the i8042 controller and its port-1 (keyboard) device (brief
/// M1-T6 step 2): disable both ports, flush stale output, probe the
/// config byte, controller self-test, port-1 test, enable port 1,
/// re-read the config byte (kernel-review, M1-T6 fix 3: self-test can
/// reset it on real hardware, so the pre-self-test read above is only a
/// responsiveness probe, never used to compute the new value) and
/// reprogram it (IRQ1 on, translation on), reset the device, normalize
/// its LEDs off (waiting for those ACKs -- fix 4), then a final flush so
/// the output buffer is guaranteed empty before `drivers::ps2::init`
/// unmasks IRQ1 at the I/O APIC. Every step is a bounded wait; any
/// failure is logged and this simply leaves `is_present()` `false` rather
/// than panicking -- real hardware, and even some QEMU machine types, may
/// simply not have one.
pub fn init() {
    // SAFETY: port 0x64 is the fixed 8042 command port; "disable port N"
    // is always a safe, defined command regardless of whatever state the
    // controller is already in.
    unsafe {
        outb(COMMAND_PORT, CMD_DISABLE_PORT1);
        outb(COMMAND_PORT, CMD_DISABLE_PORT2);
    }
    flush_output_buffer();

    if !write_command(CMD_READ_CONFIG) {
        kprintln!("[kbd] i8042 not responding (read config)");
        return;
    }
    if read_data(HANDSHAKE_BUDGET_MS).is_none() {
        kprintln!("[kbd] i8042 not responding (config byte)");
        return;
    }

    if !write_command(CMD_SELF_TEST) {
        kprintln!("[kbd] i8042 not responding (self-test cmd)");
        return;
    }
    let Some(self_test_result) = read_data(HANDSHAKE_BUDGET_MS) else {
        kprintln!("[kbd] i8042 self-test: no response");
        return;
    };
    if self_test_result != SELF_TEST_PASS {
        kprintln!("[kbd] i8042 self-test failed: 0x{self_test_result:02x}");
        return;
    }

    if !write_command(CMD_TEST_PORT1) {
        kprintln!("[kbd] i8042 not responding (port-1 test cmd)");
        return;
    }
    let Some(port_test_result) = read_data(HANDSHAKE_BUDGET_MS) else {
        kprintln!("[kbd] i8042 port-1 test: no response");
        return;
    };
    if port_test_result != PORT_TEST_PASS {
        kprintln!("[kbd] i8042 port-1 test failed: 0x{port_test_result:02x}");
        return;
    }

    // SAFETY: "enable port 1" is always a safe, defined command.
    unsafe { outb(COMMAND_PORT, CMD_ENABLE_PORT1) };

    // kernel-review M1-T6 fix 3: re-read the config byte now, rather than
    // reusing the value read (as a pure responsiveness probe) before the
    // self-test above -- some real controllers reset it to power-on
    // defaults during self-test.
    if !write_command(CMD_READ_CONFIG) {
        kprintln!("[kbd] i8042 not responding (re-read config)");
        return;
    }
    let Some(config) = read_data(HANDSHAKE_BUDGET_MS) else {
        kprintln!("[kbd] i8042 not responding (re-read config byte)");
        return;
    };

    let new_config = (config | CONFIG_PORT1_IRQ | CONFIG_PORT1_TRANSLATION) & !CONFIG_PORT1_CLOCK_DISABLE;
    if !write_command(CMD_WRITE_CONFIG) || !write_data(new_config) {
        kprintln!("[kbd] i8042 not responding (write config)");
        return;
    }

    if !write_data(DEV_RESET) {
        kprintln!("[kbd] keyboard device not responding (reset)");
        return;
    }
    // kernel-review M1-T6 fix 2: the device's post-reset ACK and BAT
    // (self-test) response get the longer `RESET_BUDGET_MS` budget, not
    // `HANDSHAKE_BUDGET_MS` -- a real keyboard's power-on self test can
    // take up to several hundred milliseconds.
    let Some(reset_ack) = read_data(RESET_BUDGET_MS) else {
        kprintln!("[kbd] keyboard reset: no ack");
        return;
    };
    if reset_ack != DEV_ACK {
        kprintln!("[kbd] keyboard reset not ack'd: 0x{reset_ack:02x}");
        return;
    }
    let Some(reset_self_test) = read_data(RESET_BUDGET_MS) else {
        kprintln!("[kbd] keyboard reset: no self-test response");
        return;
    };
    if reset_self_test != DEV_SELF_TEST_PASS {
        kprintln!("[kbd] keyboard reset self-test failed: 0x{reset_self_test:02x}");
        return;
    }

    set_leds_during_init(false);
    flush_output_buffer();

    PRESENT.store(true, Ordering::Relaxed);
    kprintln!("[kbd] i8042 ok");
}
