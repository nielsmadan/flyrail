import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func lifecyclePreviewIsReadOnlyAndRemovalPreservesForeignBytes() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("AGENTS.md")
    try NativeTransactionIO.createFile(path, data: Data("# Foreign\n".utf8))
    let target = try root.target()
    let bundle = try transactionBundle()
    let rendered = try lifecycleSection(path)
    let before = try NativeObservation.snapshot(root.path)
    let proposed = try preview(bundle: bundle, rendered: rendered, target: target)
    #expect(proposed.applicable)
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(try summarize(proposed).changes.map(\.action) == [.update])
    let result = applyPreview(proposed)
    #expect(result.status == .applied)
    #expect(result.observation.matches(bundle, rendered))
    #expect(try sync(bundle: bundle, rendered: rendered, target: target).status == .unchanged)
    #expect(remove(bundleID: "team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("# Foreign\n".utf8))
    #expect(remove(bundleID: "team", target: target).status == .unchanged)
}

@Test(arguments: ["payload", "index", "receipt", "mode", "ancestor"])
func lifecycleRejectsEveryStalePreview(_ mutation: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("nested/document")
    let target = try root.target()
    let bundle = try transactionBundle()
    let rendered = try lifecycleSection(path)
    #expect(try sync(bundle: bundle, rendered: rendered, target: target).status == .applied)
    let proposed = try preview(bundle: transactionBundle("2"), rendered: rendered, target: target)
    let resource = try #require(proposed.resources.first?.resource)
    switch mutation {
    case "payload": try Data("foreign\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
    case "index":
        let key = try InstallationIndex.key(bundleID: "team", target: target)
        let current = try #require(try target.store.read(key))
        #expect(try target.store.write(key, value: current + Data(" ".utf8), expected: current))
    case "receipt":
        let key = try StateKey.resource(resource.destination, .receipt)
        let current = try #require(try target.store.read(key))
        #expect(try target.store.write(key, value: current + Data(" ".utf8), expected: current))
    case "mode": #expect(chmod(path.displayString, 0o644) == 0)
    case "ancestor":
        let parent = try root.child("nested")
        #expect(try rename(parent.displayString, root.child("moved").displayString) == 0)
        #expect(mkdir(parent.displayString, 0o700) == 0)
    default: Issue.record("unknown mutation")
    }
    let changed = try NativeObservation.snapshot(root.path)
    #expect(applyPreview(proposed).error?.code == .concurrentChange)
    #expect(try NativeObservation.snapshot(root.path) == changed)
}

@Test(arguments: [Acquisition.conflict, .adopt, .takeover])
func lifecycleAcquisitionAndFirstBaseline(_ acquisition: Acquisition) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let original = Data("<!-- flyrail:guide:start -->\noriginal\n<!-- flyrail:guide:end -->\n".utf8)
    try NativeTransactionIO.createFile(path, data: original)
    let target = try root.target()
    let result = try sync(bundle: transactionBundle(), rendered: lifecycleSection(path, text: "original"),
                          target: target, acquisition: acquisition)
    if acquisition == .conflict {
        #expect(result.error?.code == .conflict)
        #expect(try NativeObservation.snapshot(path).revision.data == original)
        return
    }
    #expect(result.status == .applied)
    #expect(try sync(bundle: transactionBundle("2"), rendered: lifecycleSection(path, text: "changed", id: "renamed"),
                     target: target).status == .applied)
    #expect(remove(bundleID: "team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == (acquisition == .takeover ? original : Data()))
}

@Test
func lifecyclePreflightsEveryResourceAndMissingReceipt() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let first = try root.child("first")
    let second = try root.child("second")
    try NativeTransactionIO.createFile(second, data: Data("foreign".utf8))
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "first", family: .instructions, destination: first,
                         content: .file(FileContent(data: Data("new".utf8)))),
        RenderedArtifact(id: "second", family: .instructions, destination: second,
                         content: .file(FileContent(data: Data("new".utf8)))),
    ])
    let target = try root.target()
    #expect(try sync(bundle: transactionBundle(), rendered: rendered, target: target).error?.code == .conflict)
    #expect(try NativeFilesystem.metadata(first) == nil)
    #expect(try sync(bundle: transactionBundle(), rendered: rendered, target: target,
                     acquisition: .takeover).status == .applied)
    let key = try StateKey.resource(NativeResourcePlanner.reference(rendered.artifacts[0]).destination, .receipt)
    #expect(try root.store.delete(key, expected: #require(try root.store.read(key))))
    let before = try NativeObservation.snapshot(root.path)
    let keys = try root.store.keys(prefix: "")
    #expect(try inspectInstallation(bundleID: "team", target: target).isCurrent == false)
    #expect(remove(bundleID: "team", target: target).error?.code == .invalidState)
    #expect(recoverInstallation(bundleID: "team", target: target).error?.code == .invalidState)
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(try root.store.keys(prefix: "") == keys)
}

private final class FailingReadStore: StateStore, @unchecked Sendable {
    private let inner = MemoryStore()
    private let guarded = NSLock()
    private var failing = false

    func failResourceReads() {
        guarded.withLock { failing = true }
    }

    func read(_ key: String) throws -> Data? {
        if guarded.withLock({ failing }), key.hasPrefix("resource/") {
            throw StoreError.io(message: "injected read failure", path: nil, errno: EIO)
        }
        return try inner.read(key)
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
        try inner.lock(name, timeout: timeout)
    }
}

@Test
func storeReadFailuresBecomeTargetErrors() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = FailingReadStore()
    let target = try InstallationTarget(store: store)
    let rendered = try transactionRendered(root.child("document"))
    #expect(try sync(bundle: transactionBundle(), rendered: rendered, target: target).status == .applied)
    store.failResourceReads()
    let proposed = try preview(bundle: transactionBundle("2"), rendered: rendered, target: target)
    #expect(proposed.error?.code == .ioError)
    #expect(proposed.error?.errno == EIO)
    let observed = try inspectInstallation(bundleID: "team", target: target)
    #expect(observed.resources.map { $0.error?.code } == [.ioError])
}
