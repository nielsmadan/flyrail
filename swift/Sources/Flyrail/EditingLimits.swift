import Foundation

package func checkEditingFrames(_ value: SemanticValue, identity: Bool = false) throws {
    var pending = [(value, identity ? 5 : 0)]
    while let (value, used) = pending.popLast() {
        let children: [SemanticValue]
        let frames: Int
        switch value {
        case .scalar:
            continue
        case let .object(items):
            children = items.map(\.1)
            frames = used + 5
        case let .array(items):
            children = items
            frames = used + 4
        }
        guard frames < 990 else {
            throw FlyrailValueError("document nesting is too deep")
        }
        pending.append(contentsOf: children.map { ($0, frames) })
    }
}

package func decodeEditingUTF8(_ data: Data) throws -> String {
    if let text = String(data: data, encoding: .utf8) {
        return text
    }
    let bytes = Array(data)
    var start = 0
    while start < bytes.count {
        let first = bytes[start]
        if first < 128 {
            start += 1
            continue
        }
        guard (0xC2 ... 0xF4).contains(first) else {
            throw editingUTF8Error(bytes, start: start, length: 1, reason: "invalid start byte")
        }
        let width = first < 0xE0 ? 2 : first < 0xF0 ? 3 : 4
        for offset in 1 ..< width {
            guard start + offset < bytes.count else {
                throw editingUTF8Error(bytes, start: start, length: offset, reason: "unexpected end of data")
            }
            let byte = bytes[start + offset]
            let minimum: UInt8 = offset == 1 && first == 0xE0 ? 0xA0
                : offset == 1 && first == 0xF0 ? 0x90 : 0x80
            let maximum: UInt8 = offset == 1 && first == 0xED ? 0x9F
                : offset == 1 && first == 0xF4 ? 0x8F : 0xBF
            guard (minimum ... maximum).contains(byte) else {
                throw editingUTF8Error(bytes, start: start, length: offset, reason: "invalid continuation byte")
            }
        }
        start += width
    }
    return String(decoding: bytes, as: UTF8.self)
}

private func editingUTF8Error(_ bytes: [UInt8], start: Int, length: Int, reason: String) -> FlyrailValueError {
    let description = length == 1 ? String(format: "byte 0x%02x", bytes[start]) : "bytes"
    let position = length == 1 ? "position \(start)" : "position \(start)-\(start + length - 1)"
    return FlyrailValueError("'utf-8' codec can't decode \(description) in \(position): \(reason)")
}
