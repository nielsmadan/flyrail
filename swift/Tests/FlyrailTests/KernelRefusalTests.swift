import Darwin
@testable import Flyrail
import Foundation
import Testing

private func document(_ root: NativeTestRoot, _ name: String = "document") throws -> ResourceRef {
    try Data("before\n".utf8).write(to: root.url.appending(path: name))
    return try NativeAuthority.reference(root.child(name), kind: .document)
}

private func after() throws -> Revision {
    try Revision(nodes: [NativeResourcePlanner.node("", data: Data("after\n".utf8), mode: 0o644)])
}

private func contents(_ path: ResourcePath) -> Data? {
    FileManager.default.contents(atPath: path.displayString)
}

private func write(_ text: String, to path: ResourcePath) throws {
    try Data(text.utf8).write(to: URL(fileURLWithPath: path.displayString))
}

private func crashed(_ plan: ResourcePlan, store: any StateStore, at boundary: String) throws -> Journal {
    let journal = try ResourceTransaction.prepare(plan, store: store)
    let (seam, fired) = crashSeam(at: boundary)
    #expect(throws: NativeSimulatedCrash(boundary)) {
        try ResourceTransaction.publish(journal, store: store, seam: seam)
    }
    try #require(fired())
    return journal
}

private func privacyRefusal(_ path: ResourcePath) -> TargetError {
    TargetError(
        code: .unsupported,
        message: "[Errno \(ENOTSUP)] management storage must be private to its owner: PosixPath('\(path.displayString)')",
        path: path, errno: ENOTSUP,
    )
}

@Test(arguments: ["staging", "backup"])
func publishAndRecoveryRefuseANonPrivateTransactionDirectory(_ container: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try document(root)
    let plan = try storePlan(ref, store: store, after: after(), previous: nil)
    let journal = try ResourceTransaction.prepare(plan, store: store)
    let exposed = try appendPath(plan.transactionDirectory, container)
    try #require(chmod(exposed.displayString, 0o750) == 0)
    #expect(kernelError { try ResourceTransaction.publish(journal, store: store) } == TargetError(
        code: .concurrentChange, message: "resource ancestor changed", path: ref.destination,
    ))

    let recorded = try Journal(
        resource: ref, previous: nil, receipt: journal.receipt, before: journal.before, staged: journal.staged,
        published: journal.published, backup: journal.backup,
        ancestors: ResourceTransaction.journalAncestors(
            destination: ref.destination,
            directory: plan.transactionDirectory,
        ),
    )
    let key = try StateKey.resource(ref.destination, .journal)
    try StateRecords.write(store, key: key, value: recorded, expected: journal)
    #expect(kernelError { try ResourceTransaction.publish(recorded, store: store) } == privacyRefusal(exposed))
    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == privacyRefusal(exposed))
    #expect(try StateRecords.journal(store, ref.destination) == recorded)
    #expect(try ResourceTransaction.pendingRecovery(ref, store: store) == [plan.transactionDirectory])
    #expect(try contents(appendPath(plan.transactionDirectory, "staging/resource")) == Data("after\n".utf8))
    #expect(contents(ref.destination) == Data("before\n".utf8))

    try #require(chmod(exposed.displayString, 0o700) == 0)
    try StateRecords.write(store, key: key, value: journal, expected: recorded)
    try ResourceTransaction.recover(ref, store: store)
    #expect(try ResourceTransaction.pendingRecovery(ref, store: store) == [])
}

@Test
func abandonRetainsANonPrivatePreparedDirectory() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try document(root)
    let plan = try storePlan(ref, store: store, after: after(), previous: nil)
    let (seam, fired) = crashSeam(at: "staged")
    #expect(throws: NativeSimulatedCrash("staged")) {
        _ = try ResourceTransaction.prepare(plan, store: store, seam: seam)
    }
    try #require(fired())
    let staging = try appendPath(plan.transactionDirectory, "staging")
    try #require(chmod(staging.displayString, 0o750) == 0)

    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == TargetError(
        code: .recoveryNeeded, message: "unrecognized transaction data retained", path: plan.transactionDirectory,
    ))
    #expect(try ResourceTransaction.pendingRecovery(ref, store: store) == [plan.transactionDirectory])
    #expect(try contents(appendPath(staging, "resource")) == Data("after\n".utf8))
}

@Test
func prepareRefusesAPlanWhoseDirectoryIsNotRecognized() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try document(root)
    let plan = try storePlan(ref, store: root.store, after: after(), previous: nil)
    let foreign = try ResourcePlan(
        resource: ref, before: plan.before, after: plan.after, previous: nil, receipt: plan.receipt,
        ancestors: plan.ancestors, transactionDirectory: root.child("other.txn"), recoveryPaths: [],
    )
    #expect(kernelError { _ = try ResourceTransaction.prepare(foreign, store: root.store) } == TargetError(
        code: .invalidState, message: "planned transaction directory is not recognized",
    ))
    #expect(try transactionRecordKeys(root.store) == [])
}

