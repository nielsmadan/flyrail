import Darwin
@testable import Flyrail
import FlyrailInteropSupport
import Foundation
import Testing

@Test
func bundleFixturesMatchMemoryDirectoryAndArchiveSnapshots() throws {
    let fixture = try bundleFixture()
    for rawCase in fixture["cases"] as? [[String: Any]] ?? [] {
        let manifest = rawCase["manifest"] as? [String: Any] ?? [:]
        let manifestData = try JSONSerialization.data(withJSONObject: manifest, options: [.sortedKeys])
        let sourceEntries = try fixtureSourceEntries(rawCase)
        let expectedEntries = try fixtureExpectedEntries(rawCase)
        let memory = try FlyrailBundle(manifest: manifestData, entries: sourceEntries)
        #expect(memory.entries == expectedEntries)
        #expect(memory.contentDigest == rawCase["content_digest"] as? String)
        #expect(memory.sourceRoots.isEmpty)

        let root = try temporaryDirectory(name: rawCase["name"] as? String ?? "bundle")
        try writeFixtureCase(rawCase, to: root)
        let subpaths = try FileManager.default.subpathsOfDirectory(atPath: root.path)
        if subpaths.allSatisfy({ exactTextEqual($0, nfc($0)) }) {
            let directory = try FlyrailBundle(directory: ResourcePath(root.path))
            #expect(directory.entries == expectedEntries)
            #expect(directory.contentDigest == memory.contentDigest)
            #expect(try directory.sourceRoots == [ResourcePath(root.path)])
        }

        let archiveRoot = try temporaryDirectory(name: "archive-\(rawCase["name"] as? String ?? "bundle")")
        let archiveURL = archiveRoot.appending(path: "bundle.zip")
        try fixtureArchive(rawCase).write(to: archiveURL)
        let archivePath = try ResourcePath(archiveURL.path)
        let archived = try FlyrailBundle(archive: archivePath, root: ResourcePath("flyrail"))
        #expect(archived.entries == expectedEntries)
        #expect(archived.contentDigest == memory.contentDigest)
        #expect(archived.sourceRoots == [archivePath])
        try FileManager.default.removeItem(at: archiveURL)
        #expect(archived.entries == expectedEntries)
        #expect(archived.contentDigest == memory.contentDigest)
        #expect(archived.sourceRoots == [archivePath])

        if let framing = rawCase["framing_hex"] as? String {
            let expectedFraming = try Data(interopHex: framing)
            #expect(contentFraming(for: expectedEntries) == expectedFraming)
        }
    }
}

@Test(.enabled(if: repositoryCheckout))
func packagedBundleFixtureMatchesRoot() throws {
    let packagedURL = try #require(Bundle.module.url(
        forResource: "bundles",
        withExtension: "json",
        subdirectory: "Fixtures"
    ))
    let packaged = try Data(contentsOf: packagedURL)
    #expect(try packaged == Data(contentsOf: repositoryFile("spec/fixtures/bundles.json")))
}

@Test
func bundleSnapshotSurvivesSourceMutation() throws {
    let root = try temporaryDirectory(name: "detached")
    let skill = root.appending(path: "skills/review", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: skill, withIntermediateDirectories: true)
    let manifest =
        #"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"skills/review"}]}"#
    try Data(manifest.utf8).write(to: root.appending(path: "flyrail.json"))
    try Data("before".utf8).write(to: skill.appending(path: "SKILL.md"))
    try FileManager.default.createDirectory(at: skill.appending(path: "empty"), withIntermediateDirectories: true)

    let bundle = try FlyrailBundle(directory: ResourcePath(root.path))
    try Data("after".utf8).write(to: skill.appending(path: "SKILL.md"))
    try FileManager.default.removeItem(at: skill.appending(path: "empty"))
    #expect(try bundle.entries == [
        BundleEntry(path: "review"),
        BundleEntry(path: "review/SKILL.md", data: Data("before".utf8)),
        BundleEntry(path: "review/empty"),
    ])
}

@Test
func directorySourceRejectsChangesObservedDuringLoading() throws {
    let root = try temporaryDirectory(name: "changing")
    let skill = root.appending(path: "review", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: skill, withIntermediateDirectories: true)
    let manifest =
        #"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review"}]}"#
    try Data(manifest.utf8).write(to: root.appending(path: "flyrail.json"))
    let markdown = skill.appending(path: "SKILL.md")
    try Data("before".utf8).write(to: markdown)

    let source = try DirectoryBundleSource(root: ResourcePath(root.path))
    defer { source.close() }
    _ = try source.manifest()
    var budget = BundleEntryBudget(limits: .default)
    _ = try source.tree(spec: SkillSpec(name: "review", path: "review"), budget: &budget)
    try Data("after".utf8).write(to: markdown)

    #expect(throws: FlyrailValueError.self) {
        try source.finish()
    }
}

