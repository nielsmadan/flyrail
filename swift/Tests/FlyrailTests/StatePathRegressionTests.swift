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
        #expect(try transactionPath(destination: destination, transactionID: stateTransactionID).displayString
            == "/fixture/.flyrail-\(digest)-\(stateTransactionID).txn")
        let doubled = try ResourcePath("//\(mark)parent/\(mark)name")
        #expect(try statePathText(doubled) == "//\(mark)parent/\(mark)name")
        #expect(try transactionPath(destination: doubled, transactionID: stateTransactionID).displayString
            == "//\(mark)parent/.flyrail-\(digest)-\(stateTransactionID).txn")
        #expect(throws: FlyrailValueError("state paths must be absolute without traversal")) {
            try ResourceRef(destination: ResourcePath("/fixture/../\(mark)name"), kind: .document)
        }
        #expect(throws: FlyrailValueError("revision path must be an unambiguous relative path")) {
            try stateTestNode("/\(mark)name", Data())
        }
        #expect(throws: FlyrailValueError("revision parents must be directories")) {
            try Revision(nodes: [stateTestNode("", nil), stateTestNode("a/\(mark)name", Data())])
        }
    }
}

private let stateTransactionID = "0123456789abcdef0123456789abcdef"

private let stateExtendingBasenames = [
    ("\u{301}", "0cb4ad3a41b3fb2f5140117eee37487bd147a4f743c2270b63a0aadc2ff415f4"),
    ("\u{200D}", "5f508d364e660601c73b871734a1c0a36c3c451e5d9f47895f0da6abe843ac6a"),
]

func stateExtendingPathRecords() throws -> [JSONValue] {
    try stateExtendingBasenames.flatMap { mark, _ in
        let resources = try ["/fixture", "/\(mark)parent", "//\(mark)parent"].map { parent in
            try ResourceRef.fromState(stateExtendingResource("\(parent)/\(mark)name"))
        }
        let revision = try Revision(nodes: [
            stateTestNode("", nil), stateTestNode("a", nil), stateTestNode("a/\(mark)name", Data()),
        ])
        let markedRevision = try Revision(nodes: [
            stateTestNode("", nil), stateTestNode("\(mark)dir", nil),
            stateTestNode("\(mark)dir/\(mark)name", Data("x".utf8)),
        ])
        let ancestor = try Ancestor(
            path: ResourcePath("/\(mark)parent/\(mark)name"), device: 0, inode: 0, mode: 0, uid: 0, gid: 0,
        )
        return try resources.flatMap { resource in
            try [resource.stateRecord(), Receipt(
                resource: resource, transactionID: String(repeating: "1", count: 32),
                claims: [stateTestOwned(resource)],
            ).stateRecord()]
        } + [
            revision.stateRecord(), markedRevision.stateRecord(), ancestor.stateRecord(),
            stateTestNode("a/\(mark)name", Data()).stateRecord(),
            Claim(kind: .subtree, selector: .subtree("a/\(mark)name")).stateRecord(),
        ]
    }
}

func stateExtendingPathFailures() throws -> [StateCompatibilityVector] {
    try stateExtendingBasenames.flatMap { mark, _ in
        var vectors: [StateCompatibilityVector] = []
        func append(_ tag: String, _ record: JSONValue, _ message: String) throws {
            try vectors.append(StateCompatibilityVector(
                tag: tag, input: StateCodec.encodeRecord(record), message: message,
            ))
        }
        for (destination, message) in [
            ("/fixture/../\(mark)name", "state paths must be absolute without traversal"),
            ("/fixture/./\(mark)name", "state record is not canonical"),
            ("/fixture//\(mark)name", "state record is not canonical"),
            ("///\(mark)parent/name", "state record is not canonical"),
            ("\(mark)name", "state paths must be absolute without traversal"),
        ] {
            try append("ResourceRef", stateExtendingResource(destination), message)
            let ancestor = try Ancestor(
                path: ResourcePath("/fixture/a"), device: 0, inode: 0, mode: 0, uid: 0, gid: 0,
            ).stateRecord()
            try append("Ancestor", stateReplacingField(
                ancestor, "path", .object([("path", .string(destination))]),
            ), message)
        }
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
        let resource = try ResourceRef.fromState(stateExtendingResource("/fixture/\(mark)name"))
        let receipt = try Receipt(
            resource: resource, transactionID: String(repeating: "1", count: 32), claims: [stateTestOwned(resource)],
        ).stateRecord()
        let provenance = try stateReplacingField(
            stateField(stateField(receipt, "fields"), "provenance"),
            "empty_document", .object([("bytes", .string("f"))]),
        )
        try append("Receipt", stateReplacingField(receipt, "provenance", provenance),
                   "non-hexadecimal number found in fromhex() arg at position 1")
        return vectors
    }
}

private func stateExtendingResource(_ destination: String) -> JSONValue {
    StateCodec.record("ResourceRef", [
        ("destination", .object([("path", .string(destination))])),
        ("kind", StateCodec.enumeration("ResourceKind", "document")),
        ("schema_version", .number("2")),
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

@Test
func preparationValidatesItsFields() throws {
    let resource = try stateTestResource("a.md")
    let id = String(repeating: "1", count: 32)
    #expect(throws: FlyrailValueError("filesystem roots are not resources")) {
        try Preparation(resource: resource, transactionID: id, directory: ResourcePath("/"))
    }
    for directory in ["relative/x", "/a/../b"] {
        #expect(throws: FlyrailValueError("expected an absolute concrete path")) {
            try Preparation(resource: resource, transactionID: id, directory: ResourcePath(directory))
        }
    }
    #expect(throws: FlyrailValueError("invalid hexadecimal identity")) {
        try Preparation(resource: resource, transactionID: "1111", directory: ResourcePath("/a/b"))
    }
    #expect(throws: FlyrailValueError("preparation schema must be 2")) {
        try Preparation(resource: resource, transactionID: id, directory: ResourcePath("/a/b"), schemaVersion: 3)
    }
    let preparation = try Preparation(resource: resource, transactionID: id, directory: ResourcePath("/a/b"))
    #expect(try StateCodec.decode(StateCodec.encode(preparation), as: Preparation.self) == preparation)
}

@Test
func transactionPathsAreBesideTheDestination() throws {
    let destination = try ResourcePath("/fixture/project/AGENTS.md")
    for invalid in ["1111", String(repeating: "A", count: 32), String(repeating: "g", count: 32)] {
        #expect(throws: FlyrailValueError("transaction id must be 32 lowercase hexadecimal characters")) {
            try transactionPath(destination: destination, transactionID: invalid)
        }
    }
    let path = try transactionPath(destination: destination, transactionID: stateTransactionID)
    #expect(path.displayString.hasPrefix("/fixture/project/.flyrail-"))
    #expect(path.displayString.hasSuffix("-\(stateTransactionID).txn"))
}
