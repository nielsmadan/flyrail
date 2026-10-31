import Darwin
@testable import Flyrail
import Foundation
import Testing

private final class Collected<Value: Sendable>: @unchecked Sendable {
    private let guardLock = NSLock()
    private var values: [Value] = []

    func append(_ value: Value) {
        guardLock.withLock { values.append(value) }
    }

    var snapshot: [Value] {
        guardLock.withLock { values }
    }
}

private func concurrently(_ count: Int, _ body: @escaping @Sendable (Int) throws -> Void) -> [String] {
    let errors = Collected<String>()
    let start = DispatchSemaphore(value: 0)
    let group = DispatchGroup()
    for index in 0 ..< count {
        group.enter()
        Thread {
            start.wait()
            do {
                try body(index)
            } catch {
                errors.append(String(describing: error))
            }
            group.leave()
        }.start()
    }
    (0 ..< count).forEach { _ in start.signal() }
    group.wait()
    return errors.snapshot
}

private func storeError(_ body: () throws -> Void) -> StoreError? {
    do {
        try body()
        return nil
    } catch let error as StoreError {
        return error
    } catch {
        return nil
    }
}

private func targetError(_ body: () throws -> Void) -> TargetError? {
    do {
        try body()
        return nil
    } catch let error as TargetError {
        return error
    } catch {
        return nil
    }
}

private func permissions(_ path: ResourcePath) -> mode_t {
    var metadata = stat()
    return lstat(path.displayString, &metadata) == 0 ? metadata.st_mode & 0o777 : 0
}

private func privateRefusal(_ path: ResourcePath) -> StoreError {
    .io(
        message: "[Errno 13] state store must be private to its owner: \(pythonStringRepr(path.displayString))",
        path: path, errno: EACCES,
    )
}

private func expectLockReleasesOnScopeExit(_ first: any StateStore, _ second: any StateStore) throws {
    do {
        let held = try first.lock("held", timeout: .zero)
        withExtendedLifetime(held) {
            #expect(storeError {
                let lock = try second.lock("held", timeout: .zero)
                lock.release()
            } == .busy(name: "held"))
        }
    }
    let again = try second.lock("held", timeout: .zero)
    again.release()
}

@Test
func memoryStoreMeetsTheContract() throws {
    let memory = MemoryStore()
    try checkStore { memory }
}

@Test
func directoryStoreMeetsTheContract() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("state")
    try checkStore { try DirectoryStore(path) }
}

private final class IgnoringExpectedStore: StateStore, @unchecked Sendable {
    private let inner = MemoryStore()

    func read(_ key: String) throws -> Data? {
        try inner.read(key)
    }

    func write(_ key: String, value: Data, expected _: Data?) throws -> Bool {
        let current = try inner.read(key)
        return try inner.write(key, value: value, expected: current)
    }

    func delete(_ key: String, expected: Data) throws -> Bool {
        try inner.delete(key, expected: expected)
    }

    func keys(prefix: String) throws -> [String] {
        try inner.keys(prefix: prefix)
    }

    func lock(_ name: String, timeout: Duration) throws -> StoreLock {
        try inner.lock(name, timeout: timeout)
    }
}

@Test
func checkStoreNamesTheGuaranteeABrokenStoreMisses() {
    let broken = IgnoringExpectedStore()
    #expect(throws: StoreConformanceFailure("creating an existing key must fail")) {
        try checkStore { broken }
    }
}

@Test
func directoryStoreKeepsPrivateRecordsAndRefusesAPublicRoot() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("state")
    let store = try DirectoryStore(path)
    #expect(try store.write("index/app/x", value: Data("value".utf8), expected: nil))
    #expect(try permissions(root.child("state/records")) == 0o700)
    #expect(try permissions(root.child("state/records/index~app~x")) == 0o600)
    #expect(chmod(path.displayString, 0o777) == 0)
    #expect(storeError { _ = try store.read("index/app/x") } == privateRefusal(path))
}

