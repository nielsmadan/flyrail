@testable import Flyrail
import Foundation
import Testing

struct StateCompatibilityVector {
    let tag: String
    let input: Data
    let message: String
}

@Test
func stateCodecPreservesRustPythonCompatibilityDiagnostics() throws {
    for vector in try stateCompatibilityVectors() + stateExtendingPathFailures() {
        if vector.message.isEmpty {
            #expect(try StateCodec.transcode(vector.input) == StateCodec.encodeRecord(JSONParser.parse(
                vector.input.starts(with: [0xEF, 0xBB, 0xBF]) ? Data(vector.input.dropFirst(3)) : vector.input,
            )))
        } else {
            #expect(
                throws: FlyrailValueError(vector.message),
                "\(vector.tag): \(String(decoding: vector.input, as: UTF8.self))",
            ) {
                try StateCodec.transcode(vector.input)
            }
        }
    }
}

private func stateCompatibilityBases() -> [String: [(String, JSONValue)]] {
    let bytes: JSONValue = .object([("bytes", .string(""))])
    let path: JSONValue = .object([("path", .string("/a"))])
    let semantic: JSONValue = .object([("value", .array([.string("string"), .string("x")]))])
    let key = StateCodec.record("Key", [("name", .string("a"))])
    let selector = StateCodec.record("Selector", [("parts", .array([key]))])
    return [
        "Key": [("name", .string("a"))],
        "Member": [("identity", semantic), ("key", .array([]))],
        "DocumentSchema": [
            ("format", StateCodec.enumeration("DocumentFormat", "json")), ("key", .string("k")), ("value", semantic),
        ],
        "ArrayPosition": [("following", .null), ("prefix", bytes), ("previous", .null)],
        "SelectionSnapshot": [
            ("layout", bytes), ("owned_syntax", bytes), ("position", .null), ("syntax", bytes), ("value", bytes),
        ],
        "DocumentProvenance": [
            ("containers", .array([])), ("empty_document", .null), ("schema_empty_document", .null),
            ("schema_fields", .array([])), ("trailing_separators", .array([])),
        ],
        "CreatedContainer": [("empty_syntax", bytes), ("selector", selector)],
        "Ancestor": [
            ("device", .number("0")), ("gid", .number("0")), ("inode", .number("0")), ("mode", .number("0")),
            ("path", path), ("security", bytes), ("uid", .number("0")),
        ],
        "Node": [
            ("data", .object([("bytes", .string("78"))])), ("device", .number("0")), ("gid", .number("0")),
            ("inode", .number("0")), ("mode", .number("0")), ("path", .string("")),
            ("security", bytes), ("uid", .number("0")),
        ],
        "Revision": [("nodes", .array([]))],
        "Claim": [("kind", StateCodec.enumeration("Ownership", "file")), ("selector", .null)],
        "ResourceRef": [
            ("destination", .object([("path", .string("/pin/AGENTS.md"))])),
            ("kind", StateCodec.enumeration("ResourceKind", "document")), ("schema_version", .number("2")),
            ("state_root", .object([("path", .string(
                "/pin/.flyrail-b951327c37d8a1ed4d5ed18908681f6aca718ecdfe174aa39cf2237547f92fe7.state",
            ))])),
        ],
        "Index": [
            ("bundle_id", .string("owner")), ("context_digest", .string(String(repeating: "0", count: 64))),
            ("current", .null), ("pending", .null), ("previous", .null), ("residual", .array([])),
            ("schema_version", .number("2")),
        ],
    ]
}

