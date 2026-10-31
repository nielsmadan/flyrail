import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func nativeSameGenerationRepairRollbackRetainsModifiedPreimage() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try root.target()
    let store = target.store
    let bundle = try transactionBundle()
    let rendered = try transactionRendered(path)
    #expect(try transactionWrite(bundle, rendered: rendered, target: target).status == .applied)
    let ref = try NativeAuthority.reference(path, kind: .document)
    let previous = try #require(try storedReceipt(path, store: store))
    #expect(try transactionWrite(bundle, rendered: rendered, target: target).status == .unchanged)
    #expect(try storedReceipt(path, store: store) == previous)
    let receiptBytes = try store.read(StateKey.resource(path, .receipt))
    try Data("local edited content\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
    let original = try NativeObservation.snapshot(path).revision
    let plan = try NativeResourcePlanner.plan(
        ref, state: ResourceAdmission.observe(ref, store: store), store: store, bundleID: "team",
        bundle: bundle, rendered: rendered, artifacts: rendered.artifacts,
        acquisition: .conflict, replaceModified: true, transactionID: previous.transactionID,
    )
    #expect(plan.receipt.transactionID != previous.transactionID)
    let journal = try ResourceTransaction.prepare(plan, store: store)
    let (staged, backup) = try TransactionLayout.resources(plan.transactionDirectory)
    let publishFault = NativeTransactionSeam {
        if $0 == "backup" {
            throw TargetError(code: .ioError, message: "backup interrupted")
        }
    }
    expectNativeError(.ioError) { try ResourceTransaction.publish(journal, store: store, seam: publishFault) }
    #expect(try NativeObservation.snapshot(path).revision.nodes.isEmpty)
    #expect(try NativeObservation.snapshot(backup).revision == original)
    #expect(try NativeObservation.snapshot(staged).revision == journal.staged)
    #expect(try StateRecords.read(
        store, key: StateKey.resource(path, .journal), as: Journal.self, tag: "Journal",
    ) == journal)
    let recoveryFault = NativeTransactionSeam {
        if $0 == "rollback-backup" {
            throw TargetError(code: .ioError, message: "rollback interrupted")
        }
    }
    expectNativeError(.ioError) { try ResourceTransaction.recover(ref, store: store, seam: recoveryFault) }
    #expect(try NativeObservation.snapshot(path).revision == original)
    #expect(try NativeObservation.snapshot(backup).revision.nodes.isEmpty)
    #expect(try NativeObservation.snapshot(staged).revision == journal.staged)
    try ResourceTransaction.recover(ref, store: store)
    try ResourceTransaction.recover(ref, store: store)
    #expect(try NativeObservation.snapshot(path).revision == original)
    #expect(try storedReceipt(path, store: store) == previous)
    #expect(try store.read(StateKey.resource(path, .receipt)) == receiptBytes)
    #expect(try transactionRecordKeys(store) == [])
    #expect(!FileManager.default.fileExists(atPath: plan.transactionDirectory.displayString))
    #expect(try FileManager.default.contentsOfDirectory(atPath: root.stateURL.appending(path: "staging").path) == [])
}

@Test
func nativeSameGenerationRepairFaultIsNotReportedAsCommitted() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try root.target()
    let bundle = try transactionBundle()
    let rendered = try transactionRendered(path)
    #expect(try transactionWrite(bundle, rendered: rendered, target: target).status == .applied)
    try Data("local edited content\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
    let before = try NativeObservation.snapshot(path).revision
    let result = try transactionWrite(
        bundle, rendered: rendered, target: target, replaceModified: true, seam: NativeTransactionSeam {
            if $0 == "backup" {
                throw TargetError(code: .ioError, message: "backup interrupted")
            }
        },
    )
    #expect(result.status == .failed)
    #expect(result.resources.map(\.status) == [.failed])
    #expect(result.resources.first?.recoveryPaths == [])
    for _ in 0 ..< 2 {
        #expect(try transactionRecover(target).error == nil)
    }
    #expect(try NativeObservation.snapshot(path).revision == before)
    #expect(try transactionWrite(bundle, rendered: rendered, target: target,
                                 replaceModified: true).status == .applied)
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.nodes.isEmpty)
}

