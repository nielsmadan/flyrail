import Foundation
import SystemPackage

public struct FlyrailValueError: Error, Sendable, Equatable, CustomStringConvertible {
    public let message: String

    public init(_ message: String) {
        self.message = message
    }

    public var description: String {
        message
    }
}

public struct ResourcePath: Sendable, Hashable {
    private let value: FilePath
    package let doubleSlashRoot: Bool

    public init(_ value: String) throws {
        guard !value.utf8.contains(0) else {
            throw FlyrailValueError("resource path cannot contain NUL")
        }
        self.value = FilePath(value)
        doubleSlashRoot = value.utf8.starts(with: [47, 47]) && !value.utf8.starts(with: [47, 47, 47])
    }

    public init(platformBytes: [UInt8]) throws {
        guard !platformBytes.contains(0) else {
            throw FlyrailValueError("resource path cannot contain NUL")
        }
        var terminated = platformBytes.map { CChar(bitPattern: $0) }
        terminated.append(0)
        value = FilePath(platformString: terminated)
        doubleSlashRoot = platformBytes.starts(with: [47, 47]) && !platformBytes.starts(with: [47, 47, 47])
    }

    package init(filePath: FilePath, doubleSlashRoot: Bool = false) {
        value = filePath
        self.doubleSlashRoot = doubleSlashRoot
    }

    public var platformBytes: [UInt8] {
        let bytes = value.withPlatformString { pointer in
            let count = strlen(pointer)
            return UnsafeBufferPointer(start: pointer, count: count).map { UInt8(bitPattern: $0) }
        }
        return doubleSlashRoot ? [47] + bytes : bytes
    }

    public var displayString: String {
        String(decoding: platformBytes, as: UTF8.self)
    }

    package var filePath: FilePath {
        value
    }
}

public struct Binary64: Sendable, Hashable {
    public let bitPattern: UInt64

    public init(_ value: Double) throws {
        try self.init(bitPattern: value.bitPattern)
    }

    public init(bitPattern: UInt64) throws {
        guard bitPattern & 0x7FF0_0000_0000_0000 != 0x7FF0_0000_0000_0000 else {
            throw FlyrailValueError("floats must be finite")
        }
        self.bitPattern = bitPattern
    }

    public var value: Double {
        Double(bitPattern: bitPattern)
    }

    public var pythonHex: String {
        if value == 0 {
            return bitPattern >> 63 == 1 ? "-0x0.0p+0" : "0x0.0p+0"
        }
        let sign = bitPattern >> 63 == 1 ? "-" : ""
        let biased = Int((bitPattern >> 52) & 0x7FF)
        let fraction = bitPattern & 0x000F_FFFF_FFFF_FFFF
        let lead = biased == 0 ? 0 : 1
        let exponent = biased == 0 ? -1022 : biased - 1023
        let exponentText = exponent >= 0 ? "+\(exponent)" : "\(exponent)"
        return String(format: "%@0x%x.%013llxp%@", sign, lead, fraction, exponentText)
    }

    package init(pythonHex text: String) throws {
        try self.init(parsePythonHex(text))
        guard pythonHex == text else {
            throw FlyrailValueError("semantic scalar encoding is not canonical")
        }
    }
}

public enum ScalarValue: Sendable, Hashable {
    case null
    case boolean(Bool)
    case integer(Int64)
    case float(Binary64)
    case text(String)
    case date(CalendarDate)
    case time(LocalTime)
    case dateTime(LocalDateTime)
    case offsetDateTime(OffsetDateTime)

    public static func == (left: Self, right: Self) -> Bool {
        switch (left, right) {
        case (.null, .null):
            true
        case let (.boolean(left), .boolean(right)):
            left == right
        case let (.integer(left), .integer(right)):
            left == right
        case let (.float(left), .float(right)):
            left == right
        case let (.text(left), .text(right)):
            exactTextEqual(left, right)
        case let (.date(left), .date(right)):
            left == right
        case let (.time(left), .time(right)):
            left == right
        case let (.dateTime(left), .dateTime(right)):
            left == right
        case let (.offsetDateTime(left), .offsetDateTime(right)):
            left == right
        default:
            false
        }
    }

