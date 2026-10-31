@testable import Flyrail
import Foundation
import Testing

@Test(.enabled(if: repositoryCheckout), arguments: ["python", "rust"])
func skillConvenienceInstallAndInspectPreserveNameIdentityAcrossLanguages(_ language: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let source = root.url.appending(path: "source")
    try FileManager.default.createDirectory(at: source.appending(path: "review"), withIntermediateDirectories: true)
    let manifest = Data("""
    {"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":"skill","id":"review-artifact","name":"review","path":"review"}]}
    """.utf8)
    try manifest.write(to: source.appending(path: "flyrail.json"))
    try Data("skill".utf8).write(to: source.appending(path: "review/SKILL.md"))
    _ = try repositoryFile("python/src/flyrail")
    if language == "rust" {
        _ = try repositoryFile("rust/Cargo.toml")
    }
    let bundle = try FlyrailBundle(directory: ResourcePath(source.path))
    let referenceTarget = try Target.directory(root.child("reference-skills"))
    let referenceState = root.url.appending(path: "reference-state")
    let rendered = try renderSkills(bundle, target: referenceTarget)
    func reference(_ operation: String) throws -> [String: Any] {
        try skillReference(language: language, operation: operation, source: source,
                           target: referenceTarget.root, state: referenceState)
    }
    let installed = try reference("install")
    #expect(installed["status"] as? String == "applied")
    #expect(installed["render_digest"] as? String == rendered.renderDigest)
    let repeated = try reference("install")
    #expect(repeated["status"] as? String == "unchanged")
    #expect(repeated["current"] as? Bool == true)
    #expect(repeated["content_matches"] as? Bool == true)
    #expect(repeated["ids"] as? [String] == ["review"])
    #expect(repeated["droid_ids"] as? [String] == ["review"])

    let swiftTarget = try Target.directory(root.child("swift-skills"))
    #expect(install(bundle, targets: [swiftTarget], store: root.store)[0].status == .applied)
    #expect(inspect(bundle, targets: [swiftTarget], store: root.store)[0].observation.isCurrent)
    #expect(install(bundle, targets: [swiftTarget], store: root.store)[0].status == .unchanged)

    let before = try NativeObservation.snapshot(referenceTarget.root)
    let foreign = install(bundle, targets: [referenceTarget], store: root.store)[0]
    #expect(foreign.status == .failed)
    #expect(foreign.error?.code == .conflict)
    #expect(try NativeObservation.snapshot(referenceTarget.root) == before)
    #expect(!inspect(bundle, targets: [referenceTarget], store: root.store)[0].observation.isCurrent)
    let inspected = try reference("inspect")
    #expect(inspected["current"] as? Bool == true)
    #expect(inspected["content_matches"] as? Bool == true)
}

private func skillReference(language: String, operation: String, source: URL,
                            target: ResourcePath, state: URL) throws -> [String: Any]
{
    let process = Process()
    let output = Pipe()
    process.executableURL = try repositoryFile("python/.venv/bin/python3")
    process.arguments = try [
        "-B",
        repositoryFile("swift/scripts/skill_parity_reference.py").path,
        language,
        operation,
        source.path,
        target.displayString,
        state.path,
    ]
    process.standardOutput = output
    process.standardError = output
    try process.run()
    let data = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    guard process.terminationStatus == 0,
          let value = try JSONSerialization.jsonObject(with: data) as? [String: Any]
    else {
        throw FlyrailValueError(String(decoding: data, as: UTF8.self))
    }
    return value
}
