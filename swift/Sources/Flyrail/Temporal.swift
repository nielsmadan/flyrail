import Foundation

public struct CalendarDate: Sendable, Hashable, Comparable {
    public let year: UInt16
    public let month: UInt8
    public let day: UInt8

    public init(year: UInt16, month: UInt8, day: UInt8) throws {
        guard (1 ... 9999).contains(year) else {
            throw FlyrailValueError("date year must be 1-9999")
        }
        guard (1 ... 12).contains(month) else {
            throw FlyrailValueError("date month must be 1-12")
        }
        guard day >= 1, day <= daysInMonth(year: year, month: month) else {
            throw FlyrailValueError("date day is out of range for its month")
        }
        self.year = year
        self.month = month
        self.day = day
    }

    public init(_ text: String) throws {
        let bytes = Array(text.utf8)
        guard bytes.count == 10, bytes[4] == 45, bytes[7] == 45 else {
            throw FlyrailValueError("date must be written as YYYY-MM-DD")
        }
        try self.init(
            year: UInt16(asciiDigits(bytes[0 ..< 4])),
            month: UInt8(asciiDigits(bytes[5 ..< 7])),
            day: UInt8(asciiDigits(bytes[8 ..< 10])),
        )
    }

    public var canonicalString: String {
        String(format: "%04d-%02d-%02d", year, month, day)
    }

    public static func < (left: Self, right: Self) -> Bool {
        (left.year, left.month, left.day) < (right.year, right.month, right.day)
    }

    fileprivate var daysSinceEpoch: Int64 {
        daysFromCivil(year: Int64(year), month: Int64(month), day: Int64(day))
    }

    fileprivate init(daysSinceEpoch: Int64) throws {
        let parts = civilFromDays(daysSinceEpoch)
        guard (1 ... 9999).contains(parts.year) else {
            throw FlyrailValueError("datetime is out of the supported range")
        }
        try self.init(year: UInt16(parts.year), month: UInt8(parts.month), day: UInt8(parts.day))
    }
}

public struct LocalTime: Sendable, Hashable, Comparable {
    public let hour: UInt8
    public let minute: UInt8
    public let second: UInt8
    public let microsecond: UInt32

    public init(hour: UInt8, minute: UInt8, second: UInt8 = 0, microsecond: UInt32 = 0) throws {
        guard hour <= 23 else {
            throw FlyrailValueError("time hour must be 0-23")
        }
        guard minute <= 59 else {
            throw FlyrailValueError("time minute must be 0-59")
        }
        guard second <= 59 else {
            throw FlyrailValueError("time second must be 0-59")
        }
        guard microsecond <= 999_999 else {
            throw FlyrailValueError("time microsecond must be 0-999999")
        }
        self.hour = hour
        self.minute = minute
        self.second = second
        self.microsecond = microsecond
    }

    public init(_ text: String) throws {
        let bytes = Array(text.utf8)
        guard bytes.count >= 5, bytes[2] == 58 else {
            throw FlyrailValueError("time must be written as HH:MM[:SS[.ffffff]]")
        }
        let hour = try asciiDigits(bytes[0 ..< 2])
        let minute = try asciiDigits(bytes[3 ..< 5])
        var second: UInt32 = 0
        var microsecond: UInt32 = 0
        if bytes.count > 5 {
            guard bytes.count >= 8, bytes[5] == 58 else {
                throw FlyrailValueError("time must be written as HH:MM[:SS[.ffffff]]")
            }
            second = try asciiDigits(bytes[6 ..< 8])
            if bytes.count > 8 {
                guard bytes[8] == 46 else {
                    throw FlyrailValueError("time must be written as HH:MM[:SS[.ffffff]]")
                }
                let fraction = bytes[9...]
                guard (1 ... 6).contains(fraction.count) else {
                    throw FlyrailValueError("time fraction must have 1-6 digits")
                }
                microsecond = try asciiDigits(fraction) * powerOfTen(6 - fraction.count)
            }
        }
        try self.init(
            hour: UInt8(hour),
            minute: UInt8(minute),
            second: UInt8(second),
            microsecond: microsecond,
        )
    }