@Test
func directoryStoreRefusesASymlinkedTrustedDirectory() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("state")
    let store = try DirectoryStore(path)
    #expect(try store.write("index/app/x", value: Data("value".utf8), expected: nil))
    let records = try root.child("state/records")
    let elsewhere = try root.child("elsewhere")
    #expect(mkdir(elsewhere.displayString, 0o700) == 0)
    try FileManager.default.removeItem(atPath: records.displayString)
    #expect(symlink(elsewhere.displayString, records.displayString) == 0)
    #expect(storeError { _ = try store.read("index/app/x") } == privateRefusal(records))
}

@Test
func invalidKeysAreRejectedBeforeIO() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("absent")
    let store = try DirectoryStore(path)
    #expect(storeError { _ = try store.read("Upper/case") } ==
        .invalidKey(message: "state key must be slash-separated lowercase segments"))
    #expect(storeError { _ = try store.lock("Upper", timeout: .zero) } ==
        .invalidKey(message: "lock name must be slash-separated lowercase segments"))
    #expect(!FileManager.default.fileExists(atPath: path.displayString))
}

@Test
func storeErrorsMapToTargetErrors() throws {
    let path = try ResourcePath("/tmp/store")
    for code in [ENOTSUP, ENOSYS, EXDEV, EINVAL] {
        #expect(StoreError.io(message: "failed", path: path, errno: code).targetError ==
            TargetError(code: .unsupported, message: "failed", path: path, errno: code))
    }
    #expect(StoreError.io(message: "failed", path: path, errno: EIO).targetError ==
        TargetError(code: .ioError, message: "failed", path: path, errno: EIO))
    #expect(StoreError.busy(name: "index/x").targetError ==
        TargetError(code: .busy, message: "state store lock is busy: index/x"))
    #expect(StoreError.invalidKey(message: "bad").targetError == TargetError(code: .invalidState, message: "bad"))
}

@Test
func stagingDirectories() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = try DirectoryStore(root.child("state"))
    #expect(try store.location == root.child("state"))
    #expect(try store.stagingDirectory == root.child("state/staging"))
    let memory = MemoryStore()
    #expect(memory.location == nil)
    #expect(memory.stagingDirectory == nil)
    let cwd = FileManager.default.currentDirectoryPath
    #expect(try DirectoryStore(ResourcePath("relative")).location == ResourcePath("\(cwd)/relative"))
    #expect(try DirectoryStore(ResourcePath("a/./b//c/")).location == ResourcePath("\(cwd)/a/b/c"))
    #expect(try DirectoryStore(ResourcePath("//x/./y")).location == ResourcePath("//x/y"))
    #expect(throws: FlyrailValueError("directory store path must be a nonempty string path without NUL")) {
        _ = try DirectoryStore(ResourcePath(""))
    }
}

@Test
func keysFollowTheSharedScheme() throws {
    #expect(StateKey.index(bundleID: "team", contextDigest: "abc") == "index/team/abc")
    let destination = try ResourcePath("/tmp/AGENTS.md")
    let key = try StateKey.destination(destination)
    #expect(key == stateSHA256(Data("/tmp/agents.md".utf8)))
    #expect(try StateKey.resource(destination, .receipt) == "resource/\(key)/receipt")
    #expect(try StateKey.resource(destination, .preparation) == "resource/\(key)/preparation")
    #expect(try StateKey.resourceLock(destination) == "resource/\(key)")
}

@Test
func recordsRoundTripWithCompareAndSet() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let stores: [any StateStore] = try [MemoryStore(), DirectoryStore(root.child("state"))]
    for store in stores {
        let key = "index/team/x"
        let value = try Revision()
        try StateRecords.write(store, key: key, value: value, expected: nil)
        #expect(try StateRecords.read(store, key: key, as: Revision.self, tag: "Revision") == value)
        let changed = TargetError(code: .concurrentChange, message: "\(key): state changed", path: store.location)
        #expect(targetError { try StateRecords.write(store, key: key, value: value, expected: nil) } == changed)
        try StateRecords.delete(store, key: key, expected: value)
        #expect(try StateRecords.read(store, key: key, as: Revision.self, tag: "Revision") == nil)
        #expect(targetError { try StateRecords.delete(store, key: key, expected: value) } == changed)
    }
}

