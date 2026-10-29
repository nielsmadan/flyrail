import CryptoKit
@testable import Flyrail
import FlyrailInteropSupport
import Foundation
import Testing

@Test(.enabled(if: repositoryCheckout))
func valueDifferentialMatchesPythonReference() throws {
    let corpus = try repositoryFile("spec/fixtures/differential.tsv")
    let translations = try repositoryFile("spec/fixtures/translations.json")
    let lifecycleRoot = try NativeTestRoot()
    defer { try? lifecycleRoot.cleanup() }
    let reference = try pythonReference(corpus: corpus, translations: translations, lifecycleRoot: lifecycleRoot.url)
    let swift = try swiftReference(corpus: corpus, translations: translations, lifecycleRoot: lifecycleRoot.url)
    try requireLifecycleCoverage(reference, corpus: corpus)
    try requireLifecycleCoverage(swift, corpus: corpus)
    let expectedLines = reference.split(separator: "\n", omittingEmptySubsequences: false)
    let actualLines = swift.split(separator: "\n", omittingEmptySubsequences: false)
    #expect(actualLines.count == expectedLines.count)
    for (actual, expected) in zip(actualLines, expectedLines) {
        let actualColumns = actual.split(separator: "\t", omittingEmptySubsequences: false)
        let expectedColumns = expected.split(separator: "\t", omittingEmptySubsequences: false)
        #expect(actualColumns.count == expectedColumns.count)
        for (actualColumn, expectedColumn) in zip(actualColumns, expectedColumns) {
            #expect(actualColumn == expectedColumn)
        }
    }
}

private func pythonReference(corpus: URL, translations: URL, lifecycleRoot: URL) throws -> String {
    _ = try repositoryFile("python/src/flyrail")
    let process = Process()
    let output = Pipe()
    process.executableURL = try repositoryFile("python/.venv/bin/python3")
    process.arguments = try [
        "-B",
        repositoryFile("swift/scripts/reference_value_driver.py").path,
        corpus.path,
        translations.path,
        lifecycleRoot.path,
    ]
    process.standardOutput = output
    process.standardError = output
    try process.run()
    let text = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        throw FlyrailValueError(text.trimmingCharacters(in: .whitespacesAndNewlines))
    }
    return text
}

private func swiftReference(corpus: URL, translations: URL, lifecycleRoot: URL) throws -> String {
    let rows = try InteropDifferential.Corpus(text: String(contentsOf: corpus, encoding: .utf8))
    let records = try InteropDifferential.valueRecords(rows.values) + InteropDifferential.bundleRecords(rows.bundles) +
        [translationDifferential(translations)] +
        InteropDifferential.lifecycleRecords(rows.lifecycles, root: lifecycleRoot) +
        InteropDifferential.lifecycleEdgeRecords(root: lifecycleRoot)
    return records.joined(separator: "\n") + "\n"
}

private func translationDifferential(_ fixture: URL) throws -> String {
    let data = try Data(contentsOf: fixture)
    let document = try JSONSerialization.jsonObject(with: data) as? [String: Any]
    guard let cases = document?["cases"] as? [[String: Any]] else {
        throw FlyrailValueError("invalid translation fixture")
    }
    let caseRecords = try cases.flatMap { item -> [String] in
        guard let name = item["name"] as? String,
              let source = item["artifact"] as? [String: Any],
              let agentName = item["agent"] as? String,
              let agent = Agent(rawValue: agentName),
              let surfaceName = item["surface"] as? String,
              let surface = Surface(rawValue: surfaceName),
              let scope = item["scope"] as? String
        else {
            throw FlyrailValueError("invalid translation case")
        }
        let manifest: [String: Any] = [
            "schema_version": 2,
            "id": "translation-demo",
            "version": "same",
            "artifacts": [source],
        ]
        let bundle = try FlyrailBundle(
            manifest: JSONSerialization.data(withJSONObject: manifest, options: [.sortedKeys]),
            entries: [],
        )
        let root = try ResourcePath("/fixture/project")
        let target = if scope == "project" {
            try Target.project(agent: agent, root: root)
        } else {
            try Target.user(agent: agent, home: root)
        }
        var context = try RenderContext(target: target, platform: .linux, surface: surface)
        if let path = item["execution_root"] as? String {
            context = try context.executionRoot(appendPath(root, path))
        }
        if let path = item["mcp_path"] as? String {
            context = try context.mcpPath(appendPath(root, path))
        }
        let rendered = try render(bundle, for: context)
        guard let sourceID = source["id"] as? String else {
            throw FlyrailValueError("invalid translation artifact")
        }
        let artifact = rendered.artifacts.first { $0.id == sourceID }
        let encoded = try artifact.map { try $0.modelValue().semanticBytes().nativeHex } ?? "-"
        let notices = rendered.notices.map { "\($0.kind.rawValue):\($0.code)" }.joined(separator: ",")
        let support = capabilities(for: context).sorted { $0.family.rawValue < $1.family.rawValue }
        let descriptor = support.map {
            [
                $0.family.rawValue,
                $0.portable ? "1" : "0",
                $0.native ? "1" : "0",
                $0.limitations.joined(separator: ","),
                $0.prerequisites.joined(separator: ","),
            ].joined(separator: "|")
        }.joined(separator: ";")
        return [
            "translation\t\(name)\t\(rendered.supported ? 1 : 0)\t\(encoded)\t\(notices)",
            "rendering\t\(name)\t\(rendered.renderDigest)",
            "capabilities\t\(name)\t\(descriptor)",
        ]
    }
    return try (caseRecords + expandedTranslationDifferential(document: document ?? [:]))
        .joined(separator: "\n")
}