func stateCompatibilityVectors() throws -> [StateCompatibilityVector] {
    let bases = stateCompatibilityBases()
    let key = StateCodec.record("Key", [("name", .string("a"))])
    var vectors: [StateCompatibilityVector] = []
    func append(_ tag: String, _ field: String, _ value: JSONValue, _ message: String) throws {
        let fields = try #require(bases[tag]).map { ($0.0, $0.0 == field ? value : $0.1) }
        try vectors.append(StateCompatibilityVector(
            tag: tag, input: JSONWriter.canonicalData(StateCodec.record(tag, fields)), message: message,
        ))
    }
    let invalid = "invalid state record"
    let ancestorMetadata = "invalid ancestor metadata"
    let huge = "1000000000000000000000000000000"
    try append("Ancestor", "mode", .number("4095"), "")
    try append("Ancestor", "inode", .number(huge), "")
    try append("Node", "mode", .number("511"), "")
    try append("Node", "inode", .number(huge), "")
    try vectors.append(StateCompatibilityVector(
        tag: "Key", input: Data([0xEF, 0xBB, 0xBF]) + JSONWriter.canonicalData(key), message: "",
    ))
    try append("Key", "name", .number("5"), invalid)
    try append("Key", "name", .number("1.5"), "invalid state scalar")
    try append("Member", "key", .string("ab"), invalid)
    try append("Member", "identity", .string("x"), invalid)
    try append("DocumentSchema", "format", .string("json"), "document schema scaffolds require JSON or JSONC")
    try append("DocumentSchema", "value", .object([("bytes", .string("61"))]), invalid)
    try append("ArrayPosition", "prefix", .number("5"), invalid)
    try append("SelectionSnapshot", "position", .number("5"), invalid)
    try append("SelectionSnapshot", "value", .number("5"), invalid)
    try append("DocumentProvenance", "empty_document", .number("5"), invalid)
    try append(
        "DocumentProvenance", "schema_empty_document", .number("5"),
        "schema empty-document evidence must be whitespace bytes",
    )
    try append(
        "DocumentProvenance", "trailing_separators", .array([.number("5")]),
        "separator evidence must be SHA-256 digests",
    )
    try append("CreatedContainer", "empty_syntax", .number("5"), invalid)
    try append("Ancestor", "path", .string("/a"), "ancestor path must be absolute")
    try append("Ancestor", "device", .string("0"), ancestorMetadata)
    try append("Ancestor", "device", .boolean(true), ancestorMetadata)
    try append("Ancestor", "mode", .number("4096"), ancestorMetadata)
    try append("Ancestor", "inode", .number("-" + huge), ancestorMetadata)
    try append("Ancestor", "security", .number("5"), invalid)
    for (hex, message) in [
        ("abc", "non-hexadecimal number found in fromhex() arg at position 3"),
        ("zz", "non-hexadecimal number found in fromhex() arg at position 0"),
        ("ab0g", "non-hexadecimal number found in fromhex() arg at position 3"),
        ("ab cd", "state record is not canonical"),
        ("f", "non-hexadecimal number found in fromhex() arg at position 1"),
        ("AB", "state record is not canonical"),
        (" \tAB\r\n", "state record is not canonical"),
        ("a b", "non-hexadecimal number found in fromhex() arg at position 1"),
        ("ab\u{A0}cd", "non-hexadecimal number found in fromhex() arg at position 2"),
    ] {
        try append("Ancestor", "security", .object([("bytes", .string(hex))]), message)
    }
    try vectors.append(StateCompatibilityVector(
        tag: "Ancestor",
        input: JSONWriter.canonicalData(StateCodec.record("Ancestor", #require(bases["Ancestor"]).filter {
            $0.0 != "security"
        })),
        message: "state record fields do not match schema",
    ))
    return try vectors + stateCompatibilityNodeAndEnumVectors()
}