    public var canonicalString: String {
        String(format: "%02d:%02d:%02d.%06d", hour, minute, second, microsecond)
    }

    public static func < (left: Self, right: Self) -> Bool {
        (left.hour, left.minute, left.second, left.microsecond) <
            (right.hour, right.minute, right.second, right.microsecond)
    }

    fileprivate var secondsOfDay: Int64 {
        Int64(hour) * 3600 + Int64(minute) * 60 + Int64(second)
    }

    fileprivate init(secondsOfDay: Int64, microsecond: UInt32) throws {
        try self.init(
            hour: UInt8(secondsOfDay / 3600),
            minute: UInt8((secondsOfDay % 3600) / 60),
            second: UInt8(secondsOfDay % 60),
            microsecond: microsecond,
        )
    }
}

public struct LocalDateTime: Sendable, Hashable, Comparable {
    public let date: CalendarDate
    public let time: LocalTime

    public init(date: CalendarDate, time: LocalTime) {
        self.date = date
        self.time = time
    }

    public init(_ text: String) throws {
        guard let separator = text.utf8.firstIndex(of: 84) else {
            throw FlyrailValueError("datetime must separate date and time with T")
        }
        let byteOffset = text.utf8.distance(from: text.utf8.startIndex, to: separator)
        let index = text.index(text.startIndex, offsetBy: byteOffset)
        date = try CalendarDate(String(text[..<index]))
        time = try LocalTime(String(text[text.index(after: index)...]))
    }

    public var canonicalString: String {
        "\(date.canonicalString)T\(time.canonicalString)"
    }

    public static func < (left: Self, right: Self) -> Bool {
        left.date == right.date ? left.time < right.time : left.date < right.date
    }
}

public struct OffsetDateTime: Sendable, Hashable, Comparable {
    public let date: CalendarDate
    public let time: LocalTime

    public init(date: CalendarDate, time: LocalTime, offsetSeconds: Int32) throws {
        guard offsetSeconds.magnitude < 86400 else {
            throw FlyrailValueError("datetime offset must be less than one day")
        }
        let instant = date.daysSinceEpoch * 86400 + time.secondsOfDay - Int64(offsetSeconds)
        self.date = try CalendarDate(daysSinceEpoch: instant.floorDividing(by: 86400))
        self.time = try LocalTime(
            secondsOfDay: instant.floorRemainder(dividingBy: 86400),
            microsecond: time.microsecond,
        )
    }

    public init(_ text: String) throws {
        guard let separator = text.utf8.firstIndex(of: 84) else {
            throw FlyrailValueError("datetime must separate date and time with T")
        }
        let offset = text.utf8.distance(from: text.utf8.startIndex, to: separator)
        let separatorIndex = text.index(text.startIndex, offsetBy: offset)
        let parsedDate = try CalendarDate(String(text[..<separatorIndex]))
        let remainder = String(text[text.index(after: separatorIndex)...])
        let split = try splitOffset(remainder)
        guard let offsetSeconds = split.offsetSeconds else {
            throw FlyrailValueError("offset datetime must include an offset")
        }
        try self.init(date: parsedDate, time: LocalTime(split.time), offsetSeconds: offsetSeconds)
    }

    public var canonicalString: String {
        "\(date.canonicalString)T\(time.canonicalString)+00:00"
    }

    public static func < (left: Self, right: Self) -> Bool {
        left.date == right.date ? left.time < right.time : left.date < right.date
    }
}

