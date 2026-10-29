import CoreFoundation
@testable import Flyrail
import Foundation
import Testing

private func renderingRoot() throws -> ResourcePath {
    try ResourcePath("/fixture/project")
}

@Test
func destinationMatrixAndDroidRefusalMatchContract() throws {
    let expected: [(Agent, String, String)] = [
        (.claude, "CLAUDE.md", ".mcp.json"),
        (.codex, "AGENTS.md", ".codex/config.toml"),
        (.opencode, "AGENTS.md", "opencode.json"),
        (.pi, "AGENTS.md", ".pi/mcp.json"),
        (.cursor, "AGENTS.md", ".cursor/mcp.json"),
        (.copilot, ".github/copilot-instructions.md", ".mcp.json"),
    ]
    for (agent, instruction, mcp) in expected {
        let context = try RenderContext(
            target: Target.project(agent: agent, root: renderingRoot()),
            platform: .linux,
        )
        #expect(try instructionDestination(for: context).displayString == "/fixture/project/\(instruction)")
        #expect(try mcpDestination(for: context).displayString == "/fixture/project/\(mcp)")
    }
    let droid = try RenderContext(
        target: Target.project(agent: .droid, root: renderingRoot()),
        platform: .linux,
    )
    let bundle = try FlyrailBundle(
        identity: BundleIdentity(id: "demo", version: "1"),
        artifacts: [.instruction(InstructionArtifact(id: "guide", text: "Hello"))],
    )
    let rendered = try render(bundle, for: droid)
    #expect(!rendered.supported)
    #expect(rendered.artifacts.isEmpty)
    #expect(rendered.notices.map(\.code) == ["agent-unsupported"])
}

@Test
func renderRoutesRejectParentsAndPreservePlatformBytes() throws {
    let ambiguous = try Target.project(
        agent: .codex,
        root: ResourcePath("/fixture/safe/../victim"),
    )
    #expect(throws: FlyrailValueError.self) {
        try RenderContext(target: ambiguous, platform: .linux)
    }

    let rootBytes = Array("/fixture/".utf8) + [0xFF]
    let root = try ResourcePath(platformBytes: rootBytes)
    let target = try Target.project(agent: .codex, root: root)
    #expect(target.root.platformBytes == rootBytes + Array("/.agents/skills".utf8))
    let context = try RenderContext(target: target, platform: .linux)
    #expect(try instructionDestination(for: context).platformBytes == rootBytes + Array("/AGENTS.md".utf8))
    #expect(try mcpDestination(for: context).platformBytes == rootBytes + Array("/.codex/config.toml".utf8))
}

@Test
func renderingTextIdentityUsesExactUTF8() throws {
    let composed = "caf\u{E9}"
    let decomposed = "cafe\u{301}"
    let notices = try Set([
        Notice(artifactID: "guide", kind: .activation, message: composed),
        Notice(artifactID: "guide", kind: .activation, message: decomposed),
    ])
    #expect(notices.count == 2)

    let capabilities = try Set([
        Capability(family: .mcp, portable: true, native: true, limitations: [composed]),
        Capability(family: .mcp, portable: true, native: true, limitations: [decomposed]),
    ])
    #expect(capabilities.count == 2)
}

