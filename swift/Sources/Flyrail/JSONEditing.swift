import Foundation

package enum DocumentPathPart: Sendable, Hashable {
    case key(String)
    case index(Int)

    package static func == (left: Self, right: Self) -> Bool {
        switch (left, right) {
        case let (.key(left), .key(right)):
            exactTextEqual(left, right)
        case let (.index(left), .index(right)):
            left == right
        default:
            false
        }
    }

    package func hash(into hasher: inout Hasher) {
        switch self {
        case let .key(value):
            hasher.combine(0)
            hashExactText(value, into: &hasher)
        case let .index(value):
            hasher.combine(1)
            hasher.combine(value)
        }
    }
}

package final class JSONEditingDocument {
    package struct Node {
        let start: Int
        let end: Int
        let value: SemanticValue
        let children: [(DocumentPathPart, Node, Int)]
        let commas: [Int]
    }

    package private(set) var data: Data
    private let comments: Bool
    private var root: Node

    package init(data: Data, comments: Bool) throws {
        self.data = data
        self.comments = comments
        let parsed = try Self.parse(data: data, comments: comments)
        root = parsed
    }

    package func native(_ path: [DocumentPathPart]) throws -> SemanticValue {
        try node(path).value
    }

    package func syntax(_ path: [DocumentPathPart]) throws -> Data {
        let selected = try node(path)
        return data.subdata(in: selected.start ..< selected.end)
    }

    package func positionPrefix(_ path: [DocumentPathPart]) throws -> Data {
        guard case let .index(index)? = path.last else {
            throw FlyrailValueError("array position requires a member")
        }
        let parent = try node(Array(path.dropLast()))
        let start = index == 0 ? parent.start + 1 : parent.commas[index - 1] + 1
        return try data.subdata(in: start ..< (node(path).start))
    }

    package func put(
        _ path: [DocumentPathPart],
        value: SemanticValue,
        syntax: Data? = nil
    ) throws {
        let raw = try syntax ?? encodeJSONValue(value)
        guard let last = path.last else {
            try patch([(root.start, root.end, raw)])
            return
        }
        let parentPath = Array(path.dropLast())
        let parent = try node(parentPath)
        if case let .key(name) = last,
           case let .object(items) = parent.value,
           !items.contains(where: { exactTextEqual($0.0, name) })
        {
            let key = try JSONWriter.canonicalData(.string(name))
            var member = Data()
            member.append(key)
            member.append(58)
            member.append(raw)
            try insertRaw(parentPath, index: parent.children.count, raw: member, prefix: nil)
            return
        }
        let selected = try node(path)
        try patch([(selected.start, selected.end, raw)])
    }

    package func insert(
        _ path: [DocumentPathPart],
        index: Int,
        value: SemanticValue,
        syntax: Data? = nil,
        prefix: Data? = nil
    ) throws {
        try insertRaw(path, index: index, raw: syntax ?? encodeJSONValue(value), prefix: prefix)
    }

    package func delete(_ path: [DocumentPathPart]) throws {
        guard let last = path.last else {
            throw FlyrailValueError("document selector path is missing")
        }
        let parent = try node(Array(path.dropLast()))
        guard let index = parent.children.firstIndex(where: { pathPartsEqual($0.0, last) }) else {
            throw FlyrailValueError("document selector path is missing")
        }
        let child = parent.children[index]
        var patches = [(child.1.start, child.1.end, Data())]
        if case .key = last {
            var scanner = try JSONEditingScanner(data: data, comments: comments)
            scanner.position = child.2
            _ = try scanner.scanString()
            let keyEnd = scanner.position
            try scanner.skipSpace()
            patches.append((child.2, keyEnd, Data()))
            patches.append((scanner.position, scanner.position + 1, Data()))
        }
        if index < parent.commas.count {
            patches.append((parent.commas[index], parent.commas[index] + 1, Data()))
        } else if let comma = parent.commas.last {
            patches.append((comma, comma + 1, Data()))
        }
        try patch(patches)
    }

    package func relocate(
        _ path: [DocumentPathPart],
        index: Int,
        value: SemanticValue,
        syntax: Data? = nil,
        prefix: Data? = nil
    ) throws {
        let parent = Array(path.dropLast())
        try delete(path)
        try insert(parent, index: index, value: value, syntax: syntax, prefix: prefix)
    }

    private static func parse(data: Data, comments: Bool) throws -> Node {
        var scanner = try JSONEditingScanner(data: data, comments: comments)
        let root = try scanner.parseNode(depth: 0)
        try scanner.skipSpace()
        guard scanner.position == data.count else {
            throw FlyrailValueError("unexpected data after JSON document")
        }
        return root
    }

    private func node(_ path: [DocumentPathPart]) throws -> Node {
        var current = root
        for part in path {
            guard let child = current.children.first(where: { pathPartsEqual($0.0, part) }) else {
                throw FlyrailValueError("document selector path is missing")
            }
            current = child.1
        }
        return current
    }

    private func insertRaw(
        _ parentPath: [DocumentPathPart],
        index: Int,
        raw: Data,
        prefix: Data?
    ) throws {
        let parent = try node(parentPath)
        guard index >= 0, index <= parent.children.count else {
            throw FlyrailValueError("array insertion index is out of range")
        }
        if index < parent.children.count {
            let boundary = index == 0 ? parent.start + 1 : parent.commas[index - 1] + 1
            let start = try prefixed(boundary, parent.children[index].2, prefix)
            var inserted = raw
            inserted.append(44)
            try patch([(start, start, inserted)])
            return
        }
        var start = parent.end - 1
        if let prefix {
            let boundary = if parent.commas.count == parent.children.count, let comma = parent.commas.last {
                comma + 1
            } else if let child = parent.children.last {
                child.1.end
            } else {
                parent.start + 1
            }
            start = try prefixed(boundary, start, prefix)
        }
        var patches = [(start, start, raw)]
        if !parent.children.isEmpty, parent.commas.count < parent.children.count {
            let end = parent.children.last?.1.end ?? start
            patches.append((end, end, Data([44])))
        } else if !parent.commas.isEmpty {
            var inserted = raw
            inserted.append(44)
            patches[0] = (start, start, inserted)
        }
        try patch(patches)
    }

    private func prefixed(_ start: Int, _ end: Int, _ prefix: Data?) throws -> Int {
        let prefix = prefix ?? Data()
        guard data[start ..< end].starts(with: prefix) else {
            throw FlyrailValueError("original array position trivia changed")
        }
        return start + prefix.count
    }

    private func patch(_ patches: [(Int, Int, Data)]) throws {
        for patch in patches.sorted(by: {
            $0.0 == $1.0 ? $0.1 > $1.1 : $0.0 > $1.0
        }) {
            data.replaceSubrange(patch.0 ..< patch.1, with: patch.2)
        }
        root = try Self.parse(data: data, comments: comments)
    }
}

