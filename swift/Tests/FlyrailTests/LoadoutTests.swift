@testable import Flyrail
import Foundation
import Testing

private struct LoadoutVector: Decodable {
    let name: String
    let files: [String: String]
    let env: [String: String]?
    let status: String
    let code: String?
    let profile: String?
    let chain: [String]?
    let sources: [String]?
    let sourcePaths: [String]?
    let harnesses: [String]?
    let instructionPath: String?
    let instructionKey: [String]?
}

private struct LoadoutVectors: Decodable {
    let discovery: [LoadoutVector]
    let mcpPreviews: [LoadoutMCPVector]
}

private struct LoadoutMCPVector: Decodable {
    let name: String
    let files: [String: String]
    let harnesses: [String]
    let code: String?
    let envReference: Bool?
}

private func loadoutVectors() throws -> LoadoutVectors {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    let fixture = try #require(Bundle.module.url(
        forResource: "loadout",
        withExtension: "json",
        subdirectory: "Fixtures",
    ))
    return try decoder.decode(LoadoutVectors.self, from: Data(contentsOf: fixture))
}

private func sourceFile(_ root: NativeTestRoot, _ path: String, _ data: String) throws {
    let file = root.url.appending(path: path)
    try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
    try Data(data.replacingOccurrences(of: "{root}", with: root.url.path).utf8).write(to: file)
}

@Test
func loadoutUsesSharedDiscoveryVectors() throws {
    let vectors = try loadoutVectors()
    for vector in vectors.discovery {
        let root = try NativeTestRoot()
        defer { try? root.cleanup() }
        for (path, text) in vector.files {
            try sourceFile(root, path, text)
        }
        let env = vector.env?.mapValues { $0.replacingOccurrences(of: "{root}", with: root.url.path) } ?? [:]
        let found = try discoverLoadout(home: root.child("home"), environment: env)
        switch found {
        case .absent:
            #expect(vector.status == "absent", Comment(rawValue: vector.name))
        case let .invalid(_, diagnostic):
            #expect(vector.status == "invalid", Comment(rawValue: "\(vector.name): \(diagnostic)"))
            #expect(diagnostic.code == vector.code)
        case let .available(setup):
            #expect(vector.status == "available", Comment(rawValue: vector.name))
            #expect(setup.profile == vector.profile)
            let chain = try vector.chain?.map { try NativeTransactionDirectory.physicalLocation(root.child($0)) }
            #expect(setup.profileChain == chain)
            #expect(setup.sources.map(\.name) == vector.sources)
            if let paths = vector.sourcePaths {
                let expected = try paths.map { try NativeTransactionDirectory.physicalLocation(root.child($0)) }
                #expect(setup.sources.map(\.path) == expected)
            }
            #expect(setup.harnesses.map(\.agent.rawValue) == vector.harnesses)
            if let path = vector.instructionPath {
                let selector = try #require(setup.selectors.first)
                #expect(setup.selectors.count == 1)
                let expected = try NativeTransactionDirectory.physicalLocation(root.child(path))
                #expect(selector.manifest == expected)
                #expect(selector.key == vector.instructionKey)
            }
        }
    }
}

