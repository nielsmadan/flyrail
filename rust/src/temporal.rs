use crate::values::ValueError;
use std::fmt;
use std::str::FromStr;

const MIN_YEAR: i64 = 1;
const MAX_YEAR: i64 = 9999;
const SECONDS_PER_DAY: i64 = 86_400;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Date {
    year: u16,
    month: u8,
    day: u8,
}

impl Date {
    pub fn new(year: u16, month: u8, day: u8) -> Result<Self, ValueError> {
        if !(1..=9999).contains(&year) {
            return Err(ValueError::new("date year must be 1-9999"));
        }
        if !(1..=12).contains(&month) {
            return Err(ValueError::new("date month must be 1-12"));
        }
        if day < 1 || day > days_in_month(year, month) {
            return Err(ValueError::new("date day is out of range for its month"));
        }
        Ok(Self { year, month, day })
    }

    pub const fn year(self) -> u16 {
        self.year
    }

    pub const fn month(self) -> u8 {
        self.month
    }

    pub const fn day(self) -> u8 {
        self.day
    }

    fn days_since_epoch(self) -> i64 {
        days_from_civil(
            i64::from(self.year),
            i64::from(self.month),
            i64::from(self.day),
        )
    }

    fn from_days_since_epoch(days: i64) -> Result<Self, ValueError> {
        let (year, month, day) = civil_from_days(days);
        if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
            return Err(ValueError::new("datetime is out of the supported range"));
        }
        let year = u16::try_from(year).map_err(|_| ValueError::new("date year must be 1-9999"))?;
        let month = u8::try_from(month).map_err(|_| ValueError::new("date month must be 1-12"))?;
        let day = u8::try_from(day).map_err(|_| ValueError::new("date day is out of range"))?;
        Self::new(year, month, day)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

impl FromStr for Date {
    type Err = ValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bytes = text.as_bytes();
        if bytes.len() != 10 || bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
            return Err(ValueError::new("date must be written as YYYY-MM-DD"));
        }
        let year = digits(text.get(0..4))?;
        let month = digits(text.get(5..7))?;
        let day = digits(text.get(8..10))?;
        Self::new(
            u16::try_from(year).map_err(|_| ValueError::new("date year must be 1-9999"))?,
            u8::try_from(month).map_err(|_| ValueError::new("date month must be 1-12"))?,
            u8::try_from(day).map_err(|_| ValueError::new("date day is out of range"))?,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Time {
    hour: u8,
    minute: u8,
    second: u8,
    microsecond: u32,
}

impl Time {
    pub fn new(hour: u8, minute: u8, second: u8, microsecond: u32) -> Result<Self, ValueError> {
        if hour > 23 {
            return Err(ValueError::new("time hour must be 0-23"));
        }
        if minute > 59 {
            return Err(ValueError::new("time minute must be 0-59"));
        }
        if second > 59 {
            return Err(ValueError::new("time second must be 0-59"));
        }
        if microsecond > 999_999 {
            return Err(ValueError::new("time microsecond must be 0-999999"));
        }
        Ok(Self {
            hour,
            minute,
            second,
            microsecond,
        })
    }

    pub const fn hour(self) -> u8 {
        self.hour
    }

    pub const fn minute(self) -> u8 {
        self.minute
    }

    pub const fn second(self) -> u8 {
        self.second
    }

    pub const fn microsecond(self) -> u32 {
        self.microsecond
    }

    fn seconds_of_day(self) -> i64 {
        i64::from(self.hour) * 3_600 + i64::from(self.minute) * 60 + i64::from(self.second)
    }

    fn from_seconds_of_day(seconds: i64, microsecond: u32) -> Result<Self, ValueError> {
        let hour =
            u8::try_from(seconds / 3_600).map_err(|_| ValueError::new("time hour must be 0-23"))?;
        let minute = u8::try_from((seconds % 3_600) / 60)
            .map_err(|_| ValueError::new("time minute must be 0-59"))?;
        let second =
            u8::try_from(seconds % 60).map_err(|_| ValueError::new("time second must be 0-59"))?;
        Self::new(hour, minute, second, microsecond)
    }
}

impl fmt::Display for Time {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:02}:{:02}:{:02}.{:06}",
            self.hour, self.minute, self.second, self.microsecond
        )
    }
}