@Test
func memoryBundlesInferDirectoriesAndRejectCollisions() throws {
    let manifest = Data(
        #"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"skills/review"}]}"#
            .utf8
    )
    let bundle = try FlyrailBundle(
        manifest: manifest,
        entries: [BundleEntry(path: "skills/review/SKILL.md", data: Data())]
    )
    #expect(bundle.entries.map(\.path) == ["review", "review/SKILL.md"])
    #expect(throws: FlyrailValueError.self) {
        try FlyrailBundle(
            manifest: manifest,
            entries: [
                BundleEntry(path: "skills/review/SKILL.md", data: Data()),
                BundleEntry(path: "skills/REVIEW/other", data: Data()),
            ]
        )
    }
    #expect(throws: FlyrailValueError.self) {
        try snapshotEntries([
            BundleEntry(path: "a", data: Data()),
            BundleEntry(path: "a/b", data: Data()),
        ])
    }
}

@Test
func schemaOneManifestValidationIsStrict() throws {
    let valid = #"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review"}]}"#
    let rejected = [
        valid.replacingOccurrences(of: #""id":"team""#, with: #""id":"team","id":"team""#),
        valid.replacingOccurrences(of: #""schema_version":1"#, with: #""schema_version":true"#),
        valid.replacingOccurrences(of: #""path":"review""#, with: #""path":"other""#),
        valid.replacingOccurrences(of: #""skills":[{"name":"review","path":"review"}]"#, with: #""skills":[]"#),
        valid.replacingOccurrences(of: "}", with: #","unknown":null}"#),
    ]
    for text in rejected {
        #expect(throws: Error.self, "must reject \(text)") {
            try FlyrailBundle(manifest: Data(text.utf8), entries: [])
        }
    }
    #expect(throws: FlyrailValueError.self) {
        try FlyrailBundle(
            manifest: Data([0xEF, 0xBB, 0xBF] + Array(valid.utf8)),
            entries: []
        )
    }
}

@Test
func directorySourcesRejectLinksSpecialFilesAndPortableCollisions() throws {
    let root = try temporaryDirectory(name: "unsafe-source")
    let skill = root.appending(path: "review", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: skill, withIntermediateDirectories: true)
    let manifest = #"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review"}]}"#
    try Data(manifest.utf8).write(to: root.appending(path: "flyrail.json"))
    try Data().write(to: skill.appending(path: "SKILL.md"))
    try FileManager.default.createDirectory(at: skill.appending(path: "same"), withIntermediateDirectories: false)
    do {
        try FileManager.default.createDirectory(at: skill.appending(path: "SAME"), withIntermediateDirectories: false)
        #expect(throws: FlyrailValueError.self) {
            try FlyrailBundle(directory: ResourcePath(root.path))
        }
        try FileManager.default.removeItem(at: skill.appending(path: "SAME"))
    } catch CocoaError.fileWriteFileExists {}
    try FileManager.default.removeItem(at: skill.appending(path: "same"))
    try FileManager.default.createSymbolicLink(
        at: skill.appending(path: "linked"),
        withDestinationURL: skill.appending(path: "SKILL.md")
    )
    #expect(throws: FlyrailValueError.self) {
        try FlyrailBundle(directory: ResourcePath(root.path))
    }
    try FileManager.default.removeItem(at: skill.appending(path: "linked"))
    let fifo = skill.appending(path: "pipe").path
    #expect(mkfifo(fifo, 0o600) == 0)
    #expect(throws: FlyrailValueError.self) {
        try FlyrailBundle(directory: ResourcePath(root.path))
    }
}

@Test
func directorySourcesRejectNonNFCNamesWithoutRenamingThem() throws {
    let root = try temporaryDirectory(name: "non-nfc-source")
    let skill = root.appending(path: "review", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: skill, withIntermediateDirectories: true)
    let manifest = #"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review"}]}"#
    try Data(manifest.utf8).write(to: root.appending(path: "flyrail.json"))
    try Data().write(to: skill.appending(path: "SKILL.md"))
    let decomposed = "cafe\u{301}.txt"
    try Data().write(to: skill.appending(path: decomposed))
    #expect(throws: FlyrailValueError.self) {
        try FlyrailBundle(directory: ResourcePath(root.path))
    }
}

private func bundleFixture() throws -> [String: Any] {
    let url = try #require(Bundle.module.url(
        forResource: "bundles",
        withExtension: "json",
        subdirectory: "Fixtures"
    ))
    return try #require(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
}