@Test
func loadoutUsesSharedMCPPreviewVectors() throws {
    for vector in try loadoutVectors().mcpPreviews {
        let root = try NativeTestRoot()
        defer { try? root.cleanup() }
        for (path, text) in vector.files {
            try sourceFile(root, path, text)
        }
        guard case let .available(setup) = try discoverLoadout(home: root.child("home")) else {
            Issue.record(Comment(rawValue: "Expected a usable setup for \(vector.name)"))
            continue
        }
        let selected = try setup.select(
            source: "local",
            harnesses: vector.harnesses.map { try #require(Agent(rawValue: $0)) },
        )
        let env = try vector.envReference == true ? [EnvironmentEntry(
            name: "TOKEN",
            value: .reference(EnvRef("REVIEW_TOKEN")),
        )] : []
        let command = try Command(argv: [.text("review")], env: env)
        let artifact = try MCPArtifact(id: "server", name: "review", transport: .stdio(command))
        let bundle = try FlyrailBundle(
            identity: BundleIdentity(id: "toolbox", version: "1"),
            artifacts: [.mcp(artifact)],
            assets: [],
            dependencies: [],
        )
        let plan = try previewLoadout(
            bundle: bundle,
            destination: selected,
            store: root.store,
            options: LoadoutOptions(platform: .macOS),
        )
        #expect(plan.diagnostic?.code == vector.code, Comment(rawValue: vector.name))
        #expect(plan.applicable == (vector.code == nil), Comment(rawValue: vector.name))
    }
}

private func loadoutBundle(version: String, instruction: String) throws -> FlyrailBundle {
    try FlyrailBundle(
        identity: BundleIdentity(id: "loadout-probe", version: version),
        artifacts: [
            .instruction(InstructionArtifact(id: "guide", text: instruction)),
            .mcp(MCPArtifact(id: "server", name: "review-http", transport: .http(
                HttpTransport(url: "https://example.test/mcp", bearerToken: EnvRef("REVIEW_TOKEN")),
            ))),
            .skill(SkillArtifact(id: "skill", name: "review", content: TreeContent(entries: [
                BundleEntry(path: "SKILL.md", data: Data("---\nname: review\ndescription: Review\n---\n".utf8)),
                BundleEntry(path: "assets/input.bin", data: Data([0, 255])),
            ]))),
        ], assets: [], dependencies: [],
    )
}

@Test func loadoutOwnsSourceEditsAndRejectsStaleRegistration() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let registration = "home/.config/loadout/config.toml"
    let source = "source/loadout.toml"
    let valid = "source = '{root}/source'\n"
    let manifest = "# keep me\n[[source]]\nname='local'\npath='.'\n[claude]\ninstructions=[]\n[codex]\ninstructions=[]\n"
    try sourceFile(root, registration, valid)
    try sourceFile(root, source, manifest)
    let home = try root.child("home")
    func plan(_ bundle: FlyrailBundle, _ agents: [Agent]) throws -> LoadoutPreview {
        guard case let .available(setup) = try discoverLoadout(home: home) else {
            throw FlyrailValueError("Expected a synthetic Loadout setup")
        }
        return try previewLoadout(bundle: bundle, destination: setup.select(source: "local", harnesses: agents),
                                  store: root.store, options: LoadoutOptions(platform: .macOS))
    }
    let bundle = try loadoutBundle(version: "1", instruction: "Read the diff.")
    let denied = try plan(bundle, [.claude])
    #expect(denied.diagnostic?.code == "shared-scope")
    let agents: [Agent] = [.claude, .codex]
    let stale = try plan(bundle, agents)
    try sourceFile(root, registration, valid + "# changed\n")
    let staleResult = applyLoadout(stale)
    #expect(staleResult.lifecycle.error?.code == .concurrentChange)
    let first = try plan(bundle, agents)
    let applied = applyLoadout(first)
    #expect(applied.lifecycle.status == .applied, Comment(rawValue: String(describing: applied.lifecycle.error)))
    #expect(applied.syncRequired)
    #expect(try Data(contentsOf: root.url.appending(path: "source/skills/review/assets/input.bin")) == Data([0, 255]))
    let table = try String(contentsOf: root.url.appending(path: "source/mcp.toml"), encoding: .utf8)
    #expect(table.contains("auth_env_var = \"REVIEW_TOKEN\""))
    #expect(try applyLoadout(plan(bundle, agents)).lifecycle.status == .unchanged)
    let upgraded = try applyLoadout(plan(loadoutBundle(version: "2", instruction: "Review the patch."), agents))
    #expect(upgraded.lifecycle.status == .applied)
    try sourceFile(root, registration, "source = '~/missing'\n")
    let observed = try inspectLoadout(bundleID: bundle.identity.id, location: first.location, store: root.store)
    #expect(observed.isCurrent)
    let removed = try applyLoadout(previewLoadoutRemoval(
        bundleID: bundle.identity.id,
        location: first.location,
        store: root.store,
    ))
    #expect(removed.lifecycle.status == .applied)
    #expect(removed.syncRequired)
    #expect(try String(contentsOf: root.url.appending(path: source), encoding: .utf8) == manifest)
}
