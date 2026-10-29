@testable import Flyrail
import Foundation
import Testing

@Test(.enabled(if: repositoryCheckout))
func stateFixtureBytesHaveNotDrifted() throws {
    let packaged = try #require(Bundle.module.url(
        forResource: "configurations",
        withExtension: "json",
        subdirectory: "Fixtures",
    ))
    let data = try Data(contentsOf: packaged)
    _ = try JSONParser.parse(data)
    #expect(try data == Data(contentsOf: repositoryFile("spec/fixtures/configurations.json")))
}

@Test
func stateClaimAndAuthorityFixturesPinPhysicalIdentity() throws {
    let fixture = try stateFixture()
    var claims: [String: Claim] = [:]
    for item in try stateArray(stateField(fixture, "claims")) {
        let name = try stateText(stateField(item, "name"))
        let claim = try fixtureClaim(item)
        let destination = try ResourcePath(stateText(stateField(item, "destination")))
        #expect(try claim.identity(destination: destination) == stateText(stateField(item, "identity")))
        #expect(try StateCodec.decode(StateCodec.encode(claim), as: Claim.self) == claim)
        claims[name] = claim
    }
    for item in try stateArray(stateField(fixture, "claim_overlaps")) {
        let first = try #require(claims[stateText(stateField(item, "first"))])
        let second = try #require(claims[stateText(stateField(item, "second"))])
        #expect(try first.overlaps(second) == (stateField(item, "overlaps") == .boolean(true)))
    }
    for item in try stateArray(stateField(fixture, "authorities")) {
        let destination = try ResourcePath(stateText(stateField(item, "destination")))
        let authority = try ResourceAuthority(recordedDestination: destination)
        #expect(try statePathText(authority.stateRoot) == stateText(stateField(item, "state_root")))
        #expect(try statePathText(authority.lockPath) == stateText(stateField(item, "lock_path")))
    }
}

@Test
func stateReceiptsJournalsAndRecoverySnapshotsRoundTrip() throws {
    let fixture = try stateRelocatedFixture(stateFixture())
    for item in try stateArray(stateField(fixture, "receipts")) {
        let raw = try StateCodec.encodeRecord(stateField(item, "record"))
        let receipt = try StateCodec.decode(raw, as: Receipt.self)
        #expect(try StateCodec.encode(receipt) == raw)
        #expect(receipt.schemaVersion == 2)
        for owned in receipt.claims {
            #expect(try owned.claim.identity(destination: receipt.resource.destination) == owned.claimID)
        }
    }
    var journals: [String: Journal] = [:]
    for item in try stateArray(stateField(fixture, "journals")) {
        let raw = try StateCodec.encodeRecord(stateField(item, "record"))
        let journal = try StateCodec.decode(raw, as: Journal.self)
        #expect(try StateCodec.encode(journal) == raw)
        #expect(try journal.before.contentDigest == journal.backup.contentDigest)
        #expect(try journal.staged.contentDigest == journal.published.contentDigest)
        try journals[stateText(stateField(item, "name"))] = journal
    }
    var snapshots = 0
    for item in try stateArray(stateField(fixture, "transactions")) {
        let journal = try #require(journals[stateText(stateField(item, "journal"))])
        let observed = try stateField(item, "observed")
        let expected = try stateField(item, "expected")
        for record in try [
            stateField(observed, "resource"), stateField(observed, "backup"),
            stateField(observed, "staged"), stateField(expected, "resource"),
        ] where record != .null {
            let raw = try StateCodec.encodeRecord(record)
            let revision = try StateCodec.decode(raw, as: Revision.self)
            #expect(try StateCodec.encode(revision) == raw)
            snapshots += 1
        }
        let expectedResource = try stateField(expected, "resource")
        if expectedResource != .null {
            let revision = try Revision.fromState(expectedResource)
            let receipt = try stateText(stateField(expected, "receipt"))
            if receipt == "old", try stateText(stateField(expected, "status")) == "recovered" {
                #expect(revision == journal.before)
            }
        }
    }
    #expect(snapshots == 26)
}

@Test
func everyNestedDurableFixtureRecordIsStrictlyDecoded() throws {
    let records = try nestedStateRecords(stateRelocatedFixture(stateFixture())) + stateSupplementalRecords()
    var tags: Set<String> = []
    for record in records {
        let raw = try StateCodec.encodeRecord(record)
        #expect(try StateCodec.transcode(raw) == raw)
        try tags.insert(#require(stateRecordTag(record)))
        guard case let .object(items) = record,
              case let .object(fields) = items.first(where: { $0.0 == "fields" })?.1
        else { continue }
        for (name, _) in fields {
            let missing = try StateCodec.record(
                #require(stateRecordTag(record)), fields.filter { $0.0 != name },
            )
            #expect(throws: FlyrailValueError.self) {
                try StateCodec.transcode(StateCodec.encodeRecord(missing))
            }
        }
        let unknown = try StateCodec.record(
            #require(stateRecordTag(record)), fields + [("unexpected", .null)],
        )
        #expect(throws: FlyrailValueError.self) {
            try StateCodec.transcode(StateCodec.encodeRecord(unknown))
        }
    }
    #expect(tags.isSuperset(of: [
        "Claim", "Key", "Member", "Selector", "SectionBoundaries", "ArrayPosition", "CreatedContainer",
        "DocumentProvenance", "OwnedSelection", "SelectionSnapshot", "Node", "Revision", "ResourceRef",
        "OwnedClaim", "Receipt", "Journal", "Ancestor", "DocumentSchema", "Generation", "Index",
    ]))
}

func stateRepositoryRoot() -> URL {
    URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
}

func stateFixture() throws -> JSONValue {
    let url = try #require(Bundle.module.url(
        forResource: "configurations",
        withExtension: "json",
        subdirectory: "Fixtures",
    ))
    return try JSONParser.parse(Data(contentsOf: url))
}

func stateField(_ value: JSONValue, _ name: String) throws -> JSONValue {
    guard case let .object(items) = value, let field = items.first(where: { $0.0 == name }) else {
        throw FlyrailValueError("missing fixture field: \(name)")
    }
    return field.1
}

func stateArray(_ value: JSONValue) throws -> [JSONValue] {
    guard case let .array(items) = value else { throw FlyrailValueError("invalid fixture sequence") }
    return items
}

func nestedStateRecords(_ value: JSONValue) -> [JSONValue] {
    switch value {
    case let .array(items): items.flatMap(nestedStateRecords)
    case let .object(items):
        (stateRecordTag(value) != nil && items.contains(where: { $0.0 == "fields" }) ? [value] : []) +
            items.flatMap { nestedStateRecords($0.1) }
    default: []
    }
}

private func fixtureClaim(_ value: JSONValue) throws -> Claim {
    let kind = try #require(Ownership(rawValue: stateText(stateField(value, "kind"))))
    let raw = try stateField(value, "selector")
    let selector: ClaimSelector
    switch kind {
    case .file, .tree: selector = .whole
    case .subtree: selector = try .subtree(stateText(raw))
    case .section:
        let items = try stateArray(raw)
        selector = try .section(SectionBoundaries(start: stateText(items[0]), end: stateText(items[1])))
    case .structured:
        selector = try .structured(Selector(parts: stateArray(raw).map { part in
            let items = try stateArray(part)
            if try stateText(items[0]) == "key" {
                return try .key(Key(stateText(items[1])))
            }
            return try .member(Member(
                identity: SemanticValue(semanticBytes: JSONWriter.canonicalData(items[2])),
                key: stateArray(items[1]).map(stateText),
            ))
        }))
    }
    return try Claim(kind: kind, selector: selector)
}
