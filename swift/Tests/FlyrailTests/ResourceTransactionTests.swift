import Darwin
@testable import Flyrail
import Foundation
import Testing

private let committedBoundaries: Set<String> = ["receipt-reread", "receipt-committed", "cleanup-node", "cleanup"]

private func kernelStore(_ root: NativeTestRoot, directory: Bool) -> any StateStore {
    directory ? root.store : MemoryStore()
}

private func existingDocument(_ root: NativeTestRoot, _ name: String = "document") throws -> ResourceRef {
    try Data("before\n".utf8).write(to: root.url.appending(path: name))
    return try NativeAuthority.reference(root.child(name), kind: .document)
}

private func existingTree(_ root: NativeTestRoot) throws -> ResourceRef {
    let nested = root.url.appending(path: "tree/nested")
    try FileManager.default.createDirectory(at: nested, withIntermediateDirectories: true)
    try Data("before\n".utf8).write(to: nested.appending(path: "file"))
    return try NativeAuthority.reference(root.child("tree"), kind: .tree)
}

private func fileRevision(_ text: String) throws -> Revision {
    try Revision(nodes: [NativeResourcePlanner.node("", data: Data(text.utf8), mode: 0o644)])
}

private func treeRevision(_ text: String) throws -> Revision {
    try Revision(nodes: [
        NativeResourcePlanner.node("", data: nil, mode: 0o755),
        NativeResourcePlanner.node("nested", data: nil, mode: 0o755),
        NativeResourcePlanner.node("nested/file", data: Data(text.utf8), mode: 0o644),
    ])
}

private func contents(_ path: ResourcePath) -> Data? {
    FileManager.default.contents(atPath: path.displayString)
}

private func exists(_ path: ResourcePath) -> Bool {
    var metadata = stat()
    return lstat(path.displayString, &metadata) == 0
}

private func permissions(_ path: ResourcePath) -> mode_t? {
    var metadata = stat()
    return lstat(path.displayString, &metadata) == 0 ? metadata.st_mode & 0o777 : nil
}

private func transactionKeys(_ store: any StateStore) throws -> [String] {
    try store.keys(prefix: "resource/").filter { $0.hasSuffix("/preparation") || $0.hasSuffix("/journal") }
}

private func receipt(_ ref: ResourceRef, store: any StateStore) throws -> Receipt? {
    try StateRecords.read(store, key: StateKey.resource(ref.destination, .receipt), as: Receipt.self, tag: "Receipt")
}

private func preparation(_ ref: ResourceRef, store: any StateStore) throws -> Preparation? {
    try StateRecords.read(
        store, key: StateKey.resource(ref.destination, .preparation), as: Preparation.self, tag: "Preparation",
    )
}

private func commitPrevious(_ ref: ResourceRef, store: any StateStore) throws -> Receipt {
    let previous = try Receipt(resource: ref, transactionID: randomTransactionID())
    try StateRecords.write(store, key: StateKey.resource(ref.destination, .receipt), value: previous, expected: nil)
    return previous
}

private func crashForward(_ plan: ResourcePlan, store: any StateStore, at boundary: String) throws {
    let (seam, fired) = crashSeam(at: boundary)
    #expect(throws: NativeSimulatedCrash(boundary)) {
        let journal = try ResourceTransaction.prepare(plan, store: store, seam: seam)
        try ResourceTransaction.publish(journal, store: store, seam: seam)
        try ResourceTransaction.cleanup(journal, store: store, seam: seam)
    }
    try #require(fired())
}

private func modifyAfterBoundary(_ path: ResourcePath, directory: ResourcePath) throws {
    var boundary = stat()
    try #require(lstat(directory.displayString, &boundary) == 0)
    var times = [
        timespec(tv_sec: 0, tv_nsec: Int(UTIME_OMIT)),
        timespec(tv_sec: boundary.st_mtimespec.tv_sec + 60, tv_nsec: boundary.st_mtimespec.tv_nsec),
    ]
    try #require(utimensat(AT_FDCWD, path.displayString, &times, AT_SYMLINK_NOFOLLOW) == 0)
}