private struct JSONEditingFrame {
    let opening: UInt8
    let start: Int
    let depth: Int
    var entryStart = 0
    var nextKey: String?
    var keys: Set<Data> = []
    var children: [(DocumentPathPart, JSONEditingDocument.Node, Int)] = []
    var commas: [Int] = []
    var members: [(String, SemanticValue)] = []
    var values: [SemanticValue] = []

    var closing: UInt8 {
        opening == 123 ? 125 : 93
    }

    mutating func attach(_ node: JSONEditingDocument.Node) throws {
        if opening == 123 {
            guard let key = nextKey else {
                throw FlyrailValueError("missing JSON object colon")
            }
            members.append((key, node.value))
            children.append((.key(key), node, entryStart))
            nextKey = nil
        } else {
            children.append((.index(values.count), node, entryStart))
            values.append(node.value)
        }
    }

    func finish(at end: Int) throws -> JSONEditingDocument.Node {
        let value = opening == 123
            ? try SemanticValue(object: members)
            : .array(values)
        return .init(
            start: start,
            end: end,
            value: value,
            children: children,
            commas: commas
        )
    }
}

private enum JSONEditingPhase {
    case entry
    case separator
}

private struct JSONEditingScanner {
    private static let maximumDepth = 985

    let bytes: [UInt8]
    let comments: Bool
    var position = 0
    var tokenCount = 0

