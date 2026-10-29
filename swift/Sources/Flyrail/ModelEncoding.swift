import CryptoKit
import Foundation

package enum ModelEncoding {
    package static func record(
        type: String,
        fields: [(String, SemanticValue)],
    ) throws -> SemanticValue {
        try SemanticValue(object: [
            ("type", .scalar(.text(type))),
            ("fields", SemanticValue(object: fields)),
        ])
    }

    package static func bytes(_ value: Data) throws -> SemanticValue {
        try SemanticValue(object: [
            ("bytes", .scalar(.text(value.map { String(format: "%02x", $0) }.joined()))),
        ])
    }

    package static func digest(
        domain: String,
        values: [SemanticValue],
    ) throws -> String {
        var framed = Data(domain.utf8)
        framed.append(0)
        try framed.append(SemanticValue.array(values).semanticBytes())
        return SHA256.hash(data: framed).map { String(format: "%02x", $0) }.joined()
    }
}

package extension SemanticValue {
    func modelValue() throws -> SemanticValue {
        switch self {
        case let .scalar(value):
            try ModelEncoding.record(type: "Scalar", fields: [("value", .scalar(value))])
        case let .array(values):
            try ModelEncoding.record(
                type: "ArrayValue",
                fields: [("items", .array(values.map { try $0.modelValue() }))],
            )
        case let .object(items):
            try ModelEncoding.record(
                type: "ObjectValue",
                fields: [
                    ("items", .array(items.sorted { utf8Less($0.0, $1.0) }.map {
                        try .array([.scalar(.text($0.0)), $0.1.modelValue()])
                    })),
                ],
            )
        }
    }
}