@Test
func droidSelectsNoticesByAudienceAndKeepsFullRoutes() throws {
    let root = try renderingRoot()
    let claudeOnly = try NativeArtifact(
        id: "claude-only",
        family: .instructions,
        audience: Audience(agent: .claude, scope: .project),
        destination: "CLAUDE.local.md",
        content: .file(FileContent(data: Data("Claude".utf8))),
    )
    let droidOnly = try NativeArtifact(
        id: "droid-only",
        family: .instructions,
        audience: Audience(agent: .droid, scope: .project, platform: .linux),
        destination: "DROID.md",
        content: .file(FileContent(data: Data("Droid".utf8))),
    )
    let bundle = try FlyrailBundle(
        identity: BundleIdentity(id: "droid-selection", version: "1"),
        artifacts: [
            .native(claudeOnly),
            .native(droidOnly),
            .instruction(InstructionArtifact(id: "guide", text: "Portable")),
        ],
    )
    let context = try RenderContext(
        target: Target.project(agent: .droid, root: root),
        platform: .linux,
    )
    .executionRoot(appendPath(root, "execution"))
    .assetRoot(appendPath(root, "assets"))
    let rendered = try render(bundle, for: context)
    #expect(rendered.notices.map(\.artifactID) == ["droid-only", "guide"])
    let expectedRoute = try context.routingContext
    #expect(rendered.routingContext.count == expectedRoute.count)
    #expect(zip(rendered.routingContext, expectedRoute).allSatisfy {
        $0.0.0 == $0.1.0 && $0.0.1 == $0.1.1
    })

    let claude = try RenderContext(
        target: Target.project(agent: .claude, root: root),
        platform: .linux,
    )
    let mixed = try renderMany(bundle, for: [claude, context])
    #expect(!mixed.supported)
    #expect(mixed.artifacts.isEmpty)
    #expect(mixed.dependencies.isEmpty)
    #expect(mixed.routingContext.count == 2)
}

@Test
func renderingDigestsMatchSharedVectors() throws {
    let instruction = try RenderedArtifact(
        id: "guide",
        family: .instructions,
        destination: ResourcePath("/fixture/project/AGENTS.md"),
        content: .section(SectionContent(marker: "guide", text: "Hello")),
    )
    #expect(
        try RenderedBundle(artifacts: [instruction]).renderDigest ==
            "249a091ac4a021ebbd33997ba8e3b49d34765516a1d2fdfab2f28f195329838b",
    )

    let launcher = try RenderedArtifact(
        id: "launcher",
        family: .hooks,
        destination: ResourcePath("/fixture/project/launcher"),
        content: .file(FileContent(data: Data("run\n".utf8))),
    )
    let member = try Member(identity: .scalar(.text("review")), key: ["name"])
    let hook = try RenderedArtifact(
        id: "review-hook",
        family: .hooks,
        destination: ResourcePath("/fixture/project/config.json"),
        content: .structured(StructuredContent(
            format: .json,
            selector: Selector(parts: [.key(Key("hooks")), .member(member)]),
            value: SemanticValue(object: [
                ("command", .scalar(.text("/fixture/project/launcher"))),
                ("name", .scalar(.text("review"))),
            ]),
        )),
        schemaRequirements: [
            DocumentSchema(format: .json, key: "version", value: .integer(1)),
        ],
    )
    let edge = try Dependency(
        dependent: "review-hook",
        required: "launcher",
        mode: .stableReference,
    )
    #expect(
        try RenderedBundle(artifacts: [launcher, hook], dependencies: [edge]).renderDigest ==
            "60273a81e48d315630e0112cad12b5fe9c6e5ae642d84f0370f07c1b5553702c",
    )
}

@Test
func sharedAssetManifestPreservesDependenciesAndRenderingPurity() throws {
    let data = try Data(contentsOf: #require(Bundle.module.url(
        forResource: "translations",
        withExtension: "json",
        subdirectory: "Fixtures",
    )))
    let document = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
    let manifest = try #require(document["asset_manifest"] as? [String: Any])
    let bundle = try FlyrailBundle(
        manifest: JSONSerialization.data(withJSONObject: manifest, options: [.sortedKeys]),
        entries: [
            BundleEntry(path: "runtime/server.js", data: Data("console.log(\"MCP\");".utf8)),
            BundleEntry(path: "runtime/data"),
        ],
    )
    let absentRoot = try ResourcePath(
        FileManager.default.temporaryDirectory
            .appending(path: "flyrail-render-purity-\(UUID().uuidString)")
            .path,
    )
    let context = try RenderContext(
        target: Target.project(agent: .codex, root: absentRoot),
        platform: .linux,
    ).assetRoot(appendPath(absentRoot, "assets"))
    #expect(!FileManager.default.fileExists(atPath: absentRoot.displayString))
    let first = try render(bundle, for: context)
    let second = try render(bundle, for: context)
    #expect(first == second)
    #expect(!FileManager.default.fileExists(atPath: absentRoot.displayString))
    #expect(first.artifacts.map(\.id) == ["runtime", "tools"])
    #expect(try first.dependencies == [
        Dependency(dependent: "tools", required: "runtime"),
    ])
    let runtime = try #require(first.artifacts.first { $0.id == "runtime" })
    let tools = try #require(first.artifacts.first { $0.id == "tools" })
    guard case let .structured(content) = tools.content else {
        Issue.record("expected structured MCP content")
        return
    }
    let destination = runtime.destination.displayString
    let expected = try fixtureSemanticValue([
        "command": "node",
        "args": ["\(destination)/server.js"],
        "env": [String: String](),
        "cwd": "\(destination)/data",
    ])
    #expect(content.value == expected)
}