    init(data: Data, comments: Bool) throws {
        guard data.count <= JSONLimits.default.maximumInputBytes else {
            throw FlyrailValueError("JSON input exceeds the byte limit")
        }
        _ = try decodeEditingUTF8(data)
        bytes = Array(data)
        self.comments = comments
    }

    // swiftlint:disable:next function_body_length
    mutating func parseNode(depth: Int) throws -> JSONEditingDocument.Node {
        guard depth <= Self.maximumDepth else {
            throw FlyrailValueError("document nesting is too deep")
        }
        try skipSpace()
        let start = position
        guard let opening = peek else {
            throw FlyrailValueError("missing JSON value")
        }
        try consumeToken()
        guard opening == 91 || opening == 123 else {
            let value = try parseScalar()
            return .init(start: start, end: position, value: value, children: [], commas: [])
        }
        position += 1
        var stack: [JSONEditingFrame] = []
        var current = JSONEditingFrame(opening: opening, start: start, depth: depth)
        var phase = JSONEditingPhase.entry
        while true {
            switch phase {
            case .entry:
                try skipSpace()
                guard let byte = peek else {
                    throw FlyrailValueError("unterminated or malformed JSON container")
                }
                if byte == current.closing {
                    position += 1
                    let node = try current.finish(at: position)
                    guard var parent = stack.popLast() else {
                        return node
                    }
                    try parent.attach(node)
                    current = parent
                    phase = .separator
                    continue
                }
                current.entryStart = position
                if current.opening == 123 {
                    try consumeToken()
                    let key = try scanKey(depth: current.depth + 1)
                    guard current.keys.insert(Data(key.utf8)).inserted else {
                        throw FlyrailValueError("duplicate JSON object key")
                    }
                    try skipSpace()
                    guard peek == 58 else {
                        throw FlyrailValueError("missing JSON object colon")
                    }
                    position += 1
                    current.nextKey = key
                }
                try skipSpace()
                let valueStart = position
                guard let valueByte = peek else {
                    throw FlyrailValueError("missing JSON value")
                }
                try consumeToken()
                if valueByte == 91 || valueByte == 123 {
                    guard current.depth + 1 < Self.maximumDepth else {
                        throw FlyrailValueError("document nesting is too deep")
                    }
                    position += 1
                    stack.append(current)
                    current = JSONEditingFrame(
                        opening: valueByte,
                        start: valueStart,
                        depth: current.depth + 1
                    )
                    continue
                }
                let value = try parseScalar()
                try current.attach(.init(
                    start: valueStart,
                    end: position,
                    value: value,
                    children: [],
                    commas: []
                ))
                phase = .separator
            case .separator:
                try skipSpace()
                if peek == current.closing {
                    position += 1
                    let node = try current.finish(at: position)
                    guard var parent = stack.popLast() else {
                        return node
                    }
                    try parent.attach(node)
                    current = parent
                    continue
                }
                guard peek == 44 else {
                    throw FlyrailValueError("unterminated or malformed JSON container")
                }
                current.commas.append(position)
                position += 1
                try skipSpace()
                if !comments, peek == current.closing {
                    throw FlyrailValueError("JSON trailing commas are unsupported")
                }
                phase = .entry
            }
        }
    }

    mutating func skipSpace() throws {
        while let byte = peek {
            switch byte {
            case 9, 10, 13, 32:
                position += 1
            case 47 where comments && byteAt(position + 1) == 47:
                position += 2
                while let next = peek, next != 10, next != 13 {
                    position += 1
                }
            case 47 where comments && byteAt(position + 1) == 42:
                position += 2
                while position + 1 < bytes.count,
                      !(bytes[position] == 42 && bytes[position + 1] == 47)
                {
                    position += 1
                }
                guard position + 1 < bytes.count else {
                    throw FlyrailValueError("unterminated JSONC comment")
                }
                position += 2
            default:
                return
            }
        }
    }

