import Darwin
@testable import Flyrail
import Foundation
import Testing

private let forwardTransactionBoundaries = ResourceTransaction.boundaries.filter {
    !ResourceTransaction.recoveryBoundaries.contains($0)
}

private let committedBoundaries: Set = [
    "receipt-reread", "receipt-committed", "cleanup-node", "cleanup", "index-completion",
]

private let unguardedBoundaries: Set = ["resource-precondition", "index-require", "guard-lookup"]

private func faultSeam(at boundary: String, _ action: @escaping () throws -> Void = {})
    -> (NativeTransactionSeam, fired: () -> Bool)
{
    var fired = false
    let seam = NativeTransactionSeam { selected in
        guard selected == boundary, !fired else { return }
        fired = true
        try action()
        throw TargetError(code: .ioError, message: "injected \(boundary)", errno: EIO)
    }
    return (seam, { fired })
}

private func installed(_ root: NativeTestRoot, target: InstallationTarget, text: String) throws -> ResourcePath {
    let path = try root.child("document")
    try NativeTransactionIO.createFile(path, data: Data("baseline\n".utf8))
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path, text),
                                 target: target, acquisition: .takeover).status == .applied)
    return path
}

@Test(arguments: forwardTransactionBoundaries)
func nativeEveryTransactionFaultKeepsPermanentCommitsAndRecognizedRecovery(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let path = try installed(root, target: target, text: "first\n")
    let (seam, fired) = faultSeam(at: boundary)
    let result = try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second\n"),
                                      target: target, seam: seam)
    try #require(fired())
    #expect(result.error == TargetError(code: .ioError, message: "injected \(boundary)", errno: EIO))
    #expect(result.resources.first?.recoveryPaths == [])
    #expect(try transactionRecordKeys(target.store) == [])
    let committed = committedBoundaries.contains(boundary)
    #expect(try NativeObservation.snapshot(path).revision.data == Data((committed ? "second\n" : "first\n").utf8))
    #expect(result.status == (committed ? .partial : .failed))
    if committed {
        #expect(result.resources.first?.status == .applied)
    }
    #expect(try transactionRecover(target).error == nil)
    #expect(try transactionRecover(target).error == nil)
    #expect(try storedReceipt(path, store: target.store)?.claims.first?.version == (committed ? "2" : "1"))
    #expect(try transactionRecordKeys(target.store) == [])
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("baseline\n".utf8))
}

@Test
func nativeReceiptRereadFailureCannotUndoTheCommitBoundary() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let path = try installed(root, target: target, text: "first\n")
    let (seam, fired) = faultSeam(at: "receipt-reread") {
        try Data("later edit\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
    }
    let result = try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second\n"),
                                      target: target, seam: seam)
    try #require(fired())
    #expect(result.status == .partial)
    #expect(result.resources.first?.status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("later edit\n".utf8))
    #expect(try transactionRecover(target).error == nil)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("later edit\n".utf8))
    #expect(try storedReceipt(path, store: target.store)?.claims.first?.version == "2")
    #expect(try transactionRemove("team", target: target).error?.code == .modified)
}

@Test(arguments: ["journal", "prepare", "backup-move", "backup", "staging", "publish", "publish-receipt", "receipt"])
func nativeForeignChangesAtEachPublicationSeamAreRetained(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let path = try installed(root, target: target, text: "first")
    var changed: ResourcePath?
    let seam = NativeTransactionSeam {
        guard $0 == boundary, changed == nil else { return }
        let directory = try #require(try storedPreparation(path, store: target.store)).directory
        let location: ResourcePath = switch boundary {
        case "backup": try appendPath(directory, "backup/resource")
        case "staging": try appendPath(directory, "staging/resource")
        default: path
        }
        changed = location
        try Data("unexpected foreign credential".utf8).write(to: URL(fileURLWithPath: location.displayString))
    }
    let result = try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second"),
                                      target: target, seam: seam)
    let location = try #require(changed)
    let foreign = Data("unexpected foreign credential".utf8)
    #expect(FileManager.default.contents(atPath: location.displayString) == foreign)
    if boundary == "receipt" {
        #expect(result.status == .applied)
        #expect(try storedReceipt(path, store: target.store)?.claims.first?.version == "2")
        #expect(try transactionRemove("team", target: target).error?.code == .modified)
    } else {
        #expect(result.status == .incomplete)
        #expect(try storedReceipt(path, store: target.store)?.claims.first?.version == "1")
        let retained = try #require(try storedPreparation(path, store: target.store)).directory
        #expect(result.resources.first?.recoveryPaths == [retained])
        #expect(try transactionRecover(target).status == .incomplete)
    }
    #expect(FileManager.default.contents(atPath: location.displayString) == foreign)
}