@Test
func nonCanonicalAndMistypedRecordsAreInvalidState() throws {
    let store = MemoryStore()
    let canonical = try StateCodec.encode(Revision())
    let spaced = canonical.prefix(1) + Data(" ".utf8) + canonical.dropFirst()
    #expect(try store.write("index/team/spaced", value: spaced, expected: nil))
    #expect(targetError {
        _ = try StateRecords.read(store, key: "index/team/spaced", as: Revision.self, tag: "Revision")
    }
        == TargetError(code: .invalidState, message: "index/team/spaced: record is not canonical"))
    #expect(try store.write("index/team/mistyped", value: canonical, expected: nil))
    #expect(targetError {
        _ = try StateRecords.read(store, key: "index/team/mistyped", as: Receipt.self, tag: "Receipt")
    }
        == TargetError(code: .invalidState, message: "index/team/mistyped: record has the wrong type"))
}

@Test
func aHeldLockReportsBusy() throws {
    let store = MemoryStore()
    let held = try StateRecords.lock(store, name: "index/team/x", timeout: .zero)
    defer { held.release() }
    #expect(targetError {
        let lock = try StateRecords.lock(store, name: "index/team/x", timeout: .zero)
        lock.release()
    } == TargetError(code: .busy, message: "state store lock is busy: index/team/x"))
}

@Test
func storeLocksReleaseOnScopeExit() throws {
    let memory = MemoryStore()
    try expectLockReleasesOnScopeExit(memory, memory)
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("state")
    try expectLockReleasesOnScopeExit(DirectoryStore(path), DirectoryStore(path))
}

@Test
func memoryStoreWaiterAcquiresWhenReleased() throws {
    let store = MemoryStore()
    let held = DispatchSemaphore(value: 0)
    let released = Collected<Bool>()
    Thread {
        do {
            let lock = try store.lock("held", timeout: .zero)
            held.signal()
            Thread.sleep(forTimeInterval: 0.05)
            released.append(true)
            lock.release()
        } catch {
            held.signal()
        }
    }.start()
    held.wait()
    let lock = try store.lock("held", timeout: .seconds(5))
    lock.release()
    #expect(released.snapshot == [true])
}

@Test
func directoryStoreCreateIsExclusiveAcrossHandles() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("state")
    let winners = Collected<Int>()
    let errors = concurrently(8) { index in
        let store = try DirectoryStore(path)
        if try store.write("race/k", value: Data("\(index)".utf8), expected: nil) {
            winners.append(index)
        }
    }
    #expect(errors.isEmpty)
    let winner = try #require(winners.snapshot.count == 1 ? winners.snapshot.first : nil)
    #expect(try DirectoryStore(path).read("race/k") == Data("\(winner)".utf8))
}

@Test
func directoryStoreCompareAndSetIncrementsAcrossThreads() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("state")
    let errors = concurrently(4) { _ in
        let store = try DirectoryStore(path)
        for _ in 0 ..< 25 {
            var stored = false
            while !stored {
                let old = try store.read("counter/k")
                let next = (old.flatMap { Int(String(decoding: $0, as: UTF8.self)) } ?? 0) + 1
                stored = try store.write("counter/k", value: Data("\(next)".utf8), expected: old)
            }
        }
    }
    #expect(errors.isEmpty)
    #expect(try DirectoryStore(path).read("counter/k") == Data("100".utf8))
}

@Test
func directoryStoreKeysSkipTemporaryFiles() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = try DirectoryStore(root.child("state"))
    #expect(try store.write("a/b", value: Data("value".utf8), expected: nil))
    let planted = try root.child("state/records/.x.tmp")
    #expect(FileManager.default.createFile(atPath: planted.displayString, contents: Data()))
    #expect(try store.keys(prefix: "") == ["a/b"])
}