private func splitOffset(_ text: String) throws -> (time: String, offsetSeconds: Int32?) {
    if text.hasSuffix("Z") {
        return (String(text.dropLast()), 0)
    }
    guard let offsetIndex = text.lastIndex(where: { $0 == "+" || $0 == "-" }) else {
        return (text, nil)
    }
    let time = String(text[..<offsetIndex])
    let offset = String(text[offsetIndex...])
    let sign: Int64 = offset.first == "-" ? -1 : 1
    let body = Array(offset.dropFirst().utf8)
    let components: (UInt32, UInt32, UInt32)
    switch body.count {
    case 2:
        components = try (asciiDigits(body[0 ..< 2]), 0, 0)
    case 4:
        components = try (
            asciiDigits(body[0 ..< 2]),
            asciiDigits(body[2 ..< 4]),
            0,
        )
    case 5 where body[2] == 58:
        components = try (
            asciiDigits(body[0 ..< 2]),
            asciiDigits(body[3 ..< 5]),
            0,
        )
    case 8 where body[2] == 58 && body[5] == 58:
        components = try (
            asciiDigits(body[0 ..< 2]),
            asciiDigits(body[3 ..< 5]),
            asciiDigits(body[6 ..< 8]),
        )
    default:
        throw FlyrailValueError("datetime offset must be written as ±HH:MM")
    }
    let total = sign * (Int64(components.0) * 3600 + Int64(components.1) * 60 + Int64(components.2))
    guard let result = Int32(exactly: total) else {
        throw FlyrailValueError("datetime offset is out of range")
    }
    return (time, result)
}

private func asciiDigits(_ bytes: some Collection<UInt8>) throws -> UInt32 {
    guard !bytes.isEmpty, bytes.allSatisfy({ (48 ... 57).contains($0) }) else {
        throw FlyrailValueError("temporal value must use ASCII digits")
    }
    return try bytes.reduce(0) { value, byte in
        let next = value.multipliedReportingOverflow(by: 10)
        let added = next.partialValue.addingReportingOverflow(UInt32(byte - 48))
        guard !next.overflow, !added.overflow else {
            throw FlyrailValueError("temporal value is out of range")
        }
        return added.partialValue
    }
}

private func powerOfTen(_ exponent: Int) -> UInt32 {
    (0 ..< exponent).reduce(1) { result, _ in result * 10 }
}

private func isLeapYear(_ year: UInt16) -> Bool {
    year.isMultiple(of: 4) && (!year.isMultiple(of: 100) || year.isMultiple(of: 400))
}

private func daysInMonth(year: UInt16, month: UInt8) -> UInt8 {
    switch month {
    case 1, 3, 5, 7, 8, 10, 12:
        31
    case 4, 6, 9, 11:
        30
    case 2 where isLeapYear(year):
        29
    case 2:
        28
    default:
        0
    }
}

private func daysFromCivil(year: Int64, month: Int64, day: Int64) -> Int64 {
    let shiftedYear = month <= 2 ? year - 1 : year
    let era = (shiftedYear >= 0 ? shiftedYear : shiftedYear - 399) / 400
    let yearOfEra = shiftedYear - era * 400
    let shiftedMonth = month > 2 ? month - 3 : month + 9
    let dayOfYear = (153 * shiftedMonth + 2) / 5 + day - 1
    let dayOfEra = yearOfEra * 365 + yearOfEra / 4 - yearOfEra / 100 + dayOfYear
    return era * 146_097 + dayOfEra - 719_468
}

private func civilFromDays(_ days: Int64) -> (year: Int64, month: Int64, day: Int64) {
    let shifted = days + 719_468
    let era = (shifted >= 0 ? shifted : shifted - 146_096) / 146_097
    let dayOfEra = shifted - era * 146_097
    let yearOfEra = (dayOfEra - dayOfEra / 1460 + dayOfEra / 36524 - dayOfEra / 146_096) / 365
    let baseYear = yearOfEra + era * 400
    let dayOfYear = dayOfEra - (365 * yearOfEra + yearOfEra / 4 - yearOfEra / 100)
    let monthPosition = (5 * dayOfYear + 2) / 153
    let day = dayOfYear - (153 * monthPosition + 2) / 5 + 1
    let month = monthPosition < 10 ? monthPosition + 3 : monthPosition - 9
    return (month <= 2 ? baseYear + 1 : baseYear, month, day)
}

private extension Int64 {
    func floorDividing(by divisor: Int64) -> Int64 {
        let quotient = self / divisor
        let remainder = self % divisor
        return remainder < 0 ? quotient - 1 : quotient
    }

    func floorRemainder(dividingBy divisor: Int64) -> Int64 {
        let remainder = self % divisor
        return remainder < 0 ? remainder + divisor : remainder
    }
}
