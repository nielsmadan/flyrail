import Darwin
@testable import Flyrail
import Foundation
import Testing

private final class WrappedStore: StateStore, @unchecked Sendable {
    let stagingDirectory: ResourcePath?
    let failing: Set<String>
    let failingReads: Set<String>
    private let inner = MemoryStore()
    private let guarded = NSLock()
    private var calls: [String] = []

    init(staging: ResourcePath? = nil, failing: Set<String> = [], failingReads: Set<String> = []) {
        stagingDirectory = staging
        self.failing = failing
        self.failingReads = failingReads
    }

    var recorded: [String] {
        guarded.withLock { calls }
    }

    func read(_ key: String) throws -> Data? {
        try record("read", key)
        return try inner.read(key)
    }

    func write(_ key: String, value: Data, expected: Data?) throws -> Bool {
        guarded.withLock { calls.append("write \(key)") }
        return try inner.write(key, value: value, expected: expected)
    }

    func delete(_ key: String, expected: Data) throws -> Bool {
        try inner.delete(key, expected: expected)
    }

    func keys(prefix: String) throws -> [String] {
        try record("keys", prefix)
        return try inner.keys(prefix: prefix)
    }

    func lock(_ name: String, timeout: Duration) throws -> StoreLock {
        try inner.lock(name, timeout: timeout)
    }

    private func record(_ operation: String, _ argument: String) throws {
        guarded.withLock { calls.append("\(operation) \(argument)") }
        if failing.contains(operation) || (operation == "read" && failingReads.contains(argument)) {
            throw StoreError.io(message: "store failed", path: nil, errno: EIO)
        }
    }
}

private func admissionError(_ body: () throws -> Void) -> TargetError? {
    do {
        try body()
        return nil
    } catch let error as TargetError {
        return error
    } catch {
        return nil
    }
}

private func directPhysicalPath(_ path: ResourcePath) throws -> ResourcePath {
    let raw = Darwin.open(path.displayString, O_RDONLY | O_DIRECTORY | O_CLOEXEC)
    try #require(raw >= 0)
    return try NativeFilesystem.physicalPath(NativeDescriptor(raw), path: path)
}

private func devIsAnotherDevice() -> Bool {
    var dev = stat()
    var cache = stat()
    let repository = stateRepositoryRoot().appending(path: ".cache").path
    return stat("/dev", &dev) == 0 && stat(repository, &cache) == 0 && dev.st_dev != cache.st_dev
}

private let transactionID = String(repeating: "a", count: 32)

@Test
func transactionDirectoryChoiceFollowsTheStoreStaging() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let destination = try root.child("document")
    let fallback = try transactionPath(destination: destination, transactionID: transactionID)
    let staging = try NativeTransactionDirectory.physicalLocation(ResourcePath(root.stateURL.path + "/staging"))
    let staged = try NativeFilesystem.child(
        staging, Array("\(StateKey.destination(destination))-\(transactionID)".utf8),
    )
    #expect(try NativeTransactionDirectory.choose(
        store: root.store, destination: destination, transactionID: transactionID,
    ) == staged)
    #expect(NativeTransactionDirectory.recognized(
        store: root.store, destination: destination, transactionID: transactionID, directory: staged,
    ))
    #expect(NativeTransactionDirectory.recognized(
        store: root.store, destination: destination, transactionID: transactionID, directory: fallback,
    ))
    let memory = MemoryStore()
    #expect(try NativeTransactionDirectory.choose(
        store: memory, destination: destination, transactionID: transactionID,
    ) == fallback)
    #expect(NativeTransactionDirectory.staged(store: memory, destination: destination, transactionID: transactionID)
        == nil)
    #expect(try !NativeTransactionDirectory.recognized(
        store: root.store, destination: destination, transactionID: transactionID,
        directory: root.child("other.txn"),
    ))
    #expect(admissionError {
        _ = try NativeTransactionDirectory.choose(store: memory, destination: destination, transactionID: "A")
    } == TargetError(code: .invalidState, message: "transaction id must be 32 lowercase hexadecimal characters"))
}

