//! RFC 5280 section 4.1.2.5 `Time` (`UTCTime` / `GeneralizedTime`) to Unix
//! seconds, with the section's exact format rules enforced: both variants
//! require a trailing `Z` (UTC, no fractional seconds, no other time zone
//! designator) and an explicit seconds field; `UTCTime`'s two-digit year is
//! windowed 1950-2049 per section 4.1.2.5.1; certificates dated 2050 or
//! later MUST use `GeneralizedTime` per section 4.1.2.5.2, enforced here by
//! rejecting a `GeneralizedTime` whose year is before 2050 (the only date
//! range `UTCTime` cannot already represent, so a compliant generator would
//! never have chosen `GeneralizedTime` for it).
//!
//! Calendar-to-Unix-seconds conversion is Howard Hinnant's `days_from_civil`
//! (<https://howardhinnant.github.io/date_algorithms.html>), a closed-form
//! arithmetic transformation for the proleptic Gregorian calendar in the
//! public domain -- not a date/time crate (DECISIONS.md D27), just the
//! well-known formula, re-derived here and cross-checked in this module's
//! own tests against known epoch/leap-year dates and, in
//! `tests/time_python_cross.rs`, against Python's own `calendar`/`datetime`
//! standard library for several thousand random dates.

use crate::der::{DerError, Reader, tag};

