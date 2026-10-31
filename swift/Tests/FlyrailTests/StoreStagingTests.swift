import Darwin
@testable import Flyrail
import Foundation
import Testing

private func names(_ path: ResourcePath) throws -> [String] {
    try FileManager.default.contentsOfDirectory(atPath: path.displayString).sorted()
}

private func installedDocument(
    _ root: NativeTestRoot, store: any StateStore,
) throws -> (path: ResourcePath, target: InstallationTarget) {
    let path = try root.child("document")
    let target = try InstallationTarget(store: store)
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path, "one\n"), target: target)
        .status == .applied)
    return (path, target)
}

@Test
func aDriftRepairCommitsAFreshTransactionAndANoOpKeepsTheReceipt() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let (path, target) = try installedDocument(root, store: root.store)
    let rendered = try transactionRendered(path, "one\n")
    let first = try #require(try storedReceipt(path, store: root.store))
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target).status == .unchanged)
    #expect(try storedReceipt(path, store: root.store) == first)

    try Data("drifted\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target,
                                 replaceModified: true).status == .applied)
    let repaired = try #require(try storedReceipt(path, store: root.store))
    #expect(repaired.transactionID != first.transactionID)
    #expect(try Receipt(resource: repaired.resource, transactionID: first.transactionID, claims: repaired.claims,
                        provenance: repaired.provenance, createdDirectories: repaired.createdDirectories) == first)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("one\n".utf8))
}

@Test(arguments: ["directory", "backup", "publish"])
func aSameMountDirectoryStoreLeavesNothingBesideTheDestination(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let (path, target) = try installedDocument(root, store: root.store)
    let staging = try #require(root.store.stagingDirectory)
    let (seam, fired) = crashSeam(at: boundary)
    #expect(throws: NativeSimulatedCrash(boundary)) {
        try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "two\n"), target: target,
                             seam: seam)
    }
    try #require(fired())
    let preparation = try #require(try storedPreparation(path, store: root.store))
    #expect(try preparation.directory.displayString.hasPrefix(
        NativeTransactionDirectory.physicalLocation(staging).displayString + "/",
    ))
    #expect(FileManager.default.fileExists(atPath: preparation.directory.displayString))
    #expect(try names(root.path).allSatisfy { $0 == "document" })

    #expect(try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "two\n"),
                                 target: target).status == .applied)
    #expect(try names(staging) == [])
    #expect(try names(root.path) == ["document"])
}

@Test
func aMemoryStoreFallsBackToATransactionDirectoryBesideTheDestination() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = MemoryStore()
    let (path, target) = try installedDocument(root, store: store)
    let (seam, fired) = crashSeam(at: "directory")
    #expect(throws: NativeSimulatedCrash("directory")) {
        try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "two\n"), target: target,
                             seam: seam)
    }
    try #require(fired())
    let preparation = try #require(try storedPreparation(path, store: store))
    #expect(try preparation.directory == transactionPath(destination: path, transactionID: preparation.transactionID))
    #expect(FileManager.default.fileExists(atPath: preparation.directory.displayString))
    #expect(try names(root.path).count == 2)

    #expect(try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "two\n"),
                                 target: target).status == .applied)
    #expect(!FileManager.default.fileExists(atPath: preparation.directory.displayString))
    #expect(try names(root.path) == ["document"])
}

@Test(arguments: ["store", "beside"])
func recoveryRefusesThePreviousRevisionRewrittenInPlaceAfterPreparation(_ placement: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store: any StateStore = placement == "store" ? root.store : MemoryStore()
    let (path, target) = try installedDocument(root, store: store)
    let (seam, fired) = crashSeam(at: "prepare")
    #expect(throws: NativeSimulatedCrash("prepare")) {
        try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "two\n"), target: target,
                             seam: seam)
    }
    try #require(fired())
    let preparation = try #require(try storedPreparation(path, store: store))
    var boundary = stat()
    try #require(lstat(preparation.directory.displayString, &boundary) == 0)
    try Data("one\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
    var times = [
        timespec(tv_sec: 0, tv_nsec: Int(UTIME_OMIT)),
        timespec(tv_sec: boundary.st_mtimespec.tv_sec + 60, tv_nsec: boundary.st_mtimespec.tv_nsec),
    ]
    try #require(utimensat(AT_FDCWD, path.displayString, &times, AT_SYMLINK_NOFOLLOW) == 0)
    let journal = try store.read(StateKey.resource(path, .journal))
    #expect(journal != nil)

    let result = try transactionRecover(target)
    #expect(result.status == .incomplete)
    #expect(result.error?.code == .recoveryNeeded)
    #expect(result.error?.message == "destination revision changed after preparation")
    #expect(result.error?.path == path)
    #expect(FileManager.default.fileExists(atPath: preparation.directory.displayString))
    #expect(try store.read(StateKey.resource(path, .journal)) == journal)
    #expect(try storedPreparation(path, store: store) == preparation)
}
