import Darwin
@testable import Flyrail
import Foundation
import Testing

private let preparationBoundaries = ["preparation", "directory", "stage-node", "staged", "journal"]

private func installedDocument(_ root: NativeTestRoot, target: InstallationTarget) throws -> ResourcePath {
    let path = try root.child("document")
    try NativeTransactionIO.createFile(path, data: Data("baseline".utf8))
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path, "first"),
                                 target: target, acquisition: .takeover).status == .applied)
    return path
}

private func exists(_ path: ResourcePath) -> Bool {
    var metadata = stat()
    return lstat(path.displayString, &metadata) == 0
}

@Test(arguments: preparationBoundaries)
func nativeCrashedPreparationsAreAbandonedByRecovery(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let path = try installedDocument(root, target: target)
    let (seam, fired) = crashSeam(at: boundary)
    #expect(throws: NativeSimulatedCrash(boundary)) {
        try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second"),
                             target: target, seam: seam)
    }
    try #require(fired())
    let preparation = try #require(try storedPreparation(path, store: target.store))
    #expect(try transactionRecordKeys(target.store) == [StateKey.resource(path, .preparation)])

    #expect(try transactionRecover(target).status == .applied)
    #expect(FileManager.default.contents(atPath: path.displayString) == Data("first".utf8))
    #expect(try storedReceipt(path, store: target.store)?.claims.first?.version == "1")
    #expect(try transactionRecordKeys(target.store) == [])
    #expect(!exists(preparation.directory))
}

@Test(arguments: preparationBoundaries)
func nativeFailedPreparationsCompensateWithinTheCall(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let path = try installedDocument(root, target: target)
    var directory: ResourcePath?
    let seam = NativeTransactionSeam {
        guard $0 == boundary, directory == nil else { return }
        directory = try storedPreparation(path, store: target.store)?.directory
        throw TargetError(code: .ioError, message: "interrupted creation")
    }
    let result = try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second"),
                                      target: target, seam: seam)
    #expect(result.status == .failed)
    #expect(result.error == TargetError(code: .ioError, message: "interrupted creation"))
    #expect(result.resources.first?.recoveryPaths == [])
    let planned = try #require(directory)
    #expect(!exists(planned))
    #expect(try transactionRecordKeys(target.store) == [])
    #expect(FileManager.default.contents(atPath: path.displayString) == Data("first".utf8))
    #expect(try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second"),
                                 target: target).status == .applied)
}

@Test
func nativeAnExistingTransactionDirectoryReleasesThePreparation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    try NativeTransactionIO.createFile(root.child("document"), data: Data("before\n".utf8))
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let plan = try storePlan(ref, store: store, after: Revision(nodes: [
        NativeResourcePlanner.node("", data: Data("after\n".utf8), mode: 0o644),
    ]), previous: nil)
    let seam = NativeTransactionSeam {
        if $0 == "preparation" {
            try FileManager.default.createDirectory(atPath: plan.transactionDirectory.displayString,
                                                    withIntermediateDirectories: false)
        }
    }
    #expect(throws: TargetError(
        code: .recoveryNeeded, message: "unrecognized transaction data retained", path: plan.transactionDirectory,
    )) { _ = try ResourceTransaction.prepare(plan, store: store, seam: seam) }
    #expect(FileManager.default.fileExists(atPath: plan.transactionDirectory.displayString))
    #expect(try transactionRecordKeys(store) == [])
    #expect(FileManager.default.contents(atPath: ref.destination.displayString) == Data("before\n".utf8))
}

@Test
func nativeNonUTF8TransactionRoutesAreRejectedWithoutCreatingState() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let bytes = try ResourcePath(platformBytes: root.path.platformBytes + [47, 255])
    let target = try root.target()
    let result = try transactionWrite(transactionBundle(), rendered: transactionRendered(bytes), target: target)
    #expect(result.status == .failed)
    #expect(result.error?.code == .unsafePath)
    #expect(result.error?.path?.platformBytes == bytes.platformBytes)
    #expect(try NativeObservation.snapshot(root.path).revision.nodes.count == 1)
    #expect(try StateRecords.keys(target.store, prefix: "") == [])
}

@Test
func nativePartiallyStagedTreeIsDestroyedByRecovery() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("tree")
    let target = try root.target()
    func rendered(_ text: String) throws -> RenderedBundle {
        let content = try TreeContent(entries: [
            BundleEntry(path: "a/file", data: Data(text.utf8)),
            BundleEntry(path: "b/file", data: Data(text.utf8)),
        ])
        return try RenderedBundle(artifacts: [
            RenderedArtifact(id: "guide", family: .skills, destination: path, content: .tree(content)),
        ])
    }
    #expect(try transactionWrite(transactionBundle(), rendered: rendered("first"), target: target).status == .applied)
    let installed = try NativeObservation.snapshot(path)
    var staged = 0
    let seam = NativeTransactionSeam {
        guard $0 == "stage-node" else { return }
        staged += 1
        if staged == 2 {
            throw NativeSimulatedCrash("stage-node")
        }
    }
    #expect(throws: NativeSimulatedCrash("stage-node")) {
        try transactionWrite(transactionBundle("2"), rendered: rendered("second"), target: target, seam: seam)
    }
    #expect(staged == 2)
    let preparation = try #require(try storedPreparation(path, store: target.store))
    let partial = try appendPath(preparation.directory, "staging/resource")
    #expect(try NativeObservation.snapshot(partial).revision.nodes.map(\.path) == ["", "a"])

    #expect(try transactionRecover(target).status == .applied)
    #expect(!exists(preparation.directory))
    #expect(try transactionRecordKeys(target.store) == [])
    #expect(try NativeObservation.snapshot(path) == installed)
}
