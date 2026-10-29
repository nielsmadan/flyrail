@testable import Flyrail
import Foundation
import Testing

private struct ZIPSpec {
    var name: [UInt8]
    var data = Data()
    var payload: Data?
    var method: UInt16 = 0
    var flags: UInt16 = 0
    var createSystem: UInt8 = 3
    var externalAttributes: UInt32 = 0
    var descriptorSignature = true
}

@Test
func ZIPBundlesLoadStoredDeflatedUTF8AndCP437Members() throws {
    let manifest = Data(
        #"{"schema_version":1,"id":"tiny","version":"1","skills":[{"name":"a","path":"a"}]}"#.utf8,
    )
    let stored = makeZIP([
        ZIPSpec(name: Array("flyrail/flyrail.json".utf8), data: manifest),
        ZIPSpec(name: Array("flyrail/a/SKILL.md".utf8), data: Data()),
    ])
    let storedBundle = try loadZIP(stored)
    #expect(try storedBundle.entries == [
        BundleEntry(path: "a"),
        BundleEntry(path: "a/SKILL.md", data: Data()),
    ])

    let deflated = makeZIP([
        ZIPSpec(name: Array("flyrail/flyrail.json".utf8), data: manifest),
        ZIPSpec(
            name: Array("flyrail/a/SKILL.md".utf8),
            data: Data("hello".utf8),
            payload: Data([0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00]),
            method: 8,
        ),
    ])
    #expect(try loadZIP(deflated).entries.last?.data == Data("hello".utf8))

    let cp437 = makeZIP([
        ZIPSpec(name: Array("flyrail/flyrail.json".utf8), data: manifest),
        ZIPSpec(name: Array("flyrail/a/SKILL.md".utf8), data: Data()),
        ZIPSpec(name: Array("flyrail/a/caf".utf8) + [0x82] + Array(".txt".utf8), data: Data()),
    ])
    #expect(try loadZIP(cp437).entries.map(\.path) == ["a", "a/SKILL.md", "a/café.txt"])

    let descriptor = makeZIP([
        ZIPSpec(name: Array("flyrail/flyrail.json".utf8), data: manifest),
        ZIPSpec(name: Array("flyrail/a/SKILL.md".utf8), data: Data("descriptor".utf8), flags: 0x0008),
    ])
    #expect(try loadZIP(descriptor).entries.last?.data == Data("descriptor".utf8))

    let signatureCRC = Data([0xAC, 0x0A, 0x7A, 0xD5])
    let unsignedDescriptor = makeZIP([
        ZIPSpec(name: Array("flyrail/flyrail.json".utf8), data: manifest),
        ZIPSpec(
            name: Array("flyrail/a/SKILL.md".utf8),
            data: signatureCRC,
            flags: 0x0008,
            descriptorSignature: false,
        ),
    ])
    #expect(try loadZIP(unsignedDescriptor).entries.last?.data == signatureCRC)
}

@Test
func ZIPParserReadsZIP64Metadata() throws {
    let archive = makeZIP64(name: Array("large.txt".utf8), data: Data("payload".utf8))
    let parsed = try ZIPArchive(data: archive, limits: .default)
    #expect(parsed.members.count == 1)
    #expect(parsed.members[0].name == "large.txt")
    #expect(try parsed.data(at: 0, limits: .default) == Data("payload".utf8))

    #expect(throws: FlyrailValueError.self) {
        var malformed = archive
        let central = try #require(signatureOffset(0x0201_4B50, in: malformed))
        malformed[central + 46] = 2
        let parsed = try ZIPArchive(data: malformed, limits: .default)
        _ = try parsed.data(at: 0, limits: .default)
    }
}

