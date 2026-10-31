import Darwin
@testable import Flyrail
import Foundation
import Testing

private func fixtureBytes(_ item: JSONValue, _ phase: String, _ location: String) throws -> Data {
    try #require(Revision.fromState(stateField(stateField(item, phase), location)).data)
}

private func fixtureMode(_ item: JSONValue, _ phase: String, _ location: String) throws -> mode_t {
    let revision = try Revision.fromState(stateField(stateField(item, phase), location))
    return try mode_t(#require(revision.nodes.first?.mode.uint32))
}

private func exists(_ path: ResourcePath) -> Bool {
    var metadata = stat()
    return lstat(path.displayString, &metadata) == 0
}

private func stageFixture(
    _ name: String, item: JSONValue, journal: Journal, store: any StateStore, root: NativeTestRoot,
) throws {
    let path = journal.resource.destination
    let directory = try ResourceTransaction.transactionDirectory(journal, store: store)
    let (staged, backup) = try ResourceTransaction.paths(directory)
    switch name {
    case "prepared", "ownership-only-adoption":
        return
    case "backup-moved":
        try NativeTransactionIO.move(path, to: backup, revision: journal.backup)
    case "receipt-committed", "committed-foreign-edit":
        try ResourceTransaction.publish(journal, store: store)
        if name == "committed-foreign-edit" {
            try fixtureBytes(item, "observed", "resource").write(to: URL(fileURLWithPath: path.displayString))
        }
    default:
        try NativeTransactionIO.move(path, to: backup, revision: journal.backup)
        try NativeTransactionIO.move(staged, to: path, revision: journal.staged)
        switch name {
        case "foreign-edit-before-commit":
            try fixtureBytes(item, "observed", "resource").write(to: URL(fileURLWithPath: path.displayString))
        case "permissions-changed-before-commit":
            try #require(chmod(path.displayString, fixtureMode(item, "observed", "resource")) == 0)
        case "identity-changed-before-commit":
            let replacement = try root.child("AGENTS.md.replacement")
            try NativeTransactionIO.createFile(replacement, data: fixtureBytes(item, "observed", "resource"))
            try #require(rename(replacement.displayString, path.displayString) == 0)
        default:
            break
        }
    }
}

@Test
func nativeSharedTransactionFixturesDriveGuardedRecovery() throws {
    let fixture = try stateFixture()
    let cases = try stateArray(stateField(fixture, "transactions"))
    #expect(try cases.map { try stateText(stateField($0, "name")) } == [
        "prepared", "backup-moved", "published-before-receipt", "ownership-only-adoption", "receipt-committed",
        "foreign-edit-before-commit", "permissions-changed-before-commit", "identity-changed-before-commit",
        "committed-foreign-edit",
    ])
    let prepared = try #require(cases.first)
    for item in cases {
        let root = try NativeTestRoot()
        defer { try? root.cleanup() }
        let store = root.store
        let name = try stateText(stateField(item, "name"))
        let adoption = try stateText(stateField(item, "journal")) == "adopt-section"
        let path = try root.child("AGENTS.md")
        try NativeTransactionIO.createFile(path, data: fixtureBytes(adoption ? item : prepared, "observed", "resource"))
        try #require(chmod(path.displayString, fixtureMode(prepared, "observed", "resource")) == 0)
        let ref = try NativeAuthority.reference(path, kind: .document)
        _ = try NativeTransactionDirectory.createStoreParents(store)
        try ResourceAdmission.admit(ref, store: store)
        let previous = try Receipt(resource: ref, transactionID: String(repeating: "0", count: 32))
        try StateRecords.write(store, key: StateKey.resource(path, .receipt), value: previous, expected: nil)
        let before = try NativeObservation.snapshot(path).revision
        let after = try adoption ? before : Revision(nodes: [
            NativeResourcePlanner.node(
                "", data: fixtureBytes(prepared, "observed", "staged"),
                mode: UInt64(fixtureMode(prepared, "observed", "staged")),
            ),
        ])
        let plan = try storePlan(ref, store: store, after: after, previous: previous)
        let journal = try ResourceTransaction.prepare(plan, store: store)
        try stageFixture(name, item: item, journal: journal, store: store, root: root)

        let expected = try stateField(item, "expected")
        if try stateText(stateField(expected, "status")) == "INCOMPLETE" {
            let before = try NativeObservation.snapshot(plan.transactionDirectory)
            expectNativeError(.recoveryNeeded) { try ResourceTransaction.recover(ref, store: store) }
            #expect(try NativeObservation.snapshot(plan.transactionDirectory) == before, "\(name)")
        } else {
            try ResourceTransaction.recover(ref, store: store)
            try ResourceTransaction.recover(ref, store: store)
            #expect(try transactionRecordKeys(store) == [], "\(name)")
            #expect(!exists(plan.transactionDirectory), "\(name)")
        }
        #expect(try FileManager.default.contents(atPath: path.displayString) ==
            fixtureBytes(item, "expected", "resource"), "\(name)")
        let receipt = try stateText(stateField(expected, "receipt"))
        #expect(try storedReceipt(path, store: store) == (receipt == "old" ? previous : plan.receipt), "\(name)")
        #expect(try (store.read(StateKey.resource(path, .journal)) != nil) ==
            (stateField(expected, "retain_journal") == .boolean(true)), "\(name)")
        #expect(try exists(appendPath(plan.transactionDirectory, "backup/resource")) ==
            (stateField(expected, "retain_backup") == .boolean(true)), "\(name)")
    }
}

