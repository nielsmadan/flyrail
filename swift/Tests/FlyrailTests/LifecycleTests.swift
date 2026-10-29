import Darwin
@testable import Flyrail
import Foundation
import Testing

func lifecycleSection(_ path: ResourcePath, text: String = "generated", id: String = "guide") throws -> RenderedBundle {
    try RenderedBundle(artifacts: [
        RenderedArtifact(id: id, family: .instructions, destination: path,
                         content: .section(SectionContent(marker: "guide", text: text))),
    ])
}

@Test
func lifecyclePreviewIsReadOnlyAndRemovalPreservesForeignBytes() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("AGENTS.md")
    try NativeTransactionIO.createFile(path, data: Data("# Foreign\n".utf8))
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let bundle = try transactionBundle()
    let rendered = try lifecycleSection(path)
    let before = try NativeObservation.snapshot(root.path)
    let proposed = try preview(bundle: bundle, rendered: rendered, target: target)
    #expect(proposed.applicable)
    #expect(try NativeObservation.snapshot(root.path) == before)
    #expect(try summarize(proposed).changes.map(\.action) == [.update])
    let result = applyPreview(proposed)
    #expect(result.status == .applied)
    #expect(result.observation.matches(bundle, rendered))
    #expect(try sync(bundle: bundle, rendered: rendered, target: target).status == .unchanged)
    #expect(remove(bundleID: "team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == Data("# Foreign\n".utf8))
    #expect(remove(bundleID: "team", target: target).status == .unchanged)
}

@Test(arguments: ["payload", "index", "receipt", "mode", "ancestor"])
func lifecycleRejectsEveryStalePreview(_ mutation: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("nested/document")
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let bundle = try transactionBundle()
    let rendered = try lifecycleSection(path)
    #expect(try sync(bundle: bundle, rendered: rendered, target: target).status == .applied)
    let proposed = try preview(bundle: transactionBundle("2"), rendered: rendered, target: target)
    let resource = try #require(proposed.resources.first?.resource)
    switch mutation {
    case "payload": try Data("foreign\n".utf8).write(to: URL(fileURLWithPath: path.displayString))
    case "index":
        let index = try appendPath(target.indexRoot, "index.json")
        try Data(" ".utf8).appendToFile(index)
    case "receipt":
        try Data(" ".utf8).appendToFile(appendPath(resource.stateRoot, "receipt.json"))
    case "mode": #expect(chmod(path.displayString, 0o644) == 0)
    case "ancestor":
        let parent = try root.child("nested")
        #expect(try rename(parent.displayString, root.child("moved").displayString) == 0)
        #expect(mkdir(parent.displayString, 0o700) == 0)
    default: Issue.record("unknown mutation")
    }
    let changed = try NativeObservation.snapshot(root.path)
    #expect(applyPreview(proposed).error?.code == .concurrentChange)
    #expect(try NativeObservation.snapshot(root.path) == changed)
}

private extension Data {
    func appendToFile(_ path: ResourcePath) throws {
        let handle = try FileHandle(forWritingTo: URL(fileURLWithPath: path.displayString))
        defer { try? handle.close() }
        try handle.seekToEnd()
        try handle.write(contentsOf: self)
    }
}

@Test(arguments: [Acquisition.conflict, .adopt, .takeover])
func lifecycleAcquisitionAndFirstBaseline(_ acquisition: Acquisition) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let original = Data("<!-- flyrail:guide:start -->\noriginal\n<!-- flyrail:guide:end -->\n".utf8)
    try NativeTransactionIO.createFile(path, data: original)
    let target = try InstallationTarget(indexRoot: root.child("index"))
    let result = try sync(bundle: transactionBundle(), rendered: lifecycleSection(path, text: "original"),
                          target: target, acquisition: acquisition)
    if acquisition == .conflict {
        #expect(result.error?.code == .conflict)
        #expect(try NativeObservation.snapshot(path).revision.data == original)
        return
    }
    #expect(result.status == .applied)
    #expect(try sync(bundle: transactionBundle("2"), rendered: lifecycleSection(path, text: "changed", id: "renamed"),
                     target: target).status == .applied)
    #expect(remove(bundleID: "team", target: target).status == .applied)
    #expect(try NativeObservation.snapshot(path).revision.data == (acquisition == .takeover ? original : Data()))
}

@Test
func lifecyclePreflightsEveryResourceAndMissingReceipt() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let first = try root.child("first")
    let second = try root.child("second")
    try NativeTransactionIO.createFile(second, data: Data("foreign".utf8))
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "first", family: .instructions, destination: first,
                         content: .file(FileContent(data: Data("new".utf8)))),
        RenderedArtifact(id: "second", family: .instructions, destination: second,
                         content: .file(FileContent(data: Data("new".utf8)))),
    ])
    let target = try InstallationTarget(indexRoot: root.child("index"))
    #expect(try sync(bundle: transactionBundle(), rendered: rendered, target: target).error?.code == .conflict)
    #expect(try NativeFilesystem.metadata(first) == nil)
    #expect(try sync(bundle: transactionBundle(), rendered: rendered, target: target,
                     acquisition: .takeover).status == .applied)
    let receipt = try appendPath(NativeResourcePlanner.reference(rendered.artifacts[0]).stateRoot, "receipt.json")
    #expect(unlink(receipt.displayString) == 0)
    let before = try NativeObservation.snapshot(root.path)
    #expect(try inspectInstallation(bundleID: "team", target: target).isCurrent == false)
    #expect(remove(bundleID: "team", target: target).error?.code == .invalidState)
    #expect(recoverInstallation(bundleID: "team", target: target).error?.code == .invalidState)
    #expect(try NativeObservation.snapshot(root.path) == before)
}
