//! CMOS real-time clock tests for brief M8-T7, written by the orchestrator: the implementation
//! must pass these unchanged. TLS certificate checks (M8/M9) and FAT timestamps (M3) need the
//! wall clock. QEMU's RTC follows the host's UTC clock; scripts/qemu.py also compares the boot
//! line `[rtc] YYYY-MM-DD HH:MM:SS UTC` with the host's time.
//!
//! API: `rtc::RawCmos { seconds, minutes, hours, day, month, year, century: Option<u8>, status_b }`
//! (register values as read), `rtc::decode(&RawCmos) -> Option<DateTime>`, `rtc::DateTime { year:
//! u16, month, day, hour, minute, second: u8 }`, `rtc::to_unix(&DateTime) -> u64`,
//! `rtc::from_unix(u64) -> DateTime`, `rtc::read() -> Option<DateTime>` (a consistent snapshot of
//! the live clock), `time::unix_now() -> Option<u64>` (the RTC at boot plus uptime).

use otteros_kernel::rtc::{self, DateTime, RawCmos};
use otteros_kernel::time;

#[allow(clippy::too_many_arguments)]
fn raw(seconds: u8, minutes: u8, hours: u8, day: u8, month: u8, year: u8, century: Option<u8>, status_b: u8) -> RawCmos {
    RawCmos { seconds, minutes, hours, day, month, year, century, status_b }
}

