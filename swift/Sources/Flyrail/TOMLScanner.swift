import Foundation

package final class TOMLNode {
    enum Kind { case scalar, array, inline, table, aot }
    let kind: Kind
    var scalar: ScalarValue?
    var fields: [(String, TOMLNode)] = []
    var items: [TOMLNode] = []
    var start = 0
    var end = 0
    var commas: [Int] = []
    var inlineKeys: [(String, Int)] = []
    var nonfinite = false
    var explicit = false
    var dotted = false

    init(_ kind: Kind, scalar: ScalarValue? = nil) {
        self.kind = kind
        self.scalar = scalar
    }

    func field(_ key: String) -> TOMLNode? {
        fields.first { exactTextEqual($0.0, key) }?.1
    }

    func value() throws -> SemanticValue {
        switch kind {
        case .scalar: .scalar(scalar ?? .null)
        case .array, .aot: try .array(items.map { try $0.value() })
        case .inline, .table: try SemanticValue(object: fields.map { try ($0.0, $0.1.value()) })
        }
    }

    func selectedValue() throws -> SemanticValue {
        var pending = [self]
        while let node = pending.popLast() {
            guard !node.nonfinite else { throw FlyrailValueError("floats must be finite") }
            pending.append(contentsOf: node.items + node.fields.map(\.1))
        }
        return try value()
    }
}

package struct TOMLStatement {
    let path: [DocumentPathPart]
    let start: Int
    let end: Int
    let keyStart: Int
    let keyEnd: Int
    let valueStart: Int
    let valueEnd: Int
    let header: Bool
    let aot: Bool
    let spelling: String
    let keyParts: [(String, String)]
}

package struct TOMLScanResult {
    let root: TOMLNode
    let statements: [TOMLStatement]
}