@Test
func ZIPParserRejectsCorruptionFlagsMismatchesAndOverlaps() throws {
    let base = validZIP()
    #expect(throws: FlyrailValueError.self) {
        _ = try ZIPArchive(data: Data(base.dropLast()), limits: .default)
    }
    #expect(throws: FlyrailValueError.self) {
        var archive = base
        archive[30] ^= 1
        _ = try firstMemberData(archive)
    }
    #expect(throws: FlyrailValueError.self) {
        var archive = base
        archive[8] = 8
        _ = try firstMemberData(archive)
    }
    #expect(throws: FlyrailValueError.self) {
        var archive = base
        let central = try #require(signatureOffset(0x0201_4B50, in: archive))
        archive[central + 8] |= 1
        _ = try firstMemberData(archive)
    }
    #expect(throws: FlyrailValueError.self) {
        var archive = base
        let central = try #require(signatureOffset(0x0201_4B50, in: archive))
        archive[central + 16] ^= 1
        _ = try firstMemberData(archive)
    }
    #expect(throws: FlyrailValueError.self) {
        var archive = base
        let first = try #require(signatureOffset(0x0201_4B50, in: archive))
        let second = try #require(signatureOffset(0x0201_4B50, in: archive, after: first + 4))
        archive.replaceSubrange(second + 42 ..< second + 46, with: [0, 0, 0, 0])
        _ = try ZIPArchive(data: archive, limits: .default)
    }
}

@Test
func ZIPSourcesRejectTraversalCollisionsNULAndSpecialEntries() throws {
    let manifest = Data(
        #"{"schema_version":1,"id":"tiny","version":"1","skills":[{"name":"a","path":"a"}]}"#.utf8,
    )
    let required = [
        ZIPSpec(name: Array("flyrail/flyrail.json".utf8), data: manifest),
        ZIPSpec(name: Array("flyrail/a/SKILL.md".utf8), data: Data()),
    ]
    for name in ["flyrail/a/../bad", "flyrail/a/bad?", "flyrail/a/cafe\u{301}"] {
        #expect(throws: FlyrailValueError.self) {
            _ = try loadZIP(makeZIP(required + [ZIPSpec(name: Array(name.utf8), data: Data())]))
        }
    }
    #expect(throws: FlyrailValueError.self) {
        _ = try loadZIP(makeZIP(required + [
            ZIPSpec(name: Array("flyrail/a/same".utf8), data: Data()),
            ZIPSpec(name: Array("flyrail/a/SAME".utf8), data: Data()),
        ]))
    }
    #expect(throws: FlyrailValueError.self) {
        _ = try loadZIP(makeZIP(required + [
            ZIPSpec(name: Array("flyrail/a/name".utf8) + [0] + Array("hidden".utf8), data: Data()),
        ]))
    }
    #expect(throws: FlyrailValueError.self) {
        _ = try loadZIP(makeZIP(required + [
            ZIPSpec(
                name: Array("flyrail/a/link".utf8),
                data: Data("target".utf8),
                externalAttributes: UInt32(0o120600) << 16,
            ),
        ]))
    }
}

@Test
func ZIPBundlesIgnoreUnrelatedPayloadFormatsAndCorruption() throws {
    let manifest = Data(
        #"{"schema_version":1,"id":"tiny","version":"1","skills":[{"name":"a","path":"a"}]}"#.utf8,
    )
    let archive = makeZIP([
        ZIPSpec(name: Array("ignored-encrypted".utf8), data: Data(), flags: 0x0001),
        ZIPSpec(name: Array("ignored-method".utf8), data: Data(), method: 12),
        ZIPSpec(name: Array("ignored-corrupt".utf8), data: Data("expected".utf8), payload: Data("wrong".utf8)),
        ZIPSpec(name: Array("flyrail/flyrail.json".utf8), data: manifest),
        ZIPSpec(name: Array("flyrail/a/SKILL.md".utf8), data: Data("selected".utf8)),
    ])
    #expect(try loadZIP(archive).entries.last?.data == Data("selected".utf8))
}