private func stateCompatibilityNodeAndEnumVectors() throws -> [StateCompatibilityVector] {
    let bases = stateCompatibilityBases()
    let key = StateCodec.record("Key", [("name", .string("a"))])
    let selector = StateCodec.record("Selector", [("parts", .array([key]))])
    let invalid = "invalid state record"
    let nodeMetadata = "invalid revision metadata"
    let huge = "1000000000000000000000000000000"
    var vectors: [StateCompatibilityVector] = []
    func append(_ tag: String, _ field: String, _ value: JSONValue, _ message: String) throws {
        let fields = try #require(bases[tag]).map { ($0.0, $0.0 == field ? value : $0.1) }
        try vectors.append(StateCompatibilityVector(
            tag: tag, input: JSONWriter.canonicalData(StateCodec.record(tag, fields)), message: message,
        ))
    }
    try append("Node", "path", .number("5"), invalid)
    try append("Node", "data", .string("x"), invalid)
    try append("Node", "mode", .boolean(true), nodeMetadata)
    try append("Node", "mode", .number("512"), nodeMetadata)
    try append("Node", "inode", .number("-" + huge), nodeMetadata)
    try append("Revision", "nodes", .array([key]), invalid)
    for (value, repr) in [
        (JSONValue.number("5"), "5"), (.boolean(true), "True"), (.null, "None"), (.object([]), "{}"),
        (.array([]), "[]"), (.number("1.5"), "1.5"), (.string("bogus"), "'bogus'"),
    ] {
        try append(
            "Claim", "kind", .object([("enum", .string("Ownership")), ("value", value)]),
            "\(repr) is not a valid Ownership",
        )
    }
    try append("Claim", "kind", .string("file"), invalid)
    try append("Claim", "kind", StateCodec.enumeration("Disposition", "created"), invalid)
    try append("Claim", "selector", .number("5"), "whole-resource claims cannot have selectors")
    try append("Claim", "selector", selector, "whole-resource claims cannot have selectors")
    try vectors.append(StateCompatibilityVector(
        tag: "Claim", input: JSONWriter.canonicalData(StateCodec.record("Claim", [
            ("kind", StateCodec.enumeration("Ownership", "structured")), ("selector", .number("5")),
        ])), message: invalid,
    ))
    try append("ResourceRef", "schema_version", .number("3"), "authority schema must be 2")
    try append("ResourceRef", "schema_version", .boolean(true), "authority schema must be 2")
    try append("ResourceRef", "kind", .string("document"), invalid)
    try append("ResourceRef", "destination", .string("/pin/AGENTS.md"), "expected an absolute concrete path")
    try append("ResourceRef", "state_root", .string("/pin/x"), "resource state path disagrees with destination")
    try append("Index", "bundle_id", .number("5"), invalid)
    try append("Index", "current", .number("5"), invalid)
    try append("Index", "schema_version", .number("3"), "index schema must be 2")
    let nodeText = try String(decoding: JSONWriter.canonicalData(StateCodec.record(
        "Node", #require(bases["Node"]),
    )), as: UTF8.self)
    for constant in ["NaN", "Infinity", "-Infinity"] {
        vectors.append(StateCompatibilityVector(
            tag: "Node",
            input: Data(nodeText.replacingOccurrences(of: "\"mode\":0", with: "\"mode\":\(constant)").utf8),
            message: "non-JSON numeric constant: \(constant)",
        ))
    }
    vectors.append(StateCompatibilityVector(
        tag: "Key", input: Data(#"{"fields":{"name":"a"},"type":"Key","type":"Key"}"#.utf8),
        message: "duplicate JSON field: type",
    ))
    let resource = try StateCodec.record("ResourceRef", #require(bases["ResourceRef"]))
    let digest = JSONValue.string(String(repeating: "0", count: 64))
    let membership: JSONValue = .array([.array([resource, .array([digest])])])
    let generation: [(String, JSONValue)] = [
        ("bundle_digest", digest), ("membership", membership), ("render_digest", digest), ("version", .string("1.0")),
    ]
    for (field, value, message) in [
        ("version", JSONValue.number("5"), invalid),
        ("bundle_digest", .number("5"), "invalid hexadecimal identity"),
        ("membership", .array([.array([resource])]), "not enough values to unpack (expected 2, got 1)"),
        ("membership", .array([.array([resource, .array([digest]), .string("extra")])]),
         "too many values to unpack (expected 2)"),
        ("membership", .array([.array([resource, .array([.number("5")])])]), "invalid hexadecimal identity"),
    ] {
        try vectors.append(StateCompatibilityVector(
            tag: "Generation",
            input: JSONWriter.canonicalData(StateCodec.record("Generation", generation.map {
                ($0.0, $0.0 == field ? value : $0.1)
            })), message: message,
        ))
    }
    return vectors
}