package struct TOMLScanner {
    let bytes: [UInt8]
    var position = 0
    var tokens = 0

    init(data: Data) throws {
        guard data.count <= JSONLimits.default.maximumInputBytes else {
            throw FlyrailValueError("TOML input exceeds the byte limit")
        }
        _ = try decodeEditingUTF8(data)
        bytes = Array(data)
    }

    func current(_ offset: Int = 0) -> UInt8? {
        let index = position + offset
        return bytes.indices.contains(index) ? bytes[index] : nil
    }

    func text(_ start: Int, _ end: Int) -> String {
        String(decoding: bytes[start ..< end], as: UTF8.self)
    }

    func error(_ message: String, at offset: Int? = nil) -> FlyrailValueError {
        let offset = offset ?? position
        let prefix = bytes.prefix(offset)
        let line = 1 + prefix.filter { $0 == 10 }.count
        let start = prefix.lastIndex(of: 10).map { $0 + 1 } ?? 0
        let column = String(decoding: bytes[start ..< offset], as: UTF8.self).unicodeScalars.count
        return FlyrailValueError("\(message) at line \(line) col \(column)")
    }

    func unexpected(_ byte: UInt8? = nil) -> FlyrailValueError {
        let byte = byte ?? current() ?? 0
        let character = switch byte {
        case 0: "\\x00"
        case 9: "\\t"
        case 10: "\\n"
        case 13: "\\r"
        case 39: "\"'\""
        default: String(UnicodeScalar(byte))
        }
        return error("Unexpected character: \(byte == 39 ? character : "'\(character)'")")
    }

    mutating func token() throws {
        tokens += 1
        guard tokens <= JSONLimits.default.maximumTokens else {
            throw FlyrailValueError("TOML exceeds the token limit")
        }
    }

    mutating func spaces() {
        while current() == 32 || current() == 9 {
            position += 1
        }
    }

    mutating func comment() throws {
        while let byte = current(), byte != 10, byte != 13 {
            if byte < 32 && byte != 9 || byte == 127 {
                throw error(String(
                    format: "Control characters (codes less than 0x1f and 0x7f) are not allowed in comments, use \\u%04x instead",
                    byte,
                ))
            }
            position += 1
        }
    }

    mutating func trail() throws {
        spaces()
        if current() == 35 {
            try comment()
        }
        if current() == 13 {
            guard current(1) == 10 else {
                throw error(
                    "Control characters (codes less than 0x1f and 0x7f) are not allowed in comments, use \\u000d instead",
                    at: 0,
                )
            }
            position += 1
        }
        if current() == 10 {
            position += 1
        } else if current() != nil {
            throw unexpected()
        }
    }

    mutating func key() throws -> [(String, String)] {
        var parts: [(String, String)] = []
        repeat {
            if !parts.isEmpty {
                position += 1
            }
            guard parts.count < 100 else { throw error("TOML key nested more than 100 levels deep") }
            let start = position
            spaces()
            let name: String
            if current() == 34 || current() == 39 {
                name = try string(key: true)
            } else {
                let nameStart = position
                while let byte = current(),
                      (65 ... 90).contains(byte) || (97 ... 122).contains(byte) ||
                      (48 ... 57).contains(byte) || byte == 95 || byte == 45
                {
                    position += 1
                }
                guard position > nameStart else { throw error("Empty key") }
                name = text(nameStart, position)
            }
            spaces()
            parts.append((name, text(start, position)))
        } while current() == 46
        return parts
    }

    mutating func string(key: Bool = false) throws -> String {
        let quote = current() ?? 34
        let multiline = !key && current(1) == quote && current(2) == quote
        position += multiline ? 3 : 1
        if multiline {
            if current() == 13, current(1) == 10 {
                position += 2
            } else if current() == 10 {
                position += 1
            }
        }
        var result: [UInt8] = []
        while let byte = current() {
            if byte == quote {
                if !multiline {
                    position += 1; return String(decoding: result, as: UTF8.self)
                }
                var count = 0
                while current() == quote, count < 5 {
                    position += 1; count += 1
                }
                if count >= 3 {
                    result.append(contentsOf: repeatElement(quote, count: count - 3))
                    return String(decoding: result, as: UTF8.self)
                }
                result.append(contentsOf: repeatElement(quote, count: count))
                continue
            }
            if byte == 92, quote == 34 {
                position += 1
                if multiline, [9, 10, 13, 32].contains(current() ?? 0) {
                    spaces()
                    guard current() == 10 || current() == 13 else { throw unexpected() }
                    while [9, 10, 13, 32].contains(current() ?? 0) {
                        position += 1
                    }
                    continue
                }
                try result.append(contentsOf: escapedStringBytes())
                continue
            }
            if byte < 32 && byte != 9 && !(multiline && (byte == 10 || byte == 13)) || byte == 127 {
                if byte == 10 {
                    throw unexpected()
                }
                throw error(String(
                    format: "Control characters (codes less than 0x1f and 0x7f) are not allowed in strings, use \\u%04x instead",
                    byte,
                ))
            }
            if multiline, byte == 13, current(1) == 10 {
                position += 1; continue
            }
            result.append(byte)
            position += 1
            guard result.count <= JSONLimits.default.maximumStringBytes else {
                throw FlyrailValueError("TOML string exceeds the byte limit")
            }
        }
        throw error("Unexpected end of file")
    }

    mutating func escapedStringBytes() throws -> [UInt8] {
        guard let escape = current() else { throw error("Unexpected end of file") }
        let mapped: UInt8? = switch escape {
        case 98: 8
        case 116: 9
        case 110: 10
        case 102: 12
        case 114: 13
        case 34: 34
        case 92: 92
        default: nil
        }
        if let mapped {
            position += 1; return [mapped]
        }
        if escape == 117 || escape == 85 {
            let escapeStart = position
            position += 1
            let width = escape == 117 ? 4 : 8
            guard position + width <= bytes.count,
                  bytes[position ..< position + width].allSatisfy({
                      (48 ... 57).contains($0) || (65 ... 70).contains($0) || (97 ... 102).contains($0)
                  }),
                  let code = UInt32(text(position, position + width), radix: 16),
                  let scalar = UnicodeScalar(code)
            else { throw error("Invalid unicode value", at: escapeStart) }
            position += width
            return Array(String(scalar).utf8)
        }
        throw error("Invalid character '\(String(UnicodeScalar(escape)))' in string")
    }

    mutating func value(depth: Int = 0) throws -> TOMLNode {
        guard depth <= 100 else { throw error("TOML value nested more than 100 levels deep") }
        try token()
        let start = position
        let node: TOMLNode
        switch current() {
        case 34, 39:
            node = try TOMLNode(.scalar, scalar: .text(string()))
        case 91:
            guard depth < 100 else { throw error("TOML value nested more than 100 levels deep") }
            node = TOMLNode(.array)
            position += 1
            try arraySpace()
            while current() != 93 {
                guard current() != nil else { throw error("Unexpected end of file") }
                try node.items.append(value(depth: depth + 1))
                try arraySpace()
                if current() == 93 {
                    break
                }
                guard current() == 44 else { throw unexpected() }
                node.commas.append(position)
                position += 1
                try arraySpace()
            }
            position += 1
        case 123:
            guard depth < 100 else { throw error("TOML value nested more than 100 levels deep") }
            node = TOMLNode(.inline)
            position += 1
            spaces()
            while current() != 125 {
                guard current() != nil else { throw error("Unexpected end of file") }
                let keyStart = position
                let names = try key()
                guard current() == 61 else { throw unexpected() }
                position += 1
                spaces()
                let child = try value(depth: depth + names.count)
                try assign(node, keys: names.map(\.0), value: child, at: start)
                if names.count == 1 {
                    node.inlineKeys.append((names[0].0, keyStart))
                }
                spaces()
                if current() == 125 {
                    break
                }
                guard current() == 44 else { throw unexpected() }
                node.commas.append(position)
                position += 1
                spaces()
            }
            position += 1
        default:
            node = try bare()
        }
        node.start = start
        node.end = position
        return node
    }

    mutating func arraySpace() throws {
        while let byte = current() {
            if [9, 10, 13, 32].contains(byte) {
                position += 1
            } else if byte == 35 {
                try comment()
            } else {
                break
            }
        }
    }

    mutating func bare() throws -> TOMLNode {
        let start = position
        for literal in ["true", "false"] where textMatches(literal) {
            position += literal.utf8.count
            return TOMLNode(.scalar, scalar: .boolean(literal == "true"))
        }
        while let byte = current(), ![9, 10, 13, 32, 35, 44, 93, 125].contains(byte) {
            position += 1
        }
        if position - start == 10, current() == 32,
           let next = current(1), (48 ... 57).contains(next),
           current(3) == 58
        {
            position += 1
            while let byte = current(), ![9, 10, 13, 32, 35, 44, 93, 125].contains(byte) {
                position += 1
            }
        }
        let raw = text(start, position)
        guard !raw.isEmpty else { throw unexpected() }
        let clean = raw.replacingOccurrences(of: "_", with: "")
        if ["inf", "+inf", "-inf", "nan", "+nan", "-nan"].contains(raw) {
            let result = TOMLNode(.scalar, scalar: .text(raw))
            result.nonfinite = true
            return result
        }
        if raw.count >= 10, raw.utf8.dropFirst(4).first == 45 {
            guard nativeTemporalGrammar(raw) else { throw error("Invalid number") }
            do {
                return try TOMLNode(.scalar, scalar: tomlTemporal(raw))
            } catch {
                throw self.error(raw.count == 10 ? "Invalid date" : "Invalid datetime")
            }
        }
        if raw.contains(":") {
            do {
                return try TOMLNode(.scalar, scalar: .time(LocalTime(nativeTemporalPrecision(raw))))
            } catch {
                throw self.error("Invalid time")
            }
        }
        guard validNumberUnderscores(raw) else { throw error("Invalid number") }
        if clean.hasPrefix("0x") || clean.hasPrefix("0o") || clean.hasPrefix("0b") {
            let radix = clean.hasPrefix("0x") ? 16 : clean.hasPrefix("0o") ? 8 : 2
            guard let integer = Int64(clean.dropFirst(2), radix: radix) else { throw error("Invalid number") }
            return TOMLNode(.scalar, scalar: .integer(integer))
        }
        let body = clean.hasPrefix("+") || clean.hasPrefix("-") ? String(clean.dropFirst()) : clean
        guard body.first?.isNumber == true,
              !(body.count > 1 && body.first == "0" && body.dropFirst().first?.isNumber == true)
        else { throw error("Invalid number") }
        if clean.contains(".") || clean.contains("e") || clean.contains("E") {
            guard !clean.hasSuffix("."), let number = Double(clean),
                  !body.contains(".e"), !body.contains(".E")
            else { throw error("Invalid number") }
            let result = try TOMLNode(.scalar, scalar: number.isFinite ? .float(Binary64(number)) : .text(raw))
            result.nonfinite = !number.isFinite
            return result
        }
        guard let number = Int64(clean), body.utf8.allSatisfy({ (48 ... 57).contains($0) }) else {
            throw error("Invalid number")
        }
        return TOMLNode(.scalar, scalar: .integer(number))
    }

    func textMatches(_ text: String) -> Bool {
        bytes.dropFirst(position).starts(with: text.utf8)
    }

    mutating func parse() throws -> TOMLScanResult {
        let root = TOMLNode(.table)
        var context: [DocumentPathPart] = []
        var statements: [TOMLStatement] = []
        while position < bytes.count {
            let start = position
            spaces()
            if current() == 35 {
                try comment(); try trail(); continue
            }
            if current() == 10 {
                position += 1; continue
            }
            if current() == 13 {
                try trail(); continue
            }
            if current() == nil {
                break
            }
            try token()
            if current() == 91 {
                let aot = current(1) == 91
                position += aot ? 2 : 1
                let keyStart = position
                let parts = try key()
                let keyEnd = position
                guard current() == 93 else { throw current() == nil ? error("Unexpected end of file") : unexpected() }
                position += 1
                if aot {
                    guard current() == 93 else { throw unexpected() }
                    position += 1
                }
                let valueEnd = position
                try trail()
                context = try tablePath(root, keys: parts.map(\.0), aot: aot, at: start)
                statements.append(.init(
                    path: context, start: start, end: position,
                    keyStart: keyStart, keyEnd: keyEnd, valueStart: start, valueEnd: valueEnd,
                    header: true, aot: aot, spelling: text(keyStart, keyEnd), keyParts: parts,
                ))
            } else {
                let keyStart = position
                let parts = try key()
                let keyEnd = position
                guard current() == 61 else { throw unexpected() }
                position += 1
                spaces()
                let valueStart = position
                let node = try value(depth: context.count + parts.count - 1)
                let valueEnd = position
                try trail()
                let parent = try tomlNode(root, path: context)
                try assign(parent, keys: parts.map(\.0), value: node, at: start)
                statements.append(.init(
                    path: context + parts.map { .key($0.0) }, start: start, end: position,
                    keyStart: keyStart, keyEnd: keyEnd, valueStart: valueStart, valueEnd: valueEnd,
                    header: false, aot: false, spelling: text(keyStart, keyEnd), keyParts: parts,
                ))
            }
        }
        return .init(root: root, statements: statements)
    }

    func assign(_ parent: TOMLNode, keys: [String], value: TOMLNode, at offset: Int) throws {
        var node = parent
        for (index, key) in keys.enumerated() {
            if index == keys.count - 1 {
                guard node.field(key) == nil else { throw error("Key \"\(key)\" already exists.", at: offset) }
                node.fields.append((key, value))
            } else if let child = node.field(key) {
                guard child.kind == .table else {
                    throw error("Key \"\(key)\" already exists.", at: offset)
                }
                node = child
            } else {
                let child = TOMLNode(.table)
                child.dotted = true
                node.fields.append((key, child))
                node = child
            }
        }
    }

    func tablePath(_ root: TOMLNode, keys: [String], aot: Bool, at offset: Int) throws -> [DocumentPathPart] {
        var node = root
        var path: [DocumentPathPart] = []
        for (index, key) in keys.enumerated() {
            let last = index == keys.count - 1
            path.append(.key(key))
            if last, aot {
                let array: TOMLNode
                if let found = node.field(key) {
                    guard found.kind == .aot else { throw error("Key \"\(key)\" already exists.", at: offset) }
                    array = found
                } else {
                    array = TOMLNode(.aot)
                    node.fields.append((key, array))
                }
                let member = TOMLNode(.table)
                member.explicit = true
                path.append(.index(array.items.count))
                array.items.append(member)
                return path
            }
            if let found = node.field(key) {
                if found.kind == .aot, !last, let member = found.items.last {
                    path.append(.index(found.items.count - 1))
                    node = member
                    continue
                }
                guard found.kind == .table, !(last && (found.explicit || found.dotted)) else {
                    throw error("Key \"\(key)\" already exists.", at: offset)
                }
                node = found
            } else {
                let child = TOMLNode(.table)
                node.fields.append((key, child))
                node = child
            }
            if last {
                node.explicit = true
            }
        }
        return path
    }
}

