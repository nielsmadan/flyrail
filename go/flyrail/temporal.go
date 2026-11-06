package flyrail

import (
	"fmt"
	"strconv"
	"strings"
)

const secondsPerDay = 86_400

// Date is a proleptic Gregorian calendar date in years 1-9999.
type Date struct {
	year, month, day int
}

// NewDate returns the date year-month-day.
func NewDate(year, month, day int) (Date, error) {
	if year < 1 || year > 9999 {
		return Date{}, newValueError("date year must be 1-9999")
	}
	if month < 1 || month > 12 {
		return Date{}, newValueError("date month must be 1-12")
	}
	if day < 1 || day > daysInMonth(year, month) {
		return Date{}, newValueError("date day is out of range for its month")
	}
	return Date{year: year, month: month, day: day}, nil
}

// Year returns the year.
func (d Date) Year() int { return d.year }

// Month returns the month, 1-12.
func (d Date) Month() int { return d.month }

// Day returns the day of the month.
func (d Date) Day() int { return d.day }

// String returns the date as YYYY-MM-DD.
func (d Date) String() string {
	return fmt.Sprintf("%04d-%02d-%02d", d.year, d.month, d.day)
}

func (d Date) validate() error {
	_, err := NewDate(d.Year(), d.Month(), d.Day())
	return err
}

func (d Date) daysSinceEpoch() int64 {
	return daysFromCivil(int64(d.year), int64(d.month), int64(d.day))
}

func dateFromDaysSinceEpoch(days int64) (Date, error) {
	year, month, day := civilFromDays(days)
	if year < 1 || year > 9999 {
		return Date{}, newValueError("datetime is out of the supported range")
	}
	return NewDate(int(year), int(month), int(day))
}

// ParseDate parses a date written as YYYY-MM-DD.
func ParseDate(s string) (Date, error) {
	if len(s) != 10 || s[4] != '-' || s[7] != '-' {
		return Date{}, newValueError("date must be written as YYYY-MM-DD")
	}
	year, err := digits(s, 0, 4)
	if err != nil {
		return Date{}, err
	}
	month, err := digits(s, 5, 7)
	if err != nil {
		return Date{}, err
	}
	day, err := digits(s, 8, 10)
	if err != nil {
		return Date{}, err
	}
	return NewDate(year, month, day)
}

// Time is a local time of day with microsecond precision.
type Time struct {
	hour, minute, second, microsecond int
}

// NewTime returns the time hour:minute:second.microsecond.
func NewTime(hour, minute, second, microsecond int) (Time, error) {
	if hour < 0 || hour > 23 {
		return Time{}, newValueError("time hour must be 0-23")
	}
	if minute < 0 || minute > 59 {
		return Time{}, newValueError("time minute must be 0-59")
	}
	if second < 0 || second > 59 {
		return Time{}, newValueError("time second must be 0-59")
	}
	if microsecond < 0 || microsecond > 999_999 {
		return Time{}, newValueError("time microsecond must be 0-999999")
	}
	return Time{hour: hour, minute: minute, second: second, microsecond: microsecond}, nil
}

// Hour returns the hour, 0-23.
func (t Time) Hour() int { return t.hour }

// Minute returns the minute, 0-59.
func (t Time) Minute() int { return t.minute }

// Second returns the second, 0-59.
func (t Time) Second() int { return t.second }

// Microsecond returns the microsecond, 0-999999.
func (t Time) Microsecond() int { return t.microsecond }

// String returns the time as HH:MM:SS.ffffff.
func (t Time) String() string {
	return fmt.Sprintf("%02d:%02d:%02d.%06d", t.hour, t.minute, t.second, t.microsecond)
}

func (t Time) validate() error {
	_, err := NewTime(t.Hour(), t.Minute(), t.Second(), t.Microsecond())
	return err
}

func (t Time) secondsOfDay() int64 {
	return int64(t.hour)*3_600 + int64(t.minute)*60 + int64(t.second)
}

