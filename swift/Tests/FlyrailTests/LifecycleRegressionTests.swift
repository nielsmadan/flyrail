import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func summaryRoutingPreservesCanonicallyEquivalentDistinctKeys() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target(routingContext: [
        ("é", "first"), ("e\u{301}", "second"),
        ("agent", "claude"), ("scope", "project"), ("surface", "cli"),
    ])
    let proposed = try preview(
        bundle: transactionBundle(),
        rendered: lifecycleSection(root.child("document")),
        target: target,
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
    let target = try root.target()
    let proposed = try preview(bundle: transactionBundle(), rendered: rendered, target: target)
    #expect(try proposed.resources[0].resource.destination == ResourcePath(file.path))
    try FileManager.default.removeItem(at: folder)
    let summary = try summarize(proposed)
    #expect(summary.changes[0].artifactIDs == ["guide"])
    #expect(summary.changes[0].families == [.instructions])
    #expect(summary.changes[0].action == .update)
}

private func retainedPreparation(_ ref: ResourceRef, store: any StateStore) throws -> Preparation {
    let transactionID = randomTransactionID()
    let preparation = try Preparation(
        resource: ref, transactionID: transactionID,
        directory: transactionPath(destination: ref.destination, transactionID: transactionID),
    )
    try StateRecords.write(
        store, key: StateKey.resource(ref.destination, .preparation), value: preparation, expected: nil,
    )
    return preparation
}

@Test(arguments: ["retained-preparation", "conflict"])
func rejectedApplyCannotRecoverResources(_ reason: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try root.target()
    let rendered = try transactionRendered(path)
    let ref = try NativeResourcePlanner.reference(rendered.artifacts[0])
    let authorityKey = try StateKey.resource(ref.destination, .authority)
    let preparationKey = try StateKey.resource(ref.destination, .preparation)
    var retained: Data?
    if reason == "retained-preparation" {
        _ = try retainedPreparation(ref, store: root.store)
        retained = try root.store.read(preparationKey)
    } else {
        try NativeTransactionIO.createFile(path, data: Data("foreign".utf8))
    }
    let proposed = try preview(bundle: transactionBundle(), rendered: rendered, target: target)
    #expect(proposed.error == nil)
    #expect(proposed.applicable == false)
    let before = try NativeObservation.snapshot(root.path)
    let result = applyPreview(proposed)
    #expect(result.status == .failed)
    #expect(result.error?.code == (reason == "retained-preparation" ? .recoveryNeeded : .conflict))
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(try StateRecords.read(root.store, key: authorityKey, as: ResourceRef.self, tag: "ResourceRef") == ref)
    #expect(try root.store.read(preparationKey) == retained)
    let expected = reason == "retained-preparation" ? [authorityKey, preparationKey] : [authorityKey]
    #expect(try root.store.keys(prefix: "") == expected.sorted())
}

@Test
func previewNeverWritesTheStoreAndRejectsALaterPreparation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let rendered = try transactionRendered(root.child("document"))
    let ref = try NativeResourcePlanner.reference(rendered.artifacts[0])
    let proposed = try preview(bundle: transactionBundle(), rendered: rendered, target: root.target())
    #expect(proposed.applicable)
    #expect(FileManager.default.fileExists(atPath: root.stateURL.path) == false)
    _ = try retainedPreparation(ref, store: root.store)
    let result = applyPreview(proposed)
    #expect(result.status == .failed)
    #expect(result.error?.code == .concurrentChange)
    #expect(result.error?.message == "preview recovery changed")
}

@Test
func sourceFreeRecoveryReportsCompletedPrefixAndOnlyTheFailingResource() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
    let store = target.store
    let refs = try destinationOrder(["a", "b", "c"]
        .map { try NativeAuthority.reference(root.child($0), kind: .document) })
    let after = try Revision(nodes: [NativeResourcePlanner.node("", data: Data("after\n".utf8), mode: 0o644)])
    var directories: [ResourcePath] = []
    do {
        let locks = try LifecycleExecution.locks(bundleID: "team", target: target, expected: nil, resources: refs,
                                                 timeout: .zero)
        defer { locks.release() }
        for (offset, ref) in refs.enumerated() {
            let plan = try storePlan(ref, store: store, after: after, previous: nil)
            let journal = try ResourceTransaction.prepare(plan, store: store)
            directories.append(plan.transactionDirectory)
            if offset == 0 {
                try ResourceTransaction.publish(journal, store: store)
            } else if offset == 1 {
                try NativeTransactionIO.move(appendPath(plan.transactionDirectory, "staging/resource"),
                                             to: ref.destination, revision: journal.staged)
                try Data("foreign".utf8).write(to: URL(fileURLWithPath: ref.destination.displayString))
            }
        }
        let index = try Index(bundleID: "team", contextDigest: target.contextDigest, residual: refs)
        try StateRecords.write(store, key: InstallationIndex.key(bundleID: "team", target: target),
                               value: index, expected: nil)
    }
    let suffix = try ResourceTransaction.records(refs[2], store: store)
    let result = recoverInstallation(bundleID: "team", target: target)
    #expect(result.status == .incomplete)
    #expect(result.error?.code == .recoveryNeeded)
    #expect(result.resources.map(\.resource) == Array(refs.prefix(2)))
    #expect(result.resources.map(\.status) == [.applied, .incomplete])
    #expect(result.resources[0].error == nil)
    #expect(result.resources[1].error == result.error)
    #expect(result.resources[1].recoveryPaths == [directories[1]])
    #expect(try ResourceAdmission.observe(refs[0], store: store).recoveryPaths.isEmpty)
    let retained = try ResourceTransaction.records(refs[2], store: store)
    #expect(retained.preparation == suffix.preparation)
    #expect(retained.journal == suffix.journal)
    #expect(try NativeFilesystem.metadata(directories[2]) != nil)
}

@Test
func desiredGenerationMatchingIsPureAndChecksBothBundleAndRenderedContent() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try root.target()
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
