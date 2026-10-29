@testable import Flyrail
import FlyrailInteropSupport
import Foundation
import Testing

@Test(.enabled(if: repositoryCheckout))
func editingDifferentialMatchesCompletePythonRecords() throws {
    let cases = try editingDifferentialCases()
    let root = stateRepositoryRoot()
    _ = try repositoryFile("python/src/flyrail")
    let directory = root.appending(path: ".cache/swift-tests/editing-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let corpus = directory.appending(path: "cases.json")
    try JSONSerialization.data(withJSONObject: cases, options: [.sortedKeys]).write(to: corpus)
    let reference = try editingPythonReference(root: root, corpus: corpus)
    var actual: [String] = []
    for item in cases {
        let name = try #require(item["name"] as? String)
        let format = try #require(item["format"] as? String)
        var source = try editingSource(item)
        var ownership: [OwnedSelection] = []
        var provenance = DocumentProvenance()
        let steps = try #require(item["steps"] as? [[String: Any]])
        for (index, step) in steps.enumerated() {
            let observed = step.keys.contains("observed")
                ? (step["observed"] as? String).map { Data($0.utf8) }
                : source
            let record: [String: Any]
            do {
                let requests = try (step["requests"] as? [[String: Any]] ?? [step])
                    .map { try fixtureRequest(format: format, record: $0) }
                let edit: DocumentEdit
                if item["schema"] as? Bool == true {
                    let schemas = try (step["schemas"] as? [[String: Any]] ?? []).map { schema in
                        let content = try fixtureRequest(format: format, record: [
                            "action": "set", "selector": [schema["key"] as? String ?? ""],
                            "value": schema["value"] ?? NSNull(),
                        ])
                        guard case let .structured(value)? = content.content,
                              case let .scalar(scalar) = value.value
                        else {
                            throw FlyrailValueError("invalid schema fixture")
                        }
                        return try DocumentSchema(
                            format: value.format,
                            key: schema["key"] as? String ?? "",
                            value: scalar,
                        )
                    }
                    edit = try editSchemaDocument(
                        source: observed, requests: requests, ownership: ownership, provenance: provenance,
                        activeSchemas: schemas, validate: step["validate"] as? Bool ?? true,
                    )
                } else {
                    edit = try editDocument(
                        source: observed,
                        requests: requests,
                        ownership: ownership,
                        provenance: provenance,
                    )
                }
                record = try editingRecord(edit)
                source = edit.after
                ownership = edit.ownership
                provenance = edit.provenance
            } catch let error as FlyrailValueError {
                record = ["error": error.message]
            }
            try actual.append("\(name)\t\(index)\t" + editingRecordText(record))
        }
    }
    try compareEditingRecords(actual, reference: reference)
    _ = try repositoryFile("rust/Cargo.toml")
    let rustCases = cases.filter { $0["format"] as? String == "toml" || $0["schema"] as? Bool == true }
    let rustCorpus = directory.appending(path: "rust-cases.json")
    try JSONSerialization.data(withJSONObject: rustCases, options: [.sortedKeys]).write(to: rustCorpus)
    let names = Set(rustCases.compactMap { $0["name"] as? String })
    let rustActual = actual.filter { names.contains(String($0.split(separator: "\t")[0])) }
    try compareEditingRecords(rustActual, reference: editingRustReference(root: root, corpus: rustCorpus))
}

private func compareEditingRecords(_ actual: [String], reference: [String]) throws {
    #expect(actual.count == reference.count)
    for (actualLine, expectedLine) in zip(actual, reference) {
        let actualColumns = actualLine.split(separator: "\t", maxSplits: 2)
        let expectedColumns = expectedLine.split(separator: "\t", maxSplits: 2)
        #expect(actualColumns.prefix(2) == expectedColumns.prefix(2))
        let actual = try JSONParser.parse(Data(actualColumns[2].utf8))
        let expected = try JSONParser.parse(JSONWriter.canonicalData(JSONParser.parse(Data(expectedColumns[2].utf8))))
        if actual != expected {
            Issue.record("\(actualColumns[0]) step \(actualColumns[1]): \(editingDifference(actual, expected))")
        }
    }
}

private func editingRustReference(root: URL, corpus: URL) throws -> [String] {
    let process = Process()
    let output = Pipe()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.currentDirectoryURL = root.appending(path: "rust")
    process.arguments = [
        "cargo",
        "test",
        "--lib",
        "editing_differential::complete_editing_records",
        "--",
        "--nocapture",
    ]
    var environment = ProcessInfo.processInfo.environment
    environment["CARGO_HOME"] = root.appending(path: ".cache/cargo-home").path
    environment["FLYRAIL_EDIT_CORPUS"] = corpus.path
    process.environment = environment
    process.standardOutput = output
    process.standardError = output
    try process.run()
    let bytes = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    let text = String(decoding: bytes, as: UTF8.self)
    guard process.terminationStatus == 0 else { throw FlyrailValueError(text) }
    return text.split(separator: "\n").filter {
        let fields = $0.split(separator: "\t", maxSplits: 2)
        return fields.count == 3 && Int(fields[1]) != nil
    }.map(String.init)
}

private func editingDifference(_ actual: JSONValue, _ expected: JSONValue, path: String = "") -> String {
    switch (actual, expected) {
    case let (.object(left), .object(right)):
        left.compactMap { key, value in
            guard let expected = right.first(where: { $0.0 == key })?.1 else { return "\(path).\(key): missing" }
            return value == expected ? nil : editingDifference(value, expected, path: path + "." + key)
        }.joined(separator: "; ")
    case let (.array(left), .array(right)) where left.count == right.count:
        zip(left, right).enumerated().compactMap { index, pair in
            pair.0 == pair.1 ? nil : editingDifference(pair.0, pair.1, path: path + "[\(index)]")
        }.joined(separator: "; ")
    default:
        "\(path): \(actual) != \(expected)"
    }
}

private func editingPythonReference(root _: URL, corpus: URL) throws -> [String] {
    let process = Process()
    let output = Pipe()
    process.executableURL = try repositoryFile("python/.venv/bin/python3")
    process.arguments = try [
        "-B", repositoryFile("swift/scripts/reference_value_driver.py").path,
        "--edits", corpus.path,
    ]
    process.standardOutput = output
    process.standardError = output
    try process.run()
    let bytes = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    let text = String(decoding: bytes, as: UTF8.self)
    guard process.terminationStatus == 0 else {
        throw FlyrailValueError(text)
    }
    return text.split(separator: "\n").map(String.init)
}

private func editingSource(_ record: [String: Any]) throws -> Data? {
    if let hex = record["before_hex"] as? String {
        return try Data(interopHex: hex)
    }
    return (record["before"] as? String).map { Data($0.utf8) }
}

private func editingRecordText(_ record: [String: Any]) throws -> String {
    let data = try JSONSerialization.data(withJSONObject: record, options: [.sortedKeys, .withoutEscapingSlashes])
    return String(decoding: data, as: UTF8.self)
}

private func editingRecord(_ edit: DocumentEdit) throws -> [String: Any] {
    try [
        "before": editingBytes(edit.before),
        "after": editingBytes(edit.after),
        "applicable": edit.applicable,
        "ownership": edit.ownership.map(editingOwnedRecord),
        "provenance": [
            "empty_document": editingBytes(edit.provenance.emptyDocument),
            "containers": edit.provenance.containers.map {
                try [
                    "selector": editingSelectorRecord($0.selector),
                    "empty_syntax": $0.emptySyntax.nativeHex,
                ]
            },
            "trailing_separators": edit.provenance.trailingSeparators,
            "schema_fields": edit.provenance.schemaFields.map(editingOwnedRecord),
            "schema_empty_document": editingBytes(edit.provenance.schemaEmptyDocument),
        ],
        "observations": edit.observations.map {
            try [
                "claim": editingClaimRecord($0.claim),
                "status": $0.status.rawValue,
                "current": editingSnapshotRecord($0.current),
                "message": $0.message,
            ]
        },
    ]
}

private func editingBytes(_ value: Data?) -> Any {
    value.map(\.nativeHex) as Any? ?? NSNull()
}

private func editingClaimRecord(_ claim: Claim) throws -> [String: Any] {
    let selector: Any
    switch claim.selector {
    case let .section(boundaries):
        selector = ["start": boundaries.start, "end": boundaries.end]
    case let .structured(structured):
        selector = try editingSelectorRecord(structured)
    default:
        throw FlyrailValueError("invalid differential claim")
    }
    return ["kind": claim.kind.rawValue, "selector": selector]
}

private func editingSelectorRecord(_ selector: Flyrail.Selector) throws -> [Any] {
    try selector.parts.map {
        switch $0 {
        case let .key(key): key.name
        case let .member(member): try editingMemberRecord(member)
        }
    }
}

private func editingMemberRecord(_ member: Member) throws -> [String: Any] {
    try ["identity": member.identity.semanticBytes().nativeHex, "key": member.key]
}

private func editingSnapshotRecord(_ snapshot: SelectionSnapshot?) throws -> Any {
    guard let snapshot else { return NSNull() }
    let value: [String: String] = switch snapshot.value {
    case let .bytes(bytes):
        ["bytes": bytes.nativeHex]
    case let .typed(value):
        try ["semantic": value.semanticBytes().nativeHex]
    }
    let position: Any = if let selected = snapshot.position {
        try [
            "previous": selected.previous.map(editingMemberRecord) as Any? ?? NSNull(),
            "following": selected.following.map(editingMemberRecord) as Any? ?? NSNull(),
            "prefix": selected.prefix.nativeHex,
        ]
    } else {
        NSNull()
    }
    return [
        "value": value,
        "syntax": snapshot.syntax.nativeHex,
        "owned_syntax": snapshot.ownedSyntax.nativeHex,
        "position": position,
        "layout": snapshot.layout.nativeHex,
    ]
}

private func editingOwnedRecord(_ owned: OwnedSelection) throws -> [String: Any] {
    try [
        "claim": editingClaimRecord(owned.claim),
        "format": owned.format?.rawValue as Any? ?? NSNull(),
        "installed": editingSnapshotRecord(owned.installed),
        "disposition": owned.disposition.rawValue,
        "baseline": editingSnapshotRecord(owned.baseline),
        "separator": owned.separator.nativeHex,
    ]
}
