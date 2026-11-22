//! The export day: parse `YYYYMMDD`, compute the half-open local-day epoch bounds, and format
//! `HH:MM`.

use std::str::FromStr;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use crate::shared::result::{Error, Operation, Result};

/// One calendar day in the operator's local time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Day(Date);

impl Day {
    /// Returns the day before `today`.
    ///
    /// The caller passes the local today so the date policy stays testable without the system clock.
    pub fn yesterday_from(today: Date) -> Result<Self> {
        today.yesterday().map(Self).map_err(Into::into)
    }

    /// Returns the half-open `[start, end)` epoch-second bounds of this day in `tz`.
    ///
    /// jiff's default `Compatible` disambiguation resolves a DST gap at local midnight to the first
    /// representable instant, so the bounds still cover the civil date.
    pub fn bounds(self, tz: &TimeZone) -> Result<(i64, i64)> {
        let start = self
            .0
            .to_zoned_datetime(tz.clone())?
            .timestamp()
            .as_second();
        let end = self
            .0
            .tomorrow()?
            .to_zoned_datetime(tz.clone())?
            .timestamp()
            .as_second();

        Ok((start, end))
    }

    /// The ISO `YYYY-MM-DD` form, as the file header renders it.
    pub fn iso(self) -> String {
        self.0.to_string()
    }

    /// The `YYYYMMDD` form, as the file name renders it.
    pub fn compact(self) -> String {
        format!(
            "{:04}{:02}{:02}",
            self.0.year(),
            self.0.month(),
            self.0.day()
        )
    }

    /// Renders the `HH:MM` local clock time of `epoch`.
    pub fn clock(self, epoch: i64, tz: &TimeZone) -> Result<String> {
        let datetime = tz.to_datetime(Timestamp::from_second(epoch)?);

        Ok(format!("{:02}:{:02}", datetime.hour(), datetime.minute()))
    }
}

impl FromStr for Day {
    type Err = Error;

    fn from_str(raw: &str) -> Result<Self> {
        let invalid = || Error::InvalidExportDate(raw.to_owned());

        if raw.len() != 8 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }

        let year = raw[0..4].parse::<i16>().map_err(|_| invalid())?;
        let month = raw[4..6].parse::<i8>().map_err(|_| invalid())?;
        let day = raw[6..8].parse::<i8>().map_err(|_| invalid())?;

        Date::new(year, month, day).map(Self).map_err(|_| invalid())
    }
}

impl From<jiff::Error> for Error {
    fn from(source: jiff::Error) -> Self {
        Self::External {
            operation: Operation::TimeZone,
            source: Box::new(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use jiff::tz::{self, TimeZone};

    use super::*;

    fn day(raw: &str) -> Day {
        raw.parse().expect("the day should parse")
    }

    #[test]
    fn valid_dates_parse() {
        assert_eq!(day("20261122").iso(), "2026-11-22");
        assert_eq!(day("20240229").iso(), "2024-02-29");
    }

    #[test]
    fn invalid_dates_are_rejected() {
        for raw in [
            "20230229",
            "20261301",
            "2026-11-22",
            "2026112",
            "abcdefgh",
            "",
        ] {
            match raw.parse::<Day>() {
                Err(Error::InvalidExportDate(value)) => assert_eq!(value, raw),
                Err(other) => panic!("expected an invalid date, got {other}"),
                Ok(_) => panic!("{raw:?} should not parse"),
            }
        }
    }

    #[test]
    fn yesterday_crosses_month_and_year_boundaries() {
        assert_eq!(
            Day::yesterday_from(date(2024, 3, 1)).expect("a leap day should resolve"),
            day("20240229")
        );
        assert_eq!(
            Day::yesterday_from(date(2025, 1, 1)).expect("a year boundary should resolve"),
            day("20241231")
        );
    }

    #[test]
    fn bounds_are_half_open_in_a_fixed_zone() {
        let tz = TimeZone::fixed(tz::offset(3));

        let (start, end) = day("20261122").bounds(&tz).expect("bounds should resolve");

        assert_eq!(start, 1_795_294_800);
        assert_eq!(end - start, 86_400);
    }

    #[test]
    fn bounds_follow_daylight_saving_time() {
        let tz = TimeZone::get("America/New_York").expect("the zone should resolve");

        let (spring_start, spring_end) = day("20240310")
            .bounds(&tz)
            .expect("the spring day should resolve");
        let (fall_start, fall_end) = day("20241103")
            .bounds(&tz)
            .expect("the fall day should resolve");

        assert_eq!(spring_end - spring_start, 23 * 3_600);
        assert_eq!(fall_end - fall_start, 25 * 3_600);
    }

    #[test]
    fn clock_pads_the_hour_and_minute() {
        let tz = TimeZone::fixed(tz::offset(3));

        let clock = day("20261122")
            .clock(1_795_323_840, &tz)
            .expect("the clock should render");

        assert_eq!(clock, "08:04");
    }

    #[test]
    fn iso_and_compact_match_the_sample() {
        let day = day("20261122");

        assert_eq!(day.iso(), "2026-11-22");
        assert_eq!(day.compact(), "20261122");
    }
}