@Test
func selectedDependenciesRespectNativeAudiencesAndRelocations() throws {
    let root = try renderingRoot()
    let native = try NativeArtifact(
        id: "native",
        family: .mcp,
        audience: Audience(agent: .claude, scope: .project),
        destination: "native.json",
        content: .file(FileContent(data: Data("{}".utf8))),
    )
    let bundle = try FlyrailBundle(
        identity: BundleIdentity(id: "dependency", version: "1"),
        artifacts: [
            .instruction(InstructionArtifact(id: "guide", text: "Use it.")),
            .native(native),
        ],
        dependencies: [Dependency(dependent: "guide", required: "native")],
    )
    let codex = try RenderContext(
        target: Target.project(agent: .codex, root: root),
        platform: .linux,
    )
    let blocked = try render(bundle, for: codex)
    #expect(blocked.notices.filter { $0.kind == .unsupported }.map(\.code) == ["dependency-audience"])
    #expect(blocked.artifacts.isEmpty)

    let user = try RenderContext(
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
    #expect(try user.skills == appendPath(root, "configuration/opencode/skills"))
    #expect(try instructionDestination(for: user) == appendPath(root, "configuration/opencode/AGENTS.md"))
    #expect(try mcpDestination(for: user) == appendPath(root, "selected/opencode.jsonc"))
    #expect(capabilities(for: user).count == 4)
}

@Test
func instructionMarkerAndMCPTranslationRemainCompatible() throws {
    let context = try RenderContext(
        target: Target.project(agent: .codex, root: renderingRoot()),
        platform: .linux,
    )
    let instruction = try translateInstructionArtifact(
        bundleID: "translation-demo",
        artifact: InstructionArtifact(id: "guide", text: "Run the checks.\n"),
        context: context,
    )
    guard case let .section(section)? = instruction.artifact?.content else {
        Issue.record("expected instruction section")
        return
    }
    #expect(section.marker == "81a6fb0b74cac727508df8661502ff555a19792d232631d282cc3bfcd196dc93")
    let assembled = try assembleTranslations(
        translations: [instruction],
        dependencies: [],
        contexts: [context],
        routingContext: context.routingContext,
    )
    #expect(assembled.renderDigest == "51ff0633093b74dd5486198a0145530c3883c3f68c30f8140805a66f536f6f8b")

    let mcp = try translateMCPArtifact(
        artifact: MCPArtifact(
            id: "tools",
            name: "tools",
            transport: .stdio(Command(
                argv: [.text("server"), .text("--label"), .text("a b;\"quoted\"")],
                env: [EnvironmentEntry(name: "MODE", value: .text("local"))],
            )),
        ),
        context: context,
    )
    guard case let .structured(content)? = mcp.artifact?.content else {
        Issue.record("expected structured MCP content")
        return
    }
    #expect(content.format == .toml)
    #expect(try content.selector.parts == [.key(Key("mcp_servers")), .key(Key("tools"))])
}

@Test(.enabled(if: repositoryCheckout))
func packagedTranslationFixtureMatchesRoot() throws {
    let packaged = try Data(contentsOf: #require(Bundle.module.url(
        forResource: "translations",
        withExtension: "json",
        subdirectory: "Fixtures",
    )))
    #expect(try packaged == Data(contentsOf: repositoryFile("spec/fixtures/translations.json")))
}