    public func hash(into hasher: inout Hasher) {
        switch self {
        case .null:
            hasher.combine(0)
        case let .boolean(value):
            hasher.combine(1)
            hasher.combine(value)
        case let .integer(value):
            hasher.combine(2)
            hasher.combine(value)
        case let .float(value):
            hasher.combine(3)
            hasher.combine(value)
        case let .text(value):
            hasher.combine(4)
            hashExactText(value, into: &hasher)
        case let .date(value):
            hasher.combine(5)
            hasher.combine(value)
        case let .time(value):
            hasher.combine(6)
            hasher.combine(value)
        case let .dateTime(value):
            hasher.combine(7)
            hasher.combine(value)
        case let .offsetDateTime(value):
            hasher.combine(8)
            hasher.combine(value)
        }
    }
}

public indirect enum SemanticValue: Sendable, Hashable {
    case scalar(ScalarValue)
    case array([SemanticValue])
    case object([(String, SemanticValue)])

    public init(object items: [(String, SemanticValue)]) throws {
        let sorted = items.sorted { utf8Less($0.0, $1.0) }
        guard !zip(sorted, sorted.dropFirst()).contains(where: { exactTextEqual($0.0.0, $0.1.0) }) else {
            throw FlyrailValueError("duplicate object key")
        }
        self = .object(sorted)
    }

    public static func == (left: Self, right: Self) -> Bool {
        switch (left, right) {
        case let (.scalar(left), .scalar(right)):
            left == right
        case let (.array(left), .array(right)):
            left == right
        case let (.object(left), .object(right)):
            left.count == right.count && zip(sortedObjectItems(left), sortedObjectItems(right)).allSatisfy {
                exactTextEqual($0.0.0, $0.1.0) && $0.0.1 == $0.1.1
            }
        default:
            false
        }
    }

    public func hash(into hasher: inout Hasher) {
        switch self {
        case let .scalar(value):
            hasher.combine(0)
            hasher.combine(value)
        case let .array(values):
            hasher.combine(1)
            values.forEach { hasher.combine($0) }
        case let .object(items):
            hasher.combine(2)
            for item in sortedObjectItems(items) {
                hashExactText(item.0, into: &hasher)
                hasher.combine(item.1)
            }
        }
    }

    public func semanticBytes() throws -> Data {
        try JSONWriter.canonicalData(semanticRecord())
    }

    public init(semanticBytes: Data) throws {
        let parsed = try JSONParser.parse(semanticBytes)
        try self.init(record: parsed)
    }

    package func semanticRecord() throws -> JSONValue {
        switch self {
        case let .scalar(value):
            try value.semanticRecord()
        case let .array(values):
            try .array([
                .string("array"),
                .array(values.map { try $0.semanticRecord() }),
            ])
        case let .object(items):
            try .array([
                .string("object"),
                .array(canonicalObjectItems(items).map {
                    try .array([.string($0.0), $0.1.semanticRecord()])
                }),
            ])
        }
    }

    private init(record: JSONValue) throws {
        guard case let .array(entries) = record,
              !entries.isEmpty,
              case let .string(kind) = entries[0]
        else {
            throw FlyrailValueError("semantic values must be tagged arrays")
        }
        if kind == "null", entries.count == 1 {
            self = .scalar(.null)
            return
        }
        guard entries.count == 2 else {
            throw FlyrailValueError("semantic value has an invalid field count")
        }
        let raw = entries[1]
        switch (kind, raw) {
        case let ("array", .array(items)):
            self = try .array(items.map(Self.init(record:)))
        case let ("object", .array(members)):
            let pairs = try members.map { member -> (String, SemanticValue) in
                guard case let .array(pair) = member,
                      pair.count == 2,
                      case let .string(name) = pair[0]
                else {
                    throw FlyrailValueError("semantic object members must be name/value pairs")
                }
                return try (name, Self(record: pair[1]))
            }
            try self.init(object: pairs)
        case let ("boolean", .boolean(value)):
            self = .scalar(.boolean(value))
        case let (_, .string(payload)):
            let scalar = try ScalarValue(kind: kind, payload: payload)
            self = .scalar(scalar)
            guard try semanticRecord() == record else {
                throw FlyrailValueError("semantic scalar encoding is not canonical")
            }
        default:
            throw FlyrailValueError("semantic scalar payload has the wrong type")
        }
    }
}