@Test
func nativePartialResourceCommitRetainsAllIndexReferences() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let rendered = try RenderedBundle(artifacts: ["one", "two"].map {
        try RenderedArtifact(id: $0, family: .instructions, destination: root.child($0),
                             content: .file(FileContent(data: Data($0.utf8))))
    })
    var receipts = 0
    let seam = NativeTransactionSeam {
        if $0 == "receipt" {
            receipts += 1
            if receipts == 2 {
                throw TargetError(code: .ioError, message: "second receipt failed")
            }
        }
    }
    let result = try transactionWrite(transactionBundle(), rendered: rendered, target: target, seam: seam)
    #expect(receipts == 2)
    #expect(result.status == .partial)
    #expect(result.resources.map(\.status) == [.applied, .failed])
    let observed = try InstallationIndex.read(bundleID: "team", target: target)
    #expect(observed?.resources.count == 2)
    #expect(observed?.pending?.membership.count == 2)
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(root.child("one")).revision.nodes.isEmpty)
    #expect(try NativeObservation.snapshot(root.child("two")).revision.nodes.isEmpty)
}

@Test(arguments: unguardedBoundaries.sorted())
func nativeUnguardedFaultsAttemptNoRecoveryAndKeepThePendingIndex(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let path = try installed(root, target: target, text: "first\n")
    let (seam, fired) = faultSeam(at: boundary)
    let result = try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second\n"),
                                      target: target, seam: seam)
    try #require(fired())
    #expect(result.status == .failed)
    #expect(result.error == TargetError(code: .ioError, message: "injected \(boundary)", errno: EIO))
    #expect(result.resources.first?.recoveryPaths == [])
    let index = try #require(try InstallationIndex.read(bundleID: "team", target: target))
    #expect(index.pending?.version == "2")
    #expect(index.current?.version == "1")
    #expect(try transactionRecordKeys(target.store) == [])
    #expect(FileManager.default.contents(atPath: path.displayString) == Data("first\n".utf8))
}

@Test(arguments: unguardedBoundaries.sorted())
func nativeLaterPreconditionFailureRetainsTheEarlierCommit(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let rendered = try RenderedBundle(artifacts: ["first", "second"].map {
        try RenderedArtifact(id: $0, family: .instructions, destination: root.child($0),
                             content: .file(FileContent(data: Data("\($0)\n".utf8))))
    })
    var reached = 0
    let seam = NativeTransactionSeam { selected in
        guard selected == boundary else { return }
        reached += 1
        if reached == 2 {
            throw TargetError(code: .ioError, message: "injected \(boundary)", errno: EIO)
        }
    }
    let result = try transactionWrite(transactionBundle(), rendered: rendered, target: target, seam: seam)
    try #require(reached == 2)
    #expect(result.status == .partial)
    #expect(result.error == TargetError(code: .ioError, message: "injected \(boundary)", errno: EIO))
    #expect(result.resources.map(\.status) == [.applied, .failed])
    let applied = try #require(result.resources.first).resource.destination
    let failed = try #require(result.resources.last).resource.destination
    #expect(try Set([applied, failed]) == Set([root.child("first"), root.child("second")]))
    let name = URL(fileURLWithPath: applied.displayString).lastPathComponent
    #expect(FileManager.default.contents(atPath: applied.displayString) == Data("\(name)\n".utf8))
    #expect(try NativeObservation.snapshot(failed).revision.nodes.isEmpty)
    #expect(try transactionRecordKeys(target.store) == [])
}

