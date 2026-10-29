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
                schemaRequirements: owned.schemaRequirements
            )
        }
        let receipt = try Receipt(
            resource: resource, transactionID: original.transactionID, claims: claims,
            provenance: original.provenance, createdDirectories: original.createdDirectories
        )
        let lock = try NativeAdmission.reserve(resource, timeout: .zero)
        try NativeAdmission.admit(resource, lock: lock)
        let path = try appendPath(resource.stateRoot, "receipt.json")
        try StateCodec.encode(receipt).write(to: URL(fileURLWithPath: path.displayString))
        #expect(chmod(path.displayString, 0o600) == 0)
        let observation = try NativeAdmission.observe(resource, lock: lock)
        #expect(observation.receipt == receipt)
        #expect(observation.recovery.isEmpty)
        #expect(observation.revision.nodes.isEmpty)
    }
}

@Test
func nativeAdmissionReplaysOnlyMatchingPreparedHeaders() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    let prepared = try appendPath(ref.stateRoot, "authority.json.next")
    try StateCodec.encode(ref).write(to: URL(fileURLWithPath: prepared.displayString))
    #expect(chmod(prepared.displayString, 0o600) == 0)
    #expect(try NativeAdmission.validateState(ref).1 == [prepared])
    try NativeAdmission.admit(ref, lock: lock)
    #expect(try NativeObservation.read(
        appendPath(ref.stateRoot, "authority.json"),
        as: ResourceRef.self,
        tag: "ResourceRef"
    ) == ref)
    #expect(try NativeFilesystem.metadata(prepared) == nil)
    let mismatched = try ResourceRef(destination: ref.destination, kind: .tree, stateRoot: ref.stateRoot)
    expectNativeError(.invalidState) { try NativeAdmission.admit(mismatched, lock: lock) }
}

@Test
func nativeStateLinksAndCaseAliasedStorageNeverReceivePayloads() throws {
    for entry in ["state", "lock", "authority.json", "receipt.json", "staging", "backup"] {
        let root = try NativeTestRoot()
        defer { try? root.cleanup() }
        let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
        if entry == "state" {
            #expect(symlink(root.path.displayString, ref.stateRoot.displayString) == 0)
            expectNativeError(.unsafePath) { _ = try NativeAdmission.reserve(ref, timeout: .zero) }
            continue
        }
        let lock = try NativeAdmission.reserve(ref, timeout: .zero)
        let path = try appendPath(ref.stateRoot, entry)
        if entry == "lock" { #expect(unlink(path.displayString) == 0) }
        #expect(symlink(root.path.displayString, path.displayString) == 0)
        expectNativeError(entry == "lock" ? .concurrentChange : .unsafePath) {
            try NativeAdmission.admit(ref, lock: lock)
        }
    }
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    try FileManager.default.createDirectory(
        atPath: ref.stateRoot.displayString.uppercased().replacingOccurrences(
            of: root.path.displayString.uppercased(),
            with: root.path.displayString
        ),
        withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700]
    )
    expectNativeError(.unsafePath) { _ = try NativeAdmission.reserve(ref, timeout: .zero) }
}

@Test
func nativeKindValidationCannotBypassDescendantFences() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    try FileManager.default.createDirectory(at: root.url.appending(path: "tree"), withIntermediateDirectories: false)
    let ref = try NativeAuthority.reference(root.child("tree"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    expectNativeError(.unsafePath) { try NativeAdmission.admit(ref, lock: lock) }
    let aliasedFence = root.url.appending(path: "tree/.FLYRAIL-retained.STATE")
    try FileManager.default.createDirectory(at: aliasedFence, withIntermediateDirectories: false)
    let tree = try NativeAuthority.reference(root.child("tree"), kind: .tree)
    #expect(try NativeAuthority.hierarchyConflicts(tree.recordedAuthority, wholeTree: true) ==
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
    defer { if descriptor >= 0 { close(descriptor) } }
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
        path: path, errno: ENOTSUP
    ))
    #expect(nativeIO(EACCES, path: path) == TargetError(
        code: .ioError,
        message: "[Errno 13] Permission denied: '/synthetic/quote\\'and\"double'",
        path: path, errno: EACCES
    ))
    let controls = try ResourcePath("/synthetic/quote'\n\r\t\u{1}\u{7F}\u{85}\u{2028}")
    #expect(nativeIO(ENOTSUP, message: "extended attributes are unsupported", path: controls) == TargetError(
        code: .unsupported,
        message: "[Errno 45] extended attributes are unsupported: PosixPath(\"/synthetic/quote'\\n\\r\\t\\x01\\x7f\\x85\\u2028\")",
        path: controls, errno: ENOTSUP
    ))
}

@Test
func nativeTransactionPayloadModesDoNotWeakenAdministrativePrivacy() throws {
    for mode: mode_t in [0o644, 0o755] {
        for tree in [false, true] {
            let root = try NativeTestRoot()
            defer { try? root.cleanup() }
            let ref = try NativeAuthority.reference(root.child("resource"), kind: tree ? .tree : .document)
            let lock = try NativeAdmission.reserve(ref, timeout: .zero)
            try NativeAdmission.admit(ref, lock: lock)
            for container in ["staging", "backup"] {
                let directory = try appendPath(ref.stateRoot, container)
                #expect(mkdir(directory.displayString, 0o700) == 0)
                let payload = try appendPath(directory, "resource")
                let file = tree ? try appendPath(payload, "file") : payload
                if tree { #expect(mkdir(payload.displayString, 0o755) == 0) }
                try Data("resource bytes".utf8).write(to: URL(fileURLWithPath: file.displayString))
                #expect(chmod(file.displayString, mode) == 0)
                let observed = try NativeAdmission.observe(ref, lock: lock)
                let node = try #require(observed.state.revision.nodes.first {
                    $0.path == container + "/resource" + (tree ? "/file" : "")
                })
                #expect(node.mode == Count(UInt64(mode)))
                #expect(node.data == Data("resource bytes".utf8))
                #expect(node.uid == Count(UInt64(geteuid())))
                #expect(node.inode > 0)
                #expect(observed.recovery.contains(directory))
                #expect(chmod(directory.displayString, 0o755) == 0)
                expectNativeError(.unsupported) { _ = try NativeAdmission.validateState(ref) }
                #expect(chmod(directory.displayString, 0o700) == 0)
                let descriptor = open(file.displayString, O_RDONLY | O_NOFOLLOW)
                #expect(descriptor >= 0)
                defer { if descriptor >= 0 { close(descriptor) } }
                #expect(fsetxattr(descriptor, "flyrail.payload-test", nil, 0, 0, 0) == 0)
                expectNativeError(.unsupported) { _ = try NativeAdmission.validateState(ref) }
                #expect(fremovexattr(descriptor, "flyrail.payload-test", 0) == 0)
                let foreign = try appendPath(directory, "metadata")
                try Data().write(to: URL(fileURLWithPath: foreign.displayString))
                #expect(chmod(foreign.displayString, 0o644) == 0)
                expectNativeError(.unsupported) { _ = try NativeAdmission.validateState(ref) }
                #expect(unlink(foreign.displayString) == 0)
            }
            let header = try appendPath(ref.stateRoot, "authority.json")
            #expect(chmod(header.displayString, 0o644) == 0)
            expectNativeError(.unsupported) { _ = try NativeAdmission.validateState(ref) }
        }
    }
}