@Test
func nativeFixturePreparationsDecodeAtTheirTransactionPath() throws {
    let preparations = try stateArray(stateField(stateFixture(), "preparations"))
    #expect(preparations.count == 1)
    for item in preparations {
        let preparation = try Preparation.fromState(stateField(item, "record"))
        #expect(try preparation.directory == transactionPath(
            destination: preparation.resource.destination, transactionID: preparation.transactionID,
        ))
    }
}

@Test(arguments: ["rollback-stage", "rollback-backup", "rollback", "cleanup-node", "cleanup"])
func nativeRecoveryItselfCanBeInterruptedRepeatedly(_ boundary: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let path = try root.child("tree")
    let original = try NativeResourcePlanner.desired(.tree(TreeContent(entries: [
        BundleEntry(path: "a", data: Data("a".utf8)), BundleEntry(path: "b", data: Data("b".utf8)),
    ])))
    _ = try NativeTransactionIO.writeRevision(path, revision: original)
    let ref = try NativeAuthority.reference(path, kind: .tree)
    try ResourceAdmission.admit(ref, store: store)
    let before = try NativeObservation.snapshot(path).revision
    let after = try NativeResourcePlanner.desired(.tree(TreeContent(entries: [
        BundleEntry(path: "c", data: Data("c".utf8)), BundleEntry(path: "d", data: Data("d".utf8)),
    ])))
    let plan = try storePlan(ref, store: store, after: after, previous: nil)
    let (crash, crashed) = crashSeam(at: "publish")
    #expect(throws: NativeSimulatedCrash("publish")) {
        try ResourceTransaction.publish(ResourceTransaction.prepare(plan, store: store), store: store, seam: crash)
    }
    try #require(crashed())

    var reached = 0
    let seam = NativeTransactionSeam {
        if $0 == boundary {
            reached += 1
            throw TargetError(code: .ioError, message: "interrupted recovery")
        }
    }
    for _ in 0 ..< 3 {
        do {
            try ResourceTransaction.recover(ref, store: store, seam: seam)
        } catch let error as TargetError {
            #expect(error == TargetError(code: .ioError, message: "interrupted recovery"))
        }
    }
    try #require(reached > 0)
    try ResourceTransaction.recover(ref, store: store)
    try ResourceTransaction.recover(ref, store: store)
    #expect(try NativeObservation.snapshot(path).revision == before)
    #expect(try ResourceAdmission.observe(ref, store: store).recoveryPaths.isEmpty)
    #expect(try transactionRecordKeys(store) == [])
    #expect(!exists(plan.transactionDirectory))
}

@Test
func nativeRecoveryRefusesALaterBirthTime() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = root.store
    let path = try root.child("document")
    try NativeTransactionIO.createFile(path, data: Data("before\n".utf8))
    let ref = try NativeAuthority.reference(path, kind: .document)
    let plan = try storePlan(ref, store: store, after: Revision(nodes: [
        NativeResourcePlanner.node("", data: Data("after\n".utf8), mode: 0o644),
    ]), previous: nil)
    let (seam, fired) = crashSeam(at: "publish")
    #expect(throws: NativeSimulatedCrash("publish")) {
        try ResourceTransaction.publish(ResourceTransaction.prepare(plan, store: store), store: store, seam: seam)
    }
    try #require(fired())

    var boundary = stat()
    try #require(lstat(plan.transactionDirectory.displayString, &boundary) == 0)
    var born = timespec(tv_sec: boundary.st_mtimespec.tv_sec + 60, tv_nsec: boundary.st_mtimespec.tv_nsec)
    var request = attrlist(
        bitmapcount: u_short(ATTR_BIT_MAP_COUNT), reserved: 0, commonattr: attrgroup_t(ATTR_CMN_CRTIME),
        volattr: 0, dirattr: 0, fileattr: 0, forkattr: 0,
    )
    try #require(setattrlist(
        path.displayString, &request, &born, MemoryLayout<timespec>.size, UInt32(FSOPT_NOFOLLOW),
    ) == 0)
    var changed = stat()
    try #require(lstat(path.displayString, &changed) == 0)
    try #require(changed.st_birthtimespec.tv_sec == born.tv_sec)
    try #require(!ResourceTransaction.changedAfter(
        boundary: boundary.st_mtimespec, modified: changed.st_mtimespec, born: nil, birthTime: false,
    ))

    #expect(throws: TargetError(
        code: .recoveryNeeded, message: "destination revision changed after preparation", path: path,
    )) { try ResourceTransaction.recover(ref, store: store) }
    #expect(try ResourceTransaction.pendingRecovery(ref, store: store) == [plan.transactionDirectory])
    #expect(FileManager.default.contents(atPath: path.displayString) == Data("after\n".utf8))
}
