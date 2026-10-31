import Darwin
@testable import Flyrail
import Foundation
import Testing

private enum StoreOperation {
    case read
    case write
    case delete
}

private enum StorePhase {
    case before
    case after
}

private struct StoreCall {
    let operation: StoreOperation
    let phase: StorePhase
    let key: String
    let base: any StateStore
}

private final class HookedStore: StateStore, @unchecked Sendable {
    private let base: any StateStore
    private let guarded = NSLock()
    private var hook: ((StoreCall) throws -> Void)?

    init(_ base: any StateStore) {
        self.base = base
    }

    var location: ResourcePath? {
        base.location
    }

    var stagingDirectory: ResourcePath? {
        base.stagingDirectory
    }

    func set(_ hook: @escaping (StoreCall) throws -> Void) {
        guarded.withLock { self.hook = hook }
    }

    func clear() {
        guarded.withLock { hook = nil }
    }

    func read(_ key: String) throws -> Data? {
        try call(.read, .before, key)
        let result = try base.read(key)
        try call(.read, .after, key)
        return result
    }

    func write(_ key: String, value: Data, expected: Data?) throws -> Bool {
        try call(.write, .before, key)
        let result = try base.write(key, value: value, expected: expected)
        try call(.write, .after, key)
        return result
    }

    func delete(_ key: String, expected: Data) throws -> Bool {
        try call(.delete, .before, key)
        let result = try base.delete(key, expected: expected)
        try call(.delete, .after, key)
        return result
    }

    func keys(prefix: String) throws -> [String] {
        try base.keys(prefix: prefix)
    }

    func lock(_ name: String, timeout: Duration) throws -> StoreLock {
        try base.lock(name, timeout: timeout)
    }

    private func call(_ operation: StoreOperation, _ phase: StorePhase, _ key: String) throws {
        let hook = guarded.withLock { self.hook }
        try hook?(StoreCall(operation: operation, phase: phase, key: key, base: base))
    }
}

private func injected(_ message: String) -> StoreError {
    .io(message: message, path: nil, errno: EIO)
}

private func names(_ path: ResourcePath) throws -> [String] {
    try FileManager.default.contentsOfDirectory(atPath: path.displayString).sorted()
}

private func exists(_ path: ResourcePath) -> Bool {
    var metadata = stat()
    return lstat(path.displayString, &metadata) == 0
}

private func transactionDirectories(_ root: NativeTestRoot, store: any StateStore) throws -> [String] {
    let beside = try names(root.path).filter(NativeTransactionDirectory.isTransactionName)
    guard let staging = store.stagingDirectory, exists(staging) else { return beside }
    return try beside + names(staging)
}

private func section(_ path: ResourcePath, _ text: String, id: String = "guide") throws -> RenderedBundle {
    try RenderedBundle(artifacts: [
        RenderedArtifact(id: id, family: .instructions, destination: path,
                         content: .section(SectionContent(marker: id, text: text))),
    ])
}

private func tree(_ path: ResourcePath, _ text: String) throws -> RenderedBundle {
    try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .skills, destination: path, content: .tree(TreeContent(entries: [
            BundleEntry(path: "a.txt", data: Data(text.utf8)),
        ]))),
    ])
}

private func applied(_ version: String, _ rendered: RenderedBundle, target: InstallationTarget) throws {
    let result = try sync(bundle: transactionBundle(version), rendered: rendered, target: target)
    #expect(result.status == .applied, "\(String(describing: result.error))")
}

private func contents(_ path: ResourcePath) -> Data? {
    FileManager.default.contents(atPath: path.displayString)
}

@Test
func anUnwritableStoreFails() throws {
    let root = try NativeTestRoot()
    defer {
        chmod(root.stateURL.path, 0o700)
        try? root.cleanup()
    }
    try FileManager.default.createDirectory(at: root.stateURL, withIntermediateDirectories: false)
    try #require(chmod(root.stateURL.path, 0o500) == 0)
    let path = try root.child("AGENTS.md")
    let result = try sync(bundle: transactionBundle(), rendered: section(path, "generated"), target: root.target())
    try #require(chmod(root.stateURL.path, 0o700) == 0)
    #expect(result.status == .failed)
    let staging = try #require(root.store.stagingDirectory)
    #expect(result.error == TargetError(
        code: .ioError, message: "[Errno \(EACCES)] Permission denied: '\(staging.displayString)'", path: staging,
        errno: EACCES,
    ))
    #expect(!exists(path))
}

