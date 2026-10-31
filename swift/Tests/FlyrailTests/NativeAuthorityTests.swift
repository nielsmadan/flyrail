import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func nativeCanonicalizationIsReadOnlyAndBindsCaseAliases() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let folder = root.url.appending(path: "Physical")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: false)
    let document = folder.appending(path: "Rules.md")
    try Data("rules".utf8).write(to: document)
    let actual = try ResourceAuthority(destination: ResourcePath(document.path))
    let aliasPath = folder.appending(path: "rules.md")
    if FileManager.default.fileExists(atPath: aliasPath.path) {
        let alias = try ResourceAuthority(destination: ResourcePath(aliasPath.path))
        #expect(alias == actual)
        let missingActual = try ResourceAuthority(destination: ResourcePath(folder.appending(path: "new.md").path))
        let missingAlias = try ResourceAuthority(destination: root.child("physical/new.md"))
        #expect(missingActual == missingAlias)
        #expect(try StateKey.resource(alias.destination, .authority) ==
            StateKey.resource(actual.destination, .authority))
        #expect(try StateKey.resource(missingAlias.destination, .authority) ==
            StateKey.resource(missingActual.destination, .authority))
    } else {
        expectNativeError(.unsafePath) {
            _ = try ResourceAuthority(destination: ResourcePath(aliasPath.path))
        }
    }
    #expect(try FileManager.default.contentsOfDirectory(atPath: folder.path) == ["Rules.md"])
    #expect(try NativeFilesystem.metadata(ResourcePath(root.stateURL.path)) == nil)
}

@Test
func nativeAuthorityRejectsLinksKindsAndReservedPaths() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let file = root.url.appending(path: "file")
    try Data("bytes".utf8).write(to: file)
    let link = root.url.appending(path: "link")
    #expect(symlink(file.path, link.path) == 0)
    expectNativeError(.unsafePath) { _ = try ResourceAuthority(destination: root.child("link")) }
    expectNativeError(.unsafePath) { _ = try ResourceAuthority(destination: root.child("link/child")) }
    let hardlink = root.url.appending(path: "hardlink")
    #expect(Darwin.link(file.path, hardlink.path) == 0)
    expectNativeError(.unsafePath) { _ = try ResourceAuthority(destination: root.child("file")) }
    let special = root.url.appending(path: "fifo")
    #expect(mkfifo(special.path, 0o600) == 0)
    expectNativeError(.unsafePath) { _ = try ResourceAuthority(destination: root.child("fifo")) }
    #expect(throws: FlyrailValueError.self) {
        _ = try ResourceAuthority(destination: root.child(".FLYRAIL-private.TXN/child"))
    }
    _ = try ResourceAuthority(destination: root.child(".flyrail-old.state/child"))
    #expect(throws: FlyrailValueError.self) {
        _ = try ResourceAuthority(destination: root.child("../escape"))
    }
}

@Test
func nativeAuthorityNeverLossilyConvertsBytes() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let raw = try ResourcePath(platformBytes: root.path.platformBytes + [47, 255])
    expectNativeError(.unsafePath) { _ = try ResourceAuthority(destination: raw) }
    #expect(throws: FlyrailValueError.self) {
        _ = try ResourceAuthority(destination: root.child("e\u{301}.md"))
    }
}

private func authorityError(_ body: () throws -> Void) -> TargetError? {
    do {
        try body()
        return nil
    } catch let error as TargetError {
        return error
    } catch {
        return nil
    }
}

@Test(arguments: [false, true])
func admissionWritesAndReplaysTheAuthorityRecord(_ directoryStore: Bool) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store: any StateStore = directoryStore ? root.store : MemoryStore()
    let ref = try NativeAuthority.reference(root.child("rules.md"), kind: .document)
    let key = try StateKey.resource(ref.destination, .authority)
    try ResourceAdmission.admit(ref, store: store)
    let recorded = try #require(try store.read(key))
    #expect(try StateCodec.decode(recorded, as: ResourceRef.self) == ref)
    try ResourceAdmission.admit(ref, store: store)
    #expect(try store.read(key) == recorded)
    #expect(try FileManager.default.contentsOfDirectory(atPath: root.url.path) == [])
}

@Test
func reserveBeforeProbeRetainsBothAuthorityRecordsInEitherOrder() throws {
    for treeFirst in [true, false] {
        let root = try NativeTestRoot()
        defer { try? root.cleanup() }
        try FileManager.default.createDirectory(
            at: root.url.appending(path: "tree"),
            withIntermediateDirectories: false,
        )
        let tree = try NativeAuthority.reference(root.child("tree"), kind: .tree)
        let child = try NativeAuthority.reference(root.child("tree/child.md"), kind: .document)
        let (first, second) = treeFirst ? (tree, child) : (child, tree)
        try ResourceAdmission.admit(first, store: root.store)
        #expect(authorityError { try ResourceAdmission.admit(second, store: root.store) }?.code == .conflict)
        for ref in [tree, child] {
            #expect(try root.store.read(StateKey.resource(ref.destination, .authority)) == StateCodec.encode(ref))
        }
    }
}

@Test
func admissionRejectsAChangedCanonicalDestination() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    try FileManager.default.createDirectory(at: root.url.appending(path: "Config"), withIntermediateDirectories: false)
    guard FileManager.default.fileExists(atPath: root.url.appending(path: "config").path) else { return }
    let recorded = try ResourceRef(destination: root.child("config/AGENTS.md"), kind: .document)
    let store = MemoryStore()
    #expect(authorityError { try ResourceAdmission.admit(recorded, store: store) } == TargetError(
        code: .invalidState, message: "resource canonical destination changed",
    ))
    #expect(try store.keys(prefix: "") == [])
}

@Test
func admissionRefusesWrongTypedAndNoncanonicalAuthorityRecords() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = MemoryStore()
    let ref = try NativeAuthority.reference(root.child("rules.md"), kind: .document)
    let key = try StateKey.resource(ref.destination, .authority)
    #expect(try store.write(key, value: Data("{\"enum\": \"Ownership\", \"value\": \"file\"}\n".utf8), expected: nil))
    #expect(authorityError { try ResourceAdmission.admit(ref, store: store) } == TargetError(
        code: .invalidState, message: "\(key): record has the wrong type",
    ))

    let other = try NativeAuthority.reference(root.child("other.md"), kind: .document)
    let otherKey = try StateKey.resource(other.destination, .authority)
    let canonical = try StateCodec.encode(other)
    #expect(try store.write(otherKey, value: canonical.prefix(1) + Data(" ".utf8) + canonical.dropFirst(),
                            expected: nil))
    #expect(authorityError { try ResourceAdmission.admit(other, store: store) } == TargetError(
        code: .invalidState, message: "\(otherKey): record is not canonical",
    ))
}

@Test
func nativeIndexObservationDoesNotAcquireOrWrite() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    #expect(try InstallationIndex.read(bundleID: "bundle", target: target) == nil)
    #expect(try NativeFilesystem.metadata(ResourcePath(root.stateURL.path)) == nil)
}
