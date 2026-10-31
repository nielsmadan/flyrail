import Darwin
@testable import Flyrail
import Foundation
import Testing

private enum StoreCall {
    case write
    case delete
}

private struct StoreFault {
    let call: StoreCall
    let suffix: String
    var remaining: Int
    let outcome: Result<Bool, StoreError>
}

private final class FaultStore: StateStore, @unchecked Sendable {
    private let base: any StateStore
    private var faults: [StoreFault] = []

    init(_ base: any StateStore) {
        self.base = base
    }

    var location: ResourcePath? {
        base.location
    }

    var stagingDirectory: ResourcePath? {
        base.stagingDirectory
    }

    func arm(_ call: StoreCall, suffix: String, times: Int = 1, outcome: Result<Bool, StoreError>) {
        faults.append(StoreFault(call: call, suffix: suffix, remaining: times, outcome: outcome))
    }

    func disarm() {
        faults = []
    }

    func read(_ key: String) throws -> Data? {
        try base.read(key)
    }

    func write(_ key: String, value: Data, expected: Data?) throws -> Bool {
        if let outcome = take(.write, key) {
            return try outcome.get()
        }
        return try base.write(key, value: value, expected: expected)
    }

    func delete(_ key: String, expected: Data) throws -> Bool {
        if let outcome = take(.delete, key) {
            return try outcome.get()
        }
        return try base.delete(key, expected: expected)
    }

    func keys(prefix: String) throws -> [String] {
        try base.keys(prefix: prefix)
    }

    func lock(_ name: String, timeout: Duration) throws -> StoreLock {
        try base.lock(name, timeout: timeout)
    }

    private func take(_ call: StoreCall, _ key: String) -> Result<Bool, StoreError>? {
        guard let index = faults.firstIndex(where: { $0.call == call && $0.remaining > 0 && key.hasSuffix($0.suffix) })
        else { return nil }
        faults[index].remaining -= 1
        return faults[index].outcome
    }
}

private func faultTarget(_ root: NativeTestRoot, directory: Bool) throws -> (FaultStore, InstallationTarget) {
    let store = FaultStore(directory ? root.store : MemoryStore())
    return try (store, InstallationTarget(store: store))
}

private func installed(_ root: NativeTestRoot, target: InstallationTarget) throws -> ResourcePath {
    let path = try root.child("document")
    try NativeTransactionIO.createFile(path, data: Data("baseline\n".utf8))
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path, "first\n"),
                                 target: target, acquisition: .takeover).status == .applied)
    return path
}

private func second(_ path: ResourcePath, target: InstallationTarget) throws -> InstallationResult {
    try sync(bundle: transactionBundle("2"), rendered: transactionRendered(path, "second\n"), target: target)
}

@Test(arguments: [false, true])
func aReceiptCompareAndSwapMissRollsBackAndTheNextSyncApplies(_ directoryStore: Bool) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let (store, target) = try faultTarget(root, directory: directoryStore)
    let path = try installed(root, target: target)
    store.arm(.write, suffix: "/receipt", outcome: .success(false))

    let result = try second(path, target: target)
    #expect(result.status == .failed)
    #expect(try result.error == TargetError(
        code: .concurrentChange, message: "\(StateKey.resource(path, .receipt)): state changed", path: store.location,
    ))
    #expect(result.resources.first?.recoveryPaths == [])
    #expect(FileManager.default.contents(atPath: path.displayString) == Data("first\n".utf8))
    #expect(try storedReceipt(path, store: store)?.claims.first?.version == "1")
    #expect(try transactionRecordKeys(store) == [])

    #expect(try second(path, target: target).status == .applied)
    #expect(FileManager.default.contents(atPath: path.displayString) == Data("second\n".utf8))
}

@Test(arguments: [false, true], ["/journal", "/preparation"])
func aBusyMutationLockSurfacesAsBusyAndRecoveryConverges(_ directoryStore: Bool, _ record: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let (store, target) = try faultTarget(root, directory: directoryStore)
    let path = try installed(root, target: target)
    let call: StoreCall = record == "/journal" ? .write : .delete
    store.arm(call, suffix: record, outcome: .failure(.busy(name: "mutation")))

    let result = try second(path, target: target)
    #expect(result.error == TargetError(code: .busy, message: "state store lock is busy: mutation"))
    #expect(result.status == (call == .write ? .failed : .partial))
    #expect(try transactionRecover(target).error == nil)
    #expect(try transactionRecordKeys(store) == [])
    let version = call == .write ? "1" : "2"
    #expect(try storedReceipt(path, store: store)?.claims.first?.version == version)
    #expect(FileManager.default.contents(atPath: path.displayString) == Data((call == .write ? "first\n" :
            "second\n").utf8))
    #expect(try inspectInstallation(bundleID: "team", target: target).resources.map(\.recoveryPaths) == [[]])
}

@Test(arguments: [false, true])
func aPartialCleanupRetainsThePreparationUntilTheNextRecover(_ directoryStore: Bool) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let (store, target) = try faultTarget(root, directory: directoryStore)
    let path = try installed(root, target: target)
    let failure = StoreError.io(message: "injected preparation delete failure", path: store.location, errno: EIO)
    store.arm(.delete, suffix: "/preparation", times: .max, outcome: .failure(failure))

    let result = try second(path, target: target)
    store.disarm()
    #expect(result.status == .partial)
    #expect(result.error?.code == .ioError)
    #expect(result.error?.message.hasPrefix("injected preparation delete failure") == true)
    #expect(try store.read(StateKey.resource(path, .journal)) == nil)
    let preparation = try #require(try storedPreparation(path, store: store))
    #expect(result.resources.first?.recoveryPaths == [preparation.directory])
    #expect(FileManager.default.contents(atPath: path.displayString) == Data("second\n".utf8))

    #expect(try transactionRecover(target).status == .applied)
    #expect(try storedPreparation(path, store: store) == nil)
    #expect(try transactionRecordKeys(store) == [])
    let repeated = try second(path, target: target)
    #expect(repeated.status == .applied)
    #expect(try names(root) == ["document"])
}

private func names(_ root: NativeTestRoot) throws -> [String] {
    try FileManager.default.contentsOfDirectory(atPath: root.url.path)
}
