@testable import Flyrail
import FlyrailInteropSupport
import Foundation
import Testing

@Test(.enabled(if: repositoryCheckout))
func stateCodecPairsMatchPythonAndRustInBothDirections() throws {
    let root = stateRepositoryRoot()
    let python = try repositoryFile("python/.venv/bin/python")
    let driver = try repositoryFile("python/scripts/interop_driver.py")
    let manifest = try repositoryFile("rust/Cargo.toml")
    var environment = ProcessInfo.processInfo.environment
    environment["CARGO_HOME"] = root.appending(path: ".cache/cargo-home").path
    let build = try stateProcess(
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: [
            "cargo", "build", "--locked", "--quiet", "--example", "interop", "--features", "interop-test-seams",
            "--manifest-path", manifest.path,
        ],
        root: root, environment: environment,
    )
    try #require(build.status == 0, "\(String(decoding: build.error, as: UTF8.self))")
    let rust = root.appending(path: "rust/target/debug/examples/interop")
    let compatibility = try stateCompatibilityVectors()
    let records = try statePairRecords() + stateExtendingPathRecords() +
        compatibility.filter(\.message.isEmpty).map {
            try JSONParser.parse($0.input.starts(with: [0xEF, 0xBB, 0xBF]) ? Data($0.input.dropFirst(3)) : $0.input)
        }
    let enclosingRecords = try statePairRecords() + statePairEnclosingRecords(records)
    for record in records {
        let swift = try StateCodec.encodeRecord(record)
        let pythonResult = try stateProcess(
            executable: python,
            arguments: ["-B", driver.path, "encode"],
            input: swift,
            root: root,
        )
        #expect(pythonResult.status == 0, "\(String(decoding: pythonResult.error, as: UTF8.self))")
        #expect(pythonResult.output == swift)
        #expect(try StateCodec.transcode(pythonResult.output) == swift)
    }
    for record in Set(records.filter(stateRustSupports) + enclosingRecords) {
        let swift = try StateCodec.encodeRecord(record)
        let tag = try #require(stateRecordTag(record))
        let rustResult = try stateProcess(
            executable: rust,
            arguments: ["encode", "--type", tag],
            input: swift,
            root: root,
        )
        #expect(rustResult.status == 0, "\(String(decoding: rustResult.error, as: UTF8.self))")
        #expect(rustResult.output == swift)
        if rustResult.status == 0 {
            #expect(try StateCodec.transcode(rustResult.output) == swift)
        }
        let reverse = try stateProcess(
            executable: python, arguments: ["-B", driver.path, "encode"], input: rustResult.output, root: root,
        )
        #expect(reverse.status == 0)
        #expect(reverse.output == swift)
    }
    try stateNestedPairsMatchPython(python: python, driver: driver, root: root)
    let malformed = try statePairMalformedRecords(records) +
        compatibility.filter { !$0.message.isEmpty } + stateExtendingPathFailures()
    #expect(!malformed.isEmpty)
    var checkedMalformed = 0
    for vector in malformed {
        let tag = vector.tag
        let input = vector.input
        #expect(throws: FlyrailValueError(vector.message)) { try StateCodec.transcode(input) }
        let pythonResult = try stateProcess(
            executable: python,
            arguments: ["-B", driver.path, "encode"],
            input: input,
            root: root,
        )
        #expect(pythonResult.status == 1)
        #expect(pythonResult.output == Data())
        #expect(String(decoding: pythonResult.error, as: UTF8.self) == vector.message + "\n",
                "\(tag): \(String(decoding: input, as: UTF8.self))")
        let rustVector = try stateRustMalformedVector(vector, enclosing: enclosingRecords)
        #expect(throws: FlyrailValueError(vector.message)) { try StateCodec.transcode(rustVector.input) }
        let rustResult = try stateProcess(
            executable: rust,
            arguments: ["encode", "--type", rustVector.tag],
            input: rustVector.input,
            root: root,
        )
        #expect(rustResult.status == 1)
        #expect(rustResult.output == Data())
        #expect(String(decoding: rustResult.error, as: UTF8.self) == "decode \(rustVector.tag): \(vector.message)\n",
                "\(tag): \(String(decoding: input, as: UTF8.self))")
        checkedMalformed += 1
    }
    #expect(checkedMalformed == malformed.count)
}

private func stateNestedPairsMatchPython(python: URL, driver: URL, root: URL) throws {
    let semantic = try stateArray(stateField(stateFixture(), "semantic_values")).map {
        try JSONValue.object([("value", stateField($0, "record"))])
    }
    let paths: [JSONValue] = [.object([("path", .string("//fixture/é.md"))]), .object([("path", .string("//"))])]
    let nested = try Set(
        nestedStateRecords(stateRelocatedFixture(stateFixture())) + stateSupplementalRecords() + semantic + paths,
    )
    for record in nested {
        let swift = try StateCodec.encodeRecord(record)
        let result = try stateProcess(
            executable: python,
            arguments: ["-B", driver.path, "encode"],
            input: swift,
            root: root,
        )
        #expect(result.status == 0, "\(String(decoding: result.error, as: UTF8.self))")
        #expect(result.output == swift)
        #expect(try StateCodec.transcode(result.output) == swift)
    }
}