private extension ScalarValue {
    func semanticRecord() throws -> JSONValue {
        switch self {
        case .null:
            .array([.string("null")])
        case let .boolean(value):
            .array([.string("boolean"), .boolean(value)])
        case let .integer(value):
            .array([.string("integer"), .string(String(value))])
        case let .float(value):
            .array([.string("float"), .string(value.pythonHex)])
        case let .text(value):
            .array([.string("string"), .string(value)])
        case let .date(value):
            .array([.string("date"), .string(value.canonicalString)])
        case let .time(value):
            .array([.string("time"), .string(value.canonicalString)])
        case let .dateTime(value):
            .array([.string("datetime"), .string(value.canonicalString)])
        case let .offsetDateTime(value):
            .array([.string("datetime"), .string(value.canonicalString)])
        }
    }

    init(kind: String, payload: String) throws {
        switch kind {
        case "string":
            self = .text(payload)
        case "integer":
            self = try .integer(parseSemanticInteger(payload))
        case "float":
            self = try .float(Binary64(pythonHex: payload))
        case "date":
            self = try .date(CalendarDate(payload))
        case "time":
            self = try .time(LocalTime(payload))
        case "datetime":
            if payload.hasSuffix("+00:00") {
                self = try .offsetDateTime(OffsetDateTime(payload))
            } else {
                self = try .dateTime(LocalDateTime(payload))
            }
        default:
            throw FlyrailValueError("unknown semantic scalar kind")
        }
    }
}

package func exactTextEqual(_ left: String, _ right: String) -> Bool {
    left.utf8.elementsEqual(right.utf8)
}

package func utf8Less(_ left: String, _ right: String) -> Bool {
    left.utf8.lexicographicallyPrecedes(right.utf8)
}

package func hashExactText(_ value: String, into hasher: inout Hasher) {
    hasher.combine(value.utf8.count)
    value.utf8.forEach { hasher.combine($0) }
}

private func canonicalObjectItems(
    _ items: [(String, SemanticValue)],
) throws -> [(String, SemanticValue)] {
    let sorted = items.sorted { utf8Less($0.0, $1.0) }
    guard !zip(sorted, sorted.dropFirst()).contains(where: { exactTextEqual($0.0.0, $0.1.0) }) else {
        throw FlyrailValueError("duplicate object key")
    }
    return sorted
}

private func sortedObjectItems(
    _ items: [(String, SemanticValue)],
) -> [(String, SemanticValue)] {
    items.sorted { utf8Less($0.0, $1.0) }
}

private func parseSemanticInteger(_ text: String) throws -> Int64 {
    let body = text.first == "-" || text.first == "+" ? text.dropFirst() : text[...]
    guard !body.isEmpty, body.allSatisfy(\.isASCIIWholeNumber) else {
        throw FlyrailValueError("invalid literal for int() with base 10: \(pythonStringRepr(text))")
    }
    guard let value = Int64(text) else {
        throw FlyrailValueError("integers must fit signed 64 bits")
    }
    return value
}

