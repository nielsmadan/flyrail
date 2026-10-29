import Foundation

package func tomlLayoutHeaders(_ layout: Data) throws -> [([DocumentPathPart], String)] {
    guard !layout.isEmpty else { return [] }
    let record: JSONValue
    do {
        record = try JSONParser.parse(layout)
    } catch {
        throw FlyrailValueError("invalid TOML restoration layout")
    }
    guard case let .object(fields) = record,
          fields.count == 2,
          fields.allSatisfy({ $0.0 == "version" || $0.0 == "headers" }),
          fields.first(where: { $0.0 == "version" })?.1 == .number("1"),
          case let .array(headers)? = fields.first(where: { $0.0 == "headers" })?.1
    else { throw FlyrailValueError("invalid TOML restoration layout") }
    var seen = Set<[DocumentPathPart]>()
    return try headers.map { entry in
        guard case let .array(pair) = entry, pair.count == 2 else {
            throw FlyrailValueError("invalid TOML restoration header entry")
        }
        guard case let .array(parts) = pair[0], !parts.isEmpty else {
            throw FlyrailValueError("invalid TOML restoration path")
        }
        let path: [DocumentPathPart] = try parts.map {
            switch $0 {
            case let .string(name): return .key(name)
            case let .number(text):
                guard let index = Int(text),
                      index >= 0 else { throw FlyrailValueError("invalid TOML restoration path") }
                return .index(index)
            default: throw FlyrailValueError("invalid TOML restoration path")
            }
        }
        guard seen.insert(path).inserted else { throw FlyrailValueError("duplicate TOML restoration path") }
        guard case let .string(name) = pair[1] else { throw FlyrailValueError("invalid TOML restoration header") }
        _ = try tomlHeaderPath(name)
        return (path, name)
    }
}

package func tomlHeaderPath(_ name: String) throws -> [String] {
    do {
        var scanner = try TOMLScanner(data: Data(name.utf8))
        let keys = try scanner.key()
        guard scanner.position == scanner.bytes.count
        else { throw FlyrailValueError("invalid TOML restoration header") }
        return keys.map(\.0)
    } catch { throw FlyrailValueError("invalid TOML restoration header") }
}

package func validateTOMLLayout(_ snapshot: SelectionSnapshot, selector: Selector? = nil) throws {
    let headers = try tomlLayoutHeaders(snapshot.layout)
    guard !headers.isEmpty else { return }
    guard case let .typed(value) = snapshot.value else { throw FlyrailValueError("invalid TOML restoration path") }
    for (relative, name) in headers {
        var current = value
        for part in relative {
            switch (part, current) {
            case let (.key(key), .object(fields)):
                guard let child = fields.first(where: { exactTextEqual($0.0, key) })?.1 else {
                    throw FlyrailValueError("invalid TOML restoration path")
                }
                current = child
            case let (.index(index), .array(items)) where items.indices.contains(index):
                current = items[index]
            default: throw FlyrailValueError("invalid TOML restoration path")
            }
        }
        guard case .object = current else { throw FlyrailValueError("TOML restoration header requires an object") }
        if let selector {
            let destination = selector.parts.compactMap { part -> String? in
                if case let .key(key) = part { key.name } else { nil }
            } + relative.compactMap { part -> String? in
                if case let .key(key) = part { key } else { nil }
            }
            guard try tomlHeaderPath(name).map({ Data($0.utf8) }) == destination.map({ Data($0.utf8) }) else {
                throw FlyrailValueError("TOML restoration header does not match its destination")
            }
        }
    }
}

package func tomlKey(_ name: String) throws -> String {
    if !name.isEmpty, name.utf8.allSatisfy({
        (65 ... 90).contains($0) || (97 ... 122).contains($0) || (48 ... 57).contains($0) || $0 == 95 || $0 == 45
    }) { return name }
    return try tomlString(name)
}

package func tomlString(_ text: String) throws -> String {
    var result = "\""
    for scalar in text.unicodeScalars {
        switch scalar.value {
        case 8: result += "\\b"
        case 9: result += "\\t"
        case 10: result += "\\n"
        case 12: result += "\\f"
        case 13: result += "\\r"
        case 34: result += "\\\""
        case 92: result += "\\\\"
        case 0 ... 31, 127: result += String(format: "\\u%04x", scalar.value)
        default: result.unicodeScalars.append(scalar)
        }
    }
    return result + "\""
}

package func tomlInline(_ value: SemanticValue) throws -> String {
    switch value {
    case let .object(fields):
        let ordered = fields.filter { !tomlTableValue($0.1) } + fields.filter { tomlTableValue($0.1) }
        return try "{" + ordered.map { try "\(tomlKey($0.0)) = \(tomlInline($0.1))" }.joined(separator: ", ") + "}"
    case let .array(items):
        return try "[" + items.map(tomlInline).joined(separator: ", ") + "]"
    case let .scalar(scalar):
        switch scalar {
        case .null: throw FlyrailValueError("Unable to convert an object of <class 'NoneType'> to a TOML item")
        case let .boolean(value): return value ? "true" : "false"
        case let .integer(value): return String(value)
        case let .float(value): return pythonFloatDescription(value.value)
        case let .text(value): return try tomlString(value)
        case let .date(value): return value.canonicalString
        case let .time(value): return tomlTime(value)
        case let .dateTime(value): return "\(value.date.canonicalString)T\(tomlTime(value.time))"
        case let .offsetDateTime(value): return "\(value.date.canonicalString)T\(tomlTime(value.time))Z"
        }
    }
}

private func tomlTime(_ value: LocalTime) -> String {
    value.microsecond == 0 ? String(value.canonicalString.prefix(8)) : value.canonicalString
}

package func tomlTableValue(_ value: SemanticValue) -> Bool {
    if case .object = value { return true }
    if case let .array(items) = value, !items.isEmpty {
        return items.allSatisfy { if case .object = $0 { true } else { false } }
    }
    return false
}

package func tomlGenerated(
    _ value: SemanticValue, name: String, newline: String,
    element: Bool = false, forceHeader: Bool = false
) throws -> String {
    if case let .array(items) = value, tomlTableValue(value) {
        return try items.map { try tomlGenerated($0, name: name, newline: newline, element: true) }.joined()
    }
    guard case let .object(fields) = value else {
        return try "\(name) = \(tomlInline(value))\(newline)"
    }
    let direct = fields.filter { !tomlTableValue($0.1) }
    let nested = fields.filter { tomlTableValue($0.1) }
    let header = element || forceHeader || !direct.isEmpty || fields.isEmpty
    var result = header ? "\(element ? "[[" : "[")\(name)\(element ? "]]" : "]")\(newline)" : ""
    result += try direct.map { try "\(tomlKey($0.0)) = \(tomlInline($0.1))\(newline)" }.joined()
    if !direct.isEmpty, case .object? = nested.first?.1 { result += newline }
    for (index, field) in nested.enumerated() {
        if index > 0, !result.hasSuffix(newline + newline) { result += newline }
        result += try tomlGenerated(field.1, name: name + "." + tomlKey(field.0), newline: newline)
    }
    return result
}