private func expandedTranslationDifferential(document: [String: Any]) throws -> [String] {
    let root = try ResourcePath("/fixture/project")
    let assetManifest = try requiredDictionary(document["asset_manifest"])
    let assetBundle = try fixtureBundle(
        manifest: assetManifest,
        entries: [
            BundleEntry(path: "runtime/server.js", data: Data("console.log(\"MCP\");".utf8)),
            BundleEntry(path: "runtime/data"),
        ],
    )
    let assetContext = try RenderContext(
        target: Target.project(agent: .codex, root: root),
        platform: .linux,
    ).assetRoot(appendPath(root, "assets"))
    var records = try [renderRecord("asset-manifest", render(assetBundle, for: assetContext))]
    records += try combinedRenderRecords(document: document, root: root)
    records += try hookRenderRecords(root: root)
    try records.append(relocationRenderRecord(root: root))

    let dependencyBundle = try fixtureBundle(manifest: [
        "schema_version": 2,
        "id": "audience-dependency",
        "version": "same",
        "artifacts": [
            ["kind": "instruction", "id": "guide", "text": "Use it."],
            [
                "kind": "native",
                "id": "native",
                "family": "mcp",
                "audience": ["agent": "claude", "scope": "project"],
                "destination": "native.json",
                "content": [
                    "kind": "structured",
                    "format": "json",
                    "selector": ["native"],
                    "value": ["enabled": true],
                ],
            ],
        ],
        "dependencies": [["dependent": "guide", "required": "native"]],
    ])
    let codex = try RenderContext(
        target: Target.project(agent: .codex, root: root),
        platform: .linux,
    )
    try records.append(renderRecord(
        "dependency-audience",
        render(dependencyBundle, for: codex),
    ))

    let droidBundle = try fixtureBundle(manifest: [
        "schema_version": 2,
        "id": "mixed-droid",
        "version": "same",
        "artifacts": [
            [
                "kind": "native",
                "id": "claude-only",
                "family": "instructions",
                "audience": ["agent": "claude", "scope": "project"],
                "destination": "CLAUDE.local.md",
                "content": [
                    "kind": "structured",
                    "format": "json",
                    "selector": ["claude"],
                    "value": ["enabled": true],
                ],
            ],
            ["kind": "instruction", "id": "guide", "text": "Portable"],
        ],
    ])
    let claude = try RenderContext(
        target: Target.project(agent: .claude, root: root),
        platform: .linux,
    )
    let droid = try RenderContext(
        target: Target.project(agent: .droid, root: root),
        platform: .linux,
    )
    try records.append(renderRecord(
        "mixed-droid",
        renderMany(droidBundle, for: [claude, droid]),
    ))
    return records
}

