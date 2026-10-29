import Foundation

package struct JSONLimits: Sendable, Hashable {
    package var maximumInputBytes: Int
    package var maximumDepth: Int
    package var maximumTokens: Int
    package var maximumStringBytes: Int

    package static let `default` = Self(
        maximumInputBytes: 16 * 1024 * 1024,
        maximumDepth: 512,
        maximumTokens: 1_000_000,
        maximumStringBytes: 8 * 1024 * 1024
    )

    package init(
        maximumInputBytes: Int,
        maximumDepth: Int,
        maximumTokens: Int,
        maximumStringBytes: Int
    ) {
        self.maximumInputBytes = maximumInputBytes
        self.maximumDepth = maximumDepth
        self.maximumTokens = maximumTokens
        self.maximumStringBytes = maximumStringBytes
    }
}

package enum JSONValue: Sendable, Hashable {
    case null
    case boolean(Bool)
    case number(String)
    case string(String)
    case array([JSONValue])
    case object([(String, JSONValue)])

    package static func == (left: Self, right: Self) -> Bool {
        switch (left, right) {
        case (.null, .null):
            true
        case let (.boolean(left), .boolean(right)):
            left == right
        case let (.number(left), .number(right)):
            exactTextEqual(left, right)
        case let (.string(left), .string(right)):
            exactTextEqual(left, right)
        case let (.array(left), .array(right)):
            left == right
        case let (.object(left), .object(right)):
            left.count == right.count && zip(left, right).allSatisfy {
                exactTextEqual($0.0.0, $0.1.0) && $0.0.1 == $0.1.1
            }
        default:
            false
        }
    }

    package func hash(into hasher: inout Hasher) {
        switch self {
        case .null:
            hasher.combine(0)
        case let .boolean(value):
            hasher.combine(1)
            hasher.combine(value)
        case let .number(value):
            hasher.combine(2)
            value.utf8.forEach { hasher.combine($0) }
        case let .string(value):
            hasher.combine(3)
            value.utf8.forEach { hasher.combine($0) }
        case let .array(values):
            hasher.combine(4)
            values.forEach { hasher.combine($0) }
        case let .object(items):
            hasher.combine(5)
            for item in items {
                item.0.utf8.forEach { hasher.combine($0) }
                hasher.combine(item.1)
            }
        }
    }

    package var signedInteger: Int64? {
        guard case let .number(token) = self, !token.contains("."), !token.contains("e"), !token.contains("E") else {
            return nil
        }
        return Int64(token)
    }

    package var unsignedDecimalToken: String? {
        guard case let .number(token) = self,
              token.first != "-",
              !token.contains("."),
              !token.contains("e"),
              !token.contains("E")
        else {
            return nil
        }
        return token
    }
}

