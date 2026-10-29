import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func nativeSharedTransactionFixturesDriveGuardedRecovery() throws {
    let fixture = try stateFixture()
    let journals = try stateArray(stateField(fixture, "journals"))
    let cases = try stateArray(stateField(fixture, "transactions"))
    #expect(cases.count == 9)
    for item in cases {
        let root = try NativeTestRoot()
        defer { try? root.cleanup() }
        let name = try stateText(stateField(item, "name"))
        let journalName = try stateText(stateField(item, "journal"))
        let template = try #require(journals.first { try stateText(stateField($0, "name")) == journalName })
        let source = try Journal.fromState(stateField(template, "record"))
        let path = try root.child("AGENTS.md")
        try NativeTransactionIO.createFile(path, data: #require(source.before.data))
        #expect(try chmod(path.displayString, mode_t(#require(source.before.nodes.first?.mode.uint32))) == 0)
        let ref = try NativeAuthority.reference(path, kind: .document)
        let lock = try NativeAdmission.reserve(ref, timeout: .zero)
        try NativeAdmission.admit(ref, lock: lock)
        let previous = try Receipt(resource: ref, transactionID: String(repeating: "0", count: 32))
        try NativeTransactionIO.atomic(appendPath(ref.stateRoot, "receipt.json"), value: previous, phase: "fixture")
        let snapshot = try NativeAdmission.observe(ref, lock: lock)
        let next = try Receipt(resource: ref, transactionID: String(repeating: "1", count: 32))
        let after = try journalName == "adopt-section" ? snapshot.revision : Revision(nodes: [
            NativeResourcePlanner.node("", data: source.staged.data,
                                       mode: UInt64(#require(source.staged.nodes.first?.mode.uint32))),
        ])
        let plan = try ResourcePlan(
            resource: ref, before: snapshot.revision, after: after, previous: previous, receipt: next,
            ancestors: NativeObservation.ancestors(appendPath(ref.stateRoot, "transaction.json")),
            stateRevision: snapshot.state.revision, recoveryPaths: [],
        )
        let journal = try NativeTransaction.prepare(plan, lock: lock)
        let observed = try stateField(item, "observed")
        if try stateField(observed, "backup") != .null {
            try NativeTransactionIO.move(
                path,
                to: appendPath(ref.stateRoot, "backup/resource"),
                revision: journal.before,
            )
        }
        if try stateField(observed, "staged") == .null, journalName != "adopt-section" {
            try NativeTransactionIO.move(
                appendPath(ref.stateRoot, "staging/resource"),
                to: path,
                revision: journal.staged,
            )
        }
        if name == "foreign-edit-before-commit" || name == "committed-foreign-edit" {
            let revision = try Revision.fromState(stateField(observed, "resource"))
            try #require(revision.data).write(to: URL(fileURLWithPath: path.displayString))
        } else if name == "permissions-changed-before-commit" {
            let revision = try Revision.fromState(stateField(observed, "resource"))
            #expect(try chmod(path.displayString, mode_t(#require(revision.nodes.first?.mode.uint32))) == 0)
        } else if name == "identity-changed-before-commit" {
            try NativeTransactionIO.move(path, to: root.child("replacement-evidence"), revision: journal.staged)
            try NativeTransactionIO.createFile(path, data: #require(journal.staged.data))
        }
        if try stateText(stateField(observed, "receipt")) == "new" {
            try NativeTransactionIO.atomic(appendPath(ref.stateRoot, "receipt.json"), value: next, phase: "fixture")
        }
        let expected = try stateField(item, "expected")
        let incomplete = try stateText(stateField(expected, "status")) == "INCOMPLETE"
        if incomplete {
            let before = try NativeObservation.snapshot(ref.stateRoot)
            expectNativeError(.recoveryNeeded) { try NativeTransaction.recover(ref, lock: lock) }
            #expect(try NativeObservation.snapshot(ref.stateRoot) == before)
        } else {
            try NativeTransaction.recover(ref, lock: lock)
            try NativeTransaction.recover(ref, lock: lock)
        }
        let revision = try Revision.fromState(stateField(expected, "resource"))
        #expect(try NativeObservation.snapshot(path).revision.data == revision.data)
        let receipt = try stateText(stateField(expected, "receipt"))
        #expect(try NativeAdmission.observe(ref).receipt == (receipt == "old" ? previous : next))
        #expect(try (NativeFilesystem.metadata(appendPath(ref.stateRoot, "transaction.json")) != nil) ==
            (stateField(expected, "retain_journal") == .boolean(true)))
        #expect(try (NativeFilesystem.metadata(appendPath(ref.stateRoot, "backup/resource")) != nil) ==
            (stateField(expected, "retain_backup") == .boolean(true)))
    }
}

@Test(arguments: ["rollback-stage", "rollback-backup", "rollback", "cleanup-node", "cleanup"])
func nativeRecoveryItselfCanBeInterruptedRepeatedly(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("tree")
    let original = try NativeResourcePlanner.desired(.tree(TreeContent(entries: [
        BundleEntry(path: "a", data: Data("a".utf8)), BundleEntry(path: "b", data: Data("b".utf8)),
    ])))
    _ = try NativeTransactionIO.writeRevision(path, revision: original)
    let ref = try NativeAuthority.reference(path, kind: .tree)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    let observed = try NativeAdmission.observe(ref, lock: lock)
    let after = try NativeResourcePlanner.desired(.tree(TreeContent(entries: [
        BundleEntry(path: "c", data: Data("c".utf8)), BundleEntry(path: "d", data: Data("d".utf8)),
    ])))
    let plan = try ResourcePlan(
        resource: ref, before: observed.revision, after: after, previous: nil,
        receipt: Receipt(resource: ref, transactionID: String(repeating: "a", count: 32)),
        ancestors: NativeObservation.ancestors(appendPath(ref.stateRoot, "transaction.json")),
        stateRevision: observed.state.revision, recoveryPaths: [],
    )
    let journal = try NativeTransaction.prepare(plan, lock: lock)
    try NativeTransactionIO.move(path, to: appendPath(ref.stateRoot, "backup/resource"), revision: journal.before)
    try NativeTransactionIO.move(appendPath(ref.stateRoot, "staging/resource"), to: path, revision: journal.staged)
    var reached = 0
    let seam = NativeTransactionSeam {
        if $0 == boundary {
            reached += 1
            throw TargetError(code: .ioError, message: "interrupted recovery")
        }
    }
    for _ in 0 ..< 3 {
        do {
            try NativeTransaction.recover(ref, lock: lock, seam: seam)
        } catch let error as TargetError {
            #expect(error.code == .ioError)
        }
    }
    #expect(reached > 0)
    try NativeTransaction.recover(ref, lock: lock)
    try NativeTransaction.recover(ref, lock: lock)
    #expect(try NativeObservation.snapshot(path).revision == observed.revision)
    #expect(try NativeAdmission.observe(ref).recovery.isEmpty)
}

@Test
func nativeRecoveryRefusesMatchingContentWithLaterModificationTime() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    let journal = try NativeTransaction.prepare(transactionPlan(ref, lock: lock), lock: lock)
    try NativeTransactionIO.move(
        appendPath(ref.stateRoot, "staging/resource"),
        to: ref.destination,
        revision: journal.staged,
    )
    let metadata = try #require(try NativeFilesystem.metadata(appendPath(ref.stateRoot, "transaction.json")))
    var times = [metadata.st_atimespec, timespec(tv_sec: metadata.st_mtimespec.tv_sec + 60, tv_nsec: 0)]
    #expect(utimensat(AT_FDCWD, ref.destination.displayString, &times, AT_SYMLINK_NOFOLLOW) == 0)
    expectNativeError(.recoveryNeeded) { try NativeTransaction.recover(ref, lock: lock) }
    #expect(try NativeObservation.snapshot(ref.destination).revision == journal.published)
    #expect(try NativeAdmission.observe(ref).receipt == nil)
}