private final class FailingStore: StateStore, @unchecked Sendable {
    func read(_: String) throws -> Data? {
        throw StoreError.io(message: "read failed", path: nil, errno: EXDEV)
    }

    func write(_: String, value _: Data, expected _: Data?) throws -> Bool {
        throw StoreError.io(message: "write failed", path: nil, errno: EXDEV)
    }

    func delete(_: String, expected _: Data) throws -> Bool {
        throw StoreError.io(message: "delete failed", path: nil, errno: EXDEV)
    }

    func keys(prefix _: String) throws -> [String] {
        throw StoreError.io(message: "keys failed", path: nil, errno: EXDEV)
    }

    func lock(_: String, timeout _: Duration) throws -> StoreLock {
        throw StoreError.io(message: "lock failed", path: nil, errno: EXDEV)
    }
}

@Test
func stateRecordsConvertStoreErrorsToTargetErrors() {
    let store = FailingStore()
    let bodies: [() throws -> Void] = [
        { _ = try StateRecords.raw(store, key: "a/b") },
        { _ = try StateRecords.keys(store, prefix: "") },
        { _ = try StateRecords.lock(store, name: "x", timeout: .zero) },
    ]
    for body in bodies {
        do {
            try body()
            Issue.record("expected a TargetError")
        } catch let error as TargetError {
            #expect(error.code == .unsupported)
            #expect(error.errno == EXDEV)
        } catch {
            Issue.record("unexpected error \(error)")
        }
    }
}

@Test
func stateRecordsRequireIndexDetectsChange() throws {
    let store = MemoryStore()
    try StateRecords.requireIndex(store, key: "index/a/b", expected: nil)
    #expect(try store.write("index/a/b", value: Data("x".utf8), expected: nil))
    do {
        try StateRecords.requireIndex(store, key: "index/a/b", expected: nil)
        Issue.record("expected a TargetError")
    } catch let error as TargetError {
        #expect(error.code == .concurrentChange)
        #expect(error.message == "installation index changed")
    }
}

@Test
func transactionDetailMapsStoreErrors() {
    #expect(NativeTransactionExecution.detail(StoreError.busy(name: "x")).code == .busy)
    #expect(NativeTransactionExecution.detail(StoreError.invalidKey(message: "bad")).code == .invalidState)
}

@Test
func storeLocksRejectNegativeTimeouts() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let stores: [any StateStore] = try [MemoryStore(), DirectoryStore(root.child("state"))]
    for store in stores {
        #expect(throws: FlyrailValueError("lock timeout must be finite and nonnegative")) {
            _ = try store.lock("x", timeout: .seconds(-1))
        }
    }
}

@Test
func parentPathTreatsDoubleSlashAsTheRoot() throws {
    let parents = try ["/a/b", "/a/", "//a", "/a", "//", "/"].map { try parentPath(ResourcePath($0))?.displayString }
    #expect(parents == ["/a", "/", "/", "/", nil, nil])
}

@Test
func storeKeysAcceptTwoHundredBytesAndRefuseLongerKeys() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let longest = "k/" + String(repeating: "a", count: 198)
    let stores: [any StateStore] = try [MemoryStore(), DirectoryStore(root.child("state"))]
    for store in stores {
        #expect(try store.write(longest, value: Data("v".utf8), expected: nil))
        #expect(try store.read(longest) == Data("v".utf8))
        #expect(try store.keys(prefix: "k/") == [longest])
        #expect(throws: StoreError.invalidKey(message: "state key must be slash-separated lowercase segments")) {
            _ = try store.write(longest + "a", value: Data("v".utf8), expected: nil)
        }
        #expect(throws: StoreError.invalidKey(message: "lock name must be slash-separated lowercase segments")) {
            _ = try store.lock(longest + "a", timeout: .zero)
        }
    }
}