    // swiftlint:disable:next cyclomatic_complexity
    mutating func scanString() throws -> String {
        let opening = position
        position += 1
        var output = String.UnicodeScalarView()
        var surrogate = false
        while let byte = peek {
            switch byte {
            case 34:
                position += 1
                guard !surrogate else {
                    throw FlyrailValueError("scalar must contain Unicode scalar values")
                }
                return String(output)
            case 92:
                position += 1
                guard let escaped = peek else {
                    throw decodeError("Unterminated string starting at", at: opening)
                }
                switch escaped {
                case 34, 92, 47:
                    output.append(UnicodeScalar(escaped))
                    position += 1
                case 98, 102, 110, 114, 116:
                    let scalar: UInt8 = switch escaped {
                    case 98: 8
                    case 102: 12
                    case 110: 10
                    case 114: 13
                    default: 9
                    }
                    output.append(UnicodeScalar(scalar))
                    position += 1
                case 117:
                    guard let code = hex4(at: position + 1) else {
                        throw decodeError("Invalid \\uXXXX escape", at: position)
                    }
                    position += 5
                    if (0xD800 ... 0xDBFF).contains(code) {
                        if peek == 92, byteAt(position + 1) == 117,
                           let second = hex4(at: position + 2),
                           (0xDC00 ... 0xDFFF).contains(second)
                        {
                            let combined = 0x10000 + ((code - 0xD800) << 10) + second - 0xDC00
                            if let scalar = UnicodeScalar(combined) {
                                output.append(scalar)
                            }
                            position += 6
                        } else {
                            surrogate = true
                        }
                    } else if (0xDC00 ... 0xDFFF).contains(code) {
                        surrogate = true
                    } else if let scalar = UnicodeScalar(code) {
                        output.append(scalar)
                    }
                default:
                    throw decodeError("Invalid \\escape", at: position - 1)
                }
            case 0 ... 31:
                throw decodeError("Invalid control character at", at: position)
            default:
                let width = byte < 128 ? 1 : byte < 0xE0 ? 2 : byte < 0xF0 ? 3 : 4
                output
                    .append(contentsOf: String(decoding: bytes[position ..< position + width], as: UTF8.self)
                        .unicodeScalars)
                position += width
            }
        }
        throw decodeError("Unterminated string starting at", at: opening)
    }

    private mutating func parseScalar() throws -> SemanticValue {
        let start = position
        if peek == 34 {
            return try .scalar(.text(scanString()))
        }
        for (word, value) in [
            ("true", SemanticValue.scalar(.boolean(true))),
            ("false", .scalar(.boolean(false))),
            ("null", .scalar(.null)),
        ] where bytes[start...].starts(with: word.utf8) {
            position += word.utf8.count
            return value
        }
        if ["NaN", "Infinity", "-Infinity"].contains(where: { bytes[start...].starts(with: $0.utf8) }) {
            throw FlyrailValueError("floats must be finite")
        }
        return try parseNumber()
    }

    private mutating func parseNumber() throws -> SemanticValue {
        let start = position
        if peek == 45 { position += 1 }
        if peek == 48 {
            position += 1
        } else if let byte = peek, (49 ... 57).contains(byte) {
            while let byte = peek, (48 ... 57).contains(byte) {
                position += 1
            }
        } else {
            throw decodeError("Expecting value", at: start)
        }
        var floating = false
        if peek == 46, let next = byteAt(position + 1), (48 ... 57).contains(next) {
            floating = true
            position += 1
            while let byte = peek, (48 ... 57).contains(byte) {
                position += 1
            }
        }
        if peek == 101 || peek == 69 {
            var end = position + 1
            if byteAt(end) == 43 || byteAt(end) == 45 { end += 1 }
            if let byte = byteAt(end), (48 ... 57).contains(byte) {
                floating = true
                position = end
                while let byte = peek, (48 ... 57).contains(byte) {
                    position += 1
                }
            }
        }
        let text = String(decoding: bytes[start ..< position], as: UTF8.self)
        if floating {
            guard let value = Double(text), value.isFinite else {
                throw FlyrailValueError("floats must be finite")
            }
            return try .scalar(.float(Binary64(value)))
        }
        guard let value = Int64(text) else {
            throw FlyrailValueError("integers must fit signed 64 bits")
        }
        return .scalar(.integer(value))
    }