@Test(arguments: ["index/", "/authority", "/preparation", "/journal", "/receipt"])
func aStoreWriteFailureAtEachProtocolPointKeepsTheDestination(_ fragment: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = HookedStore(root.store)
    let target = try InstallationTarget(store: store)
    let path = try root.child("AGENTS.md")
    let second = try fragment == "/authority" ? root.child("CLAUDE.md") : path
    try applied("1", section(path, "one"), target: target)
    let before = contents(path)
    store.set { call in
        if call.operation == .write, call.phase == .before, call.key.contains(fragment) {
            throw injected("injected store failure")
        }
    }
    let failed = try sync(bundle: transactionBundle("2"), rendered: section(second, "two"), target: target)
    store.clear()
    #expect(failed.status == .failed)
    #expect(failed.error == TargetError(code: .ioError, message: "injected store failure", errno: EIO))
    #expect(contents(path) == before)
    #expect(try transactionDirectories(root, store: store) == [])
    if second != path {
        #expect(!exists(second))
    }
    try applied("2", section(second, "two"), target: target)
}

@Test
func anotherStoreLeavesALiveTransactionDirectoryUntouched() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("AGENTS.md")
    try Data("# project\n".utf8).write(to: root.url.appending(path: "AGENTS.md"))
    let first = HookedStore(MemoryStore())
    let other = try FlyrailBundle(
        identity: BundleIdentity(id: "other", version: "1"),
        artifacts: [.instruction(InstructionArtifact(id: "notes", text: "text"))], assets: [], dependencies: [],
    )
    let second = try root.target()
    var seen: [(ResourcePath, Revision, Revision, InstallationResult)] = []
    first.set { call in
        guard call.operation == .write, call.phase == .after, call.key.hasSuffix("/journal") else { return }
        let directory = try #require(try StateRecords.preparation(call.base, path)).directory
        let before = try NativeObservation.snapshot(directory).revision
        let result = try sync(bundle: other, rendered: section(path, "from B", id: "notes"), target: second)
        try seen.append((directory, before, NativeObservation.snapshot(directory).revision, result))
    }
    let result = try sync(bundle: transactionBundle(), rendered: section(path, "from A"), target: InstallationTarget(
        store: first,
    ))
    first.clear()
    let (directory, before, after, fromSecond) = try #require(seen.first)
    #expect(seen.count == 1)
    #expect(parentPath(directory) == root.path)
    #expect(!before.nodes.isEmpty)
    #expect(after == before)
    #expect(fromSecond.status == .applied)
    #expect(fromSecond.resources.map(\.recoveryPaths) == [[]])
    #expect(String(decoding: contents(path) ?? Data(), as: UTF8.self).contains("from B"))
    #expect(result.status == .incomplete)
    #expect(result.error == TargetError(
        code: .concurrentChange,
        message: "complete resource revision changed; recovery: cannot restore complete previous revision",
        path: path,
    ))
    #expect(recoverInstallation(bundleID: "other", target: second).status == .unchanged)
}

@Test(arguments: [false, true])
func aStoreReadFailureAfterStagingDiscardsTheStagedRevision(_ isTree: Bool) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = HookedStore(root.store)
    let target = try InstallationTarget(store: store)
    let path = try root.child(isTree ? "out" : "AGENTS.md")
    let desired = { (text: String) in try isTree ? tree(path, text) : section(path, text) }
    try applied("1", desired("one"), target: target)
    let before = try NativeObservation.snapshot(path).revision
    var armed = false
    store.set { call in
        if call.operation == .write, call.phase == .after, call.key.hasSuffix("/preparation") {
            armed = true
        }
        if armed, call.operation == .read, call.key.hasSuffix("/authority") {
            throw injected("injected store read failure")
        }
    }
    let failed = try sync(bundle: transactionBundle("2"), rendered: desired("two"), target: target)
    store.clear()
    #expect(failed.status == .failed)
    #expect(failed.error == TargetError(code: .ioError, message: "injected store read failure", errno: EIO))
    #expect(try NativeObservation.snapshot(path).revision == before)
    #expect(try transactionDirectories(root, store: store) == [])
    #expect(try StateRecords.preparation(store, path) == nil)
    try applied("2", desired("two"), target: target)
    let published = try isTree ? appendPath(path, "a.txt") : path
    #expect(String(decoding: contents(published) ?? Data(), as: UTF8.self).contains("two"))
}

@Test
func aStagingDirectoryAnotherWriterCreatesBeforeLockingIsNotApproved() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let path = try root.child("AGENTS.md")
    let proposal = try preview(bundle: transactionBundle(), rendered: section(path, "generated"), target: target)
    let staging = try #require(root.store.stagingDirectory)
    for directory in [root.stateURL.path, staging.displayString] {
        try #require(mkdir(directory, 0o700) == 0)
    }
    let result = applyPreview(proposal)
    #expect(result.status == .failed)
    #expect(try result.error == TargetError(
        code: .concurrentChange, message: "preview ancestor changed",
        path: TransactionLayout.stagingDirectory(#require(proposal.resources.first).transactionDirectory),
    ))
    #expect(!exists(path))
    #expect(try names(staging) == [])
}

