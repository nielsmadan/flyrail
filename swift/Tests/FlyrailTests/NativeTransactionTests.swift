import Darwin
@testable import Flyrail
import Foundation
import Testing

func transactionPlan(
    _ resource: ResourceRef, store: any StateStore, version: String = "1", text: String = "generated\n",
    acquisition: Acquisition = .conflict,
) throws -> ResourcePlan {
    _ = try NativeTransactionDirectory.createStoreParents(store)
    try ResourceAdmission.admit(resource, store: store)
    let bundle = try transactionBundle(version)
    let rendered = try transactionRendered(resource.destination, text)
    return try NativeResourcePlanner.plan(
        resource, state: ResourceAdmission.observe(resource, store: store), store: store, bundleID: "team",
        bundle: bundle, rendered: rendered, artifacts: rendered.artifacts, acquisition: acquisition,
        replaceModified: false,
    )
}

private func existingDocument(_ root: NativeTestRoot, _ name: String = "document") throws -> ResourceRef {
    let path = try root.child(name)
    try NativeTransactionIO.createFile(path, data: Data("original\n".utf8))
    return try NativeAuthority.reference(path, kind: .document)
}

private func commit(_ plan: ResourcePlan, store: any StateStore) throws -> Journal {
    let journal = try ResourceTransaction.prepare(plan, store: store)
    try ResourceTransaction.publish(journal, store: store)
    try ResourceTransaction.cleanup(journal, store: store)
    return journal
}

@Test
func nativeTransactionPublishesAndRemovesAnExclusiveResource() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let plan = try transactionPlan(ref, store: store)
    let journal = try commit(plan, store: store)
    let installed = try ResourceAdmission.observe(ref, store: store)
    #expect(installed.revision.data == Data("generated\n".utf8))
    #expect(installed.receipt == journal.receipt)
    #expect(installed.recoveryPaths.isEmpty)
    #expect(try transactionRecordKeys(store) == [])
    #expect(!FileManager.default.fileExists(atPath: plan.transactionDirectory.displayString))

    let removal = try NativeResourcePlanner.plan(
        ref, state: installed, store: store, bundleID: "team", bundle: nil, rendered: .empty, artifacts: [],
        acquisition: .conflict, replaceModified: false,
    )
    let retirement = try commit(removal, store: store)
    let removed = try ResourceAdmission.observe(ref, store: store)
    #expect(removed.revision.nodes.isEmpty)
    #expect(removed.receipt == retirement.receipt)
    #expect(removed.receipt?.claims == [])
    #expect(try transactionRecordKeys(store) == [])
    #expect(!FileManager.default.fileExists(atPath: removal.transactionDirectory.displayString))
    #expect(try FileManager.default.contentsOfDirectory(atPath: root.url.path) == [])
}

@Test(arguments: kernelForwardBoundaries)
func nativeRecoveryRecognizesEveryDocumentPublicationPhase(_ phase: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try existingDocument(root)
    let plan = try transactionPlan(ref, store: store, acquisition: .takeover)
    let (seam, fired) = crashSeam(at: phase)
    #expect(throws: NativeSimulatedCrash(phase)) {
        let journal = try ResourceTransaction.prepare(plan, store: store, seam: seam)
        try ResourceTransaction.publish(journal, store: store, seam: seam)
        try ResourceTransaction.cleanup(journal, store: store, seam: seam)
    }
    try #require(fired())
    try ResourceTransaction.recover(ref, store: store)
    try ResourceTransaction.recover(ref, store: store)
    let committed = ["receipt-reread", "receipt-committed", "cleanup-node", "cleanup"].contains(phase)
    let observed = try ResourceAdmission.observe(ref, store: store)
    #expect(observed.revision.data == Data((committed ? "generated\n" : "original\n").utf8))
    #expect(observed.receipt == (committed ? plan.receipt : nil))
    #expect(observed.recoveryPaths.isEmpty)
    #expect(try transactionRecordKeys(store) == [])
    #expect(!FileManager.default.fileExists(atPath: plan.transactionDirectory.displayString))
}

@Test
func nativeRecoveryRetainsUnknownReplacementAndCommittedForeignEdits() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try existingDocument(root)
    let journal = try ResourceTransaction.prepare(
        transactionPlan(ref, store: store, acquisition: .takeover), store: store,
    )
    let directory = try ResourceTransaction.transactionDirectory(journal, store: store)
    let (staged, backup) = try TransactionLayout.resources(directory)
    try NativeTransactionIO.move(ref.destination, to: backup, revision: journal.backup)
    try NativeTransactionIO.move(staged, to: ref.destination, revision: journal.staged)
    try Data("foreign\n".utf8).write(to: URL(fileURLWithPath: ref.destination.displayString))
    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == TargetError(
        code: .recoveryNeeded, message: "unrecognized published revision", path: ref.destination,
    ))
    #expect(try StateRecords.read(
        store, key: StateKey.resource(ref.destination, .journal), as: Journal.self, tag: "Journal",
    ) == journal)
    #expect(try ResourceAdmission.observe(ref, store: store).revision.data == Data("foreign\n".utf8))
    #expect(try ResourceAdmission.observe(ref, store: store).recoveryPaths == [directory])
    #expect(FileManager.default.contents(atPath: backup.displayString) == Data("original\n".utf8))

    let other = try existingDocument(root, "other")
    let plan = try transactionPlan(other, store: store, acquisition: .takeover)
    let (seam, fired) = crashSeam(at: "receipt-committed")
    #expect(throws: NativeSimulatedCrash("receipt-committed")) {
        try ResourceTransaction.publish(ResourceTransaction.prepare(plan, store: store), store: store, seam: seam)
    }
    try #require(fired())
    try Data("committed foreign\n".utf8).write(to: URL(fileURLWithPath: other.destination.displayString))
    try ResourceTransaction.recover(other, store: store)
    let committed = try ResourceAdmission.observe(other, store: store)
    #expect(committed.revision.data == Data("committed foreign\n".utf8))
    #expect(committed.receipt == plan.receipt)
    #expect(committed.recoveryPaths.isEmpty)
}

@Test
func nativePlansNeverRefreshAChangedPreimage() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try existingDocument(root)
    let plan = try transactionPlan(ref, store: store, acquisition: .takeover)
    try Data("foreign".utf8).write(to: URL(fileURLWithPath: ref.destination.displayString))
    #expect(kernelError { _ = try ResourceTransaction.prepare(plan, store: store) } == TargetError(
        code: .concurrentChange, message: "complete resource revision changed", path: ref.destination,
    ))
    #expect(try ResourceAdmission.observe(ref, store: store).revision.data == Data("foreign".utf8))
    #expect(try ResourceAdmission.observe(ref, store: store).recoveryPaths.isEmpty)
    #expect(try transactionRecordKeys(store) == [])
    #expect(!FileManager.default.fileExists(atPath: plan.transactionDirectory.displayString))
}