// ParseTime parses a time written as HH:MM[:SS[.ffffff]].
func ParseTime(s string) (Time, error) {
	const shape = "time must be written as HH:MM[:SS[.ffffff]]"
	if len(s) < 5 || s[2] != ':' {
		return Time{}, newValueError(shape)
	}
	hour, err := digits(s, 0, 2)
	if err != nil {
		return Time{}, err
	}
	minute, err := digits(s, 3, 5)
	if err != nil {
		return Time{}, err
	}
	second, microsecond := 0, 0
	if len(s) > 5 {
		if s[5] != ':' || len(s) < 8 {
			return Time{}, newValueError(shape)
		}
		if second, err = digits(s, 6, 8); err != nil {
			return Time{}, err
		}
		if len(s) > 8 {
			if s[8] != '.' {
				return Time{}, newValueError(shape)
			}
			fraction := ""
			if isCharBoundary(s, 9) {
				fraction = s[9:]
			}
			if fraction == "" || len(fraction) > 6 {
				return Time{}, newValueError("time fraction must have 1-6 digits")
			}
			value, err := digits(fraction, 0, len(fraction))
			if err != nil {
				return Time{}, err
			}
			scale := 1
			for range 6 - len(fraction) {
				scale *= 10
			}
			microsecond = value * scale
		}
	}
	return NewTime(hour, minute, second, microsecond)
}

// DateTime is a local or UTC date and time; offsets are normalized to UTC on construction.
type DateTime struct {
	date Date
	time Time
	utc  bool
}

// NewLocalDateTime returns a date and time without an offset; it is validated where it is consumed.
func NewLocalDateTime(d Date, t Time) DateTime {
	return DateTime{date: d, time: t}
}

// NewUTCDateTime returns a UTC date and time; it is validated where it is consumed.
func NewUTCDateTime(d Date, t Time) DateTime {
	return DateTime{date: d, time: t, utc: true}
}

// NewDateTimeWithOffset returns the UTC instant of d and t at offsetSeconds east of UTC.
func NewDateTimeWithOffset(d Date, t Time, offsetSeconds int) (DateTime, error) {
	if err := d.validate(); err != nil {
		return DateTime{}, err
	}
	if err := t.validate(); err != nil {
		return DateTime{}, err
	}
	if offsetSeconds <= -secondsPerDay || offsetSeconds >= secondsPerDay {
		return DateTime{}, newValueError("datetime offset must be less than one day")
	}
	instant := d.daysSinceEpoch()*secondsPerDay + t.secondsOfDay() - int64(offsetSeconds)
	days := floorDiv(instant, secondsPerDay)
	seconds := instant - days*secondsPerDay
	date, err := dateFromDaysSinceEpoch(days)
	if err != nil {
		return DateTime{}, err
	}
	timeOfDay, err := NewTime(int(seconds/3_600), int(seconds%3_600/60), int(seconds%60), t.Microsecond())
	if err != nil {
		return DateTime{}, err
	}
	return DateTime{date: date, time: timeOfDay, utc: true}, nil
}

// Date returns the date.
func (dt DateTime) Date() Date { return dt.date }

// Time returns the time of day.
func (dt DateTime) Time() Time { return dt.time }

// IsUTC reports whether the value is a UTC instant rather than a local date and time.
func (dt DateTime) IsUTC() bool { return dt.utc }

// String returns the value as YYYY-MM-DDTHH:MM:SS.ffffff, followed by +00:00 when UTC.
func (dt DateTime) String() string {
	text := dt.date.String() + "T" + dt.time.String()
	if dt.utc {
		text += "+00:00"
	}
	return text
}

func (dt DateTime) validate() error {
	if err := dt.date.validate(); err != nil {
		return err
	}
	return dt.time.validate()
}

// ParseDateTime parses a date and time separated by T with an optional Z or ±HH[:MM[:SS]] offset.
func ParseDateTime(s string) (DateTime, error) {
	dateText, rest, found := strings.Cut(s, "T")
	if !found {
		return DateTime{}, newValueError("datetime must separate date and time with T")
	}
	date, err := ParseDate(dateText)
	if err != nil {
		return DateTime{}, err
	}
	timeText, offset, hasOffset, err := splitOffset(rest)
	if err != nil {
		return DateTime{}, err
	}
	timeOfDay, err := ParseTime(timeText)
	if err != nil {
		return DateTime{}, err
	}
	switch {
	case !hasOffset:
		return NewLocalDateTime(date, timeOfDay), nil
	case offset == 0:
		return NewUTCDateTime(date, timeOfDay), nil
	}
	return NewDateTimeWithOffset(date, timeOfDay, offset)
}

