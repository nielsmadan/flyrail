import Darwin
@testable import Flyrail
import Foundation
import Testing

func lifecycleSkills(_ version: String = "1", text: String = "skill") throws -> FlyrailBundle {
    try FlyrailBundle(identity: BundleIdentity(id: "team", version: version), artifacts: [
        .skill(SkillArtifact(id: "review", name: "review", content: TreeContent(entries: [
            BundleEntry(path: "SKILL.md", data: Data(text.utf8)),
            BundleEntry(path: "run", data: Data("synthetic command".utf8), executable: true),
            BundleEntry(path: "empty"),
        ]))),
    ], assets: [], dependencies: [])
}

@Test
func skillAdaptersKeepOrderAliasesInventoryAndSourceFreeRemoval() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try Target.project(agent: .claude, root: root.path)
    let directory = try Target.directory(target.root)
    let blocked = try Target.directory(root.child("blocked"))
    try NativeTransactionIO.createFile(blocked.root, data: Data("foreign".utf8))
    let bundle = try lifecycleSkills()
    let before = try NativeObservation.snapshot(root.path)
    let observations = inspect(bundle, targets: [target, directory, blocked], store: root.store)
    #expect(observations.map(\.observation.state) == [.absent, .absent, .unknown])
    #expect(observations[1].aliasOf == 0)
    #expect(try NativeObservation.snapshot(root.path) == before)
    let installed = install(bundle, targets: [target, directory, blocked], store: root.store)
    #expect(installed.map(\.status) == [.applied, .applied, .failed])
    #expect(installed[1].aliasOf == 0)
    #expect(installed[0].observation.isCurrent)
    let entries = try #require(installed[0].observation.installed?.entries)
    #expect(entries.map(\.path) == ["review", "review/SKILL.md", "review/empty", "review/run"])
    #expect(entries.first { $0.path == "review/run" }?.executable == true)
    #expect(entries.first { $0.path == "review/SKILL.md" }?.size == 5)
    let second = try lifecycleSkills("2", text: "new")
    #expect(install(second, targets: [target], store: root.store)[0].error?.code == .updateRequired)
    #expect(update(second, targets: [directory], store: root.store)[0].status == .applied)
    try Data("edited".utf8).write(to: URL(fileURLWithPath: appendPath(target.root, "review/SKILL.md").displayString))
    #expect(inspect(second, targets: [target], store: root.store)[0].observation.modifications
        .map(\.kind) == [.contentChanged])
    #expect(uninstall(bundleID: "team", targets: [target], store: root.store)[0].error?.code == .modified)
    #expect(update(second, targets: [target], store: root.store, options: SkillOptions(replaceModified: true))[0]
        .status == .applied)
    #expect(uninstall(bundleID: "team", targets: [directory, target], store: root.store)[0].status == .applied)
    #expect(uninstall(bundleID: "team", targets: [target], store: root.store)[0].status == .unchanged)
    #expect(try NativeFilesystem.metadata(appendPath(target.root, "review")) == nil)
}

@Test
func everyDroidConvenienceBoundaryRefusesBeforeAnyTargetIO() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let blocked = try root.child("blocked")
    #expect(symlink("/unreachable/synthetic", blocked.displayString) == 0)
    let target = try Target.project(agent: .droid, root: blocked)
    let bundle = try lifecycleSkills()
    let before = try NativeFilesystem.metadata(blocked)
    #expect(try renderSkills(bundle, target: target).notices.map(\.code) == ["agent-unsupported"])
    #expect(inspect(bundle, targets: [target], store: root.store)[0].observation.error?.code == .unsupported)
    #expect(install(bundle, targets: [target], store: root.store)[0].error?.code == .unsupported)
    #expect(update(bundle, targets: [target], store: root.store)[0].error?.code == .unsupported)
    #expect(uninstall(bundleID: "team", targets: [target], store: root.store)[0].error?.code == .unsupported)
    #expect(try NativeFilesystem.metadata(blocked)?.st_ino == before?.st_ino)
    #expect(try FileManager.default.contentsOfDirectory(atPath: root.path.displayString) == ["blocked"])
}

@Test
func convenienceAdaptersRefuseNonSkillsWithoutTouchingTargets() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let target = try Target.directory(root.child("skills"))
    let bundle = try transactionBundle()
    let before = try NativeObservation.snapshot(root.path)
    #expect(inspect(bundle, targets: [target], store: root.store)[0].observation.error?
        .message == "use explicit rendering for non-skill artifacts")
    #expect(install(bundle, targets: [target], store: root.store)[0].error?
        .message == "use explicit rendering for non-skill artifacts")
    #expect(update(bundle, targets: [target], store: root.store)[0].error?
        .message == "use explicit rendering for non-skill artifacts")
    #expect(try NativeObservation.snapshot(root.path) == before)
}

@Test
func skillPublicModelsPreserveExactUTF8AndSendability() throws {
    let first = try Installation(
        bundleID: "team",
        version: "é",
        contentDigest: "digest",
        transactionID: "transaction",
        entries: [],
    )
    let second = try Installation(
        bundleID: "team",
        version: "e\u{301}",
        contentDigest: "digest",
        transactionID: "transaction",
        entries: [],
    )
    #expect(Set([first, second]).count == 2)
    #expect(Set([Conflict(path: "é"), Conflict(path: "e\u{301}")]).count == 2)
    #expect(try Set([Modification(bundleID: "team", path: "é", kind: .missing),
                     Modification(bundleID: "team", path: "e\u{301}", kind: .missing)]).count == 2)
    func sendable<T: Sendable>(_ value: T) -> T {
        value
    }
    #expect(sendable(first) == first)
    #expect(sendable(Observation(state: .absent)).state == .absent)
}