@Test
func ZIPSourceTraversesDeepSelectedTreesWithoutRebuildingPrefixes() throws {
    let depth = 1024
    let nestedPath = Array(repeating: "d", count: depth).joined(separator: "/")
    let leafPath = "flyrail/a/\(nestedPath)/leaf.txt"
    let archive = try ZIPArchive(
        data: makeZIP([
            ZIPSpec(name: Array(leafPath.utf8), data: Data("leaf".utf8)),
        ]),
        limits: .default,
    )
    let source = try ZIPBundleSource(archive: archive, root: "flyrail")
    let spec = try SkillSpec(
        name: "a",
        path: "a",
        executables: ["\(nestedPath)/leaf.txt"],
    )
    var budget = BundleEntryBudget(limits: .default)

    let entries = try source.tree(spec: spec, budget: &budget)
    let expectedLeaf = try BundleEntry(
        path: "a/\(nestedPath)/leaf.txt",
        data: Data("leaf".utf8),
        executable: true,
    )

    #expect(entries.count == depth + 2)
    #expect(entries[1].path == "a/d")
    #expect(entries.last == expectedLeaf)
}

@Test
func ZIPParserEnforcesEveryLimitBeforeAllocation() throws {
    let archive = validZIP()
    #expect(throws: FlyrailValueError.self) {
        _ = try ZIPArchive(
            data: archive,
            limits: BundleLimits(
                maximumArchiveBytes: UInt64(archive.count - 1),
                maximumMemberCount: 10,
                maximumMemberBytes: 1000,
                maximumTotalBytes: 2000,
                maximumNameBytes: 1000,
                maximumExpansionRatio: 1000,
            ),
        )
    }
    let metadataLimits = [
        BundleLimits(
            maximumArchiveBytes: 10000,
            maximumMemberCount: 1,
            maximumMemberBytes: 1000,
            maximumTotalBytes: 2000,
            maximumNameBytes: 1000,
            maximumExpansionRatio: 1000,
        ),
        BundleLimits(
            maximumArchiveBytes: 10000,
            maximumMemberCount: 10,
            maximumMemberBytes: 1000,
            maximumTotalBytes: 2000,
            maximumNameBytes: 2,
            maximumExpansionRatio: 1000,
        ),
    ]
    for limits in metadataLimits {
        #expect(throws: FlyrailValueError.self) {
            _ = try ZIPArchive(data: archive, limits: limits)
        }
    }
    for limits in [
        BundleLimits(
            maximumArchiveBytes: 10000,
            maximumMemberCount: 10,
            maximumMemberBytes: 1,
            maximumTotalBytes: 2000,
            maximumNameBytes: 1000,
            maximumExpansionRatio: 1000,
        ),
        BundleLimits(
            maximumArchiveBytes: 10000,
            maximumMemberCount: 10,
            maximumMemberBytes: 1000,
            maximumTotalBytes: 1,
            maximumNameBytes: 1000,
            maximumExpansionRatio: 1000,
        ),
    ] {
        #expect(throws: FlyrailValueError.self) {
            let parsed = try ZIPArchive(data: archive, limits: limits)
            _ = try parsed.data(at: 0, limits: limits)
        }
    }
    let deflated = makeZIP([
        ZIPSpec(
            name: Array("x".utf8),
            data: Data("hello".utf8),
            payload: Data([0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00]),
            method: 8,
        ),
    ])
    #expect(throws: FlyrailValueError.self) {
        let limits = BundleLimits(
            maximumArchiveBytes: 10000,
            maximumMemberCount: 10,
            maximumMemberBytes: 1000,
            maximumTotalBytes: 2000,
            maximumNameBytes: 1000,
            maximumExpansionRatio: 0,
        )
        let parsed = try ZIPArchive(
            data: deflated,
            limits: limits,
        )
        _ = try parsed.data(at: 0, limits: limits)
    }
}

private func validZIP() -> Data {
    makeZIP([
        ZIPSpec(name: Array("flyrail/flyrail.json".utf8), data: Data("{}".utf8)),
        ZIPSpec(name: Array("flyrail/a/SKILL.md".utf8), data: Data("skill".utf8)),
    ])
}