package enum JSONParser {
    package static func parse(
        _ data: Data,
        limits: JSONLimits = .default
    ) throws -> JSONValue {
        guard data.count <= limits.maximumInputBytes else {
            throw FlyrailValueError("JSON input exceeds the byte limit")
        }
        guard String(data: data, encoding: .utf8) != nil else {
            throw FlyrailValueError("JSON must be UTF-8")
        }
        var parser = Parser(bytes: Array(data), limits: limits)
        parser.skipWhitespace()
        let result = try parser.parseValue(depth: 0)
        parser.skipWhitespace()
        guard parser.position == parser.bytes.count else {
            throw FlyrailValueError("JSON has trailing content")
        }
        return result
    }

    private struct Parser {
        let bytes: [UInt8]
        let limits: JSONLimits
        var position = 0
        var tokenCount = 0

        private struct Frame {
            var items: [JSONValue] = []
            var fields: [(String, JSONValue)] = []
            var keys: Set<[UInt8]> = []
            var key: String?
        }

        mutating func parseValue(depth: Int) throws -> JSONValue {
            var frames: [Frame] = []
            reading: while true {
                guard depth + frames.count <= limits.maximumDepth else {
                    throw FlyrailValueError("JSON nesting is too deep")
                }
                try consumeToken()
                var value: JSONValue
                if peek == 123 || peek == 91 {
                    let object = peek == 123
                    position += 1
                    skipWhitespace()
                    if peek == (object ? 125 : 93) {
                        position += 1
                        value = object ? .object([]) : .array([])
                    } else {
                        try frames.append(Frame(key: object ? parseKey() : nil))
                        continue reading
                    }
                } else {
                    value = try parseScalar()
                }
                while var frame = frames.popLast() {
                    if let key = frame.key {
                        guard frame.keys.insert(Array(key.utf8)).inserted else {
                            throw FlyrailValueError("duplicate JSON field: \(key)")
                        }
                        frame.fields.append((key, value))
                    } else {
                        frame.items.append(value)
                    }
                    skipWhitespace()
                    if peek == 44 {
                        position += 1
                        skipWhitespace()
                        if frame.key != nil { frame.key = try parseKey() }
                        frames.append(frame)
                        continue reading
                    }
                    guard peek == (frame.key == nil ? 93 : 125) else {
                        throw FlyrailValueError(
                            frame
                                .key == nil ? "JSON array is missing a separator" : "JSON object is missing a separator"
                        )
                    }
                    position += 1
                    value = frame.key == nil ? .array(frame.items) : .object(frame.fields)
                }
                return value
            }
        }

        mutating func parseKey() throws -> String {
            guard peek == 34 else { throw FlyrailValueError("JSON object keys must be strings") }
            try consumeToken()
            let key = try parseString()
            skipWhitespace()
            guard peek == 58 else { throw FlyrailValueError("JSON object is missing a colon") }
            position += 1
            skipWhitespace()
            return key
        }

        mutating func parseScalar() throws -> JSONValue {
            guard let next = peek else { throw FlyrailValueError("JSON contains an unexpected character") }
            switch next {
            case 34: return try .string(parseString())
            case 116: return try parseLiteral("true", value: .boolean(true))
            case 102: return try parseLiteral("false", value: .boolean(false))
            case 110: return try parseLiteral("null", value: .null)
            case 78 where bytes[safe: position ..< position + 3] == Array("NaN".utf8):
                throw FlyrailValueError("non-JSON numeric constant: NaN")
            case 73 where bytes[safe: position ..< position + 8] == Array("Infinity".utf8):
                throw FlyrailValueError("non-JSON numeric constant: Infinity")
            case 45, 48 ... 57: return try .number(parseNumber())
            default: throw FlyrailValueError("JSON contains an unexpected character")
            }
        }

        mutating func parseString() throws -> String {
            position += 1
            var output: [UInt8] = []
            while position < bytes.count {
                let byte = bytes[position]
                switch byte {
                case 34:
                    position += 1
                    guard let value = String(bytes: output, encoding: .utf8) else {
                        throw FlyrailValueError("JSON string contains invalid UTF-8")
                    }
                    return value
                case 92:
                    position += 1
                    try parseEscape(into: &output)
                case 0 ... 31:
                    throw FlyrailValueError("JSON string contains a control character")
                default:
                    output.append(byte)
                    position += 1
                }
                guard output.count <= limits.maximumStringBytes else {
                    throw FlyrailValueError("JSON string exceeds the byte limit")
                }
            }
            throw FlyrailValueError("JSON string is unterminated")
        }

        mutating func parseEscape(into output: inout [UInt8]) throws {
            guard let escaped = peek else {
                throw FlyrailValueError("JSON escape is unterminated")
            }
            position += 1
            switch escaped {
            case 34, 47, 92:
                output.append(escaped)
            case 98:
                output.append(8)
            case 102:
                output.append(12)
            case 110:
                output.append(10)
            case 114:
                output.append(13)
            case 116:
                output.append(9)
            case 117:
                let first = try parseHex4()
                let scalar: UInt32
                if (0xD800 ... 0xDBFF).contains(first) {
                    guard bytes[safe: position ..< position + 2] == [92, 117] else {
                        throw FlyrailValueError("JSON escape contains an unpaired surrogate")
                    }
                    position += 2
                    let second = try parseHex4()
                    guard (0xDC00 ... 0xDFFF).contains(second) else {
                        throw FlyrailValueError("JSON escape contains an unpaired surrogate")
                    }
                    scalar = 0x10000 + (UInt32(first - 0xD800) << 10) + UInt32(second - 0xDC00)
                } else {
                    guard !(0xDC00 ... 0xDFFF).contains(first) else {
                        throw FlyrailValueError("JSON escape contains an unpaired surrogate")
                    }
                    scalar = UInt32(first)
                }
                guard let unicode = Unicode.Scalar(scalar) else {
                    throw FlyrailValueError("JSON escape encodes an invalid scalar")
                }
                output.append(contentsOf: String(unicode).utf8)
            default:
                throw FlyrailValueError("JSON contains an invalid escape")
            }
        }

        mutating func parseHex4() throws -> UInt16 {
            guard position + 4 <= bytes.count else {
                throw FlyrailValueError("JSON unicode escape is truncated")
            }
            var value: UInt16 = 0
            for byte in bytes[position ..< position + 4] {
                let nibble: UInt16
                switch byte {
                case 48 ... 57:
                    nibble = UInt16(byte - 48)
                case 65 ... 70:
                    nibble = UInt16(byte - 65 + 10)
                case 97 ... 102:
                    nibble = UInt16(byte - 97 + 10)
                default:
                    throw FlyrailValueError("JSON unicode escape is not hexadecimal")
                }
                value = value * 16 + nibble
            }
            position += 4
            return value
        }

        mutating func parseNumber() throws -> String {
            let start = position
            if peek == 45 {
                position += 1
                if bytes[safe: position ..< position + 8] == Array("Infinity".utf8) {
                    throw FlyrailValueError("non-JSON numeric constant: -Infinity")
                }
            }
            guard let first = peek else {
                throw FlyrailValueError("JSON number is malformed")
            }
            switch first {
            case 48:
                position += 1
                guard !(peek.map { (48 ... 57).contains($0) } ?? false) else {
                    throw FlyrailValueError("JSON number has a leading zero")
                }
            case 49 ... 57:
                consumeDigits()
            default:
                throw FlyrailValueError("JSON number is malformed")
            }
            if peek == 46 {
                position += 1
                guard peek.map({ (48 ... 57).contains($0) }) == true else {
                    throw FlyrailValueError("JSON number has an empty fraction")
                }
                consumeDigits()
            }
            if peek == 101 || peek == 69 {
                position += 1
                if peek == 43 || peek == 45 {
                    position += 1
                }
                guard peek.map({ (48 ... 57).contains($0) }) == true else {
                    throw FlyrailValueError("JSON number has an empty exponent")
                }
                consumeDigits()
            }
            return String(decoding: bytes[start ..< position], as: UTF8.self)
        }

        mutating func parseLiteral(_ literal: String, value: JSONValue) throws -> JSONValue {
            let literalBytes = Array(literal.utf8)
            guard bytes[safe: position ..< position + literalBytes.count] == literalBytes else {
                throw FlyrailValueError("JSON contains an invalid literal")
            }
            position += literalBytes.count
            return value
        }

        mutating func consumeDigits() {
            while peek.map({ (48 ... 57).contains($0) }) == true {
                position += 1
            }
        }

        mutating func skipWhitespace() {
            while peek.map({ [9, 10, 13, 32].contains($0) }) == true {
                position += 1
            }
        }

        mutating func consumeToken() throws {
            tokenCount += 1
            guard tokenCount <= limits.maximumTokens else {
                throw FlyrailValueError("JSON exceeds the token limit")
            }
        }

        var peek: UInt8? {
            bytes.indices.contains(position) ? bytes[position] : nil
        }
    }
}

