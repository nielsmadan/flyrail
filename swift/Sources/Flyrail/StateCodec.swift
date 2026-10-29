import Foundation

package protocol StateCodable: Sendable {
    func stateRecord() throws -> JSONValue
    static func fromState(_ value: JSONValue) throws -> Self
}

package enum StateCodec {
    package static func encode(_ value: some StateCodable, limits: JSONLimits = .default) throws -> Data {
        try encodeRecord(value.stateRecord(), limits: limits)
    }

    package static func decode<T: StateCodable>(
        _ data: Data, as _: T.Type = T.self, limits: JSONLimits = .default
    ) throws -> T {
        let raw = try parse(data, limits: limits)
        let result = try T.fromState(raw)
        guard try stateCanonicalEqual(result.stateRecord(), normalizedNumbers(raw)) else {
            throw FlyrailValueError("state record is not canonical")
        }
        return result
    }

    package static func transcode(_ data: Data, limits: JSONLimits = .default) throws -> Data {
        let raw = try parse(data, limits: limits)
        let canonical = try canonicalRecord(raw)
        guard try stateCanonicalEqual(canonical, normalizedNumbers(raw)) else {
            throw FlyrailValueError("state record is not canonical")
        }
        return try encodeRecord(canonical, limits: limits)
    }

    package static func decodeMetadata<T: StateCodable>(
        _ data: Data, as _: T.Type, tag: String, limits: JSONLimits = .default
    ) throws -> T {
        let raw = try parse(data, limits: limits)
        let canonical = try canonicalRecord(raw)
        guard try stateCanonicalEqual(canonical, normalizedNumbers(raw)) else {
            throw FlyrailValueError("state record is not canonical")
        }
        guard case let .object(items) = raw,
              items.contains(where: { $0.0 == "type" && $0.1 == .string(tag) })
        else {
            throw FlyrailValueError("metadata record has the wrong type")
        }
        return try T.fromState(raw)
    }

    private static func parse(_ data: Data, limits: JSONLimits) throws -> JSONValue {
        guard data.count <= limits.maximumInputBytes else {
            throw FlyrailValueError("JSON input exceeds the byte limit")
        }
        let input = data.starts(with: [0xEF, 0xBB, 0xBF]) ? Data(data.dropFirst(3)) : data
        let raw = try JSONParser.parse(input, limits: limits)
        try validateScalars(raw)
        return raw
    }

    package static func encodeRecord(_ record: JSONValue, limits: JSONLimits = .default) throws -> Data {
        var budget = StateEncodingBudget(limits: limits)
        try budget.validate(record, depth: 0)
        try validateScalars(record)
        var data = try JSONWriter.canonicalData(record)
        data.append(10)
        guard data.count <= limits.maximumInputBytes else {
            throw FlyrailValueError("JSON input exceeds the byte limit")
        }
        return data
    }

    package static func digest(_ record: JSONValue) throws -> String {
        try stateSHA256(encodeRecord(record))
    }

    package static func record(_ tag: String, _ fields: [(String, JSONValue)]) -> JSONValue {
        .object([("type", .string(tag)), ("fields", .object(fields))])
    }

    package static func enumeration(_ tag: String, _ value: String) -> JSONValue {
        .object([("enum", .string(tag)), ("value", .string(value))])
    }

    package static func bytes(_ data: Data) throws -> JSONValue {
        guard data.count <= JSONLimits.default.maximumStringBytes / 2 else {
            throw FlyrailValueError("JSON string exceeds the byte limit")
        }
        let digits = Array("0123456789abcdef".utf8)
        var encoded: [UInt8] = []
        encoded.reserveCapacity(data.count * 2)
        for byte in data {
            encoded.append(digits[Int(byte >> 4)])
            encoded.append(digits[Int(byte & 15)])
        }
        return .object([("bytes", .string(String(decoding: encoded, as: UTF8.self)))])
    }

    package static func path(_ path: ResourcePath) throws -> JSONValue {
        try .object([("path", .string(statePathText(path)))])
    }

    package static func semantic(_ value: SemanticValue) throws -> JSONValue {
        try .object([("value", stateSemanticRecord(value))])
    }

    package static func optional(_ value: (some StateCodable)?) throws -> JSONValue {
        try value?.stateRecord() ?? .null
    }

    package static func sequence(_ values: [some StateCodable]) throws -> JSONValue {
        guard values.count <= JSONLimits.default.maximumTokens else {
            throw FlyrailValueError("JSON exceeds the token limit")
        }
        return try .array(values.map { try $0.stateRecord() })
    }

    package static func decodeBytes(_ value: JSONValue) throws -> Data {
        guard case let .object(items) = value, items.count == 1,
              items[0].0 == "bytes", case let .string(hex) = items[0].1
        else { throw FlyrailValueError("invalid state record") }
        let bytes = Array(hex.utf8)
        var decoded = Data()
        var index = 0
        while index < bytes.count {
            if [9, 10, 11, 12, 13, 32].contains(bytes[index]) {
                index += 1
                continue
            }
            let high = try hexNibble(bytes[index], position: index)
            index += 1
            guard index < bytes.count else {
                throw malformedHex(index)
            }
            let low = try hexNibble(bytes[index], position: index)
            index += 1
            decoded.append(high * 16 + low)
        }
        guard bytes.allSatisfy({ (48 ... 57).contains($0) || (97 ... 102).contains($0) }) else {
            throw FlyrailValueError("state record is not canonical")
        }
        return decoded
    }

    private static func hexNibble(_ byte: UInt8, position: Int) throws -> UInt8 {
        switch byte {
        case 48 ... 57: byte - 48
        case 65 ... 70: byte - 55
        case 97 ... 102: byte - 87
        default: throw malformedHex(position)
        }
    }

    private static func malformedHex(_ position: Int) -> FlyrailValueError {
        FlyrailValueError("non-hexadecimal number found in fromhex() arg at position \(position)")
    }

    package static func decodePath(_ value: JSONValue) throws -> ResourcePath {
        guard case let .object(items) = value, items.count == 1,
              items[0].0 == "path", case let .string(text) = items[0].1
        else { throw FlyrailValueError("invalid state record") }
        _ = try statePathText(Array(text.utf8))
        let path = try ResourcePath(text)
        _ = try statePathText(path)
        return path
    }

    package static func decodeSemantic(_ value: JSONValue) throws -> SemanticValue {
        guard case let .object(items) = value, items.count == 1, items[0].0 == "value" else {
            throw FlyrailValueError("invalid state record")
        }
        let semantic = try stateSemanticValue(items[0].1)
        guard try stateCanonicalEqual(stateSemanticRecord(semantic), items[0].1) else {
            throw FlyrailValueError("state record is not canonical")
        }
        return semantic
    }

    package static func enumText(_ value: JSONValue, tag: String) throws -> String {
        guard case let .object(items) = value, items.count == 2,
              items.contains(where: { $0.0 == "enum" && $0.1 == .string(tag) }),
              let raw = items.first(where: { $0.0 == "value" })?.1
        else { throw FlyrailValueError("invalid state record") }
        guard case let .string(text) = raw else {
            throw invalidEnumValue(raw, tag: tag)
        }
        return text
    }

    package static func invalidEnumValue(_ value: JSONValue, tag: String) -> FlyrailValueError {
        FlyrailValueError("\(enumValueRepr(value)) is not a valid \(tag)")
    }

    private static func enumValueRepr(_ value: JSONValue) -> String {
        switch value {
        case .null: "None"
        case let .boolean(value): value ? "True" : "False"
        case let .number(token):
            token.contains(".") || token.contains("e") || token.contains("E")
                ? Double(token).map(String.init) ?? token
                : token == "-0" ? "0" : token
        case let .string(text): pythonStringRepr(text)
        case let .array(items): "[" + items.map(enumValueRepr).joined(separator: ", ") + "]"
        case let .object(items):
            "{" + items.map { "\(pythonStringRepr($0.0)): \(enumValueRepr($0.1))" }.joined(separator: ", ") + "}"
        }
    }

    private static func validateScalars(_ value: JSONValue) throws {
        var pending = [value]
        while let value = pending.popLast() {
            switch value {
            case let .number(token):
                guard !token.contains("."), !token.contains("e"), !token.contains("E") else {
                    throw FlyrailValueError("invalid state scalar")
                }
                guard token.utf8.count <= 4300 + (token.first == "-" ? 1 : 0) else {
                    throw FlyrailValueError("state integer exceeds the digit limit")
                }
            case let .array(items):
                pending.append(contentsOf: items)
            case let .object(items):
                if items.count == 2,
                   items.contains(where: { $0.0 == "enum" }),
                   items.contains(where: { $0.0 == "value" })
                {
                    continue
                }
                pending.append(contentsOf: items.map(\.1))
            case .null, .boolean, .string:
                break
            }
        }
    }

    private static func normalizedNumbers(_ value: JSONValue) throws -> JSONValue {
        try mapStateJSON(value) { item in
            item == .number("-0") ? .number("0") : item
        }
    }

    package static func canonicalRecord(_ value: JSONValue) throws -> JSONValue {
        try mapStateJSON(value, descendObjects: false) { item in
            guard case let .object(items) = item else {
                return item == .number("-0") ? .number("0") : item
            }
            if items.count == 1 {
                switch items[0].0 {
                case "bytes": return try bytes(decodeBytes(item))
                case "path": return try path(decodePath(item))
                case "value": return try semantic(decodeSemantic(item))
                default: throw FlyrailValueError("unknown state record")
                }
            }
            return try canonicalTaggedRecord(item, items: items)
        }
    }

    private static func canonicalTaggedRecord(_ value: JSONValue, items: [(String, JSONValue)]) throws -> JSONValue {
        let tag = items.first(where: { $0.0 == "type" || $0.0 == "enum" })?.1
        guard case let .string(name) = tag, let decode = decoders[name] else {
            throw FlyrailValueError("unknown state record")
        }
        return try decode(value)
    }

    private static func decoder<T: StateCodable>(_ type: T.Type) -> @Sendable (JSONValue) throws -> JSONValue {
        { try T.fromState($0).stateRecord() }
    }

    private static let decoders: [String: @Sendable (JSONValue) throws -> JSONValue] = [
        "Ownership": decoder(Ownership.self),
        "Disposition": decoder(Disposition.self),
        "DocumentFormat": decoder(DocumentFormat.self),
        "ResourceKind": decoder(ResourceKind.self),
        "Key": decoder(Key.self),
        "Member": decoder(Member.self),
        "Selector": decoder(Selector.self),
        "SectionBoundaries": decoder(SectionBoundaries.self),
        "Claim": decoder(Claim.self),
        "DocumentSchema": decoder(DocumentSchema.self),
        "ArrayPosition": decoder(ArrayPosition.self),
        "SelectionSnapshot": decoder(SelectionSnapshot.self),
        "OwnedSelection": decoder(OwnedSelection.self),
        "CreatedContainer": decoder(CreatedContainer.self),
        "DocumentProvenance": decoder(DocumentProvenance.self),
        "Ancestor": decoder(Ancestor.self),
        "Node": decoder(Node.self),
        "Revision": decoder(Revision.self),
        "ResourceRef": decoder(ResourceRef.self),
        "OwnedClaim": decoder(OwnedClaim.self),
        "Receipt": decoder(Receipt.self),
        "Generation": decoder(Generation.self),
        "Index": decoder(Index.self),
        "Journal": decoder(Journal.self),
    ]
}