private func loadZIP(_ data: Data) throws -> FlyrailBundle {
    let directory = try temporaryZIPDirectory()
    let archive = directory.appending(path: "bundle.zip")
    try data.write(to: archive)
    return try FlyrailBundle(
        archive: ResourcePath(archive.path),
        root: ResourcePath("flyrail"),
    )
}

private func temporaryZIPDirectory() throws -> URL {
    let url = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        .appending(path: ".cache/swift-tests/zip-\(UUID().uuidString)", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

private func makeZIP(_ specs: [ZIPSpec]) -> Data {
    var body = Data()
    var central = Data()
    for spec in specs {
        let payload = spec.payload ?? spec.data
        let checksum = testCRC32(spec.data)
        let offset = UInt32(body.count)
        var flags = spec.flags
        if let decoded = String(bytes: spec.name, encoding: .utf8), !decoded.allSatisfy(\.isASCII) {
            flags |= 0x0800
        }
        body.appendLittleEndian(UInt32(0x0403_4B50))
        body.appendLittleEndian(UInt16(20))
        body.appendLittleEndian(flags)
        body.appendLittleEndian(spec.method)
        body.appendLittleEndian(UInt16(0))
        body.appendLittleEndian(UInt16(0))
        body.appendLittleEndian(spec.flags & 0x0008 == 0 ? checksum : UInt32(0))
        body.appendLittleEndian(spec.flags & 0x0008 == 0 ? UInt32(payload.count) : UInt32(0))
        body.appendLittleEndian(spec.flags & 0x0008 == 0 ? UInt32(spec.data.count) : UInt32(0))
        body.appendLittleEndian(UInt16(spec.name.count))
        body.appendLittleEndian(UInt16(0))
        body.append(contentsOf: spec.name)
        body.append(payload)
        if spec.flags & 0x0008 != 0 {
            if spec.descriptorSignature {
                body.appendLittleEndian(UInt32(0x0807_4B50))
            }
            body.appendLittleEndian(checksum)
            body.appendLittleEndian(UInt32(payload.count))
            body.appendLittleEndian(UInt32(spec.data.count))
        }

        central.appendLittleEndian(UInt32(0x0201_4B50))
        central.appendLittleEndian(UInt16(spec.createSystem) << 8)
        central.appendLittleEndian(UInt16(20))
        central.appendLittleEndian(flags)
        central.appendLittleEndian(spec.method)
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(checksum)
        central.appendLittleEndian(UInt32(payload.count))
        central.appendLittleEndian(UInt32(spec.data.count))
        central.appendLittleEndian(UInt16(spec.name.count))
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(UInt16(0))
        central.appendLittleEndian(spec.externalAttributes)
        central.appendLittleEndian(offset)
        central.append(contentsOf: spec.name)
    }
    let directoryOffset = UInt32(body.count)
    body.append(central)
    body.appendLittleEndian(UInt32(0x0605_4B50))
    body.appendLittleEndian(UInt16(0))
    body.appendLittleEndian(UInt16(0))
    body.appendLittleEndian(UInt16(specs.count))
    body.appendLittleEndian(UInt16(specs.count))
    body.appendLittleEndian(UInt32(central.count))
    body.appendLittleEndian(directoryOffset)
    body.appendLittleEndian(UInt16(0))
    return body
}

private func firstMemberData(_ archive: Data) throws -> Data {
    let parsed = try ZIPArchive(data: archive, limits: .default)
    return try parsed.data(at: 0, limits: .default)
}

private func makeZIP64(name: [UInt8], data: Data) -> Data {
    let checksum = testCRC32(data)
    var localExtra = Data()
    localExtra.appendLittleEndian(UInt16(1))
    localExtra.appendLittleEndian(UInt16(16))
    localExtra.appendLittleEndian(UInt64(data.count))
    localExtra.appendLittleEndian(UInt64(data.count))

    var body = Data()
    body.appendLittleEndian(UInt32(0x0403_4B50))
    body.appendLittleEndian(UInt16(45))
    body.appendLittleEndian(UInt16(0))
    body.appendLittleEndian(UInt16(0))
    body.appendLittleEndian(UInt16(0))
    body.appendLittleEndian(UInt16(0))
    body.appendLittleEndian(checksum)
    body.appendLittleEndian(UInt32.max)
    body.appendLittleEndian(UInt32.max)
    body.appendLittleEndian(UInt16(name.count))
    body.appendLittleEndian(UInt16(localExtra.count))
    body.append(contentsOf: name)
    body.append(localExtra)
    body.append(data)

    var centralExtra = Data()
    centralExtra.appendLittleEndian(UInt16(1))
    centralExtra.appendLittleEndian(UInt16(24))
    centralExtra.appendLittleEndian(UInt64(data.count))
    centralExtra.appendLittleEndian(UInt64(data.count))
    centralExtra.appendLittleEndian(UInt64(0))
    var central = Data()
    central.appendLittleEndian(UInt32(0x0201_4B50))
    central.appendLittleEndian(UInt16(3 << 8 | 45))
    central.appendLittleEndian(UInt16(45))
    central.appendLittleEndian(UInt16(0))
    central.appendLittleEndian(UInt16(0))
    central.appendLittleEndian(UInt16(0))
    central.appendLittleEndian(UInt16(0))
    central.appendLittleEndian(checksum)
    central.appendLittleEndian(UInt32.max)
    central.appendLittleEndian(UInt32.max)
    central.appendLittleEndian(UInt16(name.count))
    central.appendLittleEndian(UInt16(centralExtra.count))
    central.appendLittleEndian(UInt16(0))
    central.appendLittleEndian(UInt16(0))
    central.appendLittleEndian(UInt16(0))
    central.appendLittleEndian(UInt32(0))
    central.appendLittleEndian(UInt32.max)
    central.append(contentsOf: name)
    central.append(centralExtra)

    let directoryOffset = UInt64(body.count)
    body.append(central)
    let end64Offset = UInt64(body.count)
    body.appendLittleEndian(UInt32(0x0606_4B50))
    body.appendLittleEndian(UInt64(44))
    body.appendLittleEndian(UInt16(45))
    body.appendLittleEndian(UInt16(45))
    body.appendLittleEndian(UInt32(0))
    body.appendLittleEndian(UInt32(0))
    body.appendLittleEndian(UInt64(1))
    body.appendLittleEndian(UInt64(1))
    body.appendLittleEndian(UInt64(central.count))
    body.appendLittleEndian(directoryOffset)
    body.appendLittleEndian(UInt32(0x0706_4B50))
    body.appendLittleEndian(UInt32(0))
    body.appendLittleEndian(end64Offset)
    body.appendLittleEndian(UInt32(1))
    body.appendLittleEndian(UInt32(0x0605_4B50))
    body.appendLittleEndian(UInt16(0))
    body.appendLittleEndian(UInt16(0))
    body.appendLittleEndian(UInt16.max)
    body.appendLittleEndian(UInt16.max)
    body.appendLittleEndian(UInt32(central.count))
    body.appendLittleEndian(UInt32.max)
    body.appendLittleEndian(UInt16(0))
    return body
}

private func signatureOffset(_ signature: UInt32, in data: Data, after start: Int = 0) -> Int? {
    let bytes = withUnsafeBytes(of: signature.littleEndian) { Array($0) }
    return data.indices.dropFirst(start).dropLast(3).first {
        Array(data[$0 ..< $0 + 4]) == bytes
    }
}

private func testCRC32(_ data: Data) -> UInt32 {
    var checksum = UInt32.max
    for byte in data {
        checksum ^= UInt32(byte)
        for _ in 0 ..< 8 {
            checksum = checksum & 1 == 0 ? checksum >> 1 : (checksum >> 1) ^ 0xEDB8_8320
        }
    }
    return ~checksum
}

private extension Data {
    mutating func appendLittleEndian(_ value: some FixedWidthInteger) {
        var encoded = value.littleEndian
        Swift.withUnsafeBytes(of: &encoded) { append(contentsOf: $0) }
    }
}