private func stateRustSupports(_ record: JSONValue) -> Bool {
    ["Index", "Journal", "Receipt", "ResourceRef"].contains(stateRecordTag(record) ?? "")
}

private func statePairEnclosingRecords(_ records: [JSONValue]) throws -> [JSONValue] {
    let resource = try stateTestResource("nested.json")
    let container = try CreatedContainer(
        selector: Selector(parts: [.key(Key("created"))]), emptySyntax: Data("{}".utf8),
    )
    let receipt = try Receipt(
        resource: resource, transactionID: String(repeating: "1", count: 32),
        provenance: DocumentProvenance(emptyDocument: nil, containers: [container]),
    )
    func journal(_ revision: Revision, ancestors: [Ancestor] = []) throws -> JSONValue {
        try Journal(
            resource: resource, previous: nil, receipt: receipt, before: revision,
            staged: Revision(), published: Revision(), backup: revision, ancestors: ancestors,
        ).stateRecord()
    }
    return try [receipt.stateRecord()] + stateSupplementalRecords().filter(stateRustSupports) +
        records.flatMap { record -> [JSONValue] in
            switch stateRecordTag(record) {
            case "Revision":
                return try [journal(Revision.fromState(record))]
            case "Ancestor":
                return try [journal(Revision(), ancestors: [Ancestor.fromState(record)])]
            case "Node":
                let node = try Node.fromState(record)
                let components = node.path.utf8.split(separator: 47, omittingEmptySubsequences: false)
                    .map { String(decoding: $0, as: UTF8.self) }
                let parents = try (0 ..< components.count).map { index in
                    try stateTestNode(components.prefix(index).joined(separator: "/"), nil)
                }.filter { $0.path != node.path }
                return try [journal(Revision(nodes: parents + [node]))]
            case "Claim":
                let claim = try Claim.fromState(record)
                guard claim.kind == .subtree else { return [] }
                let skills = try stateTestResource("nested-skills", kind: .skillContainer)
                let owned = try OwnedClaim(
                    claim: claim, claimID: claim.identity(destination: skills.destination),
                    bundleID: "team", artifactID: "guide", version: "1",
                    bundleDigest: String(repeating: "a", count: 64), renderDigest: String(repeating: "b", count: 64),
                    installed: Revision(nodes: [stateTestNode("", Data())]),
                )
                return try [Receipt(
                    resource: skills, transactionID: String(repeating: "1", count: 32), claims: [owned],
                ).stateRecord()]
            default:
                return []
            }
        }
}

private func stateRustMalformedVector(
    _ vector: StateCompatibilityVector, enclosing records: [JSONValue],
) throws -> StateCompatibilityVector {
    if ["Index", "Journal", "Receipt", "ResourceRef"].contains(vector.tag) {
        return vector
    }
    let enclosing = try #require(records.first {
        nestedStateRecords($0).contains { stateRecordTag($0) == vector.tag }
    }, "No supported enclosing Rust record for \(vector.tag)")
    let nested = try #require(nestedStateRecords(enclosing).first { stateRecordTag($0) == vector.tag })
    let text = try String(decoding: StateCodec.encodeRecord(enclosing), as: UTF8.self)
    let nestedText = try String(decoding: JSONWriter.canonicalData(nested), as: UTF8.self)
    let range = try #require(text.range(of: nestedText))
    let embedded = text.replacingCharacters(in: range, with: String(decoding: vector.input, as: UTF8.self))
    return try StateCompatibilityVector(
        tag: #require(stateRecordTag(enclosing)), input: Data(embedded.utf8), message: vector.message,
    )
}

