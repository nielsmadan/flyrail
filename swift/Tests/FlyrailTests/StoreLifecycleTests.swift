import Darwin
@testable import Flyrail
import Foundation
import Testing

private let overlap = "state store overlaps a resource or authority"

private func section(_ path: ResourcePath, marker: String, text: String) throws -> RenderedBundle {
    try RenderedBundle(artifacts: [
        RenderedArtifact(id: "guide", family: .instructions, destination: path,
                         content: .section(SectionContent(marker: marker, text: text))),
    ])
}

private func fileMode(_ path: ResourcePath) -> mode_t? {
    var metadata = stat()
    return lstat(path.displayString, &metadata) == 0 ? metadata.st_mode & 0o777 : nil
}

@Test(arguments: ["traversal", "symlink"])
func anAliasedStoreLocationStillOverlapsTheDestination(_ spelling: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    try FileManager.default.createDirectory(at: root.url.appending(path: "other"), withIntermediateDirectories: false)
    #expect(symlink(root.path.displayString, root.url.appending(path: "link").path) == 0)
    let parent = spelling == "traversal" ? "other/.." : "link"
    let store = try DirectoryStore(ResourcePath("\(root.path.displayString)/\(parent)/AGENTS.md/state"))
    let target = try InstallationTarget(store: store)
    #expect(throws: FlyrailValueError(overlap)) {
        _ = try preview(bundle: transactionBundle(), rendered: lifecycleSection(root.child("AGENTS.md")),
                        target: target)
    }
    #expect(try store.keys(prefix: "") == [])
}

@Test
func aStoreBehindADanglingSymlinkIntoASkillContainerOverlapsIt() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let container = try root.child("skills")
    #expect(symlink(container.displayString, root.url.appending(path: "link").path) == 0)
    let store = try DirectoryStore(root.child("link/state"))
    let target = try InstallationTarget(store: store)
    let content = try TreeContent(entries: [BundleEntry(path: "SKILL.md", data: Data("skill".utf8))])
    let rendered = try RenderedBundle(artifacts: [
        RenderedArtifact(id: "review", family: .skills, destination: container, content: .tree(content),
                         subtree: "review"),
    ])
    #expect(throws: FlyrailValueError(overlap)) {
        _ = try preview(bundle: lifecycleSkills(), rendered: rendered, target: target)
    }
    #expect(try store.keys(prefix: "") == [])
    #expect(try NativeFilesystem.metadata(container) == nil)
}

@Test
func aStoreInsideTheBundleSourceIsRefused() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let source = try root.child("source")
    let store = try DirectoryStore(appendPath(source, "state"))
    let bundle = try FlyrailBundle(
        identity: BundleIdentity(id: "team", version: "1"),
        artifacts: [.instruction(InstructionArtifact(id: "guide", text: "text"))],
        assets: [], dependencies: [], sourceRoots: [source],
    )
    #expect(throws: FlyrailValueError("state store overlaps bundle source")) {
        _ = try preview(bundle: bundle, rendered: lifecycleSection(root.child("AGENTS.md")),
                        target: InstallationTarget(store: store))
    }
    #expect(try store.keys(prefix: "") == [])
}

@Test
func lifecycleOperationsRejectAStoreAtTheDestinationWithoutMutation() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let destination = try root.child("payload")
    let store = try DirectoryStore(destination)
    let target = try InstallationTarget(store: store)
    let ref = try NativeAuthority.reference(destination, kind: .document)
    let index = try Index(bundleID: "team", contextDigest: target.contextDigest, residual: [ref])
    try StateRecords.write(store, key: InstallationIndex.key(bundleID: "team", target: target), value: index,
                           expected: nil)
    let before = try NativeObservation.snapshot(root.path)
    let results = try [
        recoverInstallation(bundleID: "team", target: target),
        remove(bundleID: "team", target: target),
        sync(bundle: transactionBundle(), rendered: lifecycleSection(root.child("other")), target: target),
    ]
    for result in results {
        #expect(result.status == .failed)
        #expect(result.resources.isEmpty)
        #expect(result.error == TargetError(code: .invalidState, message: overlap))
        #expect(try NativeObservation.snapshot(root.path) == before)
        #expect(try store.read(StateKey.resource(ref.destination, .authority)) == nil)
    }
}

@Test
func previewApprovesParentsCreatedForTheStore() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let store = try DirectoryStore(root.child("project/state"))
    let path = try root.child("project/AGENTS.md")
    let proposed = try preview(bundle: transactionBundle(), rendered: lifecycleSection(path),
                               target: InstallationTarget(store: store))
    #expect(proposed.applicable)
    let result = applyPreview(proposed)
    #expect(result.status == .applied)
    #expect(try String(contentsOfFile: path.displayString, encoding: .utf8).contains("generated"))
}

@Test
func holdingTheIndexLockMakesSyncBusyBeforeAnyAuthorityRecord() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let target = try root.target()
    let competing = try DirectoryStore(ResourcePath(root.stateURL.path))
    let held = try competing.lock(InstallationIndex.key(bundleID: "team", target: target), timeout: .zero)
    defer { held.release() }
    let result = try sync(bundle: transactionBundle(), rendered: lifecycleSection(path), target: target)
    #expect(result.error?.code == .busy)
    #expect(try root.store.keys(prefix: "resource/") == [])
    #expect(try NativeFilesystem.metadata(path) == nil)
}

@Test
func separateStoresCannotShareWholeFileOwnership() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let first = try root.target()
    let second = try InstallationTarget(store: DirectoryStore(root.child("second-store")))
    #expect(try sync(bundle: transactionBundle(), rendered: transactionRendered(path), target: first)
        .status == .applied)
    let before = try NativeObservation.snapshot(path)
    let mode = fileMode(path)
    let result = try sync(bundle: transactionBundle(), rendered: transactionRendered(path, "other\n"),
                          target: second)
    #expect(result.error?.code == .conflict)
    #expect(try NativeObservation.snapshot(path) == before)
    #expect(fileMode(path) == mode)
}

@Test
func sectionsFromTwoStoresCoexistAndRemovalKeepsTheOthersBlock() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("AGENTS.md")
    try NativeTransactionIO.createFile(path, data: Data("# Foreign\n".utf8))
    let first = try root.target()
    let second = try InstallationTarget(store: DirectoryStore(root.child("second-store")))
    let alpha = try section(path, marker: "alpha", text: "from first")
    let beta = try section(path, marker: "beta", text: "from second")
    #expect(try sync(bundle: transactionBundle(), rendered: alpha, target: first).status == .applied)
    #expect(try sync(bundle: transactionBundle(), rendered: beta, target: second).status == .applied)
    let both = try String(contentsOfFile: path.displayString, encoding: .utf8)
    #expect(both.hasPrefix("# Foreign\n"))
    #expect(both.contains("from first") && both.contains("from second"))
    #expect(remove(bundleID: "team", target: first).status == .applied)
    let remaining = try String(contentsOfFile: path.displayString, encoding: .utf8)
    #expect(remaining.hasPrefix("# Foreign\n"))
    #expect(remaining.contains("from second"))
    #expect(!remaining.contains("from first"))
    #expect(try inspectInstallation(bundleID: "team", target: second).isCurrent)
}