@Test(arguments: [3 * 1024 * 1024, 4 * 1024 * 1024 + 1])
func nativePredictableStateOversizeLeavesNoTransactionEvidence(_ size: Int) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try root.target()
    let original = Data(repeating: 97, count: size)
    try NativeTransactionIO.createFile(path, data: original)
    let before = try NativeObservation.snapshot(path)
    for _ in 0 ..< 2 {
        let result = try transactionWrite(transactionBundle(), rendered: transactionRendered(path, "new\n"),
                                          target: target, acquisition: .takeover)
        #expect(result.status == .failed)
        #expect(result.error?.code == .invalidState)
        #expect(result.error?.message == (size == 3 * 1024 * 1024
                ? "JSON input exceeds the byte limit" : "JSON string exceeds the byte limit"))
        #expect(try StateRecords.keys(target.store, prefix: "") == [StateKey.resource(path, .authority)])
        #expect(try transactionRecordKeys(target.store) == [])
        #expect(try FileManager.default.contentsOfDirectory(atPath: root.url.path) == ["document"])
        #expect(try NativeObservation.snapshot(path) == before)
        #expect(try transactionRecover(target).status == .unchanged)
        #expect(try transactionRemove("team", target: target).status == .unchanged)
    }
    try Data("small baseline\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path, "new\n"),
                                 target: target, acquisition: .takeover).status == .applied)
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("small baseline\n".utf8))
}

@Test
func nativeOversizedRepairPreservesExistingIndexAndCanBeRemoved() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try root.target()
    let rendered = try transactionRendered(path, String(repeating: "a", count: 3 * 1024 * 1024))
    let creation = try transactionWrite(transactionBundle(), rendered: rendered, target: target)
    #expect(creation.status == .failed)
    #expect(creation.error?.code == .invalidState)
    #expect(creation.error?.message == "JSON input exceeds the byte limit")
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path), target: target)
        .status == .applied)
    try Data("local edit".utf8).write(to: URL(fileURLWithPath: path.displayString))
    let before = try NativeObservation.snapshot(root.path)
    let storeBefore = try storeContents(target.store)
    let index = try InstallationIndex.read(bundleID: "team", target: target)
    let repair = try transactionWrite(transactionBundle(), rendered: rendered, target: target, replaceModified: true)
    #expect(repair.status == .failed)
    #expect(repair.error?.code == .invalidState)
    #expect(repair.error?.message == "JSON input exceeds the byte limit")
    for _ in 0 ..< 2 {
        #expect(try transactionRecover(target).error == nil)
    }
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(try storeContents(target.store) == storeBefore)
    #expect(try InstallationIndex.read(bundleID: "team", target: target) == index)
    #expect(remove(bundleID: "team", target: target, replaceModified: true).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.nodes.isEmpty)
}

@Test(arguments: [0o555, 0o500])
func nativeReadonlyTreeTakeoverFailsBeforePreparation(_ mode: UInt32) throws {
    let root = try NativeTestRoot()
    let path = try root.child("tree")
    let readonly = try appendPath(path, "readonly")
    defer {
        _ = chmod(readonly.displayString, 0o700)
        try? root.cleanup()
    }
    try FileManager.default.createDirectory(atPath: readonly.displayString, withIntermediateDirectories: true)
    try NativeTransactionIO.createFile(appendPath(readonly, "old"), data: Data("baseline".utf8))
    #expect(chmod(readonly.displayString, mode_t(mode)) == 0)
    let target = try root.target()
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .skills, destination: path,
                         content: .tree(TreeContent(entries: [BundleEntry(path: "new", data: Data("new".utf8))]))),
    ])
    let before = try NativeObservation.snapshot(path)
    for _ in 0 ..< 2 {
        let result = try transactionWrite(transactionBundle(), rendered: rendered, target: target,
                                          acquisition: .takeover)
        #expect(result.status == .failed)
        #expect(result.error?.code == .unsupported)
        #expect(try StateRecords.keys(target.store, prefix: "") == [StateKey.resource(path, .authority)])
        #expect(try NativeObservation.snapshot(path) == before)
        #expect(try transactionRecover(target).status == .unchanged)
        #expect(try transactionRemove("team", target: target).status == .unchanged)
    }
    #expect(chmod(readonly.displayString, 0o700) == 0)
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target,
                                 acquisition: .takeover).status == .applied)
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.nodes.map(\.path) == ["", "readonly", "readonly/old"])
}

