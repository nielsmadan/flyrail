import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func nativeRevisionsBindBytesModesOwnersAndIdentity() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("resource")
    try Data("first".utf8).write(to: root.url.appending(path: "resource"))
    #expect(chmod(path.displayString, 0o640) == 0)
    let original = try NativeObservation.snapshot(path)
    let node = try #require(original.revision.nodes.first)
    #expect(node.data == Data("first".utf8))
    #expect(node.mode == 0o640)
    #expect(node.uid == Count(UInt64(geteuid())))
    #expect(node.gid == Count(UInt64(getegid())))
    #expect(try NativeObservation.snapshot(path) == original)
    try Data("other".utf8).write(to: root.url.appending(path: "resource"))
    expectNativeError(.concurrentChange) { try NativeObservation.require(path, original) }
    #expect(chmod(path.displayString, 0o600) == 0)
    let changed = try NativeObservation.snapshot(path)
    #expect(changed.revision.nodes.first?.mode == 0o600)
    let replacement = root.url.appending(path: "replacement")
    try Data("other".utf8).write(to: replacement)
    #expect(chmod(replacement.path, 0o600) == 0)
    #expect(rename(replacement.path, path.displayString) == 0)
    let replaced = try NativeObservation.snapshot(path)
    #expect(replaced.revision != changed.revision)
    expectNativeError(.concurrentChange) { try NativeObservation.require(path, changed) }
}

@Test
func nativeTreeRevisionsHaveExactOrderedNodes() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let tree = root.url.appending(path: "tree")
    try FileManager.default.createDirectory(at: tree.appending(path: "nested"), withIntermediateDirectories: true)
    try Data().write(to: tree.appending(path: "empty"))
    try Data([0, 255, 10]).write(to: tree.appending(path: "nested/bytes"))
    let snapshot = try NativeObservation.snapshot(ResourcePath(tree.path))
    #expect(snapshot.revision.nodes.map(\.path) == ["", "empty", "nested", "nested/bytes"])
    #expect(snapshot.revision.nodes.map(\.data) == [nil, Data(), nil, Data([0, 255, 10])])
    #expect(try StateCodec.decode(StateCodec.encode(snapshot.revision), as: Revision.self) == snapshot.revision)
    try Data("foreign".utf8).write(to: tree.appending(path: "foreign"))
    expectNativeError(.concurrentChange) { try NativeObservation.require(ResourcePath(tree.path), snapshot) }
}

@Test
func nativeAncestorEvidenceIgnoresSiblingChurnButRejectsRebinding() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let parent = root.url.appending(path: "parent")
    try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: false)
    let path = try root.child("parent/absent")
    let before = try NativeObservation.snapshot(path)
    try Data().write(to: parent.appending(path: "sibling"))
    #expect(try NativeObservation.snapshot(path) == before)
    #expect(rename(parent.path, root.url.appending(path: "old").path) == 0)
    try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: false)
    expectNativeError(.concurrentChange) { try NativeObservation.require(path, before) }
}

@Test
func nativeResourceSecurityRefusesForeignXattrsAndCapturesAncestorEvidence() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("resource")
    try Data("bytes".utf8).write(to: root.url.appending(path: "resource"))
    let descriptor = open(path.displayString, O_RDONLY | O_NOFOLLOW)
    #expect(descriptor >= 0)
    defer { if descriptor >= 0 { close(descriptor) } }
    let payload = Data("opaque".utf8)
    let result = payload.withUnsafeBytes { bytes in
        fsetxattr(descriptor, "flyrail.security-test", bytes.baseAddress, bytes.count, 0, 0)
    }
    #expect(result == 0)
    expectNativeError(.unsupported) { _ = try NativeObservation.snapshot(path) }
    let captured = try NativeSecurity.capture(descriptor, path: path, ancestor: true)
    let record = try JSONParser.parse(captured)
    #expect(try stateArray(stateField(record, "xattrs")).contains(
        .array([.string(Data("flyrail.security-test".utf8).nativeHex), .string(payload.nativeHex)]),
    ))
    #expect(fremovexattr(descriptor, "flyrail.security-test", 0) == 0)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("bytes".utf8))
}