fn is_leap_year(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Howard Hinnant's `days_from_civil`: days since the Unix epoch
/// (1970-01-01) for a proleptic-Gregorian `(y, m, d)`, valid for any `y` and
/// `m` in `1..=12`. Negative for dates before the epoch.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // [0, 11], March-based month
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

fn to_unix_seconds(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Result<i64, DerError> {
    if !(1..=12).contains(&month) {
        return Err(DerError::InvalidTime);
    }
    if day == 0 || day > days_in_month(year, month) {
        return Err(DerError::InvalidTime);
    }
    if hour > 23 || minute > 59 || second > 59 {
        return Err(DerError::InvalidTime);
    }
    let days = days_from_civil(year, i64::from(month), i64::from(day));
    Ok(days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second))
}

fn digit(b: u8) -> Result<u32, DerError> {
    if b.is_ascii_digit() { Ok(u32::from(b - b'0')) } else { Err(DerError::InvalidTime) }
}

fn two_digits(bytes: &[u8]) -> Result<u32, DerError> {
    if bytes.len() != 2 {
        return Err(DerError::InvalidTime);
    }
    Ok(digit(bytes[0])? * 10 + digit(bytes[1])?)
}

/// Parses a `UTCTime` content (`YYMMDDHHMMSSZ`, exactly 13 bytes) per RFC
/// 5280 section 4.1.2.5.1: seconds and the trailing `Z` are mandatory, no
/// fractional seconds or other zone designator is permitted.
fn parse_utc_time(content: &[u8]) -> Result<i64, DerError> {
    if content.len() != 13 || content[12] != b'Z' {
        return Err(DerError::InvalidTime);
    }
    let yy = two_digits(&content[0..2])?;
    let year = if yy >= 50 { 1900 + i64::from(yy) } else { 2000 + i64::from(yy) };
    let month = two_digits(&content[2..4])?;
    let day = two_digits(&content[4..6])?;
    let hour = two_digits(&content[6..8])?;
    let minute = two_digits(&content[8..10])?;
    let second = two_digits(&content[10..12])?;
    to_unix_seconds(year, month, day, hour, minute, second)
}

/// Parses a `GeneralizedTime` content (`YYYYMMDDHHMMSSZ`, exactly 15 bytes)
/// per RFC 5280 section 4.1.2.5.2: seconds and the trailing `Z` are
/// mandatory, fractional seconds are not permitted, and (enforced here) the
/// year must be 2050 or later -- `UTCTime` covers everything earlier, and
/// section 4.1.2.5.2 requires generators to use it for those dates.
fn parse_generalized_time(content: &[u8]) -> Result<i64, DerError> {
    if content.len() != 15 || content[14] != b'Z' {
        return Err(DerError::InvalidTime);
    }
    let year = i64::from(two_digits(&content[0..2])?) * 100 + i64::from(two_digits(&content[2..4])?);
    if year < 2050 {
        return Err(DerError::InvalidTime);
    }
    let month = two_digits(&content[4..6])?;
    let day = two_digits(&content[6..8])?;
    let hour = two_digits(&content[8..10])?;
    let minute = two_digits(&content[10..12])?;
    let second = two_digits(&content[12..14])?;
    to_unix_seconds(year, month, day, hour, minute, second)
}

/// Reads a `Time` (`CHOICE { utcTime UTCTime, generalTime GeneralizedTime }`,
/// RFC 5280 section 4.1.2.5) and returns it as Unix seconds (may be negative
/// for a `UTCTime` before 1970, which the 1950 window start makes possible).
pub fn read_time(r: &mut Reader<'_>) -> Result<i64, DerError> {
    let tlv = r.read_tlv()?;
    match tlv.tag {
        tag::UTC_TIME => parse_utc_time(tlv.content),
        tag::GENERALIZED_TIME => parse_generalized_time(tlv.content),
        _ => Err(DerError::InvalidTime),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn known_dates() {
        // 2000-03-01 is a well-known day-count check (just after a leap day
        // in a leap year divisible by 400).
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        // 1969-12-31 is exactly one day before the epoch.
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn utc_time_windows_the_two_digit_year() {
        // 500101000000Z -> 1950-01-01; 490101000000Z -> 2049-01-01.
        let ts50 = parse_utc_time(b"500101000000Z").unwrap();
        let ts49 = parse_utc_time(b"490101000000Z").unwrap();
        assert!(ts50 < 0); // before the epoch
        assert!(ts49 > ts50);
        // 1950-01-01 00:00:00 UTC.
        assert_eq!(ts50, days_from_civil(1950, 1, 1) * 86_400);
        assert_eq!(ts49, days_from_civil(2049, 1, 1) * 86_400);
    }

    #[test]
    fn utc_time_requires_z_and_seconds() {
        assert_eq!(parse_utc_time(b"500101000000+"), Err(DerError::InvalidTime));
        assert_eq!(parse_utc_time(b"5001010000Z"), Err(DerError::InvalidTime)); // no seconds
    }

    #[test]
    fn utc_time_rejects_invalid_calendar_fields() {
        assert_eq!(parse_utc_time(b"500001010000Z"), Err(DerError::InvalidTime)); // month 0
        assert_eq!(parse_utc_time(b"501301010000Z"), Err(DerError::InvalidTime)); // month 13
        assert_eq!(parse_utc_time(b"500230000000Z"), Err(DerError::InvalidTime)); // Feb 30
        assert_eq!(parse_utc_time(b"500132000000Z"), Err(DerError::InvalidTime)); // day 32
        assert_eq!(parse_utc_time(b"500101240000Z"), Err(DerError::InvalidTime)); // hour 24
        assert_eq!(parse_utc_time(b"500101006000Z"), Err(DerError::InvalidTime)); // minute 60
        assert_eq!(parse_utc_time(b"500101000060Z"), Err(DerError::InvalidTime)); // second 60
    }

    #[test]
    fn utc_time_accepts_leap_day() {
        assert!(parse_utc_time(b"000229000000Z").is_ok()); // 2000 is a leap year
        assert_eq!(parse_utc_time(b"010229000000Z"), Err(DerError::InvalidTime)); // 2001 is not
    }

    #[test]
    fn generalized_time_requires_year_2050_or_later() {
        assert_eq!(parse_generalized_time(b"20491231235959Z"), Err(DerError::InvalidTime));
        assert!(parse_generalized_time(b"20500101000000Z").is_ok());
    }

    #[test]
    fn generalized_time_rejects_fractional_seconds_and_missing_z() {
        assert_eq!(parse_generalized_time(b"20500101000000.5Z"), Err(DerError::InvalidTime));
        assert_eq!(parse_generalized_time(b"20500101000000"), Err(DerError::InvalidTime));
    }

    #[test]
    fn read_time_dispatches_on_tag() {
        let der = [0x17, 13, b'5', b'0', b'0', b'1', b'0', b'1', b'0', b'0', b'0', b'0', b'0', b'0', b'Z'];
        let mut r = Reader::new(&der);
        assert!(read_time(&mut r).is_ok());

        let der2 = [0x02, 0x01, 0x00]; // an INTEGER, not a time
        let mut r2 = Reader::new(&der2);
        assert_eq!(read_time(&mut r2), Err(DerError::InvalidTime));
    }
}