fn dt(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> DateTime {
    DateTime { year, month, day, hour, minute, second }
}

const BCD_24H: u8 = 0x02; // status B: 24-hour mode, BCD
const BIN_24H: u8 = 0x06; // 24-hour mode, binary
const BCD_12H: u8 = 0x00;
const BIN_12H: u8 = 0x04;

#[test_case]
fn rtc_decodes_bcd_and_binary() {
    assert_eq!(rtc::decode(&raw(0x59, 0x07, 0x23, 0x31, 0x12, 0x26, Some(0x20), BCD_24H)), Some(dt(2026, 12, 31, 23, 7, 59)));
    assert_eq!(rtc::decode(&raw(59, 7, 23, 31, 12, 26, Some(20), BIN_24H)), Some(dt(2026, 12, 31, 23, 7, 59)));
    assert_eq!(rtc::decode(&raw(0x05, 0x04, 0x03, 0x02, 0x01, 0x99, None, BCD_24H)), Some(dt(2099, 1, 2, 3, 4, 5)), "no century register: 20xx");
    assert_eq!(rtc::decode(&raw(0x00, 0x00, 0x00, 0x01, 0x01, 0x00, Some(0x21), BCD_24H)), Some(dt(2100, 1, 1, 0, 0, 0)));
}

#[test_case]
fn rtc_decodes_twelve_hour_clocks() {
    let cases = [(0x12u8, 0u8), (0x01, 1), (0x11, 11), (0x92, 12), (0x81, 13), (0x91, 23)];
    for (hours, expected) in cases {
        let got = rtc::decode(&raw(0, 0, hours, 0x15, 0x06, 0x26, Some(0x20), BCD_12H)).map(|d| d.hour);
        assert_eq!(got, Some(expected), "BCD 12-hour register {hours:#04x}");
    }
    for (hours, expected) in [(12u8, 0u8), (0x80 | 12, 12), (0x80 | 1, 13), (7, 7)] {
        let got = rtc::decode(&raw(0, 0, hours, 15, 6, 26, Some(20), BIN_12H)).map(|d| d.hour);
        assert_eq!(got, Some(expected), "binary 12-hour register {hours:#04x}");
    }
}

#[test_case]
fn rtc_rejects_impossible_values() {
    let bad = [
        raw(0, 0, 0, 0x01, 0x00, 0x26, Some(0x20), BCD_24H), // month 0
        raw(0, 0, 0, 0x01, 0x13, 0x26, Some(0x20), BCD_24H), // month 13
        raw(0, 0, 0, 0x00, 0x01, 0x26, Some(0x20), BCD_24H), // day 0
        raw(0, 0, 0, 0x32, 0x01, 0x26, Some(0x20), BCD_24H), // January 32
        raw(0, 0, 0, 0x30, 0x02, 0x24, Some(0x20), BCD_24H), // February 30
        raw(0, 0, 0, 0x29, 0x02, 0x26, Some(0x20), BCD_24H), // February 29 in a common year
        raw(0, 0, 0, 0x31, 0x04, 0x26, Some(0x20), BCD_24H), // April 31
        raw(0, 0, 0x24, 0x01, 0x01, 0x26, Some(0x20), BCD_24H), // hour 24
        raw(0, 0x60, 0, 0x01, 0x01, 0x26, Some(0x20), BCD_24H), // minute 60
        raw(0x60, 0, 0, 0x01, 0x01, 0x26, Some(0x20), BCD_24H), // second 60
        raw(0, 0x1A, 0, 0x01, 0x01, 0x26, Some(0x20), BCD_24H), // not BCD
        raw(0, 0, 0x13, 0x01, 0x01, 0x26, Some(0x20), BCD_12H), // 13 on a 12-hour clock
        raw(0, 0, 0x00, 0x01, 0x01, 0x26, Some(0x20), BCD_12H), // 0 on a 12-hour clock
        raw(0, 0, 0, 0x01, 0x01, 0x26, Some(0x9A), BCD_24H),   // century not BCD
    ];
    for (i, r) in bad.iter().enumerate() {
        assert_eq!(rtc::decode(r), None, "case {i}");
    }
    assert!(rtc::decode(&raw(0, 0, 0, 0x29, 0x02, 0x24, Some(0x20), BCD_24H)).is_some(), "2024 is a leap year");
    assert!(rtc::decode(&raw(0, 0, 0, 0x29, 0x02, 0x00, Some(0x20), BCD_24H)).is_some(), "2000 is a leap year");
    assert!(rtc::decode(&raw(0, 0, 0, 0x29, 0x02, 0x00, Some(0x21), BCD_24H)).is_none(), "2100 is not");
}

#[test_case]
fn rtc_unix_conversions() {
    let known = [
        (dt(1970, 1, 1, 0, 0, 0), 0u64),
        (dt(2000, 3, 1, 0, 0, 0), 951_868_800),
        (dt(2024, 2, 29, 12, 0, 0), 1_709_208_000),
        (dt(2026, 9, 25, 21, 40, 7), 1_790_372_407),
        (dt(2038, 1, 19, 3, 14, 8), 2_147_483_648),
        (dt(2099, 12, 31, 23, 59, 59), 4_102_444_799),
        (dt(2100, 3, 1, 0, 0, 0), 4_107_542_400),
    ];
    for (d, secs) in known {
        assert_eq!(rtc::to_unix(&d), secs, "{d:?}");
        assert_eq!(rtc::from_unix(secs), d, "{secs}");
    }
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..10_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let secs = x % (1u64 << 33);
        assert_eq!(rtc::to_unix(&rtc::from_unix(secs)), secs);
    }
}

#[test_case]
fn rtc_reads_a_plausible_live_clock() {
    let now = rtc::read().expect("QEMU has a CMOS RTC");
    assert!((2026..=2099).contains(&now.year), "{now:?}");
    let unix = time::unix_now().expect("the wall clock is set at boot");
    let rtc_unix = rtc::to_unix(&now);
    assert!(unix.abs_diff(rtc_unix) <= 2, "unix_now {unix} vs RTC {rtc_unix}");
}

#[test_case]
fn rtc_advances_with_real_time() {
    let t0 = rtc::to_unix(&rtc::read().unwrap());
    let u0 = time::unix_now().unwrap();
    time::sleep_ms(2100);
    let t1 = rtc::to_unix(&rtc::read().unwrap());
    let u1 = time::unix_now().unwrap();
    assert!((2..=10).contains(&(t1 - t0)), "RTC advanced {} s over a 2.1 s sleep", t1 - t0);
    assert!((2..=10).contains(&(u1 - u0)), "unix_now advanced {} s over a 2.1 s sleep", u1 - u0);
}
