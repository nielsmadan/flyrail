import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func summaryRoutingPreservesCanonicallyEquivalentDistinctKeys() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try InstallationTarget(indexRoot: root.child("index"), routingContext: [
        ("é", "first"), ("e\u{301}", "second"),
        ("agent", "claude"), ("scope", "project"), ("surface", "cli"),
    ])
    let proposed = try preview(
        bundle: transactionBundle(),
        rendered: lifecycleSection(root.child("document")),
        target: target
    )
    let summary = try summarize(proposed)
    #expect(summary.agent == .claude)
    #expect(summary.scope == .project)
    #expect(summary.surface == .cli)
    #expect(summary.changes[0].artifactIDs == ["guide"])
}

@Test(arguments: ["leaf", "parent"])
func summaryRetainsPlannerAliasAttributionWithoutFilesystemReads(_ alias: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let folder = root.url.appending(path: "Physical")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: false)
    let file = folder.appending(path: "Agents.md")
    try NativeTransactionIO.createFile(ResourcePath(file.path), data: Data("foreign\n".utf8))
    let selected = alias == "leaf" ? folder.appending(path: "AGENTS.md") : root.url
        .appending(path: "physical/Agents.md")
    guard FileManager.default.fileExists(atPath: selected.path) else { return }
    let rendered = try lifecycleSection(ResourcePath(selected.path))
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let proposed = try preview(bundle: transactionBundle(), rendered: rendered, target: target)
    #expect(try proposed.resources[0].resource.destination == ResourcePath(file.path))
    try FileManager.default.removeItem(at: folder)
    let summary = try summarize(proposed)
    #expect(summary.changes[0].artifactIDs == ["guide"])
    #expect(summary.changes[0].families == [.instructions])
    #expect(summary.changes[0].action == .update)
}

@Test(arguments: ["authority-preparation", "conflict"])
func rejectedApplyCannotAdmitOrRecoverResources(_ reason: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let rendered = try transactionRendered(path)
    let ref = try NativeResourcePlanner.reference(rendered.artifacts[0])
    if reason == "authority-preparation" {
        do {
            let lock = try NativeAdmission.reserve(ref, timeout: .zero)
            try NativeTransactionIO.createFile(
                appendPath(ref.stateRoot, "authority.json.next"),
                data: StateCodec.encode(ref)
            )
            try lock.validate()
        }
    } else {
        try NativeTransactionIO.createFile(path, data: Data("foreign".utf8))
    }
    let proposed = try preview(bundle: transactionBundle(), rendered: rendered, target: target)
    #expect(proposed.error == nil)
    #expect(proposed.applicable == false)
    let before = try NativeObservation.snapshot(root.path)
    let result = applyPreview(proposed)
    #expect(result.status == .failed)
    #expect(result.error?.code == (reason == "authority-preparation" ? .recoveryNeeded : .conflict))
    #expect(try NativeObservation.snapshot(root.path) == before)
    if reason == "authority-preparation" {
        #expect(try NativeObservation.read(appendPath(ref.stateRoot, "authority.json.next"),
                                           as: ResourceRef.self, tag: "ResourceRef") == ref)
        #expect(try NativeFilesystem.metadata(appendPath(ref.stateRoot, "authority.json")) == nil)
    }
}

@Test
func previewAdmissionNeverReconcilesAnExistingAuthorityPreparation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let ref = try NativeAuthority.reference(root.child("document"), kind: .document)
    let lock = try NativeAdmission.reserve(ref, timeout: .zero)
    let next = try appendPath(ref.stateRoot, "authority.json.next")
    try NativeTransactionIO.createFile(next, data: StateCodec.encode(ref))
    let before = try NativeObservation.snapshot(ref.stateRoot)
    expectNativeError(.recoveryNeeded) { try NativeAdmission.admit(ref, lock: lock, allowRecovery: false) }
    #expect(try NativeObservation.snapshot(ref.stateRoot) == before)
}

@Test
func sourceFreeRecoveryReportsCompletedPrefixAndOnlyTheFailingResource() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let refs = try ["a", "b", "c"].map { try NativeAuthority.reference(root.child($0), kind: .document) }
        .sorted { $0.stateRoot.platformBytes.lexicographicallyPrecedes($1.stateRoot.platformBytes) }
    do {
        let locks = try NativeTransactionLocks(target: target, bundleID: "team", resources: refs, timeout: .zero)
        for (offset, ref) in refs.enumerated() {
            let lock = try locks.guardFor(ref)
            let journal = try NativeTransaction.prepare(transactionPlan(ref, lock: lock), lock: lock)
            if offset == 0 {
                try NativeTransaction.publish(journal, lock: lock)
            } else if offset == 1 {
                try NativeTransactionIO.move(appendPath(ref.stateRoot, "staging/resource"),
                                             to: ref.destination, revision: journal.staged)
                try Data("foreign".utf8).write(to: URL(fileURLWithPath: ref.destination.displayString))
            }
        }
        let index = try Index(bundleID: "team", contextDigest: target.contextDigest, residual: refs)
        try NativeTransactionIO.createFile(appendPath(target.indexRoot, "index.json"), data: StateCodec.encode(index))
    }
    let suffix = try NativeObservation.snapshot(refs[2].stateRoot)
    let result = recoverInstallation(bundleID: "team", target: target)
    #expect(result.status == .incomplete)
    #expect(result.error?.code == .recoveryNeeded)
    #expect(result.resources.map(\.resource) == Array(refs.prefix(2)))
    #expect(result.resources.map(\.status) == [.applied, .incomplete])
    #expect(result.resources[0].error == nil)
    #expect(result.resources[1].error == result.error)
    #expect(try NativeAdmission.observe(refs[0]).recovery.isEmpty)
    #expect(try NativeObservation.snapshot(refs[2].stateRoot) == suffix)
}

@Test
func desiredGenerationMatchingIsPureAndChecksBothBundleAndRenderedContent() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let rendered = try transactionRendered(root.child("document"))
    let bundle = try transactionBundle()
    #expect(try sync(bundle: bundle, rendered: rendered, target: target).status == .applied)
    let observed = try inspectInstallation(bundleID: "team", target: target)
    let before = try NativeObservation.snapshot(root.path)
    #expect(observed.matches(bundle, rendered))
    #expect(try observed.matches(transactionBundle("2"), rendered) == false)
    #expect(try observed.matches(bundle, transactionRendered(root.child("document"), "changed")) == false)
    #expect(try NativeObservation.snapshot(root.path) == before)
}