private let foreign = Data("foreign edit\n".utf8)

private let publicationEditPoints = [
    "before-publish-move", "after-publish-move", "before-receipt", "after-receipt", "before-rollback",
    "after-rollback-unpublish",
]

private let foreignEditCases = ["replace-document", "replace-tree", "create-document", "create-tree"].flatMap { shape in
    (shape.hasPrefix("replace") ? ["before-backup-move", "after-backup-move"] + publicationEditPoints :
        publicationEditPoints).map { (shape, $0) }
}

private func foreignEditSeam(_ point: String) -> String {
    switch point {
    case "before-backup-move": "backup-move"
    case "after-backup-move": "backup"
    case "before-publish-move": "staging"
    case "after-publish-move": "publish"
    case "after-receipt": "receipt-committed"
    case "after-rollback-unpublish": "rollback-stage"
    default: "publish-receipt"
    }
}

private func appendForeign(_ path: ResourcePath, tree: Bool) throws {
    let location = URL(fileURLWithPath: path.displayString)
    if tree, !FileManager.default.fileExists(atPath: location.path) {
        try FileManager.default.createDirectory(at: location, withIntermediateDirectories: false)
    }
    let file = tree ? location.appending(path: "foreign") : location
    guard FileManager.default.fileExists(atPath: file.path) else {
        try foreign.write(to: file)
        return
    }
    let handle = try FileHandle(forWritingTo: file)
    defer { try? handle.close() }
    try handle.seekToEnd()
    try handle.write(contentsOf: foreign)
}

private func retainsForeign(_ root: NativeTestRoot) -> Bool {
    [root.url, root.stateURL.appending(path: "staging")].flatMap { base in
        (FileManager.default.enumerator(atPath: base.path)?.allObjects as? [String] ?? []).map {
            base.appending(path: $0).path
        }
    }.contains { FileManager.default.contents(atPath: $0)?.range(of: foreign) != nil }
}

@Test(arguments: foreignEditCases)
func nativeForeignEditsDuringPublicationAreNeverLost(_ shape: String, _ point: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let tree = shape.hasSuffix("tree")
    let path = try root.child(tree ? "skill" : "AGENTS.md")
    func rendered(_ text: String) throws -> RenderedBundle {
        guard tree else { return try transactionRendered(path, text) }
        return try RenderedBundle(artifacts: [
            RenderedArtifact(id: "guide", family: .skills, destination: path, content: .tree(TreeContent(entries: [
                BundleEntry(path: "SKILL.md", data: Data(text.utf8)),
            ]))),
        ])
    }
    if shape.hasPrefix("replace") {
        #expect(try transactionWrite(transactionBundle(), rendered: rendered("one\n"), target: target)
            .status == .applied)
    }
    let rollback = point.hasSuffix("rollback") || point.hasPrefix("after-rollback")
    var edits = 0
    var faulted = false
    let seam = NativeTransactionSeam { selected in
        if selected == foreignEditSeam(point), edits == 0 {
            edits += 1
            let backup = try appendPath(#require(try storedPreparation(path, store: target.store)).directory,
                                        "backup/resource")
            try appendForeign(point == "after-backup-move" ? backup : path, tree: tree)
        }
        if rollback, selected == "publish-receipt", !faulted {
            faulted = true
            throw TargetError(code: .ioError, message: "injected publish-receipt", errno: EIO)
        }
    }
    _ = try transactionWrite(transactionBundle("2"), rendered: rendered("two\n"), target: target, seam: seam)
    #expect(edits == 1)
    #expect(faulted == rollback)
    #expect(retainsForeign(root))
}
