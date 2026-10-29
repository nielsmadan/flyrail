@testable import Flyrail
import Foundation
import Testing

@Test
func statePathSeparatorsAreExactScalarsInFullRecords() throws {
    for record in try stateExtendingPathRecords() {
        let bytes = try StateCodec.encodeRecord(record)
        #expect(try StateCodec.transcode(bytes) == bytes)
    }
    for (mark, digest) in stateExtendingBasenames {
        let destination = try ResourcePath("/fixture/\(mark)name")
        #expect(try statePath(for: destination).displayString == "/fixture/.flyrail-\(digest).state")
        let doubled = try ResourcePath("//\(mark)parent/\(mark)name")
        #expect(try statePathText(doubled) == "//\(mark)parent/\(mark)name")
        #expect(try statePath(for: doubled).displayString == "//\(mark)parent/.flyrail-\(digest).state")
        #expect(throws: FlyrailValueError("state paths must be absolute without traversal")) {
            try ResourceRef(
                destination: ResourcePath("/fixture/../\(mark)name"), kind: .document,
                stateRoot: ResourcePath("/fixture/.flyrail-\(digest).state")
            )
        }
        #expect(throws: FlyrailValueError("revision path must be an unambiguous relative path")) {
            try stateTestNode("/\(mark)name", Data())
        }
        #expect(throws: FlyrailValueError("revision parents must be directories")) {
            try Revision(nodes: [stateTestNode("", nil), stateTestNode("a/\(mark)name", Data())])
        }
    }
}

private let stateExtendingBasenames = [
    ("\u{301}", "0cb4ad3a41b3fb2f5140117eee37487bd147a4f743c2270b63a0aadc2ff415f4"),
    ("\u{200D}", "5f508d364e660601c73b871734a1c0a36c3c451e5d9f47895f0da6abe843ac6a"),
]

func stateExtendingPathRecords() throws -> [JSONValue] {
    try stateExtendingBasenames.flatMap { mark, digest in
        let resources = try ["/fixture", "/\(mark)parent", "//\(mark)parent"].map { parent in
            try ResourceRef.fromState(stateExtendingResource(
                "\(parent)/\(mark)name", "\(parent)/.flyrail-\(digest).state"
            ))
        }
        let revision = try Revision(nodes: [
            stateTestNode("", nil), stateTestNode("a", nil), stateTestNode("a/\(mark)name", Data()),
        ])
        let markedRevision = try Revision(nodes: [
            stateTestNode("", nil), stateTestNode("\(mark)dir", nil),
            stateTestNode("\(mark)dir/\(mark)name", Data("x".utf8)),
        ])
        let ancestor = try Ancestor(
            path: ResourcePath("/\(mark)parent/\(mark)name"), device: 0, inode: 0, mode: 0, uid: 0, gid: 0
        )
        return try resources.flatMap { resource in
            try [resource.stateRecord(), Receipt(
                resource: resource, transactionID: String(repeating: "1", count: 32),
                claims: [stateTestOwned(resource)]
            ).stateRecord()]
        } + [
            revision.stateRecord(), markedRevision.stateRecord(), ancestor.stateRecord(),
            stateTestNode("a/\(mark)name", Data()).stateRecord(),
            Claim(kind: .subtree, selector: .subtree("a/\(mark)name")).stateRecord(),
        ]
    }
}

func stateExtendingPathFailures() throws -> [StateCompatibilityVector] {
    try stateExtendingBasenames.flatMap { mark, digest in
        var vectors: [StateCompatibilityVector] = []
        func append(_ tag: String, _ record: JSONValue, _ message: String) throws {
            try vectors.append(StateCompatibilityVector(
                tag: tag, input: StateCodec.encodeRecord(record), message: message
            ))
        }
        for (destination, message) in [
            ("/fixture/../\(mark)name", "state paths must be absolute without traversal"),
            ("/fixture/./\(mark)name", "state record is not canonical"),
            ("/fixture//\(mark)name", "state record is not canonical"),
            ("///\(mark)parent/name", "state record is not canonical"),
            ("\(mark)name", "state paths must be absolute without traversal"),
        ] {
            let stateRoot = destination.utf8.starts(with: [47, 47, 47])
                ? "/\(mark)parent/.flyrail-82a3537ff0dbce7eec35d69edc3a189ee6f17d82f353a553f9aa96cb0be3ce89.state"
                : "/fixture/.flyrail-\(digest).state"
            try append("ResourceRef", stateExtendingResource(destination, stateRoot), message)
            let ancestor = try Ancestor(
                path: ResourcePath("/fixture/a"), device: 0, inode: 0, mode: 0, uid: 0, gid: 0
            ).stateRecord()
            try append("Ancestor", stateReplacingField(
                ancestor, "path", .object([("path", .string(destination))])
            ), message)
        }
        try append("ResourceRef", stateExtendingResource(
            "/fixture/\(mark)name", "/other/.flyrail-\(digest).state"
        ), "resource state path disagrees with destination")
        for path in [
            "/\(mark)name", "a/../\(mark)name", "a/./\(mark)name", "a//\(mark)name", "a/\(mark)name/",
        ] {
            let node = try stateExtendingNode(path, data: Data())
            try append("Node", node, "revision path must be an unambiguous relative path")
            try append("Revision", StateCodec.record("Revision", [
                ("nodes", .array([stateExtendingNode("", data: nil), node])),
            ]), "revision path must be an unambiguous relative path")
        }
        for parentData in [nil, Data()] {
            let nodes = try [stateExtendingNode("", data: nil)] +
                (parentData.map { try [stateExtendingNode("a", data: $0)] } ?? []) +
                [stateExtendingNode("a/\(mark)name", data: Data())]
            try append("Revision", StateCodec.record("Revision", [("nodes", .array(nodes))]),
                       "revision parents must be directories")
        }
        let resource = try ResourceRef.fromState(stateExtendingResource(
            "/fixture/\(mark)name", "/fixture/.flyrail-\(digest).state"
        ))
        let receipt = try Receipt(
            resource: resource, transactionID: String(repeating: "1", count: 32), claims: [stateTestOwned(resource)]
        ).stateRecord()
        let provenance = try stateReplacingField(
            stateField(stateField(receipt, "fields"), "provenance"),
            "empty_document", .object([("bytes", .string("f"))])
        )
        try append("Receipt", stateReplacingField(receipt, "provenance", provenance),
                   "non-hexadecimal number found in fromhex() arg at position 1")
        return vectors
    }
}

private func stateExtendingResource(_ destination: String, _ stateRoot: String) -> JSONValue {
    StateCodec.record("ResourceRef", [
        ("destination", .object([("path", .string(destination))])),
        ("kind", StateCodec.enumeration("ResourceKind", "document")),
        ("state_root", .object([("path", .string(stateRoot))])), ("schema_version", .number("2")),
    ])
}

private func stateExtendingNode(_ path: String, data: Data?) throws -> JSONValue {
    try stateReplacingField(stateTestNode("", data).stateRecord(), "path", .string(path))
}

private func stateReplacingField(_ record: JSONValue, _ field: String, _ value: JSONValue) throws -> JSONValue {
    let tag = try #require(stateRecordTag(record))
    guard case let .object(fields) = try stateField(record, "fields") else {
        throw FlyrailValueError("invalid state record")
    }
    return StateCodec.record(tag, fields.map { ($0.0, $0.0 == field ? value : $0.1) })
}
