import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func nativeEmptyReadableLocksInteroperateWithoutExposingPayloads() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    _ = try NativeAdmission.createPrivateDirectories(ref.stateRoot)
    let path = try appendPath(ref.stateRoot, "lock")
    let raw = open(path.displayString, O_CREAT | O_EXCL | O_WRONLY | O_NOFOLLOW, 0o644)
    #expect(raw >= 0)
    if raw >= 0 { close(raw) }
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    try NativeAdmission.admit(ref, lock: lock)
    #expect(try (NativeFilesystem.metadata(path)?.st_mode ?? 0) & 0o777 == 0o644)
    #expect(try NativeAdmission.observe(ref, lock: lock).state.revision.nodes.first?.mode == 0o700)
    let receipt = try appendPath(ref.stateRoot, "receipt.json")
    try StateCodec.encode(Receipt(resource: ref, transactionID: String(repeating: "1", count: 32)))
        .write(to: URL(fileURLWithPath: receipt.displayString))
    #expect(chmod(receipt.displayString, 0o644) == 0)
    expectNativeError(.unsupported) { _ = try NativeAdmission.observe(ref, lock: lock) }
}

@Test
func nativeCoordinationLocksRejectForeignWritesAndAnyPayload() throws {
    for payload in [Data(), Data("private".utf8)] {
        let root = try NativeTestRoot()
        defer { try? root.cleanup() }
        let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
        _ = try NativeAdmission.createPrivateDirectories(ref.stateRoot)
        let path = try appendPath(ref.stateRoot, "lock")
        try payload.write(to: URL(fileURLWithPath: path.displayString))
        #expect(chmod(path.displayString, payload.isEmpty ? 0o666 : 0o600) == 0)
        expectNativeError(.unsupported) { _ = try NativeAdmission.reserve(ref, timeout: .zero) }
    }
}

@Test
func nativeStateRootReplacementInvalidatesHeldLock() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    #expect(rename(ref.stateRoot.displayString, root.url.appending(path: "replaced-state").path) == 0)
    try FileManager.default.createDirectory(
        atPath: ref.stateRoot.displayString, withIntermediateDirectories: false,
        attributes: [.posixPermissions: 0o700]
    )
    expectNativeError(.concurrentChange) { try lock.validate() }
    #expect(try NativeFilesystem.metadata(appendPath(ref.stateRoot, "authority.json")) == nil)
}

@Test
func nativeEqualByteRewritesCarryPostJournalReplacementEvidence() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let resource = try root.child("resource")
    let path = root.url.appending(path: "resource")
    try Data("bytes".utf8).write(to: path)
    let original = try NativeObservation.snapshot(resource)
    Thread.sleep(forTimeInterval: 0.003)
    try Data("journal".utf8).write(to: root.url.appending(path: "journal"))
    let journal = try #require(try NativeObservation.snapshot(root.child("journal")).stamps.first)
    #expect(original.hasReplacementEvidence(after: journal) == false)
    Thread.sleep(forTimeInterval: 0.003)
    try Data("bytes".utf8).write(to: path)
    let changed = try NativeObservation.snapshot(resource)
    #expect(changed.revision == original.revision)
    #expect(changed.hasReplacementEvidence(after: journal))
    expectNativeError(.concurrentChange) { try NativeObservation.require(resource, original) }
}