@Test(.enabled(if: devIsAnotherDevice()))
func transactionDirectoryChoiceFallsBackAcrossDevices() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let destination = try root.child("document")
    let store = try WrappedStore(staging: ResourcePath("/dev/flyrail-staging-test"))
    let staged = try #require(NativeTransactionDirectory.staged(
        store: store, destination: destination, transactionID: transactionID,
    ))
    #expect(!NativeTransactionDirectory.sameMount(staged, destination))
    #expect(try NativeTransactionDirectory.choose(
        store: store, destination: destination, transactionID: transactionID,
    ) == transactionPath(destination: destination, transactionID: transactionID))
    #expect(NativeTransactionDirectory.recognized(
        store: store, destination: destination, transactionID: transactionID, directory: staged,
    ))
}

@Test
func physicalLocationResolvesLeniently() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    try FileManager.default.createDirectory(at: root.url.appending(path: "a/b"), withIntermediateDirectories: true)
    let physical = try directPhysicalPath(root.child("a/b"))
    let base = root.path.displayString
    try FileManager.default.createSymbolicLink(atPath: base + "/relative", withDestinationPath: "a")
    try FileManager.default.createSymbolicLink(
        atPath: base + "/absolute", withDestinationPath: directPhysicalPath(root.child("a")).displayString,
    )
    for spelling in ["\(base)/a/./b", "/\(base)/a/b", "\(base)/x/../a/b", "\(base)/relative/b",
                     "\(base)/absolute/b", "\(base)/a/b", "\(base)/a/b/"]
    {
        #expect(try NativeTransactionDirectory.physicalLocation(ResourcePath(platformBytes: Array(spelling.utf8)))
            == physical)
    }
    for prefix in ["a/b", "relative/b", "absolute/b", "x/../a/b"] {
        #expect(try NativeTransactionDirectory.physicalLocation(ResourcePath("\(base)/\(prefix)/Missing/tail"))
            == appendPath(physical, "Missing/tail"))
    }
}

@Test
func storeAdmissionFencesTreesAndChildren() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let child = try NativeAuthority.reference(root.child("tree/child.md"), kind: .document)
    let tree = try NativeAuthority.reference(root.child("tree"), kind: .tree)
    let store = WrappedStore()
    try ResourceAdmission.admit(child, store: store)
    try ResourceAdmission.admit(child, store: store)
    let childKey = try StateKey.resource(child.destination, .authority)
    #expect(store.recorded.filter { $0.hasPrefix("write ") } == ["write \(childKey)"])
    #expect(try store.read(childKey) == StateCodec.encode(child))
    let conflict = TargetError(code: .conflict, message: "ancestor or descendant resource conflicts",
                               path: child.destination)
    #expect(admissionError { try ResourceAdmission.admit(tree, store: store) } == conflict)
    #expect(try store.read(StateKey.resource(tree.destination, .authority)) == StateCodec.encode(tree))
    #expect(try store.read(childKey) == StateCodec.encode(child))

    let reversed = MemoryStore()
    try ResourceAdmission.admit(tree, store: reversed)
    #expect(admissionError { try ResourceAdmission.admit(child, store: reversed) } == TargetError(
        code: .conflict, message: "ancestor or descendant resource conflicts", path: tree.destination,
    ))
    #expect(try reversed.read(childKey) == StateCodec.encode(child))

    let fence = try root.child("fenced/sub/.flyrail-x-\(transactionID).txn")
    try FileManager.default.createDirectory(atPath: fence.displayString, withIntermediateDirectories: true)
    let fenced = try NativeAuthority.reference(root.child("fenced"), kind: .tree)
    let fencePath = try appendPath(fenced.destination, "sub/.flyrail-x-\(transactionID).txn")
    #expect(admissionError { try ResourceAdmission.admit(fenced, store: MemoryStore()) } == TargetError(
        code: .conflict, message: "ancestor or descendant resource conflicts", path: fencePath,
    ))

    let document = try NativeAuthority.reference(root.child("kind.md"), kind: .document)
    let kinds = MemoryStore()
    try ResourceAdmission.admit(document, store: kinds)
    let mistyped = try ResourceRef(destination: document.destination, kind: .tree)
    #expect(admissionError { try ResourceAdmission.admit(mistyped, store: kinds) } == TargetError(
        code: .invalidState, message: "authority identity disagrees", path: document.destination,
    ))
}

