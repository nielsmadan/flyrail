@testable import Flyrail
import Foundation
import Testing

private final class StagedStore: StateStore {
    let stagingDirectory: ResourcePath?
    private let inner = MemoryStore()

    init(staging: ResourcePath) {
        stagingDirectory = staging
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
        try inner.lock(name, timeout: timeout)
    }
}

private func pairs(_ values: [(String, String)]) -> [String] {
    values.map { "\($0.0)=\($0.1)" }
}

@Test
func targetDigestIgnoresTheStore() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let memory = MemoryStore()
    let empty = "37517e5f3dc66819f61f5a7bb8ace1921282415f10551d2defa5c3eb0985b570"
    #expect(try InstallationTarget(store: memory).contextDigest == empty)
    #expect(try InstallationTarget(store: root.store).contextDigest == empty)
    let context = [("destination", "/x")]
    let destination = "5b134fc5c9711eb90652d0fecdbb9c2dc5c84098880e0ba529376e1a7a734c0c"
    #expect(try InstallationTarget(store: memory, context: context).contextDigest == destination)
    #expect(try InstallationTarget(store: memory, context: context) ==
        InstallationTarget(store: memory, context: context))
    #expect(try InstallationTarget(store: memory, context: context) !=
        InstallationTarget(store: MemoryStore(), context: context))
    #expect(try NativeFilesystem.metadata(ResourcePath(root.stateURL.path)) == nil)
}

@Test
func oneStoreKeepsIndependentIndexesPerContext() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = MemoryStore()
    let targets = try ["one", "two"].map { try Target.directory(root.child($0)) }
    let results = try install(lifecycleSkills(), targets: targets, store: store)
    #expect(results.map(\.status) == [.applied, .applied])
    #expect(try store.keys(prefix: "index/team/").count == 2)
}

@Test
func malformedIndexRecordAdmitsNothing() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let path = try root.child("document")
    let key = try InstallationIndex.key(bundleID: "team", target: target)
    let index = try Index(bundleID: "team", contextDigest: target.contextDigest)
    #expect(try root.store.write(key, value: StateCodec.encode(index) + Data(" ".utf8), expected: nil))
    #expect(try inspectInstallation(bundleID: "team", target: target).error?.code == .invalidState)
    let result = try sync(bundle: transactionBundle(), rendered: transactionRendered(path), target: target)
    #expect(result.status == .failed)
    #expect(result.error?.code == .invalidState)
    let ref = try NativeAuthority.reference(path, kind: .document)
    #expect(try root.store.read(StateKey.resource(ref.destination, .authority)) == nil)
    #expect(try NativeFilesystem.metadata(path) == nil)
}

@Test
func indexContextMismatchIsInvalidState() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = MemoryStore()
    let target = try InstallationTarget(store: store)
    let other = try InstallationTarget(store: store, context: [("destination", "/x")])
    let index = try Index(bundleID: "team", contextDigest: other.contextDigest)
    try StateRecords.write(store, key: InstallationIndex.key(bundleID: "team", target: target), value: index,
                           expected: nil)
    do {
        _ = try InstallationIndex.read(bundleID: "team", target: target)
        Issue.record("mismatched index context was accepted")
    } catch let error as TargetError {
        #expect(error.code == .invalidState)
        #expect(error.message == "installation index context disagrees")
    }
    #expect(try inspectInstallation(bundleID: "team", target: target).error?.code == .invalidState)
}

@Test
func aStoreInsideAManagedDestinationIsRefused() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let skills = try root.child("skills")
    let store = try DirectoryStore(appendPath(skills, "state"))
    let result = try install(lifecycleSkills(), targets: [Target.directory(skills)], store: store)[0]
    #expect(result.status == .failed)
    let error = try #require(result.error)
    #expect(error.message.contains("overlap"))
    #expect(result.observation.state == .unknown)
    #expect(result.observation.error == error)
    #expect(try store.keys(prefix: "") == [])
    #expect(try NativeFilesystem.metadata(skills) == nil)
}

@Test
func aStagingDirectoryThatOverlapsADestinationIsRefused() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let staging = try root.child("state.staging")
    let store = StagedStore(staging: staging)
    let target = try InstallationTarget(store: store)
    let rendered = try lifecycleSection(appendPath(staging, "AGENTS.md"))
    #expect(throws: FlyrailValueError("state store overlaps a resource or authority")) {
        try preview(bundle: transactionBundle(), rendered: rendered, target: target)
    }
    #expect(try store.keys(prefix: "") == [])
    #expect(try NativeObservation.snapshot(root.path).revision.nodes.map(\.path) == [""])
}

@Test
func renderContextInstallationTargets() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = MemoryStore()
    let context = try RenderContext(target: .project(agent: .claude, root: root.path), platform: .macOS)
    let derived = try context.installation(store: store)
    #expect(pairs(derived.context) == [
        "agent=claude", "root=\(root.path.displayString)", "scope=project", "surface=cli",
    ])
    #expect(try pairs(derived.routingContext) == pairs(context.routingContext))
    let explicit = try context.installation(store: store, context: [("app", "notes")])
    #expect(pairs(explicit.context) == ["app=notes"])
    #expect(try pairs(explicit.routingContext) == pairs(context.routingContext))
    #expect(explicit.store === store)
}