@Test
func sharedTranslationVectorsRenderExpectedContent() throws {
    let data = try Data(contentsOf: #require(Bundle.module.url(
        forResource: "translations",
        withExtension: "json",
        subdirectory: "Fixtures",
    )))
    let document = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
    let cases = try #require(document["cases"] as? [[String: Any]])
    #expect(cases.count == 35)
    for item in cases {
        let name = try #require(item["name"] as? String)
        let artifact = try #require(item["artifact"] as? [String: Any])
        let manifest: [String: Any] = [
            "schema_version": 2,
            "id": "translation-demo",
            "version": "same",
            "artifacts": [artifact],
        ]
        let bundle = try FlyrailBundle(
            manifest: JSONSerialization.data(withJSONObject: manifest, options: [.sortedKeys]),
            entries: [],
        )
        let context = try translationContext(item)
        let rendered = try render(bundle, for: context)
        let expected = try #require(item["expected"] as? [String: Any])
        if let code = expected["unsupported"] as? String {
            #expect(!rendered.supported, Comment(rawValue: name))
            #expect(rendered.notices.filter { $0.kind == .unsupported }.map(\.code) == [code])
            continue
        }
        #expect(rendered.supported, Comment(rawValue: name))
        let output = try #require(rendered.artifacts.first { $0.id == artifact["id"] as? String })
        if let destination = expected["destination"] as? String {
            #expect(try output.destination == appendPath(renderingRoot(), destination))
        } else {
            let directory = try #require(expected["directory"] as? String)
            let suffix = try #require(expected["suffix"] as? String)
            #expect(output.destination.displayString.hasPrefix("/fixture/project/\(directory)/"))
            #expect(output.destination.displayString.hasSuffix(suffix))
        }
        switch output.content {
        case let .structured(content):
            #expect(content.format.rawValue == expected["format"] as? String)
            var expectedValue = try #require(expected["value"] as? [String: Any])
            if let relative = expected["cwd_from_root"] as? String {
                expectedValue["cwd"] = "/fixture/project/\(relative)"
            }
            #expect(try content.value == fixtureSemanticValue(expectedValue))
        case let .section(content):
            #expect(content.text == expected["text"] as? String)
        case let .file(content):
            #expect(String(decoding: content.data, as: UTF8.self) == expected["text"] as? String)
        case .tree:
            Issue.record("translation fixture unexpectedly rendered a tree")
        }
    }
}

@Test
func sharedCombinedTranslationVectorsPreserveCollisionSemantics() throws {
    let data = try Data(contentsOf: #require(Bundle.module.url(
        forResource: "translations",
        withExtension: "json",
        subdirectory: "Fixtures",
    )))
    let document = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
    let cases = try #require(document["combined_cases"] as? [[String: Any]])
    #expect(cases.count == 4)
    for item in cases {
        let artifacts = try #require(item["artifacts"] as? [[String: Any]])
        let manifest: [String: Any] = [
            "schema_version": 2,
            "id": "combined",
            "version": "same",
            "artifacts": artifacts,
        ]
        let bundle = try FlyrailBundle(
            manifest: JSONSerialization.data(withJSONObject: manifest, options: [.sortedKeys]),
            entries: [],
        )
        let contexts = try #require(item["contexts"] as? [[String: Any]]).map { context in
            let agent = try fixtureAgent(#require(context["agent"] as? String))
            return try RenderContext(
                target: Target.project(agent: agent, root: renderingRoot()),
                platform: .linux,
            )
        }
        let forward = try renderMany(bundle, for: contexts)
        let reverse = try renderMany(bundle, for: contexts.reversed())
        #expect(forward == reverse)
        let expected = try #require(item["expected"] as? [String: Any])
        if let code = expected["unsupported"] as? String {
            #expect(!forward.supported)
            #expect(forward.artifacts.isEmpty)
            #expect(forward.notices.filter { $0.kind == .unsupported }.map(\.code) == [code])
        } else {
            #expect(forward.supported)
            #expect(forward.artifacts.count == 2)
            #expect(forward.routingContext.count == 2)
        }
    }
}