@Test
func prepareRefusesARequiredResourceThatDisappeared() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let plan = try storePlan(ref, store: root.store, after: after(), previous: nil)
    let required = try ResourcePlan(
        resource: ref, before: plan.before, after: plan.after, previous: nil, receipt: plan.receipt,
        ancestors: plan.ancestors, transactionDirectory: plan.transactionDirectory, recoveryPaths: [],
        requireExisting: true,
    )
    #expect(kernelError { _ = try ResourceTransaction.prepare(required, store: root.store) } == TargetError(
        code: .concurrentChange, message: "required existing resource disappeared", path: ref.destination,
    ))
    #expect(try transactionRecordKeys(root.store) == [])
}

@Test
func prepareRefusesAReceiptChangedSincePlanning() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try document(root)
    let plan = try storePlan(ref, store: root.store, after: after(), previous: nil)
    let foreign = try Receipt(resource: ref, transactionID: randomTransactionID())
    try StateRecords.write(root.store, key: StateKey.resource(ref.destination, .receipt), value: foreign, expected: nil)
    #expect(kernelError { _ = try ResourceTransaction.prepare(plan, store: root.store) } == TargetError(
        code: .concurrentChange, message: "receipt changed during preparation", path: ref.destination,
    ))
    #expect(try transactionRecordKeys(root.store) == [])
}

@Test
func prepareRefusesARetainedPreparation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try document(root)
    let first = try storePlan(ref, store: root.store, after: after(), previous: nil)
    let (seam, fired) = crashSeam(at: "preparation")
    #expect(throws: NativeSimulatedCrash("preparation")) {
        _ = try ResourceTransaction.prepare(first, store: root.store, seam: seam)
    }
    try #require(fired())
    let retained = try #require(try StateRecords.preparation(root.store, ref.destination))
    let second = try storePlan(ref, store: root.store, after: after(), previous: nil)
    #expect(kernelError { _ = try ResourceTransaction.prepare(second, store: root.store) } == TargetError(
        code: .recoveryNeeded, message: "transaction record retained", path: first.transactionDirectory,
    ))
    #expect(try StateRecords.preparation(root.store, ref.destination) == retained)
}

@Test
func publishRefusesAReceiptChangedBeforeCommit() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try document(root)
    let plan = try storePlan(ref, store: store, after: after(), previous: nil)
    let journal = try ResourceTransaction.prepare(plan, store: store)
    let foreign = try Receipt(resource: ref, transactionID: randomTransactionID())
    let seam = NativeTransactionSeam { boundary in
        if boundary == "publish-receipt" {
            try StateRecords.write(store, key: StateKey.resource(ref.destination, .receipt), value: foreign,
                                   expected: nil)
        }
    }
    #expect(kernelError { try ResourceTransaction.publish(journal, store: store, seam: seam) } == TargetError(
        code: .concurrentChange, message: "receipt changed before commit", path: ref.destination,
    ))
    #expect(try StateRecords.receipt(store, ref.destination) == foreign)
    #expect(try StateRecords.journal(store, ref.destination) == journal)
}

@Test
func publishRefusesAChangedTransactionRecord() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try document(root)
    let plan = try storePlan(ref, store: store, after: after(), previous: nil)
    let journal = try ResourceTransaction.prepare(plan, store: store)
    let changed = try Journal(
        resource: ref, previous: nil, receipt: journal.receipt, before: journal.before, staged: journal.staged,
        published: journal.published, backup: journal.backup, ancestors: [],
    )
    try StateRecords.write(store, key: StateKey.resource(ref.destination, .journal), value: changed, expected: journal)
    #expect(kernelError { try ResourceTransaction.publish(journal, store: store) } == TargetError(
        code: .recoveryNeeded, message: "transaction record changed", path: plan.transactionDirectory,
    ))
    #expect(contents(ref.destination) == Data("before\n".utf8))
}

@Test
func publishRefusesAChangedHierarchyReservation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    try FileManager.default.createDirectory(at: root.url.appending(path: "tree"), withIntermediateDirectories: false)
    let ref = try document(root, "tree/document")
    let plan = try storePlan(ref, store: store, after: after(), previous: nil)
    let journal = try ResourceTransaction.prepare(plan, store: store)
    let tree = try NativeAuthority.reference(root.child("tree"), kind: .tree)
    try StateRecords.write(store, key: StateKey.resource(tree.destination, .authority), value: tree, expected: nil)
    #expect(kernelError { try ResourceTransaction.publish(journal, store: store) } == TargetError(
        code: .conflict, message: "resource hierarchy reservation changed", path: tree.destination,
    ))
    #expect(contents(ref.destination) == Data("before\n".utf8))
}

