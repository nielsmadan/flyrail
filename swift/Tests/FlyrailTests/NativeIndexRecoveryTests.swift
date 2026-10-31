import Darwin
@testable import Flyrail
import Foundation
import Testing

private final class LockRecordingStore: StateStore, @unchecked Sendable {
    private let inner: any StateStore
    private let guardLock = NSLock()
    private var acquired: [String] = []

    init(_ inner: any StateStore) {
        self.inner = inner
    }

    var location: ResourcePath? {
        inner.location
    }

    var stagingDirectory: ResourcePath? {
        inner.stagingDirectory
    }

    var lockNames: [String] {
        guardLock.withLock { acquired }
    }

    func read(_ key: String) throws -> Data? {
        try inner.read(key)
    }

    func write(_ key: String, value: Data, expected: Data?) throws -> Bool {
        try inner.write(key, value: value, expected: expected)
    }

    func delete(_ key: String, expected: Data) throws -> Bool {
        try inner.delete(key, expected: expected)
    }

    func keys(prefix: String) throws -> [String] {
        try inner.keys(prefix: prefix)
    }

    func lock(_ name: String, timeout: Duration) throws -> StoreLock {
        let held = try inner.lock(name, timeout: timeout)
        guardLock.withLock { acquired.append(name) }
        return held
    }
}

@Test
func nativeTransactionLocksAreIndexFirstThenSortedPermanentAuthorities() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let competing = try DirectoryStore(ResourcePath(root.stateURL.path))
    let references = try ["z", "a", "m"].map { try NativeAuthority.reference(root.child($0), kind: .document) }
    do {
        let held = try competing.lock(InstallationIndex.key(bundleID: "team", target: target), timeout: .zero)
        defer { held.release() }
        let clock = ContinuousClock()
        let start = clock.now
        expectNativeError(.busy) {
            let locks = try NativeTransactionLocks(target: target, bundleID: "team", resources: references,
                                                   timeout: .milliseconds(25)) {}
            locks.release()
        }
        #expect(start.duration(to: clock.now) >= .milliseconds(25))
        #expect(try root.store.keys(prefix: "resource/") == [])
    }
    let recording = LockRecordingStore(root.store)
    let recorded = try InstallationTarget(store: recording)
    let locks = try NativeTransactionLocks(target: recorded, bundleID: "team", resources: references,
                                           timeout: .zero) {}
    defer { locks.release() }
    #expect(locks.resources == destinationOrder(references))
    #expect(try recording.lockNames == [InstallationIndex.key(bundleID: "team", target: recorded)] +
        destinationOrder(references).map { try StateKey.resourceLock($0.destination) })
    for resource in references {
        let name = try StateKey.resourceLock(resource.destination)
        #expect(throws: StoreError.busy(name: name)) { _ = try competing.lock(name, timeout: .zero) }
        #expect(try root.store.read(StateKey.resource(resource.destination, .authority)) == StateCodec.encode(resource))
    }
}

@Test(arguments: ["current", "previous"])
func nativeMissingCommittedReceiptPreservesAllIndexAndRecoveryEvidence(_ membership: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try root.target()
    let store = root.store
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path), target: target)
        .status == .applied)
    let current = try #require(try InstallationIndex.read(bundleID: "team", target: target))
    var recorded = current
    if membership == "previous" {
        recorded = try Index(bundleID: "team", contextDigest: target.contextDigest, previous: current.current,
                             residual: current.resources)
        try StateRecords.write(store, key: InstallationIndex.key(bundleID: "team", target: target),
                               value: recorded, expected: current)
    }
    let receipt = try #require(try storedReceipt(path, store: store))
    try StateRecords.delete(store, key: StateKey.resource(path, .receipt), expected: receipt)
    let storeBefore = try storeContents(store)
    let filesBefore = try NativeObservation.snapshot(root.path)
    let message = "recorded ownership receipt is missing; restore metadata before retrying"
    let inspection = try inspectInstallation(bundleID: "team", target: target)
    #expect(!inspection.isCurrent)
    #expect(inspection.resources.map(\.error?.code) == [.invalidState])
    #expect(inspection.resources.first?.error?.message == message)
    for result in try [transactionRecover(target), transactionRemove("team", target: target)] {
        #expect(result.status == .failed)
        #expect(result.error?.code == .invalidState)
        #expect(result.error?.message == message)
    }
    #expect(try storeContents(store) == storeBefore)
    #expect(try NativeObservation.snapshot(root.path) == filesBefore)
    #expect(try InstallationIndex.read(bundleID: "team", target: target) == recorded)
    try StateRecords.write(store, key: StateKey.resource(path, .receipt), value: receipt, expected: nil)
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.nodes.isEmpty)
}

@Test
func nativeJournalRepeatingThePreviousReceiptIsRefusedWithEvidenceRetained() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try root.target()
    let store = root.store
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path, "one\n"), target: target)
        .status == .applied)
    let (seam, fired) = crashSeam(at: "backup")
    #expect(throws: NativeSimulatedCrash("backup")) {
        try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "two\n"), target: target,
                             seam: seam)
    }
    try #require(fired())
    let key = try StateKey.resource(path, .journal)
    let crashed = try #require(try StateRecords.read(store, key: key, as: Journal.self, tag: "Journal"))
    let repeated = try Journal(
        resource: crashed.resource, previous: crashed.receipt, receipt: crashed.receipt, before: crashed.before,
        staged: crashed.staged, published: crashed.published, backup: crashed.backup, ancestors: crashed.ancestors,
    )
    try StateRecords.write(store, key: key, value: repeated, expected: crashed)
    let directory = try #require(try storedPreparation(path, store: store)).directory
    let staged = try appendPath(directory, "staging/resource")
    let backup = try appendPath(directory, "backup/resource")
    let evidence = try (NativeObservation.snapshot(staged), NativeObservation.snapshot(backup))
    #expect(!FileManager.default.fileExists(atPath: path.displayString))
    #expect(evidence.1.revision.data == Data("one\n".utf8))

    let result = try transactionRecover(target)

    #expect(result.status == .incomplete)
    #expect(result.error?.code == .recoveryNeeded)
    #expect(result.error?.message == "transaction record repeats the previous receipt")
    #expect(result.resources.first?.recoveryPaths == [directory])
    #expect(!FileManager.default.fileExists(atPath: path.displayString))
    #expect(try (NativeObservation.snapshot(staged), NativeObservation.snapshot(backup)) == evidence)
    #expect(try StateRecords.read(store, key: key, as: Journal.self, tag: "Journal") == repeated)
}