@Test(arguments: [false, true])
func preparePublishAndCleanupCommitAResourceAndReceipt(_ directoryStore: Bool) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = kernelStore(root, directory: directoryStore)
    let ref = try existingDocument(root)
    let plan = try storePlan(ref, store: store, after: fileRevision("after\n"), previous: nil)
    let directory = plan.transactionDirectory
    let expectedDirectory = try directoryStore ?
        NativeTransactionDirectory.staged(
            store: store, destination: ref.destination, transactionID: plan.receipt.transactionID,
        ) : transactionPath(destination: ref.destination, transactionID: plan.receipt.transactionID)
    #expect(directory == expectedDirectory)
    var seen: [String] = []
    let seam = NativeTransactionSeam { seen.append($0) }

    let journal = try ResourceTransaction.prepare(plan, store: store, seam: seam)
    #expect(try preparation(ref, store: store) == Preparation(
        resource: ref, transactionID: plan.receipt.transactionID, directory: directory,
    ))
    #expect(try StateRecords.read(
        store, key: StateKey.resource(ref.destination, .journal), as: Journal.self, tag: "Journal",
    ) == journal)
    let storage = try [directory, appendPath(directory, "staging"), appendPath(directory, "backup")]
    #expect(storage.map(permissions) == [0o700, 0o700, 0o700])
    #expect(try ResourceAdmission.observe(ref, store: store).recoveryPaths == [directory])

    try ResourceTransaction.publish(journal, store: store, seam: seam)
    #expect(try receipt(ref, store: store) == journal.receipt)
    #expect(contents(ref.destination) == Data("after\n".utf8))

    try ResourceTransaction.cleanup(journal, store: store, seam: seam)
    #expect(!exists(directory))
    #expect(try transactionKeys(store) == [])
    #expect(try ResourceAdmission.observe(ref, store: store) == ResourceState(
        revision: NativeObservation.snapshot(ref.destination).revision, receipt: journal.receipt, recoveryPaths: [],
    ))
    #expect(seen == [
        "preparation", "directory", "stage-node", "staged", "journal", "prepare", "backup-move", "backup", "rename",
        "staging", "rename", "publish", "publish-receipt", "receipt", "receipt-reread", "receipt-committed",
        "cleanup-node", "cleanup",
    ])
    #expect(Set(seen) == Set(kernelForwardBoundaries))
}

enum KernelShape: String, CaseIterable, Sendable {
    case updateDocument
    case createDocument
    case createTree
}

struct KernelCrash: Sendable, CustomTestStringConvertible {
    let shape: KernelShape
    let boundary: String

    var testDescription: String {
        "\(shape.rawValue) at \(boundary)"
    }
}

private let kernelCrashes = KernelShape.allCases.flatMap { shape in
    kernelForwardBoundaries
        .filter { shape == .updateDocument || !existingResourceBoundaries.contains($0) }
        .map { KernelCrash(shape: shape, boundary: $0) }
}

private func published(_ ref: ResourceRef, shape: KernelShape) throws -> Data? {
    try contents(shape == .createTree ? appendPath(ref.destination, "nested/file") : ref.destination)
}

@Test(arguments: [false, true], kernelCrashes)
func everyKernelBoundaryCrashRecoversToARecognizedRevision(_ directoryStore: Bool, _ crash: KernelCrash) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = kernelStore(root, directory: directoryStore)
    let ref: ResourceRef
    let previous: Receipt?
    switch crash.shape {
    case .updateDocument:
        ref = try existingDocument(root)
        previous = try commitPrevious(ref, store: store)
    case .createDocument:
        ref = try NativeAuthority.reference(root.child("document"), kind: .document)
        previous = nil
    case .createTree:
        ref = try NativeAuthority.reference(root.child("tree"), kind: .tree)
        previous = nil
    }
    let after = try crash.shape == .createTree ? treeRevision("after\n") : fileRevision("after\n")
    let plan = try storePlan(ref, store: store, after: after, previous: previous)

    try crashForward(plan, store: store, at: crash.boundary)
    try ResourceTransaction.recover(ref, store: store)
    try ResourceTransaction.recover(ref, store: store)

    let committed = committedBoundaries.contains(crash.boundary)
    let restored = crash.shape == .updateDocument ? Data("before\n".utf8) : nil
    #expect(try published(ref, shape: crash.shape) == (committed ? Data("after\n".utf8) : restored))
    #expect(exists(ref.destination) == (committed || crash.shape == .updateDocument))
    #expect(try receipt(ref, store: store) == (committed ? plan.receipt : previous))
    #expect(try transactionKeys(store) == [])
    #expect(!exists(plan.transactionDirectory))
}

@Test(arguments: ResourceTransaction.recoveryBoundaries)
func recoveryBoundariesFireOnlyDuringRollback(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try existingDocument(root)
    let previous = try commitPrevious(ref, store: store)
    let plan = try storePlan(ref, store: store, after: fileRevision("after\n"), previous: previous)
    try crashForward(plan, store: store, at: "publish")

    let (seam, fired) = crashSeam(at: boundary)
    #expect(throws: NativeSimulatedCrash(boundary)) {
        try ResourceTransaction.recover(ref, store: store, seam: seam)
    }
    try #require(fired())
    try ResourceTransaction.recover(ref, store: store)

    #expect(contents(ref.destination) == Data("before\n".utf8))
    #expect(try receipt(ref, store: store) == previous)
    #expect(try transactionKeys(store) == [])
    #expect(!exists(plan.transactionDirectory))
}