@Test
func cleanupRefusesAnUnexpectedBackupRevision() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try document(root)
    let plan = try storePlan(ref, store: store, after: after(), previous: nil)
    let journal = try ResourceTransaction.prepare(plan, store: store)
    try ResourceTransaction.publish(journal, store: store)
    let (_, backup) = try TransactionLayout.resources(plan.transactionDirectory)
    try write("foreign\n", to: backup)
    #expect(kernelError { try ResourceTransaction.cleanup(journal, store: store) } == TargetError(
        code: .recoveryNeeded, message: "unexpected cleanup revision", path: backup,
    ))
    #expect(contents(backup) == Data("foreign\n".utf8))
    #expect(try StateRecords.receipt(store, ref.destination) == journal.receipt)
}

@Test(arguments: ["staged", "backup"])
func rollbackRefusesAnUnexpectedRetainedRevision(_ location: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try document(root)
    let plan = try storePlan(ref, store: store, after: after(), previous: nil)
    _ = try crashed(plan, store: store, at: "backup")
    let (staged, backup) = try TransactionLayout.resources(plan.transactionDirectory)
    let changed = location == "staged" ? staged : backup
    let message = location == "staged" ? "unexpected staged revision" : "unexpected backup revision"
    try write("foreign\n", to: changed)
    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == TargetError(
        code: .recoveryNeeded, message: message, path: changed,
    ))
    #expect(try ResourceTransaction.pendingRecovery(ref, store: store) == [plan.transactionDirectory])
    #expect(contents(changed) == Data("foreign\n".utf8))
}

@Test
func recoveryRefusesAPreparationForADifferentResource() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let tree = try NativeAuthority.reference(root.child("resource"), kind: .tree)
    let plan = try storePlan(tree, store: store, after: Revision(), previous: nil)
    let (seam, fired) = crashSeam(at: "preparation")
    #expect(throws: NativeSimulatedCrash("preparation")) {
        _ = try ResourceTransaction.prepare(plan, store: store, seam: seam)
    }
    try #require(fired())
    let document = try NativeAuthority.reference(root.child("resource"), kind: .document)
    #expect(kernelError { try ResourceTransaction.recover(document, store: store) } == TargetError(
        code: .recoveryNeeded, message: "transaction record disagrees", path: plan.transactionDirectory,
    ))
    #expect(try StateRecords.preparation(store, tree.destination)?.resource == tree)
}

@Test
func recoveryRefusesAReceiptMatchingNeitherSide() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try document(root)
    let plan = try storePlan(ref, store: store, after: after(), previous: nil)
    _ = try crashed(plan, store: store, at: "publish")
    let foreign = try Receipt(resource: ref, transactionID: randomTransactionID())
    try StateRecords.write(store, key: StateKey.resource(ref.destination, .receipt), value: foreign, expected: nil)
    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == TargetError(
        code: .recoveryNeeded, message: "receipt disagrees with transaction", path: ref.destination,
    ))
    #expect(contents(ref.destination) == Data("after\n".utf8))
}

@Test
func recoveryRefusesToRestoreOverAForeignRevision() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try document(root)
    let plan = try storePlan(ref, store: store, after: after(), previous: nil)
    _ = try ResourceTransaction.prepare(plan, store: store)
    try write("foreign\n", to: ref.destination)
    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == TargetError(
        code: .recoveryNeeded, message: "cannot restore complete previous revision", path: ref.destination,
    ))
    #expect(contents(ref.destination) == Data("foreign\n".utf8))
    #expect(try ResourceTransaction.pendingRecovery(ref, store: store) == [plan.transactionDirectory])
}

@Test(arguments: ["extra", "staging/extra"])
func recoveryRefusesUnknownTransactionData(_ relative: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try document(root)
    let plan = try storePlan(ref, store: store, after: after(), previous: nil)
    _ = try crashed(plan, store: store, at: "publish")
    let extra = try appendPath(plan.transactionDirectory, relative)
    try write("", to: extra)
    let reported = try relative == "extra" ? extra : appendPath(plan.transactionDirectory, "staging")
    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == TargetError(
        code: .recoveryNeeded, message: "unknown transaction data", path: reported,
    ))
    #expect(try ResourceTransaction.pendingRecovery(ref, store: store) == [plan.transactionDirectory])
    #expect(contents(ref.destination) == Data("after\n".utf8))
}