package func tomlNode(_ root: TOMLNode, path: [DocumentPathPart]) throws -> TOMLNode {
    var node = root
    for part in path {
        switch part {
        case let .key(key):
            guard let child = node.field(key) else { throw FlyrailValueError("document selector path is missing") }
            node = child
        case let .index(index):
            guard node.items.indices.contains(index)
            else { throw FlyrailValueError("document selector path is missing") }
            node = node.items[index]
        }
    }
    return node
}

private func validNumberUnderscores(_ raw: String) -> Bool {
    let bytes = Array(raw.utf8)
    let hexadecimal = raw.hasPrefix("0x")
    let digit: (UInt8) -> Bool = {
        (48 ... 57).contains($0) || (hexadecimal && ((65 ... 70).contains($0) || (97 ... 102).contains($0)))
    }
    return bytes.indices.allSatisfy { index in
        bytes[index] != 95 ||
            (index > 0 && index + 1 < bytes.count &&
                digit(bytes[index - 1]) && digit(bytes[index + 1]))
    }
}

private func tomlTemporal(_ raw: String) throws -> ScalarValue {
    if raw.utf8.count == 10 {
        return try .date(CalendarDate(raw))
    }
    let text = nativeTemporalPrecision(raw).replacingOccurrences(of: "t", with: "T")
        .replacingOccurrences(of: " ", with: "T")
        .replacingOccurrences(of: "z", with: "Z")
    if text.hasSuffix("Z") || text.utf8.dropFirst(19).contains(43) || text.utf8.dropFirst(19).contains(45) {
        return try .offsetDateTime(OffsetDateTime(text))
    }
    return try .dateTime(LocalDateTime(text))
}

private func nativeTemporalGrammar(_ raw: String) -> Bool {
    raw.range(
        of: #"^[0-9]{4}-[0-9]{2}-[0-9]{2}(?:[Tt ][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:[Zz]|[+-][0-9]{2}:[0-9]{2})?)?$"#,
        options: .regularExpression,
    ) != nil
}

private func nativeTemporalPrecision(_ raw: String) -> String {
    guard let fraction = raw.range(of: #"\.[0-9]+"#, options: .regularExpression) else { return raw }
    let digits = raw[fraction].dropFirst()
    guard digits.count > 6 else { return raw }
    return String(raw[..<fraction.lowerBound]) + "." + digits.prefix(6) + raw[fraction.upperBound...]
}