impl FromStr for Time {
    type Err = ValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bytes = text.as_bytes();
        if bytes.len() < 5 || bytes.get(2) != Some(&b':') {
            return Err(ValueError::new(
                "time must be written as HH:MM[:SS[.ffffff]]",
            ));
        }
        let hour = digits(text.get(0..2))?;
        let minute = digits(text.get(3..5))?;
        let mut second = 0;
        let mut microsecond = 0;
        if bytes.len() > 5 {
            if bytes.get(5) != Some(&b':') || bytes.len() < 8 {
                return Err(ValueError::new(
                    "time must be written as HH:MM[:SS[.ffffff]]",
                ));
            }
            second = digits(text.get(6..8))?;
            if bytes.len() > 8 {
                if bytes.get(8) != Some(&b'.') {
                    return Err(ValueError::new(
                        "time must be written as HH:MM[:SS[.ffffff]]",
                    ));
                }
                let fraction = text.get(9..).unwrap_or_default();
                if fraction.is_empty() || fraction.len() > 6 {
                    return Err(ValueError::new("time fraction must have 1-6 digits"));
                }
                let scale = 10_u32.pow(6 - u32::try_from(fraction.len()).unwrap_or(6));
                microsecond = digits(Some(fraction))? * scale;
            }
        }
        Self::new(
            u8::try_from(hour).map_err(|_| ValueError::new("time hour must be 0-23"))?,
            u8::try_from(minute).map_err(|_| ValueError::new("time minute must be 0-59"))?,
            u8::try_from(second).map_err(|_| ValueError::new("time second must be 0-59"))?,
            microsecond,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DateTime {
    date: Date,
    time: Time,
    coordinated: bool,
}

impl DateTime {
    pub const fn local(date: Date, time: Time) -> Self {
        Self {
            date,
            time,
            coordinated: false,
        }
    }

    pub const fn coordinated(date: Date, time: Time) -> Self {
        Self {
            date,
            time,
            coordinated: true,
        }
    }

    pub fn with_offset(date: Date, time: Time, offset_seconds: i32) -> Result<Self, ValueError> {
        if offset_seconds.unsigned_abs() >= 86_400 {
            return Err(ValueError::new("datetime offset must be less than one day"));
        }
        let instant = date.days_since_epoch() * SECONDS_PER_DAY + time.seconds_of_day()
            - i64::from(offset_seconds);
        let days = instant.div_euclid(SECONDS_PER_DAY);
        let seconds = instant.rem_euclid(SECONDS_PER_DAY);
        Ok(Self {
            date: Date::from_days_since_epoch(days)?,
            time: Time::from_seconds_of_day(seconds, time.microsecond)?,
            coordinated: true,
        })
    }

    pub const fn date(self) -> Date {
        self.date
    }

    pub const fn time(self) -> Time {
        self.time
    }

    pub const fn is_coordinated(self) -> bool {
        self.coordinated
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}T{}", self.date, self.time)?;
        if self.coordinated {
            formatter.write_str("+00:00")?;
        }
        Ok(())
    }
}

impl FromStr for DateTime {
    type Err = ValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (date_text, rest) = text
            .split_once('T')
            .ok_or_else(|| ValueError::new("datetime must separate date and time with T"))?;
        let date = date_text.parse::<Date>()?;
        let (time_text, offset) = split_offset(rest)?;
        let time = time_text.parse::<Time>()?;
        match offset {
            None => Ok(Self::local(date, time)),
            Some(0) => Ok(Self::coordinated(date, time)),
            Some(seconds) => Self::with_offset(date, time, seconds),
        }
    }
}

fn split_offset(text: &str) -> Result<(&str, Option<i32>), ValueError> {
    if let Some(time) = text.strip_suffix('Z') {
        return Ok((time, Some(0)));
    }
    let Some(position) = text.rfind(['+', '-']) else {
        return Ok((text, None));
    };
    let (time, offset) = text.split_at(position);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let body = offset.get(1..).unwrap_or_default();
    let (hours, minutes, seconds) = match body.len() {
        2 => (digits(Some(body))?, 0, 0),
        4 => (digits(body.get(0..2))?, digits(body.get(2..4))?, 0),
        5 if body.as_bytes().get(2) == Some(&b':') => {
            (digits(body.get(0..2))?, digits(body.get(3..5))?, 0)
        }
        8 if body.as_bytes().get(2) == Some(&b':') && body.as_bytes().get(5) == Some(&b':') => (
            digits(body.get(0..2))?,
            digits(body.get(3..5))?,
            digits(body.get(6..8))?,
        ),
        _ => return Err(ValueError::new("datetime offset must be written as ±HH:MM")),
    };
    let total = i64::from(hours) * 3_600 + i64::from(minutes) * 60 + i64::from(seconds);
    let total =
        i32::try_from(total).map_err(|_| ValueError::new("datetime offset is out of range"))?;
    Ok((time, Some(sign * total)))
}