func splitOffset(text string) (string, int, bool, error) {
	if timeText, ok := strings.CutSuffix(text, "Z"); ok {
		return timeText, 0, true, nil
	}
	position := strings.LastIndexAny(text, "+-")
	if position < 0 {
		return text, 0, false, nil
	}
	timeText, offset := text[:position], text[position:]
	sign := 1
	if offset[0] == '-' {
		sign = -1
	}
	body := offset[1:]
	var hours, minutes, seconds int
	var err error
	switch {
	case len(body) == 2:
		hours, err = digits(body, 0, 2)
	case len(body) == 4:
		if hours, err = digits(body, 0, 2); err == nil {
			minutes, err = digits(body, 2, 4)
		}
	case len(body) == 5 && body[2] == ':':
		if hours, err = digits(body, 0, 2); err == nil {
			minutes, err = digits(body, 3, 5)
		}
	case len(body) == 8 && body[2] == ':' && body[5] == ':':
		if hours, err = digits(body, 0, 2); err == nil {
			if minutes, err = digits(body, 3, 5); err == nil {
				seconds, err = digits(body, 6, 8)
			}
		}
	default:
		return "", 0, false, newValueError("datetime offset must be written as ±HH:MM")
	}
	if err != nil {
		return "", 0, false, err
	}
	return timeText, sign * (hours*3_600 + minutes*60 + seconds), true, nil
}

func isCharBoundary(s string, index int) bool {
	return index == len(s) || (index < len(s) && (s[index] < 0x80 || s[index] >= 0xC0))
}

func digits(s string, start, end int) (int, error) {
	if end > len(s) || !isCharBoundary(s, start) || !isCharBoundary(s, end) {
		return 0, newValueError("temporal value is truncated")
	}
	text := s[start:end]
	if text == "" || strings.IndexFunc(text, func(r rune) bool { return r < '0' || r > '9' }) >= 0 {
		return 0, newValueError("temporal value must use ASCII digits")
	}
	value, err := strconv.Atoi(text)
	if err != nil || value > 0xFFFF_FFFF {
		return 0, newValueError("temporal value is out of range")
	}
	return value, nil
}

func isLeapYear(year int) bool {
	return year%4 == 0 && (year%100 != 0 || year%400 == 0)
}

func daysInMonth(year, month int) int {
	switch month {
	case 1, 3, 5, 7, 8, 10, 12:
		return 31
	case 4, 6, 9, 11:
		return 30
	case 2:
		if isLeapYear(year) {
			return 29
		}
		return 28
	}
	return 0
}

func floorDiv(a, b int64) int64 {
	quotient := a / b
	if a%b != 0 && (a < 0) != (b < 0) {
		quotient--
	}
	return quotient
}

func daysFromCivil(year, month, day int64) int64 {
	if month <= 2 {
		year--
	}
	era := year
	if era < 0 {
		era -= 399
	}
	era /= 400
	yearOfEra := year - era*400
	shifted := month + 9
	if month > 2 {
		shifted = month - 3
	}
	dayOfYear := (153*shifted+2)/5 + day - 1
	dayOfEra := yearOfEra*365 + yearOfEra/4 - yearOfEra/100 + dayOfYear
	return era*146_097 + dayOfEra - 719_468
}

func civilFromDays(days int64) (int64, int64, int64) {
	shifted := days + 719_468
	era := shifted
	if era < 0 {
		era -= 146_096
	}
	era /= 146_097
	dayOfEra := shifted - era*146_097
	yearOfEra := (dayOfEra - dayOfEra/1_460 + dayOfEra/36_524 - dayOfEra/146_096) / 365
	year := yearOfEra + era*400
	dayOfYear := dayOfEra - (365*yearOfEra + yearOfEra/4 - yearOfEra/100)
	monthPosition := (5*dayOfYear + 2) / 153
	day := dayOfYear - (153*monthPosition+2)/5 + 1
	month := monthPosition - 9
	if monthPosition < 10 {
		month = monthPosition + 3
	}
	if month <= 2 {
		year++
	}
	return year, month, day
}