@Test
func hookRenderingEmitsRuntimeBridgeAndBlocksUnsupportedOutcomes() throws {
    let runtime = try HookRuntime(
        node: ResourcePath("/opt/node/bin/node"),
        shell: .posix,
    )
    let command = try Command(argv: [.text("/bin/echo")])
    let hook = try HookArtifact(
        id: "start",
        event: .sessionStart,
        command: command,
    )
    let bundle = try FlyrailBundle(
        identity: BundleIdentity(id: "hook-matrix", version: "same"),
        artifacts: [.hook(hook)],
    )
    for agent in [Agent.claude, .codex, .cursor, .copilot, .opencode, .pi] {
        let context = try RenderContext(
            target: Target.project(agent: agent, root: renderingRoot()),
            platform: .linux,
        ).hookRuntime(runtime)
        let rendered = try render(bundle, for: context)
        #expect(rendered.supported)
        #expect(rendered.notices.map(\.code) == ["hook-runtime-prerequisite", "hook-activation"])
        #expect(rendered.artifacts.contains {
            if case .tree = $0.content {
                true
            } else {
                false
            }
        })
        #expect(rendered.artifacts.first { $0.id == "start" } != nil)
    }

    let unsupported = try FlyrailBundle(
        identity: BundleIdentity(id: "hook-matrix", version: "same"),
        artifacts: [.hook(HookArtifact(
            id: "before",
            event: .beforeTool,
            command: command,
            outcomes: [.modifyInput],
            tools: ["Read"],
        ))],
    )
    let codex = try RenderContext(
        target: Target.project(agent: .codex, root: renderingRoot()),
        platform: .linux,
    ).hookRuntime(runtime)
    let blocked = try render(unsupported, for: codex)
    #expect(!blocked.supported)
    #expect(blocked.artifacts.isEmpty)
    #expect(blocked.notices.filter { $0.kind == .unsupported }.map(\.code) == ["hook-modification-approval"])
}

@Test
func renderManyUsesPythonCompatibleUnicodeRoutes() throws {
    let root = try ResourcePath("/fixture/space ü😀")
    let bundle = try FlyrailBundle(
        identity: BundleIdentity(id: "unicode", version: "same"),
        artifacts: [.instruction(InstructionArtifact(id: "guide", text: "Hello"))],
    )
    let contexts = try [Agent.claude, .codex].map {
        try RenderContext(target: Target.project(agent: $0, root: root), platform: .linux)
    }
    let rendered = try renderMany(bundle, for: contexts)
    #expect(rendered.routingContext.allSatisfy {
        $0.1.contains("\\u00fc\\ud83d\\ude00")
    })
}

private func translationContext(_ item: [String: Any]) throws -> RenderContext {
    let agent = try fixtureAgent(#require(item["agent"] as? String))
    let surface = try fixtureSurface(#require(item["surface"] as? String))
    let scope = try #require(item["scope"] as? String)
    let target = if scope == "project" {
        try Target.project(agent: agent, root: renderingRoot())
    } else {
        try Target.user(agent: agent, home: renderingRoot())
    }
    var context = try RenderContext(target: target, platform: .linux, surface: surface)
    if let path = item["execution_root"] as? String {
        context = try context.executionRoot(appendPath(renderingRoot(), path))
    }
    if let path = item["mcp_path"] as? String {
        context = try context.mcpPath(appendPath(renderingRoot(), path))
    }
    return context
}

private func fixtureAgent(_ value: String) throws -> Agent {
    guard let result = Agent(rawValue: value) else { throw FlyrailValueError("invalid fixture agent") }
    return result
}

private func fixtureSurface(_ value: String) throws -> Surface {
    guard let result = Surface(rawValue: value) else { throw FlyrailValueError("invalid fixture surface") }
    return result
}

private func fixtureSemanticValue(_ value: Any) throws -> SemanticValue {
    if value is NSNull {
        return .scalar(.null)
    }
    if let value = value as? NSNumber {
        if CFGetTypeID(value) == CFBooleanGetTypeID() {
            return .scalar(.boolean(value.boolValue))
        }
        return .scalar(.integer(value.int64Value))
    }
    if let value = value as? String {
        return .scalar(.text(value))
    }
    if let value = value as? [Any] {
        return try .array(value.map(fixtureSemanticValue))
    }
    if let value = value as? [String: Any] {
        return try SemanticValue(object: value.map { try ($0.key, fixtureSemanticValue($0.value)) })
    }
    throw FlyrailValueError("invalid fixture value")
}
