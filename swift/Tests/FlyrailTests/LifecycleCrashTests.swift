@testable import Flyrail
import Foundation
import Testing

private let lifecycleBoundaries = [
    "resource-precondition", "index-require", "guard-lookup", "preparation", "directory", "stage-node", "staged",
    "journal", "prepare", "backup-move", "backup", "rename", "staging", "publish", "publish-receipt", "receipt",
    "receipt-reread", "receipt-committed", "cleanup-node", "cleanup", "index-completion",
]

@Test(arguments: [false, true])
func lifecycleSyncRecoversFromACrashAtEveryForwardBoundary(_ directoryStore: Bool) throws {
    for boundary in lifecycleBoundaries {
        let root = try NativeTestRoot()
        defer { try? root.cleanup() }
        let store: any StateStore = directoryStore ? root.store : MemoryStore()
        let target = try InstallationTarget(store: store)
        let path = try root.child("document")
        let updated = try transactionRendered(path, "two\n")
        #expect(try sync(bundle: transactionBundle(), rendered: transactionRendered(path, "one\n"), target: target)
            .status == .applied)
        let (seam, fired) = crashSeam(at: boundary)
        #expect(throws: NativeSimulatedCrash(boundary)) {
            try transactionWrite(transactionBundle("2"), rendered: updated, target: target, seam: seam)
        }
        try #require(fired())
        #expect(try sync(bundle: transactionBundle("2"), rendered: updated, target: target).status == .applied)
        #expect(FileManager.default.contents(atPath: path.displayString) == Data("two\n".utf8))
        let records = try store.keys(prefix: "resource/").filter {
            $0.hasSuffix("/preparation") || $0.hasSuffix("/journal")
        }
        #expect(records == [])
        #expect(try FileManager.default.contentsOfDirectory(atPath: root.url.path) == ["document"])
        if directoryStore {
            let staging = root.stateURL.appending(path: "staging").path
            #expect(try FileManager.default.contentsOfDirectory(atPath: staging) == [])
        }
    }
}

private func names(_ path: ResourcePath) throws -> [String] {
    try FileManager.default.contentsOfDirectory(atPath: path.displayString).sorted()
}

@Test(arguments: [false, true])
func aCrashAfterCreatingTheTransactionDirectoryIsCleanedUpByTheNextRun(_ directoryStore: Bool) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store: any StateStore = directoryStore ? root.store : MemoryStore()
    let target = try InstallationTarget(store: store)
    let path = try root.child("AGENTS.md")
    try Data("# Foreign\n".utf8).write(to: URL(filePath: path.displayString))
    #expect(try sync(bundle: transactionBundle(), rendered: lifecycleSection(path, text: "one"), target: target)
        .status == .applied)
    let (seam, fired) = crashSeam(at: "directory")
    #expect(throws: NativeSimulatedCrash("directory")) {
        try transactionWrite(transactionBundle("2"), rendered: lifecycleSection(path, text: "two"), target: target,
                             seam: seam)
    }
    try #require(fired())
    let preparation = try #require(try storedPreparation(path, store: store))
    #expect(try store.read(StateKey.resource(path, .journal)) == nil)
    #expect(try names(preparation.directory) == ["backup", "staging"])
    #expect(try inspectInstallation(bundleID: "team", target: target).resources.map(\.recoveryPaths) == [
        [preparation.directory],
    ])

    #expect(try sync(bundle: transactionBundle("2"), rendered: lifecycleSection(path, text: "two"), target: target)
        .status == .applied)
    #expect(try storedPreparation(path, store: store) == nil)
    #expect(!FileManager.default.fileExists(atPath: preparation.directory.displayString))
    #expect(try names(root.path) == ["AGENTS.md"])
    #expect(try String(decoding: Data(contentsOf: root.url.appending(path: "AGENTS.md")), as: UTF8.self)
        .contains("two"))
}

