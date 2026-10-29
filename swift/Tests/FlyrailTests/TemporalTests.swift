@testable import Flyrail
import Testing

@Test
func temporalValuesValidateRangesAndCanonicalize() throws {
    #expect(try CalendarDate(year: 2024, month: 2, day: 29).canonicalString == "2024-02-29")
    #expect(throws: FlyrailValueError.self) {
        try CalendarDate(year: 1900, month: 2, day: 29)
    }
    #expect(try LocalTime("01:02").canonicalString == "01:02:00.000000")
    #expect(try LocalTime("23:59:59.123").canonicalString == "23:59:59.123000")
    #expect(throws: FlyrailValueError.self) {
        try LocalTime("24:00")
    }
}

@Test
func offsetDateTimesNormalizeToCoordinatedTime() throws {
    let shifted = try OffsetDateTime("2026-01-02T05:02:03.000000+05:00")
    #expect(shifted.canonicalString == "2026-01-02T00:02:03.000000+00:00")
    let midnight = try OffsetDateTime("2026-01-01T01:00:00.000000+02:00")
    #expect(midnight.canonicalString == "2025-12-31T23:00:00.000000+00:00")
    #expect(throws: FlyrailValueError.self) {
        try OffsetDateTime("0001-01-01T00:00:00.000000+01:00")
    }
}

@Test
func localDateTimesRemainOffsetless() throws {
    let local = try LocalDateTime("2026-01-02T03:04:05.000006")
    #expect(local.canonicalString == "2026-01-02T03:04:05.000006")
    #expect(throws: FlyrailValueError.self) {
        try LocalDateTime("2026-01-02 03:04:05")
    }
}
