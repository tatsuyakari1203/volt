//! Injectable time, so journal timestamps are deterministic in tests.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A point in time as milliseconds since the Unix epoch (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Builds a timestamp from Unix milliseconds.
    #[must_use]
    pub const fn from_unix_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn as_unix_millis(self) -> i64 {
        self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

/// Source of the current time.
pub trait Clock: Send + Sync {
    /// The current time.
    fn now(&self) -> Timestamp;
}

/// [`Clock`] backed by the system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0);
        Timestamp(millis)
    }
}

impl Timestamp {
    /// The time as ISO 8601 in UTC with millisecond precision, such as
    /// `2026-11-10T03:01:00.123Z`. Pure arithmetic, so it needs no time zone
    /// data and gives the same text on every machine.
    #[must_use]
    pub fn to_iso8601(self) -> String {
        const DAY_MS: i64 = 86_400_000;
        let days = self.0.div_euclid(DAY_MS);
        let ms_of_day = self.0.rem_euclid(DAY_MS);
        // Civil date from days since 1970-01-01 (proleptic Gregorian).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        let hour = ms_of_day / 3_600_000;
        let minute = ms_of_day / 60_000 % 60;
        let second = ms_of_day / 1000 % 60;
        let milli = ms_of_day % 1000;
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}Z")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_8601_is_utc_with_milliseconds() {
        let iso = |ms| Timestamp::from_unix_millis(ms).to_iso8601();
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        // Leap day, and the day after it.
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso(951_868_800_000), "2000-03-01T00:00:00.000Z");
        // Before the epoch.
        assert_eq!(iso(-1), "1969-12-31T23:59:59.999Z");
    }
}
