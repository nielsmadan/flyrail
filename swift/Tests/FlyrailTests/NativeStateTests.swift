import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func nativeFixtureRevisionsBindRealPhysicalMetadata() throws {
    let fixture = try stateRelocatedFixture(stateFixture())
    for item in try stateArray(stateField(fixture, "journals")) {
        let journal = try Journal.fromState(stateField(item, "record"))
        for revision in [journal.before, journal.staged] {
            let root = try NativeTestRoot()
            defer { try? root.cleanup() }
            let destination = root.url.appending(path: "resource")
            for node in revision.nodes {
                let path = node.path.isEmpty ? destination : destination.appending(path: node.path)
                if let data = node.data {
                    try data.write(to: path)
                } else {
                    try FileManager.default.createDirectory(at: path, withIntermediateDirectories: false)
                }
                #expect(try chmod(path.path, mode_t(#require(node.mode.uint64))) == 0)
            }
            let snapshot = try NativeObservation.snapshot(ResourcePath(destination.path))
            #expect(snapshot.revision.nodes.map(\.path) == revision.nodes.map(\.path))
            #expect(snapshot.revision.nodes.map(\.data) == revision.nodes.map(\.data))
            #expect(snapshot.revision.nodes.map(\.mode) == revision.nodes.map(\.mode))
            #expect(try snapshot.revision.contentDigest == revision.contentDigest)
            #expect(snapshot.revision.nodes.allSatisfy { $0.uid == Count(UInt64(geteuid())) && $0.inode > 0 })
            #expect(try StateCodec.decode(StateCodec.encode(snapshot.revision), as: Revision.self) == snapshot.revision)
        }
    }
}

@Test
func nativeFixtureReceiptsAreReadThroughTheirAuthority() throws {
    let fixture = try stateRelocatedFixture(stateFixture())
    for item in try stateArray(stateField(fixture, "receipts")) {
        let original = try Receipt.fromState(stateField(item, "record"))
        let root = try NativeTestRoot()
        defer { try? root.cleanup() }
        let resource = try NativeAuthority.reference(root.child("resource"), kind: original.resource.kind)
        let claims = try original.claims.map { owned in
            try OwnedClaim(
                claim: owned.claim, claimID: owned.claim.identity(destination: resource.destination),
                bundleID: owned.bundleID, artifactID: owned.artifactID, version: owned.version,
                bundleDigest: owned.bundleDigest, renderDigest: owned.renderDigest,
                selection: owned.selection, installed: owned.installed, disposition: owned.disposition,
                baseline: owned.baseline, requirements: owned.requirements,
                stableRequirements: owned.stableRequirements,
                schemaRequirements: owned.schemaRequirements,
            )
        }
        let receipt = try Receipt(
            resource: resource, transactionID: original.transactionID, claims: claims,
            provenance: original.provenance, createdDirectories: original.createdDirectories,
        )
        let key = try StateKey.resource(resource.destination, .receipt)
        try StateRecords.write(root.store, key: key, value: receipt, expected: nil)
        #expect(try ResourceAdmission.storedReceipt(resource, store: root.store) == receipt)
        let observation = try ResourceAdmission.observe(resource, store: root.store)
        #expect(observation.receipt == receipt)
        #expect(observation.recoveryPaths.isEmpty)
        #expect(observation.revision.nodes.isEmpty)
        let other = try ResourceRef(
            destination: resource.destination,
            kind: original.resource.kind == .tree ? .document : .tree,
        )
        #expect(throws: TargetError(code: .invalidState, message: "receipt resource disagrees",
                                    path: resource.destination))
        {
            _ = try ResourceAdmission.storedReceipt(other, store: root.store)
        }
    }
}

@Test
func aSymlinkAtTheFallbackTransactionPathIsRefusedWithoutWritingThrough() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let store = MemoryStore()
    let plan = try storePlan(ref, store: store, after: Revision(nodes: [
        NativeResourcePlanner.node("", data: Data("after".utf8), mode: 0o644),
    ]), previous: nil)
    let elsewhere = try root.child("elsewhere")
    #expect(mkdir(elsewhere.displayString, 0o700) == 0)
    #expect(symlink(elsewhere.displayString, plan.transactionDirectory.displayString) == 0)
    do {
        _ = try ResourceTransaction.prepare(plan, store: store)
        Issue.record("prepare accepted a symlinked transaction directory")
    } catch let error as TargetError {
        #expect(error.code == .unsafePath)
        #expect(error.message == "resource ancestor is not an ordinary directory")
    }
    #expect(try FileManager.default.contentsOfDirectory(atPath: elsewhere.displayString) == [])
}

@Test
func aSymlinkedStoreStagingDirectoryIsRefusedWithoutWritingThrough() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let elsewhere = try root.child("elsewhere")
    #expect(mkdir(elsewhere.displayString, 0o700) == 0)
    try FileManager.default.createDirectory(at: root.stateURL, withIntermediateDirectories: true,
                                            attributes: [.posixPermissions: 0o700])
    #expect(symlink(elsewhere.displayString, root.stateURL.appending(path: "staging").path) == 0)
    let path = try root.child("document")
    let result = try sync(bundle: transactionBundle(), rendered: transactionRendered(path), target: target)
    #expect(result.status == .failed)
    #expect(result.error?.code == .ioError)
    #expect(result.error?.message.hasPrefix("[Errno 13] state store must be private to its owner") == true)
    #expect(try FileManager.default.contentsOfDirectory(atPath: elsewhere.displayString) == [])
    #expect(try NativeFilesystem.metadata(path) == nil)
}