@Test(arguments: [false, true], lifecycleBoundaries.filter { !["stage-node", "staging"].contains($0) })
func lifecycleRemovalRecoversFromACrashAtEveryForwardBoundary(_ directoryStore: Bool, _ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store: any StateStore = directoryStore ? root.store : MemoryStore()
    let target = try InstallationTarget(store: store)
    let path = try root.child("document")
    #expect(try sync(bundle: transactionBundle(), rendered: transactionRendered(path, "one\n"), target: target)
        .status == .applied)
    let (seam, fired) = crashSeam(at: boundary)
    #expect(throws: NativeSimulatedCrash(boundary)) {
        try transactionRemove("team", target: target, seam: seam)
    }
    try #require(fired())
    #expect(try transactionRemove("team", target: target).status == .applied)
    let index = try InstallationIndex.read(bundleID: "team", target: target)
    #expect(index?.current == nil && index?.pending == nil && index?.resources == [])
    #expect(try transactionRecordKeys(store) == [])
    #expect(try names(root.path) == [])
    if directoryStore {
        #expect(try names(ResourcePath(root.stateURL.appending(path: "staging").path)) == [])
    }
}

@Test(arguments: [false, true], lifecycleBoundaries)
func lifecycleTreeSyncRecoversFromACrashAtEveryForwardBoundary(_ directoryStore: Bool, _ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store: any StateStore = directoryStore ? root.store : MemoryStore()
    let target = try InstallationTarget(store: store)
    let path = try root.child("tree")
    func rendered(_ text: String) throws -> RenderedBundle {
        try RenderedBundle(artifacts: [
            RenderedArtifact(id: "guide", family: .skills, destination: path, content: .tree(TreeContent(entries: [
                BundleEntry(path: "nested/file", data: Data(text.utf8)),
                BundleEntry(path: "other", data: Data(text.utf8)),
            ]))),
        ])
    }
    #expect(try sync(bundle: transactionBundle(), rendered: rendered("one\n"), target: target).status == .applied)
    let (seam, fired) = crashSeam(at: boundary)
    #expect(throws: NativeSimulatedCrash(boundary)) {
        try transactionWrite(transactionBundle("2"), rendered: rendered("two\n"), target: target, seam: seam)
    }
    try #require(fired())
    #expect(try sync(bundle: transactionBundle("2"), rendered: rendered("two\n"), target: target).status ==
        .applied)
    #expect(try FileManager.default.contents(atPath: appendPath(path, "nested/file").displayString) ==
        Data("two\n".utf8))
    #expect(try transactionRecordKeys(store) == [])
    #expect(try names(root.path) == ["tree"])
    if directoryStore {
        #expect(try names(ResourcePath(root.stateURL.appending(path: "staging").path)) == [])
    }
}

@Test(arguments: [false, true], ResourceTransaction.recoveryBoundaries)
func lifecycleRecoveryCrashesConvergeOnTheNextRecover(_ directoryStore: Bool, _ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store: any StateStore = directoryStore ? root.store : MemoryStore()
    let target = try InstallationTarget(store: store)
    let path = try root.child("document")
    #expect(try sync(bundle: transactionBundle(), rendered: transactionRendered(path, "one\n"), target: target)
        .status == .applied)
    let (publish, published) = crashSeam(at: "publish")
    #expect(throws: NativeSimulatedCrash("publish")) {
        try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "two\n"), target: target,
                             seam: publish)
    }
    try #require(published())

    let (seam, fired) = crashSeam(at: boundary)
    #expect(throws: NativeSimulatedCrash(boundary)) { try transactionRecover(target, seam: seam) }
    try #require(fired())
    #expect(try transactionRecover(target).status == .applied)
    #expect(FileManager.default.contents(atPath: path.displayString) == Data("one\n".utf8))
    #expect(try storedReceipt(path, store: store)?.claims.first?.version == "1")
    #expect(try transactionRecordKeys(store) == [])
    #expect(try names(root.path) == ["document"])
}