@Test
func abandonOnlyDestroysRecognizedStaging() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try existingDocument(root)
    let key = try StateKey.resource(ref.destination, .preparation)
    func crashedPreparation() throws -> ResourcePlan {
        let plan = try storePlan(ref, store: store, after: fileRevision("after\n"), previous: nil)
        let (seam, fired) = crashSeam(at: "staged")
        #expect(throws: NativeSimulatedCrash("staged")) {
            _ = try ResourceTransaction.prepare(plan, store: store, seam: seam)
        }
        try #require(fired())
        return plan
    }

    let staged = try crashedPreparation()
    try ResourceTransaction.recover(ref, store: store)
    #expect(!exists(staged.transactionDirectory))
    #expect(try transactionKeys(store) == [])
    #expect(contents(ref.destination) == Data("before\n".utf8))

    let extra = try crashedPreparation()
    let extraFile = try appendPath(extra.transactionDirectory, "extra")
    try Data().write(to: URL(filePath: extraFile.displayString))
    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == TargetError(
        code: .recoveryNeeded, message: "unrecognized transaction data retained", path: extra.transactionDirectory,
    ))
    #expect(try ResourceTransaction.pendingRecovery(ref, store: store) == [extra.transactionDirectory])
    try FileManager.default.removeItem(atPath: extraFile.displayString)
    try ResourceTransaction.recover(ref, store: store)
    #expect(try transactionKeys(store) == [])

    let other = try root.child("other.txn")
    try FileManager.default.createDirectory(atPath: other.displayString, withIntermediateDirectories: false)
    let unrecognized = try Preparation(resource: ref, transactionID: randomTransactionID(), directory: other)
    try StateRecords.write(store, key: key, value: unrecognized, expected: nil)
    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == TargetError(
        code: .recoveryNeeded, message: "transaction record names an unrecognized directory", path: store.location,
    ))
    #expect(try preparation(ref, store: store) == unrecognized)

    try FileManager.default.removeItem(atPath: other.displayString)
    try ResourceTransaction.recover(ref, store: store)
    #expect(try transactionKeys(store) == [])
}

@Test
func recoveryRefusesADestinationModifiedAfterTheBoundary() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store

    let document = try existingDocument(root)
    let documentPlan = try storePlan(document, store: store, after: fileRevision("after\n"), previous: nil)
    try crashForward(documentPlan, store: store, at: "publish")
    try modifyAfterBoundary(document.destination, directory: documentPlan.transactionDirectory)
    #expect(kernelError { try ResourceTransaction.recover(document, store: store) } == TargetError(
        code: .recoveryNeeded, message: "destination revision changed after preparation", path: document.destination,
    ))
    #expect(try ResourceTransaction.pendingRecovery(document, store: store) == [documentPlan.transactionDirectory])
    #expect(contents(document.destination) == Data("after\n".utf8))
    #expect(try contents(appendPath(documentPlan.transactionDirectory, "backup/resource")) == Data("before\n".utf8))

    let tree = try existingTree(root)
    let treePlan = try storePlan(tree, store: store, after: treeRevision("after\n"), previous: nil)
    try crashForward(treePlan, store: store, at: "publish")
    let child = try appendPath(tree.destination, "nested/file")
    try modifyAfterBoundary(child, directory: treePlan.transactionDirectory)
    #expect(kernelError { try ResourceTransaction.recover(tree, store: store) } == TargetError(
        code: .recoveryNeeded, message: "destination revision changed after preparation", path: child,
    ))
    #expect(try ResourceTransaction.pendingRecovery(tree, store: store) == [treePlan.transactionDirectory])

    let untouched = try existingDocument(root, "untouched")
    let untouchedPlan = try storePlan(untouched, store: store, after: fileRevision("after\n"), previous: nil)
    try crashForward(untouchedPlan, store: store, at: "publish")
    try ResourceTransaction.recover(untouched, store: store)
    #expect(contents(untouched.destination) == Data("before\n".utf8))
    #expect(try ResourceTransaction.pendingRecovery(untouched, store: store) == [])
}