package enum JSONWriter {
    private enum Work {
        case value(JSONValue)
        case byte(UInt8)
        case string(String)
    }

    package static func canonicalData(_ value: JSONValue) throws -> Data {
        var output: [UInt8] = []
        var pending: [Work] = [.value(value)]
        while let work = pending.popLast() {
            switch work {
            case let .byte(byte): output.append(byte)
            case let .string(text): writeString(text, into: &output)
            case let .value(value): try write(value, into: &output, pending: &pending)
            }
        }
        return Data(output)
    }

    private static func write(_ value: JSONValue, into output: inout [UInt8], pending: inout [Work]) throws {
        switch value {
        case .null:
            output.append(contentsOf: "null".utf8)
        case let .boolean(value):
            output.append(contentsOf: (value ? "true" : "false").utf8)
        case let .number(token):
            guard try JSONParser.parse(Data(token.utf8)) == .number(token) else {
                throw FlyrailValueError("JSON number token is invalid")
            }
            output.append(contentsOf: token.utf8)
        case let .string(value):
            writeString(value, into: &output)
        case let .array(values):
            output.append(91)
            pending.append(.byte(93))
            for (index, item) in values.enumerated().reversed() {
                pending.append(.value(item))
                if index > 0 { pending.append(.byte(44)) }
            }
        case let .object(items):
            let sorted = items.sorted { utf8Less($0.0, $1.0) }
            if let duplicate = zip(sorted, sorted.dropFirst()).first(where: { exactTextEqual($0.0.0, $0.1.0) }) {
                throw FlyrailValueError("duplicate JSON field: \(duplicate.0.0)")
            }
            output.append(123)
            pending.append(.byte(125))
            for (index, item) in sorted.enumerated().reversed() {
                pending.append(.value(item.1))
                pending.append(.byte(58))
                pending.append(.string(item.0))
                if index > 0 { pending.append(.byte(44)) }
            }
        }
    }

    private static func writeString(_ value: String, into output: inout [UInt8]) {
        output.append(34)
        for scalar in value.unicodeScalars {
            switch scalar.value {
            case 34:
                output.append(contentsOf: "\\\"".utf8)
            case 92:
                output.append(contentsOf: "\\\\".utf8)
            case 8:
                output.append(contentsOf: "\\b".utf8)
            case 12:
                output.append(contentsOf: "\\f".utf8)
            case 10:
                output.append(contentsOf: "\\n".utf8)
            case 13:
                output.append(contentsOf: "\\r".utf8)
            case 9:
                output.append(contentsOf: "\\t".utf8)
            case 0 ..< 32:
                output.append(contentsOf: String(format: "\\u%04x", scalar.value).utf8)
            default:
                output.append(contentsOf: String(scalar).utf8)
            }
        }
        output.append(34)
    }
}

private extension [UInt8] {
    subscript(safe range: Range<Int>) -> [UInt8]? {
        guard range.lowerBound >= 0, range.upperBound <= count else {
            return nil
        }
        return Array(self[range])
    }
}
