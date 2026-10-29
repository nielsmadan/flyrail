import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func nativeSameGenerationRepairRollbackRetainsModifiedPreimage() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let bundle = try transactionBundle()
    let rendered = try transactionRendered(path)
    #expect(try transactionWrite(bundle, rendered: rendered, target: target).status == .applied)
    let ref = try NativeAuthority.reference(path, kind: .document)
    let previous = try #require(NativeAdmission.observe(ref).receipt)
    #expect(try transactionWrite(bundle, rendered: rendered, target: target).status == .unchanged)
    #expect(try NativeAdmission.observe(ref).receipt == previous)
    try Data("local edited content\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
    let original = try NativeObservation.snapshot(path).revision
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    let plan = try NativeResourcePlanner.plan(
        ref, observation: NativeAdmission.observe(ref, lock: lock), bundleID: "team",
        bundle: bundle, rendered: rendered, artifacts: rendered.artifacts,
        acquisition: .conflict, replaceModified: true, transactionID: previous.transactionID
    )
    #expect(plan.receipt.transactionID != previous.transactionID)
    let journal = try NativeTransaction.prepare(plan, lock: lock)
    let interrupted = NativeTransactionSeam {
        if $0 == "backup" { throw TargetError(code: .ioError, message: "backup interrupted") }
    }
    expectNativeError(.ioError) { try NativeTransaction.publish(journal, lock: lock, seam: interrupted) }
    #expect(try NativeObservation.snapshot(path).revision.nodes.isEmpty)
    #expect(try NativeObservation.snapshot(NativeTransaction.path(ref, "backup/resource")).revision == original)
    #expect(try NativeObservation.snapshot(NativeTransaction.path(ref, "staging/resource")).revision == journal.staged)
    #expect(try NativeObservation.read(
        NativeTransaction.path(ref, "transaction.json"), as: Journal.self, tag: "Journal"
    ) == journal)
    expectNativeError(.ioError) {
        try NativeTransaction.recover(ref, lock: lock, seam: NativeTransactionSeam {
            if $0 == "rollback-backup" { throw TargetError(code: .ioError, message: "rollback interrupted") }
        })
    }
    #expect(try NativeObservation.snapshot(path).revision == original)
    #expect(try NativeObservation.snapshot(NativeTransaction.path(ref, "backup/resource")).revision.nodes.isEmpty)
    #expect(try NativeObservation.snapshot(NativeTransaction.path(ref, "staging/resource")).revision == journal.staged)
    try NativeTransaction.recover(ref, lock: lock)
    try NativeTransaction.recover(ref, lock: lock)
    #expect(try NativeObservation.snapshot(path).revision == original)
    #expect(try NativeAdmission.observe(ref).receipt == previous)
    #expect(try NativeAdmission.observe(ref).state.revision.nodes.map(\.path) ==
        ["", "authority.json", "backup", "lock", "receipt.json", "staging"])
}