private final class ResultBox: @unchecked Sendable {
    private let guarded = NSLock()
    private var stored: InstallationResult?

    var value: InstallationResult? {
        guarded.withLock { stored }
    }

    func set(_ result: InstallationResult) {
        guarded.withLock { stored = result }
    }
}

@Test
func aParentRecreatedDuringTheLockWaitIsNotApproved() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let location = try root.child("new/state")
    let store = try DirectoryStore(location)
    let target = try InstallationTarget(store: store)
    let path = try root.child("AGENTS.md")
    let index = try InstallationIndex.key(bundleID: "team", target: target)
    let held = try store.lock(index, timeout: .zero)
    let proposal = try preview(bundle: transactionBundle(), rendered: section(path, "generated"), target: target)
    let staging = try #require(store.stagingDirectory)
    #expect(!exists(staging))
    let box = ResultBox()
    let done = DispatchSemaphore(value: 0)
    Thread.detachNewThread {
        box.set(applyPreview(proposal, lockTimeout: .seconds(20)))
        done.signal()
    }
    let deadline = ContinuousClock.now.advanced(by: .seconds(20))
    while !exists(staging) {
        try #require(box.value == nil && ContinuousClock.now < deadline)
        usleep(2000)
    }
    usleep(200_000)
    #expect(box.value == nil)
    try #require(rename(staging.displayString, root.url.appending(path: "replaced").path) == 0)
    try #require(mkdir(staging.displayString, 0o500) == 0)
    held.release()
    done.wait()
    let result = try #require(box.value)
    #expect(result.status == .failed)
    #expect(try result.error == TargetError(
        code: .concurrentChange, message: "preview ancestor changed",
        path: TransactionLayout.stagingDirectory(#require(proposal.resources.first).transactionDirectory),
    ))
    #expect(!exists(path))
    #expect(try store.read(index) == nil)
    #expect(try store.read(StateKey.resource(path, .receipt)) == nil)
}

@Test(arguments: [false, true])
func anUnreadableJournalReportsOnlyTheDirectoryItsPreparationNames(_ preparationReadable: Bool) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = HookedStore(root.store)
    let target = try InstallationTarget(store: store)
    let path = try root.child("AGENTS.md")
    try applied("1", section(path, "one"), target: target)
    let unreadable = preparationReadable ? ["/journal"] : ["/journal", "/preparation"]
    var directories: [ResourcePath] = []
    var armed = false
    store.set { call in
        if armed, call.operation == .read, unreadable.contains(where: { call.key.hasSuffix($0) }) {
            throw injected("injected store read failure")
        }
        guard call.operation == .write, call.key.hasSuffix("/journal") else { return }
        if call.phase == .before {
            try directories.append(#require(try StateRecords.preparation(call.base, path)).directory)
        } else {
            armed = true
        }
    }
    let result = try sync(bundle: transactionBundle("2"), rendered: section(path, "two"), target: target)
    store.clear()
    let directory = try #require(directories.first)
    #expect(try parentPath(directory) == NativeTransactionDirectory.physicalLocation(#require(store.stagingDirectory)))
    #expect(result.resources.map(\.recoveryPaths) == [preparationReadable ? [directory] : []])
    #expect(result.status == (preparationReadable ? .incomplete : .failed))
    #expect(result.error == TargetError(
        code: .ioError, message: "injected store read failure; recovery: injected store read failure", errno: EIO,
    ))
    #expect(recoverInstallation(bundleID: "team", target: target).status == .applied)
    try applied("2", section(path, "two"), target: target)
    #expect(String(decoding: contents(path) ?? Data(), as: UTF8.self).contains("two"))
}

@Test
func lockAcquisitionRefusesAnIndexChangedSinceObservation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let path = try root.child("document")
    try applied("1", transactionRendered(path), target: target)
    let ref = try NativeAuthority.reference(path, kind: .document)
    #expect(kernelError {
        _ = try LifecycleExecution.locks(bundleID: "team", target: target, expected: nil, resources: [ref],
                                         timeout: .zero)
    } == TargetError(code: .concurrentChange, message: "index changed while acquiring locks",
                     path: root.store.location))
    let index = try InstallationIndex.key(bundleID: "team", target: target)
    let reacquired = try root.store.lock(index, timeout: .zero)
    reacquired.release()
    #expect(try sync(bundle: transactionBundle(), rendered: transactionRendered(path), target: target).status ==
        .unchanged)
}
