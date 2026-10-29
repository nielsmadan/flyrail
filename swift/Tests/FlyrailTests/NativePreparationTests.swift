import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test(arguments: [
    "stage-node",
    "index-pending-created",
    "journal-created",
    "receipt-created",
    "index-completion-created",
])
func nativeIncompletePreparationsRemainEvidenceWithoutInventingRecovery(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    try NativeTransactionIO.createFile(path, data: Data("baseline".utf8))
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path, "first"),
                                 target: target, acquisition: .takeover).status == .applied)
    var reached = false
    let seam = NativeTransactionSeam {
        if $0 == boundary {
            reached = true
            throw TargetError(code: .ioError, message: "interrupted creation")
        }
    }
    do {
        let result = try transactionWrite(transactionBundle("2"), rendered: transactionRendered(path, "second"),
                                          target: target, seam: seam)
        #expect(result.status == (boundary == "index-completion-created" ? .partial : .incomplete))
    } catch let error as TargetError { #expect(error.code == .ioError) }
    #expect(reached)
    let before = try NativeObservation.snapshot(root.path)
    do {
        _ = try NativeTransactionDriver.recover("team", target: target, timeout: .zero)
        Issue.record("incomplete preparation was discarded")
    } catch let error as TargetError {
        #expect(error.code == (boundary == "stage-node" ? .recoveryNeeded : .invalidState))
    }
    #expect(try NativeObservation.snapshot(root.path) == before)
    let ref = try NativeAuthority.reference(path, kind: .document)
    let observed = try NativeAdmission.observe(ref)
    #expect(observed.receipt?.claims.first?.version == (boundary == "index-completion-created" ? "2" : "1"))
    #expect(observed.revision.data == Data(
        (boundary == "receipt-created" || boundary == "index-completion-created" ? "second" : "first").utf8,
    ))
}

@Test
func nativeRecognizedIndexPreparationRecoveryCanBeInterruptedAfterPromotion() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let seam = NativeTransactionSeam {
        if $0 == "index-pending-prepared" {
            throw TargetError(code: .ioError, message: "pending interruption")
        }
    }
    do {
        _ = try transactionWrite(transactionBundle(), rendered: transactionRendered(path), target: target, seam: seam)
        Issue.record("initial seam was not reached")
    } catch let error as TargetError { #expect(error.code == .ioError) }
    var reached = false
    let retry = NativeTransactionSeam {
        if $0 == "recover-index" {
            reached = true
            throw TargetError(code: .ioError, message: "index recovery interruption")
        }
    }
    do {
        _ = try NativeTransactionDriver.recover("team", target: target, timeout: .zero, seam: retry)
        Issue.record("recovery seam was not reached")
    } catch let error as TargetError { #expect(error.code == .ioError) }
    #expect(reached)
    let recovered = try NativeTransactionDriver.recover("team", target: target, timeout: .zero)
    #expect(recovered.status == .unchanged)
    #expect(recovered.observation.pending)
    #expect(recovered.observation.resources.map(\.destination) == [path])
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.nodes.isEmpty)
}

@Test
func nativeNonUTF8TransactionRoutesAreRejectedWithoutCreatingState() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let bytes = try ResourcePath(platformBytes: root.path.platformBytes + [47, 255])
    let target = try InstallationTarget(indexRoot: root.child("index"))
    do {
        _ = try transactionWrite(transactionBundle(), rendered: transactionRendered(bytes), target: target)
        Issue.record("nonportable route was accepted")
    } catch let error as TargetError {
        #expect(error.code == .unsafePath)
        #expect(error.path?.platformBytes == bytes.platformBytes)
    } catch is FlyrailValueError {
        #expect(try NativeFilesystem.metadata(target.indexRoot) == nil)
    }
    #expect(try NativeObservation.snapshot(root.path).revision.nodes.count == 1)
}

@Test
func nativePartiallyStagedTreeRetainsItsUnjournaledPrefix() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("tree")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let content = try TreeContent(entries: [
        BundleEntry(path: "a/file", data: Data("first".utf8)),
        BundleEntry(path: "b/file", data: Data("second".utf8)),
    ])
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .skills, destination: path, content: .tree(content)),
    ])
    var staged = 0
    let seam = NativeTransactionSeam {
        if $0 == "stage-node" {
            staged += 1
            if staged == 2 {
                throw TargetError(code: .ioError, message: "partial tree interruption")
            }
        }
    }
    #expect(try transactionWrite(transactionBundle(), rendered: rendered, target: target,
                                 seam: seam).status == .incomplete)
    #expect(staged == 2)
    let ref = try NativeAuthority.reference(path, kind: .tree)
    let preparation = try appendPath(ref.stateRoot, "staging/resource")
    #expect(try NativeObservation.snapshot(preparation).revision.nodes.map(\.path) == ["", "a"])
    #expect(try NativeAdmission.observeIndex(target, bundleID: "team").current?.resources == [ref])
    let before = try NativeObservation.snapshot(root.path)
    do {
        _ = try NativeTransactionDriver.recover("team", target: target, timeout: .zero)
        Issue.record("unjournaled partial tree was discarded")
    } catch let error as TargetError { #expect(error.code == .recoveryNeeded) }
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(try NativeObservation.snapshot(path).revision.nodes.isEmpty)
}