@Test
func treeAdmissionReadsOnlyAuthorityRecords() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let tree = try NativeAuthority.reference(root.child("tree"), kind: .tree)
    let other = try NativeAuthority.reference(root.child("other.md"), kind: .document)
    let store = WrappedStore()
    #expect(try store.write(StateKey.resource(other.destination, .receipt), value: Data("{}".utf8), expected: nil))
    try ResourceAdmission.admit(tree, store: store)
    #expect(store.recorded.filter { $0.hasPrefix("keys ") } == ["keys resource/"])

    let canonical = try StateCodec.encode(other)
    let spaced = canonical.prefix(1) + Data(" ".utf8) + canonical.dropFirst()
    let otherKey = try StateKey.resource(other.destination, .authority)
    #expect(try store.write(otherKey, value: spaced, expected: nil))
    #expect(admissionError { try ResourceAdmission.admit(tree, store: store) } == TargetError(
        code: .invalidState, message: "\(otherKey): record is not canonical",
    ))
}

@Test
func storeFailuresDuringAdmissionAreTargetErrors() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let tree = try NativeAuthority.reference(root.child("tree"), kind: .tree)
    let other = try NativeAuthority.reference(root.child("other.md"), kind: .document)
    let child = try NativeAuthority.reference(root.child("tree/child.md"), kind: .document)
    let treeKey = try StateKey.resource(tree.destination, .authority)
    let otherKey = try StateKey.resource(other.destination, .authority)
    let expected = TargetError(code: .ioError, message: "store failed", errno: EIO)

    let ownRead = WrappedStore(failing: ["read"])
    #expect(admissionError { try ResourceAdmission.admit(tree, store: ownRead) } == expected)
    #expect(ownRead.recorded == ["read \(treeKey)"])

    let listing = WrappedStore(failing: ["keys"])
    #expect(admissionError { try ResourceAdmission.admit(tree, store: listing) } == expected)
    #expect(listing.recorded.last == "keys resource/")

    let scan = WrappedStore(failingReads: [otherKey])
    #expect(try scan.write(otherKey, value: StateCodec.encode(other), expected: nil))
    #expect(admissionError { try ResourceAdmission.admit(tree, store: scan) } == expected)
    let listed = try #require(scan.recorded.lastIndex(of: "keys resource/"))
    #expect(scan.recorded.last == "read \(otherKey)")
    #expect(scan.recorded.dropFirst(listed + 1).allSatisfy { $0.hasPrefix("read resource/") })

    let ancestor = WrappedStore(failingReads: [treeKey])
    #expect(admissionError { try ResourceAdmission.admit(child, store: ancestor) } == expected)
    #expect(ancestor.recorded.last == "read \(treeKey)")
}

@Test
func transactionEntryWalkStopsAtTheDepthLimit() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let limit = NativeLimits.default.maximumDepth
    let nested = Array(repeating: "d", count: limit + 2).joined(separator: "/")
    try FileManager.default.createDirectory(at: root.url.appending(path: "tree/\(nested)"),
                                            withIntermediateDirectories: true)
    let tree = try root.child("tree")
    let deepest = try appendPath(tree, Array(repeating: "d", count: limit + 1).joined(separator: "/"))
    #expect(admissionError { _ = try NativeTransactionDirectory.entries(inside: tree) } ==
        nativeIO(ENOTSUP, message: "resource exceeds the node limit", path: deepest))
}