@Test
func changedAfterComparesModificationAndBirthTime() {
    let boundary = timespec(tv_sec: 1_000_000, tv_nsec: 0)
    let later = timespec(tv_sec: 1_000_000, tv_nsec: 1)
    #expect(ResourceTransaction.changedAfter(boundary: boundary, modified: boundary, born: later))
    #expect(ResourceTransaction.changedAfter(boundary: boundary, modified: later, born: boundary))
    #expect(!ResourceTransaction.changedAfter(boundary: boundary, modified: boundary, born: boundary))
}

@Test
func aJournalRepeatingThePreviousReceiptIsRefused() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try existingDocument(root)
    let plan = try storePlan(ref, store: store, after: fileRevision("after\n"), previous: nil)
    let journal = try ResourceTransaction.prepare(plan, store: store)
    let repeated = try Journal(
        resource: ref, previous: journal.receipt, receipt: journal.receipt, before: journal.before,
        staged: journal.staged, published: journal.published, backup: journal.backup, ancestors: journal.ancestors,
    )
    try StateRecords.write(store, key: StateKey.resource(ref.destination, .journal), value: repeated, expected: journal)

    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == TargetError(
        code: .recoveryNeeded, message: "transaction record repeats the previous receipt",
        path: plan.transactionDirectory,
    ))
    #expect(try ResourceTransaction.pendingRecovery(ref, store: store) == [plan.transactionDirectory])
}

@Test
func aJournalWithoutMatchingPreparationIsInvalidState() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try existingDocument(root)
    let plan = try storePlan(ref, store: store, after: fileRevision("after\n"), previous: nil)
    _ = try ResourceTransaction.prepare(plan, store: store)
    let recorded = try #require(try preparation(ref, store: store))
    let journalKey = try StateKey.resource(ref.destination, .journal)
    let journal = try #require(try StateRecords.read(store, key: journalKey, as: Journal.self, tag: "Journal"))
    try StateRecords.delete(store, key: StateKey.resource(ref.destination, .preparation), expected: recorded)

    let expected = TargetError(
        code: .invalidState, message: "transaction record has no matching preparation", path: store.location,
    )
    #expect(kernelError { _ = try ResourceTransaction.pendingRecovery(ref, store: store) } == expected)
    #expect(kernelError { try ResourceTransaction.recover(ref, store: store) } == expected)
    #expect(try StateRecords.read(store, key: journalKey, as: Journal.self, tag: "Journal") == journal)
    #expect(contents(ref.destination) == Data("before\n".utf8))
}

@Test
func aFailedPreparationCompensates() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try existingDocument(root)
    let plan = try storePlan(ref, store: store, after: fileRevision("after\n"), previous: nil)
    let seam = NativeTransactionSeam { boundary in
        if boundary == "staged" {
            throw TargetError(code: .ioError, message: "injected staged fault")
        }
    }

    #expect(kernelError { _ = try ResourceTransaction.prepare(plan, store: store, seam: seam) } == TargetError(
        code: .ioError, message: "injected staged fault",
    ))
    #expect(try transactionKeys(store) == [])
    #expect(!exists(plan.transactionDirectory))
    #expect(try NativeObservation.snapshot(ref.destination).revision == plan.before)
}

@Test
func aCrashDuringPreparationSkipsCompensation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let ref = try existingDocument(root)
    let plan = try storePlan(ref, store: store, after: fileRevision("after\n"), previous: nil)
    let (seam, fired) = crashSeam(at: "staged")

    #expect(throws: NativeSimulatedCrash("staged")) {
        _ = try ResourceTransaction.prepare(plan, store: store, seam: seam)
    }
    try #require(fired())
    #expect(try transactionKeys(store) == [StateKey.resource(ref.destination, .preparation)])
    #expect(try contents(appendPath(plan.transactionDirectory, "staging/resource")) == Data("after\n".utf8))
    #expect(try NativeObservation.snapshot(ref.destination).revision == plan.before)
}

@Test
func publishRefusesADestinationRespelledAfterPreparation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let respelled = root.url.deletingLastPathComponent().appending(path: root.url.lastPathComponent.lowercased())
    guard FileManager.default.fileExists(atPath: respelled.path) else { return }
    let ref = try existingDocument(root, "Document")
    let plan = try storePlan(ref, store: root.store, after: fileRevision("after\n"), previous: nil)
    let journal = try ResourceTransaction.prepare(plan, store: root.store)
    try #require(rename(ref.destination.displayString, root.url.appending(path: "document").path) == 0)
    #expect(kernelError { try ResourceTransaction.publish(journal, store: root.store) } == TargetError(
        code: .invalidState, message: "resource canonical destination changed",
    ))
    #expect(try contents(root.child("document")) == Data("before\n".utf8))
}
