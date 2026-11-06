package flyrail

import "testing"

func mustDate(t *testing.T, year, month, day int) Date {
	t.Helper()
	d, err := NewDate(year, month, day)
	if err != nil {
		t.Fatal(err)
	}
	return d
}

func mustTime(t *testing.T, hour, minute, second, microsecond int) Time {
	t.Helper()
	value, err := NewTime(hour, minute, second, microsecond)
	if err != nil {
		t.Fatal(err)
	}
	return value
}

func TestFormatsDatesAndTimesLikePythonIsoformat(t *testing.T) {
	cases := map[string]string{
		mustDate(t, 2026, 1, 2).String():          "2026-01-02",
		mustDate(t, 7, 12, 31).String():           "0007-12-31",
		mustTime(t, 1, 2, 3, 0).String():          "01:02:03.000000",
		mustTime(t, 23, 59, 59, 999_999).String(): "23:59:59.999999",
	}
	for got, expected := range cases {
		if got != expected {
			t.Errorf("got %s, want %s", got, expected)
		}
	}
}

func TestRejectsOutOfRangeComponentsWithReferenceMessages(t *testing.T) {
	cases := []struct {
		err     error
		message string
	}{
		{errorOf(NewDate(2026, 2, 29)), "date day is out of range for its month"},
		{errorOf(NewDate(1900, 2, 29)), "date day is out of range for its month"},
		{errorOf(NewDate(0, 1, 1)), "date year must be 1-9999"},
		{errorOf(NewDate(2026, 13, 1)), "date month must be 1-12"},
		{errorOf(NewTime(24, 0, 0, 0)), "time hour must be 0-23"},
		{errorOf(NewTime(0, 60, 0, 0)), "time minute must be 0-59"},
		{errorOf(NewTime(0, 0, 60, 0)), "time second must be 0-59"},
		{errorOf(NewTime(0, 0, 0, 1_000_000)), "time microsecond must be 0-999999"},
		{errorOf(NewTime(-1, 0, 0, 0)), "time hour must be 0-23"},
	}
	for _, c := range cases {
		if c.err == nil || c.err.Error() != c.message {
			t.Errorf("error = %v, want %q", c.err, c.message)
		}
	}
	if got := mustDate(t, 2024, 2, 29).String(); got != "2024-02-29" {
		t.Errorf("leap day = %s", got)
	}
	if got := mustDate(t, 2000, 2, 29).String(); got != "2000-02-29" {
		t.Errorf("leap day = %s", got)
	}
}

func TestNormalizesOffsetDateTimesToUTC(t *testing.T) {
	cases := map[string]string{
		"2026-01-02T05:02:03.000000+05:00": "2026-01-02T00:02:03.000000+00:00",
		"2026-01-01T01:00:00.000000+02:00": "2025-12-31T23:00:00.000000+00:00",
		"2025-12-31T23:30:00.000000-02:00": "2026-01-01T01:30:00.000000+00:00",
		"2026-01-02T00:02:03Z":             "2026-01-02T00:02:03.000000+00:00",
		"2026-01-02T00:02:03+0130":         "2026-01-01T22:32:03.000000+00:00",
		"2026-01-02T00:02:03-01:00:30":     "2026-01-02T01:02:33.000000+00:00",
		"2026-01-02T00:02":                 "2026-01-02T00:02:00.000000",
		"2026-01-02T00:02:03.5":            "2026-01-02T00:02:03.500000",
	}
	for input, expected := range cases {
		parsed, err := ParseDateTime(input)
		if err != nil || parsed.String() != expected {
			t.Errorf("ParseDateTime(%s) = %s, %v, want %s", input, parsed, err, expected)
		}
	}
	naive, err := ParseDateTime("2026-01-02T00:02:03.000000")
	if err != nil || naive.IsUTC() || naive.String() != "2026-01-02T00:02:03.000000" {
		t.Fatalf("naive = %s, %v", naive, err)
	}
	shifted, err := NewDateTimeWithOffset(mustDate(t, 2026, 1, 1), mustTime(t, 1, 0, 0, 0), 3_600)
	if err != nil || shifted != NewUTCDateTime(mustDate(t, 2026, 1, 1), mustTime(t, 0, 0, 0, 0)) {
		t.Fatalf("shifted = %s, %v", shifted, err)
	}
}

func TestRoundTripsEveryDayOfALeapYear(t *testing.T) {
	for month := 1; month <= 12; month++ {
		for day := 1; day <= 31; day++ {
			date, err := NewDate(2024, month, day)
			if err != nil {
				continue
			}
			parsed, err := ParseDate(date.String())
			if err != nil || parsed != date {
				t.Errorf("ParseDate(%s) = %s, %v", date, parsed, err)
			}
		}
	}
}

func TestRejectsMalformedTemporalText(t *testing.T) {
	dates := map[string]string{
		"2026-1-02":        "date must be written as YYYY-MM-DD",
		"2026/01/02":       "date must be written as YYYY-MM-DD",
		"20260102":         "date must be written as YYYY-MM-DD",
		"2026-01-02x":      "date must be written as YYYY-MM-DD",
		"+026-01-02":       "temporal value must use ASCII digits",
		"2026-\xc3\xa9-02": "temporal value must use ASCII digits",
	}
	for text, message := range dates {
		if _, err := ParseDate(text); err == nil || err.Error() != message {
			t.Errorf("ParseDate(%q) = %v, want %q", text, err, message)
		}
	}
	times := map[string]string{
		"1:02:03":          "time must be written as HH:MM[:SS[.ffffff]]",
		"01:02:03.":        "time fraction must have 1-6 digits",
		"01:02:03.1234567": "time fraction must have 1-6 digits",
		"01:02:0a":         "temporal value must use ASCII digits",
		"01:02:03,5":       "time must be written as HH:MM[:SS[.ffffff]]",
		"01:0\xc3\xa9":     "temporal value is truncated",
	}
	for text, message := range times {
		if _, err := ParseTime(text); err == nil || err.Error() != message {
			t.Errorf("ParseTime(%q) = %v, want %q", text, err, message)
		}
	}
	datetimes := map[string]string{
		"2026-01-02 00:02:03":    "datetime must separate date and time with T",
		"2026-01-02T00:02:03+5":  "datetime offset must be written as ±HH:MM",
		"2026-01-02":             "datetime must separate date and time with T",
		"2026-01-02T00:02:03+24": "datetime offset must be less than one day",
	}
	for text, message := range datetimes {
		if _, err := ParseDateTime(text); err == nil || err.Error() != message {
			t.Errorf("ParseDateTime(%q) = %v, want %q", text, err, message)
		}
	}
}

func TestRejectsOffsetsThatLeaveTheSupportedYearRange(t *testing.T) {
	if _, err := NewDateTimeWithOffset(mustDate(t, 1, 1, 1), mustTime(t, 0, 0, 0, 0), 3_600); err == nil ||
		err.Error() != "datetime is out of the supported range" {
		t.Errorf("earliest = %v", err)
	}
	if _, err := NewDateTimeWithOffset(mustDate(t, 9999, 12, 31), mustTime(t, 23, 0, 0, 0), -7_200); err == nil ||
		err.Error() != "datetime is out of the supported range" {
		t.Errorf("latest = %v", err)
	}
	if _, err := NewDateTimeWithOffset(Date{}, mustTime(t, 0, 0, 0, 0), 0); err == nil || err.Error() != "date year must be 1-9999" {
		t.Errorf("zero date = %v", err)
	}
}