private func parsePythonHex(_ text: String) throws -> Double {
    let malformed = FlyrailValueError("float hexadecimal literal is malformed")
    let negative = text.first == "-"
    let unsigned = text.first == "-" || text.first == "+" ? text.dropFirst() : text[...]
    guard unsigned.hasPrefix("0x") || unsigned.hasPrefix("0X") else {
        throw malformed
    }
    let literal = unsigned.dropFirst(2)
    let exponentParts = literal.split(
        omittingEmptySubsequences: false,
        whereSeparator: { $0 == "p" || $0 == "P" },
    )
    guard exponentParts.count <= 2 else {
        throw malformed
    }
    let digitParts = exponentParts[0].split(separator: ".", omittingEmptySubsequences: false)
    guard digitParts.count <= 2 else {
        throw malformed
    }
    let whole = digitParts[0]
    let fraction = digitParts.count == 2 ? digitParts[1] : Substring()
    guard !whole.isEmpty || !fraction.isEmpty else {
        throw malformed
    }
    guard whole.count + fraction.count <= 16 else {
        throw FlyrailValueError("float hexadecimal literal has too many digits")
    }
    var mantissa: UInt64 = 0
    for digit in whole + fraction {
        guard let nibble = digit.hexDigitValue else {
            throw malformed
        }
        mantissa = mantissa * 16 + UInt64(nibble)
    }
    let exponent: Int32
    if exponentParts.count == 2 {
        let text = exponentParts[1]
        let magnitude = text.first == "-" || text.first == "+" ? text.dropFirst() : text[...]
        guard !magnitude.isEmpty, magnitude.allSatisfy(\Character.isASCIIWholeNumber) else {
            throw malformed
        }
        guard let parsed = Int32(text) else {
            throw FlyrailValueError("float exponent is out of range")
        }
        exponent = parsed
    } else {
        exponent = 0
    }
    let scale = Int64(exponent) - 4 * Int64(fraction.count)
    let parsed = scaleByPowerOfTwo(Double(mantissa), exponent: scale)
    guard parsed.isFinite else {
        throw FlyrailValueError("float hexadecimal literal is out of range")
    }
    return negative ? -parsed : parsed
}

private func scaleByPowerOfTwo(_ value: Double, exponent: Int64) -> Double {
    let up = Double(bitPattern: 0x7FE0_0000_0000_0000)
    let down = Double(bitPattern: 0x0360_0000_0000_0000)
    var result = value
    var remaining = min(4096, max(-4096, exponent))
    while remaining > 1023 {
        result *= up
        remaining -= 1023
        if !result.isFinite {
            return result
        }
    }
    while remaining < -1022 {
        result *= down
        remaining += 969
        if result == 0 {
            return result
        }
    }
    let biased = UInt64(1023 + remaining)
    return result * Double(bitPattern: biased << 52)
}

package func pythonStringRepr(_ text: String) -> String {
    let quote: Character = text.contains("'") && !text.contains("\"") ? "\"" : "'"
    var output = String(quote)
    for scalar in text.unicodeScalars {
        switch scalar.value {
        case 0x5C:
            output += "\\\\"
        case 0x0A:
            output += "\\n"
        case 0x0D:
            output += "\\r"
        case 0x09:
            output += "\\t"
        case 0 ..< 0x20, 0x7F:
            output += String(format: "\\x%02x", scalar.value)
        default:
            let category = scalar.properties.generalCategory
            if scalar.value != 0x20,
               [.control, .format, .surrogate, .privateUse, .unassigned,
                .spaceSeparator, .lineSeparator, .paragraphSeparator].contains(category)
            {
                let format = scalar.value <= 0xFF ? "\\x%02x" : scalar.value <= 0xFFFF ? "\\u%04x" : "\\U%08x"
                output += String(format: format, scalar.value)
            } else if Character(scalar) == quote {
                output.append("\\")
                output.unicodeScalars.append(scalar)
            } else {
                output.unicodeScalars.append(scalar)
            }
        }
    }
    output.append(quote)
    return output
}

private extension Character {
    var isASCIIWholeNumber: Bool {
        asciiValue.map { (48 ... 57).contains($0) } ?? false
    }
}