private func relocationRenderRecord(root: ResourcePath) throws -> String {
    let bundle = try fixtureBundle(manifest: [
        "schema_version": 2,
        "id": "relocation",
        "version": "same",
        "artifacts": [
            ["kind": "instruction", "id": "guide", "text": "Relocated."],
            [
                "kind": "mcp",
                "id": "tools",
                "name": "tools",
                "transport": [
                    "kind": "stdio",
                    "command": ["argv": ["server"]],
                ],
            ],
        ],
    ])
    let context = try RenderContext(
        target: Target.user(
            agent: .opencode,
            home: root,
            environment: [
                "XDG_CONFIG_HOME": "~/configuration",
                "OPENCODE_CONFIG": "~/selected/opencode.jsonc",
            ],
        ),
        platform: .linux,
    )
    return try renderRecord("relocation-opencode", render(bundle, for: context))
}

private func combinedRenderRecords(
    document: [String: Any],
    root: ResourcePath,
) throws -> [String] {
    try requiredDictionaries(document["combined_cases"]).map { item in
        let artifacts = try requiredDictionaries(item["artifacts"])
        let bundle = try fixtureBundle(manifest: [
            "schema_version": 2,
            "id": "combined",
            "version": "same",
            "artifacts": artifacts,
        ])
        let contexts = try requiredDictionaries(item["contexts"]).map { value in
            guard let name = value["agent"] as? String, let agent = Agent(rawValue: name) else {
                throw FlyrailValueError("invalid combined context")
            }
            return try RenderContext(
                target: Target.project(agent: agent, root: root),
                platform: .linux,
            )
        }
        guard let name = item["name"] as? String else {
            throw FlyrailValueError("invalid combined name")
        }
        return try renderRecord("combined-\(name)", renderMany(bundle, for: contexts))
    }
}

private func hookRenderRecords(root: ResourcePath) throws -> [String] {
    let hookBundle = try fixtureBundle(manifest: [
        "schema_version": 2,
        "id": "hook-differential",
        "version": "same",
        "artifacts": [[
            "kind": "hook",
            "id": "start",
            "protocol_version": 1,
            "event": "session-start",
            "command": ["argv": ["/bin/echo", "space ü😀"]],
            "timeout_ms": 1000,
            "outcomes": ["continue"],
            "tools": [],
        ]],
    ])
    let runtime = try HookRuntime(node: ResourcePath("/opt/node/bin/node"), shell: .posix)
    return try [Agent.claude, .codex, .opencode, .pi, .cursor, .copilot].map { agent in
        let context = try RenderContext(
            target: Target.project(agent: agent, root: root),
            platform: .linux,
        ).hookRuntime(runtime)
        return try renderRecord("hook-\(agent.rawValue)", render(hookBundle, for: context))
    }
}

private func fixtureBundle(
    manifest: [String: Any],
    entries: [BundleEntry] = [],
) throws -> FlyrailBundle {
    try FlyrailBundle(
        manifest: JSONSerialization.data(withJSONObject: manifest, options: [.sortedKeys]),
        entries: entries,
    )
}

private func renderRecord(_ name: String, _ rendered: RenderedBundle) throws -> String {
    let artifacts = try rendered.artifacts.map {
        try Data(SHA256.hash(data: $0.modelValue().semanticBytes())).nativeHex
    }.joined(separator: ",")
    let dependencies = try rendered.dependencies.map {
        try Data(SHA256.hash(data: $0.modelValue().semanticBytes())).nativeHex
    }.joined(separator: ",")
    let notices = rendered.notices.map {
        [
            Data($0.artifactID.utf8).nativeHex,
            $0.kind.rawValue,
            Data($0.code.utf8).nativeHex,
            Data($0.message.utf8).nativeHex,
        ].joined(separator: ":")
    }.joined(separator: ",")
    let routes = rendered.routingContext.map {
        "\(Data($0.0.utf8).nativeHex):\(Data($0.1.utf8).nativeHex)"
    }.joined(separator: ",")
    return [
        "render-record",
        name,
        rendered.supported ? "1" : "0",
        rendered.renderDigest,
        artifacts,
        dependencies,
        notices,
        routes,
    ].joined(separator: "\t")
}

private func requiredDictionary(_ value: Any?) throws -> [String: Any] {
    guard let result = value as? [String: Any] else {
        throw FlyrailValueError("invalid translation fixture dictionary")
    }
    return result
}

private func requiredDictionaries(_ value: Any?) throws -> [[String: Any]] {
    guard let result = value as? [[String: Any]] else {
        throw FlyrailValueError("invalid translation fixture array")
    }
    return result
}
