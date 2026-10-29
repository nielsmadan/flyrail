@testable import Flyrail
import Foundation
import Testing

@Test(.enabled(if: repositoryCheckout), arguments: ["python", "swift-python", "rust", "swift-rust"])
func skillConvenienceInstallAndInspectPreserveNameIdentityAcrossLanguages(_ first: String) throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let source = root.url.appending(path: "source")
    try FileManager.default.createDirectory(at: source.appending(path: "review"), withIntermediateDirectories: true)
    let manifest = Data("""
    {"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":"skill","id":"review-artifact","name":"review","path":"review"}]}
    """.utf8)
    try manifest.write(to: source.appending(path: "flyrail.json"))
    try Data("skill".utf8).write(to: source.appending(path: "review/SKILL.md"))
    let bundle = try FlyrailBundle(directory: ResourcePath(source.path))
    let target = try Target.directory(root.child("skills"))
    let rendered = try renderSkills(bundle, target: target)
    #expect(rendered.artifacts.map(\.id) == ["review"])
    let droid = try renderSkills(bundle, target: Target.project(agent: .droid, root: root.path))
    #expect(droid.notices.map(\.artifactID) == ["review"])
    let language = first.hasSuffix("rust") ? "rust" : "python"
    _ = try repositoryFile("python/src/flyrail")
    if language == "rust" { _ = try repositoryFile("rust/Cargo.toml") }
    if !first.hasPrefix("swift") {
        let installed = try skillReference(
            language: language,
            operation: "install",
            source: source,
            target: target.root
        )
        #expect(installed["status"] as? String == "applied")
        #expect(installed["render_digest"] as? String == rendered.renderDigest)
    } else {
        #expect(install(bundle, targets: [target])[0].status == .applied)
    }
    let before = try NativeObservation.snapshot(root.path)
    let observed = inspect(bundle, targets: [target])[0].observation
    #expect(observed.isCurrent)
    #expect(observed.contentMatches == true)
    #expect(install(bundle, targets: [target])[0].status == .unchanged)
    let reference = try skillReference(
        language: language,
        operation: "install",
        source: source,
        target: target.root
    )
    #expect(reference["current"] as? Bool == true)
    #expect(reference["content_matches"] as? Bool == true)
    #expect(reference["status"] as? String == "unchanged")
    #expect(reference["ids"] as? [String] == ["review"])
    #expect(reference["droid_ids"] as? [String] == ["review"])
    #expect(reference["render_digest"] as? String == rendered.renderDigest)
    #expect(try NativeObservation.snapshot(root.path) == before)
}

private func skillReference(language: String, operation: String, source: URL,
                            target: ResourcePath) throws -> [String: Any]
{
    let process = Process()
    let output = Pipe()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = try [
        "python3",
        "-B",
        repositoryFile("swift/scripts/skill_parity_reference.py").path,
        language,
        operation,
        source.path,
        target.displayString,
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
