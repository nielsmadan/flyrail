@testable import Flyrail
import Foundation
import Testing

func stateSupplementalRecords() throws -> [JSONValue] {
    let container = try CreatedContainer(
        selector: Selector(parts: [.key(Key("created"))]),
        emptySyntax: Data("{}".utf8)
    )
    let ancestor = try Ancestor(
        path: ResourcePath("/pin"), device: 1, inode: Count(UInt64.max), mode: 0o7777, uid: 0, gid: 0,
        security: Data([0, 255])
    )
    let resource = try stateTestResource("supplemental.json")
    let generation = try stateTestGeneration([resource])
    let index = try Index(bundleID: "team", contextDigest: String(repeating: "d", count: 64), current: generation)
    return try [container.stateRecord(), ancestor.stateRecord(), generation.stateRecord(), index.stateRecord()]
}

@Test
func stateModelsRejectInvalidOwnershipAndRequirementShapes() throws {
    let resource = try stateTestResource("model.json")
    let claim = try Claim(kind: .file, selector: .whole)
    let revision = try Revision(nodes: [stateTestNode("", Data("value".utf8))])
    func owned(
        installed: Revision? = nil, disposition: Disposition = .created, baseline: Revision? = nil,
        requirements: [ClaimRequirement] = [], stable: [ClaimRequirement] = []
    ) throws -> OwnedClaim {
        try OwnedClaim(
            claim: claim, claimID: claim.identity(destination: resource.destination), bundleID: "team",
            artifactID: "guide", version: "1", bundleDigest: String(repeating: "a", count: 64),
            renderDigest: String(repeating: "b", count: 64), installed: installed,
            disposition: disposition, baseline: baseline, requirements: requirements, stableRequirements: stable
        )
    }
    #expect(throws: FlyrailValueError("whole claims require an installed revision")) { try owned() }
    #expect(throws: FlyrailValueError("owned revisions cannot be absent")) { try owned(installed: Revision()) }
    #expect(throws: FlyrailValueError("only takeover has a baseline")) {
        try owned(installed: revision, disposition: .takenOver)
    }
    #expect(throws: FlyrailValueError("takeover needs an original revision")) {
        try owned(installed: revision, disposition: .takenOver, baseline: Revision())
    }
    let requirement = try ClaimRequirement(
        destination: ResourcePath("/pin/required"),
        claimID: String(repeating: "f", count: 64)
    )
    #expect(throws: FlyrailValueError("duplicate claim requirement")) {
        try owned(installed: revision, requirements: [requirement, requirement])
    }
    #expect(throws: FlyrailValueError("stable requirements must be a unique subset of requirements")) {
        try owned(installed: revision, stable: [requirement])
    }
    let valid = try owned(installed: revision, requirements: [requirement], stable: [requirement])
    #expect(try StateCodec.decode(StateCodec.encode(valid), as: OwnedClaim.self) == valid)
    #expect(throws: FlyrailValueError("receipt claims overlap")) {
        try Receipt(resource: resource, transactionID: String(repeating: "1", count: 32), claims: [valid, valid])
    }
    #expect(throws: FlyrailValueError("directory provenance requires a skill container")) {
        try Receipt(
            resource: resource, transactionID: String(repeating: "1", count: 32),
            createdDirectories: [stateTestNode("", nil)]
        )
    }
    let skills = try stateTestResource("skills", kind: .skillContainer)
    #expect(throws: FlyrailValueError("created directory evidence must contain distinct directories")) {
        try Receipt(
            resource: skills,
            transactionID: String(repeating: "1", count: 32),
            createdDirectories: revision.nodes
        )
    }
}

@Test
func stateJournalAndResourcePlanBindCompleteMatchingEvidence() throws {
    let resource = try stateTestResource("plan.json")
    let receipt = try Receipt(resource: resource, transactionID: String(repeating: "1", count: 32))
    let before = try Revision(nodes: [stateTestNode("", Data("before".utf8))])
    let after = try Revision(nodes: [stateTestNode("", Data("after".utf8), inode: 2)])
    let journal = try Journal(
        resource: resource, previous: nil, receipt: receipt, before: before,
        staged: after, published: after, backup: before, ancestors: []
    )
    #expect(try StateCodec.decode(StateCodec.encode(journal), as: Journal.self) == journal)
    #expect(throws: FlyrailValueError("journal private revisions must retain complete content")) {
        try Journal(
            resource: resource, previous: nil, receipt: receipt, before: before,
            staged: after, published: before, backup: before, ancestors: []
        )
    }
    let other = try Receipt(resource: stateTestResource("other.json"), transactionID: String(repeating: "1", count: 32))
    #expect(throws: FlyrailValueError("journal receipt resources disagree")) {
        try Journal(
            resource: resource, previous: other, receipt: receipt, before: before,
            staged: after, published: after, backup: before, ancestors: []
        )
    }
    #expect(throws: FlyrailValueError("planned receipts disagree with resource")) {
        try ResourcePlan(
            resource: resource, before: before, after: after, previous: nil, receipt: other,
            ancestors: [], stateRevision: Revision(), recoveryPaths: []
        )
    }
}

@Test
func stateContextFingerprintsBindSortedExactPairsAndExcludeRouting() throws {
    let root = try ResourcePath("/pin/index")
    let first = try InstallationTarget(recordedIndexRoot: root, context: [("z", "1"), ("a", "e\u{301}")])
    let second = try InstallationTarget(
        recordedIndexRoot: root, context: [("a", "e\u{301}"), ("z", "1")], routingContext: [("agent", "claude")]
    )
    #expect(try first.contextDigest == second.contextDigest)
    let composed = try InstallationTarget(recordedIndexRoot: root, context: [("a", "é"), ("z", "1")])
    #expect(try first.contextDigest != composed.contextDigest)
    #expect(first != second)
    let exactKeys = try InstallationTarget(recordedIndexRoot: root, context: [("é", "1"), ("e\u{301}", "2")])
    #expect(exactKeys.context.count == 2)
    #expect(throws: FlyrailValueError.self) {
        try InstallationTarget(recordedIndexRoot: root, context: [("a", "1"), ("a", "2")])
    }
}
