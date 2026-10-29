import Darwin
@testable import Flyrail
import Foundation
import Testing

func transactionWrite(
    _ bundle: FlyrailBundle, rendered: RenderedBundle, target: InstallationTarget,
    acquisition: Acquisition = .conflict, replaceModified: Bool = false,
    seam: NativeTransactionSeam = .none
) throws -> InstallationResult {
    try NativeTransactionDriver.write(
        bundle: bundle, bundleID: bundle.identity.id, rendered: rendered, target: target,
        acquisition: acquisition, replaceModified: replaceModified, timeout: .zero, seam: seam
    )
}

func transactionRemove(_ owner: String, target: InstallationTarget) throws -> InstallationResult {
    try NativeTransactionDriver.write(
        bundle: nil, bundleID: owner, rendered: .empty, target: target, acquisition: .conflict,
        replaceModified: false, timeout: .zero
    )
}

@Test(arguments: [Acquisition.adopt, .takeover])
func nativeWholeAcquisitionRetainsFirstBaselineAndOwnership(_ acquisition: Acquisition) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let original = acquisition == .adopt ? "generated\n" : "foreign\n"
    try NativeTransactionIO.createFile(path, data: Data(original.utf8))
    let before = try NativeObservation.snapshot(path).revision
    let mode: FileMode = .private
    let first = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .instructions, destination: path,
                         content: .file(FileContent(data: Data("generated\n".utf8), mode: mode))),
    ])
    #expect(try transactionWrite(transactionBundle(), rendered: first, target: target,
                                 acquisition: acquisition).observation.isCurrent)
    if acquisition == .adopt { #expect(try NativeObservation.snapshot(path).revision == before) }
    let ref = try NativeAuthority.reference(path, kind: .document)
    let receipt = try #require(NativeAdmission.observe(ref).receipt)
    #expect(receipt.claims.first?.disposition == (acquisition == .adopt ? .adopted : .takenOver))
    #expect(receipt.claims.first?.baseline == (acquisition == .adopt ? nil : before))
    #expect(try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second\n"),
                                 target: target).observation.isCurrent)
    let updated = try #require(NativeAdmission.observe(ref).receipt)
    #expect(updated.claims.first?.baseline == receipt.claims.first?.baseline)
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == (acquisition == .adopt ? nil : Data(original.utf8)))
}

@Test
func nativeRetargetRetiresTheOldPathAndPreservesBothBaselines() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let first = try root.child("first")
    let second = try root.child("second")
    try NativeTransactionIO.createFile(first, data: Data("first baseline".utf8))
    try NativeTransactionIO.createFile(second, data: Data("second baseline".utf8))
    let target = try InstallationTarget(indexRoot: root.child("index"))
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(first),
                                 target: target, acquisition: .takeover).status == .applied)
    #expect(try transactionWrite(transactionBundle("2"), rendered: transactionRendered(second),
                                 target: target, acquisition: .takeover).observation.isCurrent)
    #expect(try NativeObservation.snapshot(first).revision.data == Data("first baseline".utf8))
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(second).revision.data == Data("second baseline".utf8))
}

@Test
func nativeSharedDocumentClaimsAndRestrictiveMetadataSurviveAcrossOwners() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    try NativeTransactionIO.createFile(path, data: Data("# foreign\n".utf8))
    let first = try InstallationTarget(indexRoot: root.child("first"))
    let second = try InstallationTarget(indexRoot: root.child("second"))
    func section(_ marker: String) throws -> RenderedBundle {
        try RenderedBundle(artifacts: [
            RenderedArtifact(id: "guide", family: .instructions, destination: path,
                             content: .section(SectionContent(marker: marker, text: "generated"))),
        ])
    }
    #expect(try transactionWrite(transactionBundle(), rendered: section("one"), target: first).observation.isCurrent)
    #expect(try transactionWrite(transactionBundle(owner: "other"), rendered: section("two"),
                                 target: second).observation.isCurrent)
    #expect(chmod(path.displayString, 0o400) == 0)
    #expect(try transactionRemove("team", target: first).status == .applied)
    let ref = try NativeAuthority.reference(path, kind: .document)
    #expect(try NativeAdmission.observe(ref).receipt?.claims.map(\.bundleID) == ["other"])
    #expect(try NativeObservation.snapshot(path).revision.nodes.first?.mode == 0o400)
    #expect(try transactionRemove("other", target: second).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("# foreign\n".utf8))
    #expect(try NativeObservation.snapshot(path).revision.nodes.first?.mode == 0o400)
}

@Test
func nativeSkillContainerDirectoryProvenancePrunesOnlyUnchangedEmptyParents() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("skills")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let tree = try TreeContent(entries: [BundleEntry(path: "SKILL.md", data: Data("skill".utf8))])
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .skills, destination: path, content: .tree(tree), subtree: "team/guide"),
    ])
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target).observation.isCurrent)
    let ref = try NativeAuthority.reference(path, kind: .skillContainer)
    #expect(try NativeAdmission.observe(ref).receipt?.createdDirectories.map(\.path) == ["", "team"])
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.nodes.isEmpty)
}

@Test
func nativeAllKnownConflictsFailBeforeAnyResourcePublication() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let first = try root.child("first")
    let second = try root.child("second")
    try NativeTransactionIO.createFile(second, data: Data("foreign".utf8))
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "one", family: .instructions, destination: first,
                         content: .file(FileContent(data: Data("one".utf8)))),
        RenderedArtifact(id: "two", family: .instructions, destination: second,
                         content: .file(FileContent(data: Data("two".utf8)))),
    ])
    let result = try transactionWrite(transactionBundle(), rendered: rendered, target: target)
    #expect(result.status == .failed)
    #expect(result.error?.code == .conflict)
    #expect(try NativeObservation.snapshot(first).revision.nodes.isEmpty)
    #expect(try NativeAdmission.observeIndex(target, bundleID: "team").current == nil)
    #expect(try NativeObservation.snapshot(second).revision.data == Data("foreign".utf8))
}