@Test
func nativeKindValidationCannotBypassDescendantFences() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    try FileManager.default.createDirectory(at: root.url.appending(path: "tree"), withIntermediateDirectories: false)
    let ref = try NativeAuthority.reference(root.child("tree"), kind: .document)
    #expect(throws: TargetError(code: .unsafePath, message: "resource kind disagrees with existing destination",
                                path: ref.destination))
    {
        try ResourceAdmission.admit(ref, store: root.store)
    }
    let aliasedFence = root.url.appending(path: "tree/.FLYRAIL-retained.TXN")
    try FileManager.default.createDirectory(at: aliasedFence, withIntermediateDirectories: false)
    let tree = try NativeAuthority.reference(root.child("tree"), kind: .tree)
    #expect(try ResourceAdmission.hierarchyConflicts(tree.recordedAuthority, store: root.store, wholeTree: true) ==
        [ResourcePath(aliasedFence.path)])
}

@Test
func nativeAncestorsBindRestrictiveModesAndXattrTransitions() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let absent = try root.child("absent")
    let before = try NativeObservation.snapshot(absent)
    let descriptor = open(root.path.displayString, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
    #expect(descriptor >= 0)
    defer {
        if descriptor >= 0 {
            close(descriptor)
        }
    }
    #expect(fchmod(descriptor, 0o700) == 0)
    expectNativeError(.concurrentChange) { try NativeObservation.require(absent, before) }
    let restrictive = try NativeObservation.snapshot(absent)
    let data = Data([0, 255, 10])
    #expect(data.withUnsafeBytes {
        fsetxattr(descriptor, "flyrail.ancestor-test", $0.baseAddress, $0.count, 0, 0)
    } == 0)
    expectNativeError(.concurrentChange) { try NativeObservation.require(absent, restrictive) }
    let captured = try NativeObservation.snapshot(absent)
    let evidence = try #require(captured.ancestors.last)
    #expect(try stateArray(stateField(JSONParser.parse(evidence.security), "xattrs")).contains(.array([
        .string(Data("flyrail.ancestor-test".utf8).nativeHex), .string(data.nativeHex),
    ])))
}

@Test
func nativeReadlinkPreservesBytesWithoutFollowingTheLink() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("link")
    #expect(symlink("missing-target", path.displayString) == 0)
    let parent = try #require(try NativeFilesystem.parent(path))
    #expect(try NativeFilesystem
        .readlink(parent.descriptor.raw, name: parent.name, path: path) == Array("missing-target".utf8))
    expectNativeError(.unsafePath) { _ = try NativeObservation.snapshot(path) }
}

@Test
func nativeErrnoDiagnosticsHavePythonFraming() throws {
    let path = try ResourcePath("/synthetic/quote'and\"double")
    #expect(nativeIO(ENOTSUP, message: "extended ACLs are unsupported", path: path) == TargetError(
        code: .unsupported,
        message: "[Errno 45] extended ACLs are unsupported: PosixPath('/synthetic/quote\\'and\"double')",
        path: path, errno: ENOTSUP,
    ))
    #expect(nativeIO(EACCES, path: path) == TargetError(
        code: .ioError,
        message: "[Errno 13] Permission denied: '/synthetic/quote\\'and\"double'",
        path: path, errno: EACCES,
    ))
    let controls = try ResourcePath("/synthetic/quote'\n\r\t\u{1}\u{7F}\u{85}\u{2028}")
    #expect(nativeIO(ENOTSUP, message: "extended attributes are unsupported", path: controls) == TargetError(
        code: .unsupported,
        message: "[Errno 45] extended attributes are unsupported: PosixPath(\"/synthetic/quote'\\n\\r\\t\\x01\\x7f\\x85\\u2028\")",
        path: controls, errno: ENOTSUP,
    ))
}

@Test(arguments: [mode_t(0o644), mode_t(0o755)], [false, true])
func transactionPayloadModesDoNotWeakenAdministrativePrivacy(_ mode: mode_t, _ tree: Bool) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let destination = root.url.appending(path: "resource")
    let payload = tree ? destination.appending(path: "file") : destination
    if tree {
        try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: false)
    }
    try Data("before".utf8).write(to: payload)
    #expect(chmod(payload.path, mode) == 0)
    let ref = try NativeAuthority.reference(root.child("resource"), kind: tree ? .tree : .document)
    let after = try Revision(nodes: tree ? [
        NativeResourcePlanner.node("", data: nil, mode: 0o755),
        NativeResourcePlanner.node("file", data: Data("after".utf8), mode: UInt64(mode)),
    ] : [NativeResourcePlanner.node("", data: Data("after".utf8), mode: UInt64(mode))])
    let plan = try storePlan(ref, store: root.store, after: after, previous: nil)
    var observed: [String: mode_t] = [:]
    let directory = plan.transactionDirectory
    func record(_ label: String, _ relative: String) {
        var metadata = stat()
        if let path = try? appendPath(directory, relative), lstat(path.displayString, &metadata) == 0 {
            observed[label] = metadata.st_mode & 0o777
        }
    }
    let seam = NativeTransactionSeam { boundary in
        switch boundary {
        case "prepare":
            record("directory", "")
            record("staging", "staging")
            record("staged", tree ? "staging/resource/file" : "staging/resource")
        case "backup":
            record("backup", "backup")
            record("backed", tree ? "backup/resource/file" : "backup/resource")
        default: break
        }
    }
    let journal = try ResourceTransaction.prepare(plan, store: root.store, seam: seam)
    try ResourceTransaction.publish(journal, store: root.store, seam: seam)
    #expect(observed == [
        "directory": 0o700, "staging": 0o700, "staged": mode, "backup": 0o700, "backed": mode,
    ])
}
