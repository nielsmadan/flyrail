import Darwin
@testable import Flyrail
import Foundation
import Testing

func transactionBundle(_ version: String = "1", owner: String = "team") throws -> FlyrailBundle {
    try FlyrailBundle(
        identity: BundleIdentity(id: owner, version: version),
        artifacts: [.instruction(InstructionArtifact(id: "guide", text: "text"))],
        assets: [], dependencies: []
    )
}

func transactionRendered(_ path: ResourcePath, _ text: String = "generated\n") throws -> RenderedBundle {
    try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .instructions, destination: path,
                         content: .file(FileContent(data: Data(text.utf8), mode: .readable))),
    ])
}

func transactionPlan(
    _ resource: ResourceRef, lock: NativeDirectoryLock, version: String = "1",
    text: String = "generated\n", acquisition: Acquisition = .conflict
) throws -> ResourcePlan {
    let bundle = try transactionBundle(version)
    let rendered = try transactionRendered(resource.destination, text)
    return try NativeResourcePlanner.plan(
        resource, observation: NativeAdmission.observe(resource, lock: lock),
        bundleID: "team", bundle: bundle, rendered: rendered, artifacts: rendered.artifacts,
        acquisition: acquisition, replaceModified: false
    )
}

@Test
func nativeTransactionPublishesAndRemovesAnExclusiveResource() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    let plan = try transactionPlan(ref, lock: lock)
    let journal = try NativeTransaction.prepare(plan, lock: lock)
    #expect(try NativeObservation.read(
        appendPath(ref.stateRoot, "transaction.json"), as: Journal.self, tag: "Journal"
    ) == journal)
    try NativeTransaction.publish(journal, lock: lock)
    try NativeTransaction.cleanup(journal, lock: lock)
    #expect(try NativeAdmission.observe(ref).revision.data == Data("generated\n".utf8))
    #expect(try NativeAdmission.observe(ref).receipt == plan.receipt)
    #expect(try NativeAdmission.observe(ref).recovery.isEmpty)
    let removal = try NativeResourcePlanner.plan(
        ref, observation: NativeAdmission.observe(ref, lock: lock), bundleID: "team",
        bundle: nil, rendered: .empty, artifacts: [], acquisition: .conflict, replaceModified: false
    )
    let retirement = try NativeTransaction.prepare(removal, lock: lock)
    try NativeTransaction.publish(retirement, lock: lock)
    try NativeTransaction.cleanup(retirement, lock: lock)
    #expect(try NativeAdmission.observe(ref).revision.nodes.isEmpty)
    #expect(try NativeAdmission.observe(ref).receipt?.claims == [])
    #expect(try NativeFilesystem.metadata(ref.stateRoot) != nil)
}

@Test(arguments: ["prepare", "backup", "publish", "receipt-prepared", "receipt-committed"])
func nativeRecoveryRecognizesEveryDocumentPublicationPhase(_ phase: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    try NativeTransactionIO.createFile(path, data: Data("original\n".utf8))
    let ref = try NativeAuthority.reference(path, kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    let plan = try transactionPlan(ref, lock: lock, acquisition: .takeover)
    let seam = NativeTransactionSeam { boundary in
        if boundary == phase { throw TargetError(code: .ioError, message: "interrupted") }
    }
    do {
        let journal = try NativeTransaction.prepare(plan, lock: lock, seam: seam)
        try NativeTransaction.publish(journal, lock: lock, seam: seam)
        Issue.record("interruption seam was not reached")
    } catch let error as TargetError {
        #expect(error.message == "interrupted")
    }
    try NativeTransaction.recover(ref, lock: lock)
    try NativeTransaction.recover(ref, lock: lock)
    let observed = try NativeAdmission.observe(ref)
    #expect(observed.revision.data == Data((phase == "receipt-committed" ? "generated\n" : "original\n").utf8))
    #expect(observed.receipt == (phase == "receipt-committed" ? plan.receipt : nil))
    #expect(observed.recovery.isEmpty)
}

@Test
func nativeRecoveryRetainsUnknownReplacementAndCommittedForeignEdits() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    let journal = try NativeTransaction.prepare(transactionPlan(ref, lock: lock), lock: lock)
    try NativeTransactionIO.move(
        appendPath(ref.stateRoot, "staging/resource"), to: ref.destination, revision: journal.staged
    )
    try Data("foreign\n".utf8).write(to: URL(fileURLWithPath: ref.destination.displayString))
    expectNativeError(.recoveryNeeded) { try NativeTransaction.recover(ref, lock: lock) }
    #expect(try NativeObservation.read(
        appendPath(ref.stateRoot, "transaction.json"), as: Journal.self, tag: "Journal"
    ) == journal)
    #expect(try NativeAdmission.observe(ref).revision.data == Data("foreign\n".utf8))
}

@Test
func nativePlansNeverRefreshAChangedPreimage() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    let plan = try transactionPlan(ref, lock: lock)
    try NativeTransactionIO.createFile(ref.destination, data: Data("foreign".utf8))
    expectNativeError(.concurrentChange) { _ = try NativeTransaction.prepare(plan, lock: lock) }
    #expect(try NativeAdmission.observe(ref).revision.data == Data("foreign".utf8))
    #expect(try NativeAdmission.observe(ref).recovery.isEmpty)
}