@Test
func nativeResourceSecurityRejectsACLsFlagsAndSpecialModes() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("resource")
    try Data("bytes".utf8).write(to: root.url.appending(path: "resource"))
    let descriptor = open(path.displayString, O_RDONLY | O_NOFOLLOW)
    #expect(descriptor >= 0)
    defer { if descriptor >= 0 { close(descriptor) } }
    #expect(fchmod(descriptor, 0o4600) == 0)
    expectNativeError(.unsafePath) { try NativeFilesystem.resourceMode(0o4600, path: path) }
    if try (NativeFilesystem.metadata(path)?.st_mode ?? 0) & 0o7000 != 0 {
        expectNativeError(.unsafePath) { _ = try NativeObservation.snapshot(path) }
    }
    #expect(fchmod(descriptor, 0o600) == 0)
    if fchflags(descriptor, UInt32(UF_HIDDEN)) == 0 {
        defer { _ = fchflags(descriptor, 0) }
        expectNativeError(.unsupported) { _ = try NativeObservation.snapshot(path) }
        let evidence = try NativeSecurity.capture(descriptor, path: path, ancestor: true)
        #expect(try stateField(JSONParser.parse(evidence), "flags") == .number(String(UF_HIDDEN)))
    }
    #expect(fchflags(descriptor, 0) == 0)
    let text = "!#acl 1\nuser:\(NSUserName()) allow read\n"
    if let acl = acl_from_text(text) {
        defer { acl_free(UnsafeMutableRawPointer(acl)) }
        if acl_set_fd_np(descriptor, acl, ACL_TYPE_EXTENDED) == 0 {
            expectNativeError(.unsupported) { _ = try NativeObservation.snapshot(path) }
            let evidence = try NativeSecurity.capture(descriptor, path: path, ancestor: true)
            #expect(try stateField(JSONParser.parse(evidence), "acl") != .string(""))
        }
    }
}

@Test
func nativeObservationRejectsNonUTF8AndPortableSiblingCollisions() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let invalid = try ResourcePath(platformBytes: root.path.platformBytes + [47, 255])
    let created = try NativeFilesystem.withName(invalid.platformBytes) { name in
        let descriptor = open(name, O_WRONLY | O_CREAT | O_EXCL, 0o600)
        if descriptor >= 0 { close(descriptor); return true }
        #expect([EILSEQ, EINVAL, EPERM].contains(errno))
        return false
    }
    expectNativeError(.unsafePath) { _ = try NativeObservation.snapshot(invalid) }
    if created {
        expectNativeError(.unsafePath) { _ = try NativeObservation.snapshot(root.path) }
        try NativeFilesystem.withName(invalid.platformBytes) { #expect(unlink($0) == 0) }
    }
    expectNativeError(.unsafePath) {
        try NativeFilesystem.validatePortableNames([Array("ss".utf8), Array("ß".utf8)], path: root.path)
    }
    expectNativeError(.unsafePath) {
        try NativeFilesystem.validatePortableNames([Array("é".utf8), Array("e\u{301}".utf8)], path: root.path)
    }
    try Data().write(to: root.url.appending(path: "ss"))
    let second = open(root.url.appending(path: "ß").path, O_WRONLY | O_CREAT | O_EXCL, 0o600)
    if second >= 0 {
        close(second)
        expectNativeError(.unsafePath) { _ = try NativeObservation.snapshot(root.path) }
    } else {
        #expect(errno == EEXIST)
        #expect(try ResourceAuthority(destination: root.child("ß")).destination == root.child("ss"))
    }
}

@Test
func nativeBoundedReadsNeverAllocateUnboundedResources() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("large")
    try Data(repeating: 1, count: 1025).write(to: root.url.appending(path: "large"))
    expectNativeError(.unsupported) {
        _ = try NativeObservation.snapshot(path, limits: NativeLimits(maximumFileBytes: 1024))
    }
}
