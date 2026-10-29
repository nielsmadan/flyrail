import Foundation

private enum StateJSONWork {
    case value(JSONValue)
    case array(Int)
    case object([String])
}

package func mapStateJSON(
    _ value: JSONValue, descendObjects: Bool = true, transform: (JSONValue) throws -> JSONValue,
) throws -> JSONValue {
    var pending: [StateJSONWork] = [.value(value)]
    var results: [JSONValue] = []
    while let work = pending.popLast() {
        switch work {
        case let .value(value):
            switch value {
            case let .array(items):
                pending.append(.array(items.count))
                pending.append(contentsOf: items.reversed().map(StateJSONWork.value))
            case let .object(items) where descendObjects:
                pending.append(.object(items.map(\.0)))
                pending.append(contentsOf: items.reversed().map { .value($0.1) })
            default:
                try results.append(transform(value))
            }
        case let .array(count):
            let items = Array(results.suffix(count))
            results.removeLast(count)
            try results.append(transform(.array(items)))
        case let .object(keys):
            let items = Array(results.suffix(keys.count))
            results.removeLast(keys.count)
            try results.append(transform(.object(Array(zip(keys, items)))))
        }
    }
    guard let result = results.first, results.count == 1 else {
        throw FlyrailValueError("invalid state record")
    }
    return result
}

package func stateCanonicalEqual(_ left: JSONValue, _ right: JSONValue) throws -> Bool {
    try JSONWriter.canonicalData(left) == JSONWriter.canonicalData(right)
}
