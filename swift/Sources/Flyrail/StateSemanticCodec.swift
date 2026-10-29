import Foundation

private enum SemanticEncodingWork {
    case value(SemanticValue, Int)
    case array(Int)
    case object([String])
}

package func stateSemanticRecord(_ value: SemanticValue) throws -> JSONValue {
    var pending: [SemanticEncodingWork] = [.value(value, 0)]
    var results: [JSONValue] = []
    var count = 0
    while let work = pending.popLast() {
        switch work {
        case let .value(value, depth):
            count += 1
            guard count <= JSONLimits.default.maximumTokens
            else { throw FlyrailValueError("JSON exceeds the token limit") }
            guard depth <= JSONLimits.default.maximumDepth else { throw FlyrailValueError("JSON nesting is too deep") }
            switch value {
            case .scalar:
                try results.append(value.semanticRecord())
            case let .array(items):
                pending.append(.array(items.count))
                pending.append(contentsOf: items.reversed().map { .value($0, depth + 2) })
            case let .object(items):
                _ = try SemanticValue(object: items)
                let sorted = items.sorted { utf8Less($0.0, $1.0) }
                pending.append(.object(sorted.map(\.0)))
                pending.append(contentsOf: sorted.reversed().map { .value($0.1, depth + 3) })
            }
        case let .array(count):
            let items = Array(results.suffix(count))
            results.removeLast(count)
            results.append(.array([.string("array"), .array(items)]))
        case let .object(keys):
            let items = Array(results.suffix(keys.count))
            results.removeLast(keys.count)
            results.append(.array([
                .string("object"), .array(zip(keys, items).map { .array([.string($0.0), $0.1]) }),
            ]))
        }
    }
    guard let result = results.first, results.count == 1 else { throw FlyrailValueError("invalid state record") }
    return result
}

private enum SemanticDecodingWork {
    case value(JSONValue)
    case array(Int)
    case object([String])
}

package func stateSemanticValue(_ record: JSONValue) throws -> SemanticValue {
    var pending: [SemanticDecodingWork] = [.value(record)]
    var results: [SemanticValue] = []
    while let work = pending.popLast() {
        switch work {
        case let .value(value):
            guard case let .array(fields) = value, case let .string(tag) = fields.first else {
                throw FlyrailValueError("semantic values must be tagged arrays")
            }
            if tag == "array" || tag == "object" {
                guard fields.count == 2, case let .array(items) = fields[1] else {
                    throw FlyrailValueError("semantic value has an invalid field count")
                }
                if tag == "array" {
                    pending.append(.array(items.count))
                    pending.append(contentsOf: items.reversed().map(SemanticDecodingWork.value))
                } else {
                    let pairs = try items.map { item -> (String, JSONValue) in
                        guard case let .array(pair) = item, pair.count == 2, case let .string(key) = pair[0] else {
                            throw FlyrailValueError("semantic object members must be name/value pairs")
                        }
                        return (key, pair[1])
                    }
                    pending.append(.object(pairs.map(\.0)))
                    pending.append(contentsOf: pairs.reversed().map { .value($0.1) })
                }
            } else {
                try results.append(SemanticValue(semanticBytes: JSONWriter.canonicalData(value)))
            }
        case let .array(count):
            let items = Array(results.suffix(count))
            results.removeLast(count)
            results.append(.array(items))
        case let .object(keys):
            let items = Array(results.suffix(keys.count))
            results.removeLast(keys.count)
            try results.append(SemanticValue(object: Array(zip(keys, items))))
        }
    }
    guard let result = results.first, results.count == 1 else { throw FlyrailValueError("invalid state record") }
    return result
}

package func stateSemanticFraming(_ value: JSONValue) throws -> JSONValue {
    try mapStateJSON(value) { item in
        switch item {
        case .null: return .array([.string("null")])
        case let .boolean(value): return .array([.string("boolean"), .boolean(value)])
        case let .string(value): return .array([.string("string"), .string(value)])
        case let .number(token):
            guard let value = Int64(token) else { throw FlyrailValueError("invalid semantic identity") }
            return try SemanticValue.scalar(.integer(value)).semanticRecord()
        case let .array(items): return .array([.string("array"), .array(items)])
        case let .object(items):
            return .array([
                .string("object"),
                .array(items.sorted { utf8Less($0.0, $1.0) }.map { .array([.string($0.0), $0.1]) }),
            ])
        }
    }
}