private struct StateEncodingBudget {
    let limits: JSONLimits
    var tokens = 0
    var minimumBytes = 1

    mutating func validate(_ value: JSONValue, depth: Int) throws {
        var pending = [(value, depth)]
        while let (value, depth) = pending.popLast() {
            guard depth <= limits.maximumDepth else { throw FlyrailValueError("JSON nesting is too deep") }
            tokens += 1
            guard tokens <= limits.maximumTokens else { throw FlyrailValueError("JSON exceeds the token limit") }
            switch value {
            case let .string(text):
                try string(text)
            case let .number(token):
                minimumBytes += token.utf8.count
            case let .array(items):
                minimumBytes += 2 + max(0, items.count - 1)
                pending.append(contentsOf: items.map { ($0, depth + 1) })
            case let .object(items):
                minimumBytes += 2 + max(0, items.count - 1) + items.count
                for (key, item) in items {
                    tokens += 1
                    guard tokens <= limits.maximumTokens
                    else { throw FlyrailValueError("JSON exceeds the token limit") }
                    try string(key)
                    pending.append((item, depth + 1))
                }
            case .null: minimumBytes += 4
            case let .boolean(value): minimumBytes += value ? 4 : 5
            }
            guard minimumBytes <= limits.maximumInputBytes else {
                throw FlyrailValueError("JSON input exceeds the byte limit")
            }
        }
    }