@Test
func nativeSameGenerationRepairFaultIsNotReportedAsCommitted() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let bundle = try transactionBundle()
    let rendered = try transactionRendered(path)
    #expect(try transactionWrite(bundle, rendered: rendered, target: target).status == .applied)
    try Data("local edited content\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
    let before = try NativeObservation.snapshot(path).revision
    let result = try transactionWrite(
        bundle, rendered: rendered, target: target, replaceModified: true, seam: NativeTransactionSeam {
            if $0 == "backup" { throw TargetError(code: .ioError, message: "backup interrupted") }
        }
    )
    #expect(result.status == .failed)
    #expect(result.resources.map(\.status) == [.failed])
    #expect(result.resources.first?.recoveryPaths == [])
    for _ in 0 ..< 2 {
        _ = try NativeTransactionDriver.recover("team", target: target, timeout: .zero)
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
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let original = Data(repeating: 97, count: size)
    try NativeTransactionIO.createFile(path, data: original)
    let before = try NativeObservation.snapshot(path)
    for _ in 0 ..< 2 {
        do {
            _ = try transactionWrite(transactionBundle(), rendered: transactionRendered(path, "new\n"),
                                     target: target, acquisition: .takeover)
            Issue.record("oversized journal was accepted")
        } catch let error as FlyrailValueError {
            #expect(error.message == (size == 3 * 1024 * 1024
                    ? "JSON input exceeds the byte limit" : "JSON string exceeds the byte limit"))
        }
        let ref = try NativeAuthority.reference(path, kind: .document)
        #expect(try NativeAdmission.observe(ref).state.revision.nodes.map(\.path) == ["", "authority.json", "lock"])
        #expect(try NativeAdmission.observe(ref).recovery == [])
        #expect(try NativeObservation.snapshot(target.indexRoot).revision.nodes.map(\.path) == ["", "lock"])
        #expect(try NativeObservation.snapshot(path) == before)
        #expect(try NativeTransactionDriver.recover("team", target: target, timeout: .zero).status == .unchanged)
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
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let rendered = try transactionRendered(path, String(repeating: "a", count: 3 * 1024 * 1024))
    do {
        _ = try transactionWrite(transactionBundle(), rendered: rendered, target: target)
        Issue.record("oversized creation journal was accepted")
    } catch let error as FlyrailValueError { #expect(error.message == "JSON input exceeds the byte limit") }
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path), target: target)
        .status == .applied)
    try Data("local edit".utf8).write(to: URL(fileURLWithPath: path.displayString))
    let before = try NativeObservation.snapshot(root.path)
    do {
        _ = try transactionWrite(transactionBundle(), rendered: rendered, target: target, replaceModified: true)
        Issue.record("oversized repair journal was accepted")
    } catch let error as FlyrailValueError { #expect(error.message == "JSON input exceeds the byte limit") }
    for _ in 0 ..< 2 {
        _ = try NativeTransactionDriver.recover("team", target: target, timeout: .zero)
    }
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(try NativeTransactionDriver.write(
        bundle: nil, bundleID: "team", rendered: .empty, target: target, acquisition: .conflict,
        replaceModified: true, timeout: .zero
    ).status == .applied)
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
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .skills, destination: path,
                         content: .tree(TreeContent(entries: [BundleEntry(path: "new", data: Data("new".utf8))]))),
    ])
    let before = try NativeObservation.snapshot(path)
    for _ in 0 ..< 2 {
        expectNativeError(.unsupported) {
            _ = try transactionWrite(transactionBundle(), rendered: rendered, target: target, acquisition: .takeover)
        }
        let ref = try NativeAuthority.reference(path, kind: .tree)
        #expect(try NativeAdmission.observe(ref).state.revision.nodes.map(\.path) == ["", "authority.json", "lock"])
        #expect(try NativeObservation.snapshot(path) == before)
        #expect(try NativeTransactionDriver.recover("team", target: target, timeout: .zero).status == .unchanged)
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

@Test
func nativeIntactStructuredMemberDoesNotRequireItsBaselineAnchorsForObservation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    try NativeTransactionIO.createFile(path, data: Data(#"{"items":[{"id":"a"},{"id":"x","v":1},{"id":"b"}]}"#.utf8))
    let content = try StructuredContent(
        format: .json, selector: Selector(parts: [
            .key(Key("items")), .member(Member(identity: .scalar(.text("x")), key: ["id"])),
        ]),
        value: .object([("id", .scalar(.text("x"))), ("v", .scalar(.integer(2)))])
    )
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .instructions, destination: path, content: .structured(content)),
    ])
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target,
                                 acquisition: .takeover).status == .applied)
    let ref = try NativeAuthority.reference(path, kind: .document)
    let baseline = try #require(NativeAdmission.observe(ref).receipt?.claims.first?.selection?.baseline)
    try Data(#"{"items":[{"id":"c"},{"id":"x","v":2},{"id":"d"}]}"#.utf8)
        .write(to: URL(fileURLWithPath: path.displayString))
    let before = try NativeObservation.snapshot(root.path)
    let observation = try NativeTransactionDriver.inspect("team", target: target)
    #expect(observation.isCurrent)
    #expect(observation.resources.first?.claims.map(\.status) == [.current])
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target).status == .applied)
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target).status == .unchanged)
    let stable = try NativeObservation.snapshot(root.path)
    let removal = try transactionRemove("team", target: target)
    #expect(removal.status == .failed)
    #expect(removal.error?.message.contains("no surviving anchors") == true)
    #expect(try NativeObservation.snapshot(root.path) == stable)
    #expect(try NativeAdmission.observe(ref).receipt?.claims.first?.selection?.baseline == baseline)
}