fn digits(text: Option<&str>) -> Result<u32, ValueError> {
    let text = text.ok_or_else(|| ValueError::new("temporal value is truncated"))?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ValueError::new("temporal value must use ASCII digits"));
    }
    text.parse()
        .map_err(|_| ValueError::new("temporal value is out of range"))
}

const fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

const fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::{Date, DateTime, Time};

    #[test]
    fn formats_dates_and_times_like_python_isoformat() {
        assert_eq!(Date::new(2026, 1, 2).unwrap().to_string(), "2026-01-02");
        assert_eq!(Date::new(7, 12, 31).unwrap().to_string(), "0007-12-31");
        assert_eq!(
            Time::new(1, 2, 3, 0).unwrap().to_string(),
            "01:02:03.000000"
        );
        assert_eq!(
            Time::new(23, 59, 59, 999_999).unwrap().to_string(),
            "23:59:59.999999"
        );
    }

    #[test]
    fn rejects_out_of_range_components() {
        assert!(Date::new(2026, 2, 29).is_err());
        assert!(Date::new(2024, 2, 30).is_err());
        assert!(Date::new(0, 1, 1).is_err());
        assert!(Time::new(24, 0, 0, 0).is_err());
        assert!(Time::new(0, 0, 60, 0).is_err());
        assert!(Time::new(0, 0, 0, 1_000_000).is_err());
    }

    #[test]
    fn accepts_leap_days_in_leap_years() {
        assert_eq!(Date::new(2024, 2, 29).unwrap().to_string(), "2024-02-29");
        assert_eq!(Date::new(2000, 2, 29).unwrap().to_string(), "2000-02-29");
        assert!(Date::new(1900, 2, 29).is_err());
    }

    #[test]
    fn normalizes_offset_datetimes_to_utc() {
        let parsed: DateTime = "2026-01-02T05:02:03.000000+05:00".parse().unwrap();
        assert_eq!(parsed.to_string(), "2026-01-02T00:02:03.000000+00:00");
        let across_midnight: DateTime = "2026-01-01T01:00:00.000000+02:00".parse().unwrap();
        assert_eq!(
            across_midnight.to_string(),
            "2025-12-31T23:00:00.000000+00:00"
        );
        let backwards: DateTime = "2025-12-31T23:30:00.000000-02:00".parse().unwrap();
        assert_eq!(backwards.to_string(), "2026-01-01T01:30:00.000000+00:00");
    }

    #[test]
    fn keeps_naive_datetimes_offsetless() {
        let parsed: DateTime = "2026-01-02T00:02:03.000000".parse().unwrap();
        assert_eq!(parsed.to_string(), "2026-01-02T00:02:03.000000");
        assert!(!parsed.is_coordinated());
    }

    #[test]
    fn round_trips_every_day_of_a_leap_year() {
        for month in 1..=12u8 {
            for day in 1..=31u8 {
                if let Ok(date) = Date::new(2024, month, day) {
                    let text = date.to_string();
                    assert_eq!(text.parse::<Date>().unwrap(), date);
                }
            }
        }
    }

    #[test]
    fn rejects_malformed_temporal_text() {
        for text in [
            "2026-1-02",
            "2026/01/02",
            "20260102",
            "2026-01-02x",
            "+026-01-02",
        ] {
            assert!(text.parse::<Date>().is_err(), "must reject {text}");
        }
        for text in ["1:02:03", "01:02:03.", "01:02:03.1234567", "01:02:0a"] {
            assert!(text.parse::<Time>().is_err(), "must reject {text}");
        }
        for text in ["2026-01-02 00:02:03", "2026-01-02T00:02:03+5", "2026-01-02"] {
            assert!(text.parse::<DateTime>().is_err(), "must reject {text}");
        }
    }

    #[test]
    fn rejects_offsets_that_leave_the_supported_year_range() {
        let earliest = DateTime::with_offset(
            Date::new(1, 1, 1).unwrap(),
            Time::new(0, 0, 0, 0).unwrap(),
            3_600,
        );
        assert!(earliest.is_err());
        let latest = DateTime::with_offset(
            Date::new(9999, 12, 31).unwrap(),
            Time::new(23, 0, 0, 0).unwrap(),
            -7_200,
        );
        assert!(latest.is_err());
    }
}
