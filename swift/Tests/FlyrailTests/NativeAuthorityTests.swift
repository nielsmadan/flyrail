import Darwin
@testable import Flyrail
import Foundation
import Testing

struct NativeTestRoot {
    let url: URL
    let path: ResourcePath

    init() throws {
        url = stateRepositoryRoot().appending(path: ".cache/swift-native-tests/\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        path = try ResourcePath(url.path)
    }

    func child(_ name: String) throws -> ResourcePath { try appendPath(path, name) }
    func cleanup() throws { try FileManager.default.removeItem(at: url) }
}

func expectNativeError(_ code: ErrorCode, _ body: () throws -> Void) {
    do {
        try body()
        Issue.record("expected \(code.rawValue)")
    } catch let error as TargetError {
        #expect(error.code == code)
    } catch {
        Issue.record("unexpected error: \(error)")
    }
}

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
    } else {
        expectNativeError(.unsafePath) {
            _ = try ResourceAuthority(destination: ResourcePath(aliasPath.path))
        }
    }
    #expect(try NativeFilesystem.metadata(actual.stateRoot) == nil)
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
        _ = try ResourceAuthority(destination: root.child(".FLYRAIL-private.STATE/child"))
    }
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

@Test
func nativeAdmissionRetainsPermanentFencesAndHeader() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("rules.md"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    let header = try appendPath(ref.stateRoot, "authority.json")
    #expect(try NativeObservation.read(header, as: ResourceRef.self, tag: "ResourceRef") == ref)
    #expect(try (NativeFilesystem.metadata(ref.stateRoot)?.st_mode ?? 0) & 0o777 == 0o700)
    #expect(try (NativeFilesystem.metadata(header)?.st_mode ?? 0) & 0o777 == 0o600)
    expectNativeError(.busy) { _ = try NativeAdmission.reserve(ref, timeout: .zero) }
    let observed = try NativeAdmission.observe(ref)
    #expect(observed.revision.nodes.isEmpty)
    #expect(observed.receipt == nil)
    #expect(observed.recovery.isEmpty)
    try lock.validate()
}

@Test
func nativeHierarchyReserveBeforeProbeExcludesTreeAndChild() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    try FileManager.default.createDirectory(at: root.url.appending(path: "tree"), withIntermediateDirectories: false)
    let tree = try NativeAuthority.reference(root.child("tree"), kind: .tree)
    let child = try NativeAuthority.reference(root.child("tree/child.md"), kind: .document)
    let treeLock = try NativeAdmission.reserve(tree, timeout: .zero)
    let childLock = try NativeAdmission.reserve(child, timeout: .zero)
    expectNativeError(.conflict) { try NativeAdmission.admit(tree, lock: treeLock) }
    expectNativeError(.conflict) { try NativeAdmission.admit(child, lock: childLock) }
    #expect(try NativeFilesystem.metadata(tree.stateRoot) != nil)
    #expect(try NativeFilesystem.metadata(child.stateRoot) != nil)
}

@Test
func nativeAdmissionRefusesUnsafeStateWithoutRepairingIt() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("rules.md"), kind: .document)
    try FileManager.default.createDirectory(
        atPath: ref.stateRoot.displayString, withIntermediateDirectories: false,
        attributes: [.posixPermissions: 0o750]
    )
    expectNativeError(.unsupported) { _ = try NativeAdmission.reserve(ref, timeout: .zero) }
    #expect(try (NativeFilesystem.metadata(ref.stateRoot)?.st_mode ?? 0) & 0o777 == 0o750)
    #expect(chmod(ref.stateRoot.displayString, 0o700) == 0)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try Data("unknown".utf8).write(to: URL(fileURLWithPath: ref.stateRoot.displayString).appending(path: "unknown"))
    #expect(try chmod(appendPath(ref.stateRoot, "unknown").displayString, 0o600) == 0)
    expectNativeError(.invalidState) { try NativeAdmission.admit(ref, lock: lock) }
}

@Test
func nativeHeaderAndLockReplacementAreRejected() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("rules.md"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    let lockPath = try appendPath(ref.stateRoot, "lock")
    #expect(unlink(lockPath.displayString) == 0)
    let replacement = open(lockPath.displayString, O_WRONLY | O_CREAT | O_EXCL, 0o600)
    #expect(replacement >= 0)
    if replacement >= 0 { close(replacement) }
    expectNativeError(.concurrentChange) { try lock.validate() }
}

@Test
func nativeIndexObservationDoesNotAcquireOrWrite() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let absent = try NativeAdmission.observeIndex(target, bundleID: "bundle")
    #expect(absent.current == nil)
    #expect(absent.prepared == nil)
    #expect(try NativeFilesystem.metadata(target.indexRoot) == nil)
    let lock = try NativeAdmission.reserveIndex(target, timeout: .zero)
    try lock.validate()
    #expect(try NativeAdmission.observeIndex(target, bundleID: "bundle").current == nil)
}