    mutating func string(_ text: String) throws {
        guard text.utf8.count <= limits.maximumStringBytes else {
            throw FlyrailValueError("JSON string exceeds the byte limit")
        }
        minimumBytes += 2 + text.utf8.count
    }
}

package struct StateFields {
    private let values: [(String, JSONValue)]

    package init(_ value: JSONValue, tag: String, names: [String]) throws {
        guard case let .object(items) = value, items.count == 2,
              items.contains(where: { $0.0 == "type" && $0.1 == .string(tag) })
        else { throw FlyrailValueError("invalid state record") }
        guard let raw = items.first(where: { $0.0 == "fields" })?.1, case let .object(fields) = raw else {
            throw FlyrailValueError("invalid state record")
        }
        guard fields.count == names.count,
              Set(fields.map { Array($0.0.utf8) }) == Set(names.map { Array($0.utf8) })
        else {
            throw FlyrailValueError("state record fields do not match schema")
        }
        values = fields
    }

    package func raw(_ name: String) throws -> JSONValue {
        guard let value = values.first(where: { $0.0 == name })?.1 else {
            throw FlyrailValueError("state record fields do not match schema")
        }
        return value
    }

    package func text(_ name: String, message: String = "invalid state record") throws -> String {
        guard case let .string(text) = try raw(name) else { throw FlyrailValueError(message) }
        return text
    }

    package func count(_ name: String, message: String) throws -> Count {
        let value = try raw(name)
        guard let token = value.unsignedDecimalToken ?? (value == .number("-0") ? "0" : nil) else {
            throw FlyrailValueError(message)
        }
        return try Count(decimal: token)
    }

    package func schema(_ message: String) throws -> Int {
        guard try raw("schema_version").signedInteger == 2 else { throw FlyrailValueError(message) }
        return 2
    }

    package func bytes(_ name: String) throws -> Data { try StateCodec.decodeBytes(raw(name)) }
    package func path(_ name: String, message: String = "invalid state record") throws -> ResourcePath {
        let value = try raw(name)
        guard case .object = value else { throw FlyrailValueError(message) }
        return try StateCodec.decodePath(value)
    }

    package func semantic(_ name: String) throws -> SemanticValue { try StateCodec.decodeSemantic(raw(name)) }

    package func decode<T: StateCodable>(_ name: String, as _: T.Type = T.self) throws -> T {
        try T.fromState(raw(name))
    }

    package func optional<T: StateCodable>(_ name: String, as _: T.Type = T.self) throws -> T? {
        let value = try raw(name)
        return try value == .null ? nil : T.fromState(value)
    }

    package func optionalBytes(_ name: String) throws -> Data? {
        let value = try raw(name)
        return try value == .null ? nil : StateCodec.decodeBytes(value)
    }

    package func sequence<T: StateCodable>(_ name: String, as _: T.Type = T.self) throws -> [T] {
        try array(name).map(T.fromState)
    }

    package func array(_ name: String) throws -> [JSONValue] {
        guard case let .array(items) = try raw(name) else { throw FlyrailValueError("invalid state record") }
        return items
    }
}
