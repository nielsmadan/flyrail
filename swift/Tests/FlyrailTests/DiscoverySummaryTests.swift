import Darwin
@testable import Flyrail
import Foundation
import Testing

@Test
func discoveryUsesOnlyExplicitOverridesAndStableAgentOrder() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let bin = try root.child("bin")
    #expect(mkdir(bin.displayString, 0o700) == 0)
    for name in ["droid", "codex", "claude", "pi"] {
        let path = try appendPath(bin, name)
        try NativeTransactionIO.createFile(path, data: Data("not executed".utf8))
        #expect(chmod(path.displayString, name == "pi" ? 0o600 : 0o700) == 0)
    }
    let applications = try root.child("apps")
    #expect(mkdir(applications.displayString, 0o700) == 0)
    for name in ["Cursor.app", "Visual Studio Code.app"] {
        #expect(try mkdir(appendPath(applications, name).displayString, 0o700) == 0)
    }
    let before = try NativeObservation.snapshot(root.path)
    let found = try detectAgents(path: [bin], applications: [applications])
    #expect(found.map(\.agent) == [.claude, .codex, .cursor, .copilot, .droid])
    #expect(try found.first?.executable == appendPath(bin, "claude"))
    #expect(try found.first { $0.agent == .cursor }?.application == appendPath(applications, "Cursor.app"))
    #expect(try detectAgents(path: [], applications: []).isEmpty)
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(throws: FlyrailValueError.self) { try AgentPresence(agent: .droid) }
    #expect(throws: FlyrailValueError.self) { try AgentPresence(agent: .claude, executable: ResourcePath("relative")) }
}

@Test
func summariesReportAllArtifactsRoutingActionsErrorsAndNotices() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try InstallationTarget(indexRoot: root.child("index"),
                                        routingContext: [("agent", "claude"), ("scope", "project"), ("surface", "cli")])
    let notice = try Notice(artifactID: "first", kind: .activation, message: "Reload the host.", code: "reload")
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "second", family: .instructions, destination: path,
                         content: .section(SectionContent(marker: "second", text: "two"))),
        RenderedArtifact(id: "first", family: .instructions, destination: path,
                         content: .section(SectionContent(marker: "first", text: "one"))),
    ], notices: [notice])
    let first = try preview(bundle: transactionBundle(), rendered: rendered, target: target)
    let summary = try summarize(first)
    #expect(summary.agent == .claude && summary.scope == .project && summary.surface == .cli)
    #expect(summary.changes.count == 1)
    #expect(summary.changes[0].artifactIDs == ["first", "second"])
    #expect(summary.changes[0].families == [.instructions, .instructions])
    #expect(summary.changes[0].action == .create)
    #expect(summary.notices == [notice])
    let applied = applyPreview(first)
    #expect(applied.notices == [notice])
    #expect(try summarize(preview(bundle: transactionBundle(), rendered: rendered, target: target)).changes[0]
        .action == .unchanged)
    #expect(try summarize(previewRemoval(bundleID: "team", target: target)).changes[0].action == .delete)
    try Data("modified".utf8).write(to: URL(fileURLWithPath: path.displayString))
    let conflict = try summarize(previewRemoval(bundleID: "team", target: target))
    #expect(conflict.applicable == false)
    #expect(conflict.changes[0].action == .conflict)
    #expect(conflict.changes[0].error?.code == .modified)
    #expect(throws: FlyrailValueError.self) {
        try PlannedChange(artifactIDs: ["guide"], families: [], kind: .document, destination: path, action: .create)
    }
}