    private mutating func scanKey(depth: Int) throws -> String {
        if peek == 34 { return try scanString() }
        if peek == 91 || peek == 123 {
            _ = try parseNode(depth: depth)
        } else {
            _ = try parseScalar()
        }
        throw FlyrailValueError("JSON object keys must be strings")
    }

    private func hex4(at start: Int) -> UInt32? {
        guard start + 4 <= bytes.count else { return nil }
        let digits = bytes[start ..< start + 4]
        guard digits.allSatisfy({ (48 ... 57).contains($0) || (65 ... 70).contains($0) || (97 ... 102).contains($0) })
        else {
            return nil
        }
        return UInt32(String(decoding: digits, as: UTF8.self), radix: 16)
    }

    private func decodeError(_ message: String, at position: Int) -> FlyrailValueError {
        let prefix = String(decoding: bytes[..<position], as: UTF8.self).unicodeScalars
        let line = prefix.filter { $0 == "\n" }.count + 1
        let column = prefix.reversed().prefix { $0 != "\n" }.count + 1
        return FlyrailValueError("\(message): line \(line) column \(column) (char \(prefix.count))")
    }

    private mutating func consumeToken() throws {
        tokenCount += 1
        guard tokenCount <= JSONLimits.default.maximumTokens else {
            throw FlyrailValueError("JSON exceeds the token limit")
        }
    }

    private var peek: UInt8? {
        byteAt(position)
    }

    private func byteAt(_ index: Int) -> UInt8? {
        bytes.indices.contains(index) ? bytes[index] : nil
    }
}

private enum JSONEditingOutput {
    case value(SemanticValue, Int)
    case raw(Data)
}

private func encodeJSONValue(_ value: SemanticValue) throws -> Data {
    var pending = [JSONEditingOutput.value(value, 0)]
    var output = Data()
    while let token = pending.popLast() {
        switch token {
        case let .raw(data):
            output.append(data)
        case let .value(value, depth):
            guard depth <= 985 else {
                throw FlyrailValueError("document nesting is too deep")
            }
            switch value {
            case let .scalar(scalar):
                try output.append(encodeJSONScalar(scalar))
            case let .array(values):
                output.append(91)
                pending.append(.raw(Data([93])))
                let children = values.enumerated().flatMap { index, value in
                    (index == 0 ? [] : [JSONEditingOutput.raw(Data([44]))]) + [.value(value, depth + 1)]
                }
                pending.append(contentsOf: children.reversed())
            case let .object(items):
                output.append(123)
                pending.append(.raw(Data([125])))
                let children = try items.enumerated().flatMap { index, item in
                    let key = try JSONWriter.canonicalData(.string(item.0)) + Data([58])
                    return (index == 0 ? [] : [JSONEditingOutput.raw(Data([44]))]) +
                        [.raw(key), .value(item.1, depth + 1)]
                }
                pending.append(contentsOf: children.reversed())
            }
        }
    }
    return output
}

private func encodeJSONScalar(_ scalar: ScalarValue) throws -> Data {
    let json: JSONValue
    switch scalar {
    case .null:
        json = .null
    case let .boolean(value):
        json = .boolean(value)
    case let .integer(value):
        json = .number(String(value))
    case let .float(value):
        json = .number(pythonFloatDescription(value.value))
    case let .text(value):
        json = .string(value)
    case .date:
        throw FlyrailValueError("Object of type date is not JSON serializable")
    case .time:
        throw FlyrailValueError("Object of type time is not JSON serializable")
    case .dateTime, .offsetDateTime:
        throw FlyrailValueError("Object of type datetime is not JSON serializable")
    }
    return try JSONWriter.canonicalData(json)
}

package func pythonFloatDescription(_ value: Double) -> String {
    if value == 0 {
        return value.sign == .minus ? "-0.0" : "0.0"
    }
    var text = String(value)
    if !text.contains("."), !text.contains("e"), !text.contains("E") {
        text += ".0"
    }
    return text.replacingOccurrences(of: "E", with: "e")
}

private func pathPartsEqual(_ left: DocumentPathPart, _ right: DocumentPathPart) -> Bool {
    left == right
}