private func fixtureSourceEntries(_ rawCase: [String: Any]) throws -> [BundleEntry] {
    var result = try (rawCase["directories"] as? [String] ?? []).map {
        try BundleEntry(path: $0)
    }
    for file in rawCase["files"] as? [[String: Any]] ?? [] {
        try result.append(BundleEntry(
            path: #require(file["path"] as? String),
            data: Data(interopHex: #require(file["data_hex"] as? String))
        ))
    }
    return result
}

private func fixtureExpectedEntries(_ rawCase: [String: Any]) throws -> [BundleEntry] {
    try (rawCase["entries"] as? [[String: Any]] ?? []).map { entry in
        let text = entry["data_hex"] as? String
        return try BundleEntry(
            path: #require(entry["path"] as? String),
            data: text.map { try Data(interopHex: $0) },
            executable: entry["executable"] as? Bool ?? false
        )
    }
}

private func writeFixtureCase(_ rawCase: [String: Any], to root: URL) throws {
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    let manifest = try JSONSerialization.data(
        withJSONObject: rawCase["manifest"] as? [String: Any] ?? [:],
        options: [.sortedKeys]
    )
    try manifest.write(to: root.appending(path: "flyrail.json"))
    for directory in rawCase["directories"] as? [String] ?? [] {
        try FileManager.default.createDirectory(
            at: root.appending(path: directory),
            withIntermediateDirectories: true
        )
    }
    for file in rawCase["files"] as? [[String: Any]] ?? [] {
        let path = try root.appending(path: #require(file["path"] as? String))
        try FileManager.default.createDirectory(
            at: path.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try Data(interopHex: #require(file["data_hex"] as? String)).write(to: path)
    }
}

private func fixtureArchive(_ rawCase: [String: Any]) throws -> Data {
    var members: [(name: String, data: Data)] = try [
        (
            "flyrail/flyrail.json",
            JSONSerialization.data(
                withJSONObject: rawCase["manifest"] as? [String: Any] ?? [:],
                options: [.sortedKeys]
            )
        ),
    ]
    members.append(contentsOf: (rawCase["directories"] as? [String] ?? []).map {
        ("flyrail/\($0)/", Data())
    })
    for file in rawCase["files"] as? [[String: Any]] ?? [] {
        try members.append((
            "flyrail/\(#require(file["path"] as? String))",
            Data(interopHex: #require(file["data_hex"] as? String))
        ))
    }
    var body = Data()
    var central = Data()
    for member in members {
        let name = Array(member.name.utf8)
        let checksum = fixtureCRC32(member.data)
        let offset = UInt32(body.count)
        body.appendLittleEndian(UInt32(0x0403_4B50))
        body.appendLittleEndian(UInt16(20))
        body.appendLittleEndian(UInt16(0x0800))
        body.appendLittleEndian(UInt16(0))
        body.appendLittleEndian(UInt32(0))
        body.appendLittleEndian(checksum)
        body.appendLittleEndian(UInt32(member.data.count))
        body.appendLittleEndian(UInt32(member.data.count))
        body.appendLittleEndian(UInt16(name.count))
        body.appendLittleEndian(UInt16(0))
        body.append(contentsOf: name)
        body.append(member.data)

        central.appendLittleEndian(UInt32(0x0201_4B50))
        central.appendLittleEndian(UInt16(3 << 8 | 20))
        central.appendLittleEndian(UInt16(20))
        central.appendLittleEndian(UInt16(0x0800))
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(UInt32(0))
        central.appendLittleEndian(checksum)
        central.appendLittleEndian(UInt32(member.data.count))
        central.appendLittleEndian(UInt32(member.data.count))
        central.appendLittleEndian(UInt16(name.count))
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(member.name.hasSuffix("/") ? UInt32(0o040755) << 16 : 0)
        central.appendLittleEndian(offset)
        central.append(contentsOf: name)
    }
    let directoryOffset = UInt32(body.count)
    body.append(central)
    body.appendLittleEndian(UInt32(0x0605_4B50))
    body.appendLittleEndian(UInt32(0))
    body.appendLittleEndian(UInt16(members.count))
    body.appendLittleEndian(UInt16(members.count))
    body.appendLittleEndian(UInt32(central.count))
    body.appendLittleEndian(directoryOffset)
    body.appendLittleEndian(UInt16(0))
    return body
}

private func fixtureCRC32(_ data: Data) -> UInt32 {
    var checksum = UInt32.max
    for byte in data {
        checksum ^= UInt32(byte)
        for _ in 0 ..< 8 {
            checksum = checksum & 1 == 0 ? checksum >> 1 : (checksum >> 1) ^ 0xEDB8_8320
        }
    }
    return ~checksum
}

private func temporaryDirectory(name: String) throws -> URL {
    let root = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        .appending(path: ".cache/swift-tests/\(name)-\(UUID().uuidString)", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    return root
}

private extension Data {
    mutating func appendLittleEndian(_ value: some FixedWidthInteger) {
        var encoded = value.littleEndian
        Swift.withUnsafeBytes(of: &encoded) { append(contentsOf: $0) }
    }
}
