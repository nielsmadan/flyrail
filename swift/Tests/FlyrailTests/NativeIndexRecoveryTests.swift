import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func nativeTransactionLocksAreIndexFirstThenSortedPermanentAuthorities() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let references = try ["z", "a", "m"].map { try NativeAuthority.reference(root.child($0), kind: .document) }
    do {
        let index = try NativeAdmission.reserveIndex(target, timeout: .zero)
        let clock = ContinuousClock()
        let start = clock.now
        expectNativeError(.busy) {
            _ = try NativeTransactionLocks(
                target: target,
                bundleID: "team",
                resources: references,
                timeout: .milliseconds(25),
            )
        }
        #expect(start.duration(to: clock.now) >= .milliseconds(25))
        #expect(start.duration(to: clock.now) < .seconds(2))
        for resource in references {
            #expect(try NativeFilesystem.metadata(resource.stateRoot) == nil)
        }
        try index.validate()
    }
    let locks = try NativeTransactionLocks(target: target, bundleID: "team", resources: references, timeout: .zero)
    #expect(locks.guards.map(\.stateRoot) == references.map(\.stateRoot).sorted {
        $0.platformBytes.lexicographicallyPrecedes($1.platformBytes)
    })
    for resource in references {
        expectNativeError(.busy) { _ = try NativeAdmission.reserve(resource, timeout: .zero) }
    }
    try locks.validate()
}

@Test
func nativeUnrecognizedIndexPreparationNeverReservesItsUnknownReferences() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path), target: target)
        .status == .applied)
    let current = try #require(try NativeAdmission.observeIndex(target, bundleID: "team").current)
    let other = try NativeAuthority.reference(root.child("other"), kind: .document)
    let generation = try Generation(version: "unexpected", bundleDigest: String(repeating: "a", count: 64),
                                    renderDigest: String(repeating: "b", count: 64),
                                    membership: [GenerationMember(resource: other, claimIDs: [])])
    let unknown = try Index(bundleID: "team", contextDigest: target.contextDigest, current: generation)
    try NativeTransactionIO.createFile(
        appendPath(target.indexRoot, "index.json.next"),
        data: StateCodec.encode(unknown),
    )
    let before = try NativeObservation.snapshot(root.path)
    expectNativeError(.recoveryNeeded) {
        _ = try NativeTransactionDriver.recover("team", target: target, timeout: .zero)
    }
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(try NativeAdmission.observeIndex(target, bundleID: "team").current == current)
    #expect(try NativeFilesystem.metadata(other.stateRoot) == nil)
}

@Test(arguments: ["current", "previous", "prepared-completion"])
func nativeMissingCommittedReceiptPreservesAllIndexAndRecoveryEvidence(_ membership: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    #expect(try transactionWrite(transactionBundle(), rendered: transactionRendered(path), target: target)
        .status == .applied)
    let current = try #require(try NativeAdmission.observeIndex(target, bundleID: "team").current)
    if membership != "current" {
        let pending = try Index(
            bundleID: "team", contextDigest: target.contextDigest,
            pending: membership == "prepared-completion" ? current.current : nil,
            previous: membership == "previous" ? current.current : nil,
            residual: current.resources,
        )
        try NativeTransactionIO.atomic(appendPath(target.indexRoot, "index.json"), value: pending, phase: "fixture")
        if membership == "prepared-completion" {
            try NativeTransactionIO.createFile(
                appendPath(target.indexRoot, "index.json.next"),
                data: StateCodec.encode(current),
            )
        }
    }
    let ref = try NativeAuthority.reference(path, kind: .document)
    let receiptPath = try appendPath(ref.stateRoot, "receipt.json")
    let receipt = try NativeObservation.snapshot(receiptPath).revision
    try NativeTransactionIO.unlink(receiptPath, revision: receipt)
    let before = try NativeObservation.snapshot(root.path)
    expectNativeError(.invalidState) { _ = try NativeTransactionDriver.recover("team", target: target, timeout: .zero) }
    expectNativeError(.invalidState) { _ = try transactionRemove("team", target: target) }
    #expect(try NativeObservation.snapshot(root.path) == before)
    try NativeTransactionIO.createFile(receiptPath, data: #require(receipt.data))
    #expect(try transactionRemove("team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.nodes.isEmpty)
}

@Test
func nativeUnknownReceiptPreparationCannotDestroyTheRecognizedBackup() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    try NativeTransactionIO.createFile(ref.destination, data: Data("baseline".utf8))
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    let journal = try NativeTransaction.prepare(transactionPlan(ref, lock: lock, acquisition: .takeover), lock: lock)
    try NativeTransaction.publish(journal, lock: lock)
    let unknown = try Receipt(resource: ref, transactionID: String(repeating: "f", count: 32))
    try NativeTransactionIO.createFile(appendPath(ref.stateRoot, "receipt.json.next"), data: StateCodec.encode(unknown))
    let before = try NativeObservation.snapshot(root.path)
    expectNativeError(.recoveryNeeded) { try NativeTransaction.recover(ref, lock: lock) }
    #expect(try NativeObservation.snapshot(root.path) == before)
}

@Test(arguments: ["recover-journal", "cleanup-metadata"])
func nativeRecoveryOfCompleteMetadataPreparationsSurvivesAnotherInterruption(_ phase: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    let interrupted = NativeTransactionSeam {
        if $0 == (phase == "recover-journal" ? "journal-prepared" : "receipt-prepared") {
            throw TargetError(code: .ioError, message: "initial interruption")
        }
    }
    do {
        let journal = try NativeTransaction.prepare(transactionPlan(ref, lock: lock), lock: lock, seam: interrupted)
        try NativeTransaction.publish(journal, lock: lock, seam: interrupted)
        Issue.record("initial seam was not reached")
    } catch let error as TargetError { #expect(error.code == .ioError) }
    var reached = false
    let recovery = NativeTransactionSeam {
        if $0 == phase {
            reached = true
            throw TargetError(code: .ioError, message: "recovery interruption")
        }
    }
    do { try NativeTransaction.recover(ref, lock: lock, seam: recovery) } catch let error as TargetError {
        #expect(error.code == .ioError)
    }
    #expect(reached)
    try NativeTransaction.recover(ref, lock: lock)
    try NativeTransaction.recover(ref, lock: lock)
    #expect(try NativeAdmission.observe(ref).receipt == nil)
    #expect(try NativeAdmission.observe(ref).revision.nodes.isEmpty)
    #expect(try NativeAdmission.observe(ref).recovery.isEmpty)
}
