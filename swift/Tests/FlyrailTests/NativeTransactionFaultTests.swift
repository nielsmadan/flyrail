import Darwin
@testable import Flyrail
import Foundation
import Testing

let transactionFaultBoundaries = [
    "index-pending-prepared", "index-pending-reread", "index-pending-committed",
    "resource-precondition", "index-require", "index-ancestor", "guard-lookup",
    "journal-prepared", "journal-reread", "journal-committed", "prepare",
    "rename", "backup", "staging", "publish", "receipt", "receipt-prepared",
    "receipt-reread", "receipt-committed", "cleanup-node", "cleanup",
    "index-completion", "index-completion-prepared", "index-completion-reread", "index-completion-committed",
]

@Test(arguments: transactionFaultBoundaries)
func nativeEveryTransactionFaultKeepsPermanentCommitsAndRecognizedRecovery(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    try NativeTransactionIO.createFile(path, data: Data("baseline\n".utf8))
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path, "first\n"),
                                 target: target, acquisition: .takeover).status == .applied)
    var reached = false
    let seam = NativeTransactionSeam {
        if $0 == boundary {
            reached = true
            throw TargetError(code: .ioError, message: "injected \(boundary)", errno: EIO)
        }
    }
    var result: InstallationResult?
    do {
        result = try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second\n"),
                                      target: target, seam: seam)
    } catch let error as TargetError { #expect(error.code == .ioError) }
    #expect(reached)
    let committed = ["receipt-reread", "receipt-committed", "cleanup-node", "cleanup",
                     "index-completion", "index-completion-prepared", "index-completion-reread",
                     "index-completion-committed"].contains(boundary)
    #expect(try NativeObservation.snapshot(path).revision.data == Data((committed ? "second\n" : "first\n").utf8))
    if let result, committed {
        #expect(result.status == .partial)
        #expect(result.resources.first?.status == .applied)
    }
    _ = try NativeTransactionDriver.recover("team", target: target, timeout: .zero)
    _ = try NativeTransactionDriver.recover("team", target: target, timeout: .zero)
    let ref = try NativeAuthority.reference(path, kind: .document)
    #expect(try NativeAdmission.observe(ref).receipt?.claims.first?.version == (committed ? "2" : "1"))
    #expect(try NativeAdmission.observe(ref).recovery.isEmpty)
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("baseline\n".utf8))
}

@Test
func nativeReceiptRereadFailureCannotUndoTheCommitBoundary() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let seam = NativeTransactionSeam {
        if $0 == "receipt-reread" {
            try Data("later edit\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
            throw TargetError(code: .ioError, message: "reread failed")
        }
    }
    let result = try transactionWrite(
        transactionBundle(),
        rendered: transactionRendered(path),
        target: target,
        seam: seam
    )
    #expect(result.status == .partial)
    #expect(result.resources.first?.status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("later edit\n".utf8))
    _ = try NativeTransactionDriver.recover("team", target: target, timeout: .zero)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("later edit\n".utf8))
    #expect(try transactionRemove("team", target: target).error?.code == .modified)
}

@Test(arguments: ["journal-prepared", "prepare", "backup", "staging", "publish", "receipt", "receipt-prepared"])
func nativeForeignChangesAtEachPublicationSeamAreRetained(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    try NativeTransactionIO.createFile(path, data: Data("baseline".utf8))
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path, "first"),
                                 target: target, acquisition: .takeover).status == .applied)
    let ref = try NativeAuthority.reference(path, kind: .document)
    var changed: ResourcePath?
    let seam = NativeTransactionSeam {
        guard $0 == boundary, changed == nil else { return }
        let location: ResourcePath = switch boundary {
        case "backup": try appendPath(ref.stateRoot, "backup/resource")
        case "staging": try appendPath(ref.stateRoot, "staging/resource")
        default: path
        }
        changed = location
        try Data("unexpected foreign credential".utf8).write(to: URL(fileURLWithPath: location.displayString))
    }
    let result = try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second"),
                                      target: target, seam: seam)
    #expect(changed != nil)
    #expect(result.status == .incomplete)
    #expect(try NativeObservation.snapshot(#require(changed)).revision
        .data == Data("unexpected foreign credential".utf8))
    #expect(try NativeAdmission.observe(ref).receipt?.claims.first?.version == "1")
    #expect(try NativeAdmission.observe(ref).recovery.isEmpty == false)
}

@Test
func nativePartialResourceCommitRetainsAllIndexReferences() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let rendered = try RenderedBundle(artifacts: ["one", "two"].map {
        try RenderedArtifact(id: $0, family: .instructions, destination: root.child($0),
                             content: .file(FileContent(data: Data($0.utf8))))
    })
    var receipts = 0
    let seam = NativeTransactionSeam {
        if $0 == "receipt" {
            receipts += 1
            if receipts == 2 { throw TargetError(code: .ioError, message: "second receipt failed") }
        }
    }
    let result = try transactionWrite(transactionBundle(), rendered: rendered, target: target, seam: seam)
    #expect(result.status == .partial)
    #expect(result.resources.map(\.status) == [.applied, .failed])
    let observed = try NativeAdmission.observeIndex(target, bundleID: "team")
    #expect(observed.current?.resources.count == 2)
    #expect(observed.current?.pending?.membership.count == 2)
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(root.child("one")).revision.nodes.isEmpty)
    #expect(try NativeObservation.snapshot(root.child("two")).revision.nodes.isEmpty)
}