@Test
func nativeRestagingChecksEveryDirectoryBeforePopulatingTheTree() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("stage")
    let unsafe = try Revision(nodes: [
        NativeResourcePlanner.node("", data: nil, mode: 0o755),
        NativeResourcePlanner.node("a", data: nil, mode: 0o555),
        NativeResourcePlanner.node("a/file", data: Data("baseline".utf8), mode: 0o444),
    ])
    expectNativeError(.unsupported) { _ = try NativeTransactionIO.writeRevision(path, revision: unsafe) }
    #expect(try NativeObservation.snapshot(root.path).revision.nodes.map(\.path) == [""])
    let supported = try Revision(nodes: [
        NativeResourcePlanner.node("", data: nil, mode: 0o700),
        NativeResourcePlanner.node("a", data: nil, mode: 0o750),
        NativeResourcePlanner.node("a/file", data: Data("baseline".utf8), mode: 0o444),
    ])
    let actual = try NativeTransactionIO.writeRevision(path, revision: supported)
    #expect(actual.nodes.map(\.mode) == [0o700, 0o750, 0o444])
    #expect(actual.nodes.last?.data == Data("baseline".utf8))
    try NativeTransactionIO.destroy(path, revision: actual)
    #expect(try NativeObservation.snapshot(root.path).revision.nodes.map(\.path) == [""])
}

private func structuredFixture(_ root: NativeTestRoot) throws -> (ResourcePath, InstallationTarget, RenderedBundle) {
    let path = try root.child("document")
    let target = try root.target()
    try NativeTransactionIO.createFile(path, data: Data(#"{"items":[{"id":"a"},{"id":"x","v":1},{"id":"b"}]}"#.utf8))
    let content = try StructuredContent(
        format: .json, selector: Selector(parts: [
            .key(Key("items")), .member(Member(identity: .scalar(.text("x")), key: ["id"])),
        ]),
        value: .object([("id", .scalar(.text("x"))), ("v", .scalar(.integer(2)))]),
    )
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .instructions, destination: path, content: .structured(content)),
    ])
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target,
                                 acquisition: .takeover).status == .applied)
    try Data(#"{"items":[{"id":"c"},{"id":"x","v":2},{"id":"d"}]}"#.utf8)
        .write(to: URL(fileURLWithPath: path.displayString))
    return (path, target, rendered)
}

@Test
func nativeIntactStructuredMemberDoesNotRequireItsBaselineAnchorsForObservation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let (path, target, rendered) = try structuredFixture(root)
    let before = try NativeObservation.snapshot(root.path)
    let storeBefore = try storeContents(target.store)
    let observation = try inspectInstallation(bundleID: "team", target: target)
    #expect(observation.isCurrent)
    #expect(observation.resources.first?.claims.map(\.status) == [.current])
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(try storeContents(target.store) == storeBefore)
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target).status == .applied)
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target).status == .unchanged)
    #expect(try Data(contentsOf: URL(fileURLWithPath: path.displayString)) ==
        Data(#"{"items":[{"id":"c"},{"id":"x","v":2},{"id":"d"}]}"#.utf8))
}

@Test
func nativeForeignAnchorsMakeRemovalConflictWithoutChangingFiles() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let (_, target, _) = try structuredFixture(root)
    let before = try NativeObservation.snapshot(root.path)
    let storeBefore = try storeContents(target.store)
    for _ in 0 ..< 2 {
        let removal = try transactionRemove("team", target: target)
        #expect(removal.status == .failed)
        #expect(removal.error?.code == .conflict)
        #expect(removal.error?.message.contains("no surviving anchors") == true)
        #expect(try NativeObservation.snapshot(root.path) == before)
        #expect(try storeContents(target.store) == storeBefore)
    }
}