@Test
func stateInteropCodecCommandsValidateAndReportRecords() throws {
    let root = stateRepositoryRoot()
    let directory = root.appending(path: ".cache/swift/state-interop-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let claim = try Claim(kind: .file, selector: .whole)
    let encoded = try StateCodec.encode(claim)
    let input = directory.appending(path: "input")
    let output = directory.appending(path: "output")
    let error = directory.appending(path: "error")
    func run(_ arguments: [String], _ data: Data) throws -> (Int32, Data, Data) {
        try data.write(to: input)
        try Data().write(to: output)
        try Data().write(to: error)
        let stdin = try FileHandle(forReadingFrom: input)
        let stdout = try FileHandle(forWritingTo: output)
        let stderr = try FileHandle(forWritingTo: error)
        defer {
            try? stdin.close()
            try? stdout.close()
            try? stderr.close()
        }
        let status = runInterop(
            arguments: arguments,
            standardInput: stdin,
            standardOutput: stdout,
            standardError: stderr,
        )
        return try (status, Data(contentsOf: output), Data(contentsOf: error))
    }
    for command in ["encode", "decode"] {
        let result = try run([command, "--type", "Claim"], encoded)
        #expect(result.0 == 0)
        #expect(result.1 == encoded)
        #expect(result.2 == Data())
    }
    let request = try JSONWriter.canonicalData(.array([
        .object([("path", .string("/fixture/state/receipt.json")), ("bytes", .string(encoded.base64EncodedString()))]),
    ]))
    let result = try run(["records"], request)
    #expect(result.0 == 0)
    let records = try stateArray(stateField(JSONParser.parse(result.1), "records"))
    #expect(records.count == 1)
    #expect(try stateField(records[0], "type") == .string("Claim"))
    #expect(try stateField(records[0], "canonical") == .string(encoded.base64EncodedString()))
    let native = try run(["records", "--index", directory.appending(path: "absent-index").path], Data())
    #expect(native.0 == 0)
    #expect(try stateField(JSONParser.parse(native.1), "records") == .array([]))
    for arguments in [
        ["encode", "--type", "Receipt"], ["encode", "--type", "Claim", "--type", "Claim"],
        ["encode", "--bad"], ["unknown"], ["write"], ["records", "--index", "/fixture", "--unexpected", "value"],
    ] {
        let failure = try run(arguments, encoded)
        #expect(failure.0 == 1)
        #expect(!failure.2.isEmpty)
    }
}

private func statePairRecords() throws -> [JSONValue] {
    let fixture = try stateRelocatedFixture(stateFixture())
    let fixtures = try ["receipts", "journals"].flatMap { key in
        try stateArray(stateField(fixture, key)).map { try stateField($0, "record") }
    }
    let resource = try stateTestResource("pair.json")
    let highCount = try Count(decimal: "18446744073709551616")
    let receipt = try Receipt(
        resource: resource, transactionID: String(repeating: "1", count: 32),
        claims: [stateTestOwned(resource, version: "e\u{301} \0", inode: highCount)],
    )
    let generation = try stateTestGeneration([resource])
    let first = try Index(bundleID: "team", contextDigest: String(repeating: "d", count: 64), pending: generation)
    let complete = try Index(
        bundleID: "team", contextDigest: String(repeating: "d", count: 64), current: generation, residual: [resource],
    )
    return try fixtures + [
        resource.stateRecord(), receipt.stateRecord(), first.stateRecord(), complete.stateRecord(),
        Index(bundleID: "team", contextDigest: String(repeating: "d", count: 64)).stateRecord(),
    ]
}

private func statePairMalformedRecords(_ records: [JSONValue]) throws -> [StateCompatibilityVector] {
    try records.flatMap { record -> [StateCompatibilityVector] in
        let tag = try #require(stateRecordTag(record))
        let fields = try stateField(record, "fields")
        guard case let .object(items) = fields else { throw FlyrailValueError("invalid record") }
        var result = try items.map { name, _ in
            try StateCompatibilityVector(
                tag: tag, input: StateCodec.encodeRecord(StateCodec.record(tag, items.filter { $0.0 != name })),
                message: "state record fields do not match schema",
            )
        }
        try result.append(StateCompatibilityVector(
            tag: tag, input: StateCodec.encodeRecord(StateCodec.record(tag, items + [("unexpected", .null)])),
            message: "state record fields do not match schema",
        ))
        if tag == "ResourceRef" {
            let replacement = StateCodec.record(tag, items.map { key, value in
                (key, key == "state_root" ? .object([("path", .string("/wrong/state"))]) : value)
            })
            try result.append(StateCompatibilityVector(
                tag: tag, input: StateCodec.encodeRecord(replacement),
                message: "resource state path disagrees with destination",
            ))
        }
        return result
    }
}

private struct StateProcessResult {
    let status: Int32
    let output: Data
    let error: Data
}

private func stateProcess(
    executable: URL, arguments: [String], input: Data = Data(), root: URL,
    environment: [String: String]? = nil,
) throws -> StateProcessResult {
    let process = Process()
    let stdin = Pipe()
    let stdout = Pipe()
    let stderr = Pipe()
    process.executableURL = executable
    process.arguments = arguments
    process.currentDirectoryURL = root
    process.environment = environment
    process.standardInput = stdin
    process.standardOutput = stdout
    process.standardError = stderr
    try process.run()
    try stdin.fileHandleForWriting.write(contentsOf: input)
    try stdin.fileHandleForWriting.close()
    let output = stdout.fileHandleForReading.readDataToEndOfFile()
    let error = stderr.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    return StateProcessResult(status: process.terminationStatus, output: output, error: error)
}
