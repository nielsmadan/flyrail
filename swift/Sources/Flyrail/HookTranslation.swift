import CryptoKit
import Foundation

// swiftformat:disable conditionalAssignment hoistTry

package struct HookTranslation {
    let artifacts: [RenderedArtifact]
    let dependencies: [Dependency]
    let notices: [Notice]
}

package func supportAssetArtifacts(
    bundle: FlyrailBundle,
    context: RenderContext
) throws -> ([RenderedArtifact], [String: ResourcePath]) {
    let root = try context.selectedAssetRoot ?? appendPath(context.root, ".flyrail-assets")
    var artifacts: [RenderedArtifact] = []
    var destinations: [String: ResourcePath] = [:]
    for asset in bundle.assets {
        let bytes = try asset.content.modelValue().semanticBytes()
        let revision = SHA256.hash(data: bytes).hexadecimal
        let destination = try appendPath(root, "\(bundle.identity.id)/\(asset.id)/\(revision)")
        destinations[asset.id] = destination
        try artifacts.append(RenderedArtifact(
            id: asset.id,
            family: asset.family,
            destination: destination,
            content: .tree(asset.content)
        ))
    }
    return (artifacts, destinations)
}

package func translateNativeArtifact(
    _ artifact: NativeArtifact,
    context: RenderContext
) throws -> RenderedArtifact? {
    let audience = artifact.audience
    guard audience.agent == context.audience.agent,
          audience.scope == context.target.scope,
          audience.surface == context.surface,
          audience.platform == nil || audience.platform == context.platform
    else {
        return nil
    }
    return try RenderedArtifact(
        id: artifact.id,
        family: artifact.family,
        destination: appendPath(context.selectedNativeRoot ?? context.root, artifact.destination),
        content: artifact.content
    )
}

private func supportedHook(_ artifact: HookArtifact, context: RenderContext) throws {
    guard isTranslatable(context.audience.agent) else {
        throw UnsupportedTranslation(
            code: "agent-unsupported",
            message: untranslatableMessage(context.audience.agent)
        )
    }
    guard context.surface != .vscode else {
        throw UnsupportedTranslation(code: "hook-surface", message: "Portable hooks target Copilot CLI, not VS Code.")
    }
    let agent = context.audience.agent
    if artifact.event == .beforeTool,
       artifact.outcomes.contains(.modifyInput),
       agent == .codex || agent == .cursor
    {
        throw UnsupportedTranslation(
            code: "hook-modification-approval",
            message: "This host has no verified argument-replacement channel preserving normal approval flow."
        )
    }
    var outcomes: Set<HookOutcome> = [.continue]
    switch artifact.event {
    case .beforeTool:
        outcomes.formUnion([.block, .modifyInput])
        if agent == .claude || agent == .copilot { outcomes.insert(.ask) }
        if agent == .claude || agent == .codex { outcomes.insert(.context) }
    case .afterTool:
        outcomes.insert(.context)
    case .sessionStart:
        if agent != .opencode, agent != .pi { outcomes.insert(.context) }
    case .prompt:
        if agent == .opencode { outcomes.removeAll() }
        if agent == .claude || agent == .codex || agent == .cursor { outcomes.insert(.block) }
        if agent == .claude || agent == .codex { outcomes.insert(.context) }
        if agent == .pi { outcomes.insert(.modifyInput) }
    case .stop:
        if agent == .opencode { outcomes.removeAll() }
    }
    let unavailable = artifact.outcomes.filter { !outcomes.contains($0) }
    guard unavailable.isEmpty else {
        throw UnsupportedTranslation(
            code: "hook-outcome",
            message: "\(agent.rawValue) cannot preserve the declared \(artifact.event.rawValue) outcomes: " +
                unavailable.map(\.rawValue).joined(separator: ", ")
        )
    }
    if context.platform == .windows {
        let names = artifact.command.env.map { $0.name.lowercased() }
        guard Set(names).count == names.count else {
            throw UnsupportedTranslation(
                code: "windows-env-collision",
                message: "Hook environment names collide on Windows."
            )
        }
    }
}

private func shell(_ argv: [String], context: RenderContext) throws -> String {
    guard let runtime = context.selectedHookRuntime else {
        throw UnsupportedTranslation(
            code: "hook-runtime",
            message: "Native hooks require an explicit Node 24+ executable."
        )
    }
    if context.platform == .windows {
        guard context.audience.agent != .cursor else {
            throw UnsupportedTranslation(
                code: "cursor-windows-shell",
                message: "Cursor Windows hook shell selection is unverified; use native content."
            )
        }
        guard runtime.shell == .cmd else {
            throw UnsupportedTranslation(
                code: "hook-shell",
                message: "Windows Codex requires a caller-confirmed cmd.exe /C hook shell."
            )
        }
        guard !argv.contains(where: { value in
            value.contains { "%!&|<>^()\"\r\n".contains($0) }
        }) else {
            throw UnsupportedTranslation(
                code: "hook-shell-literal",
                message: "These native launcher paths cannot be represented by the cmd hook bridge."
            )
        }
        return argv.map { "\"\($0)\"" }.joined(separator: " ")
    }
    guard runtime.shell == .posix else {
        throw UnsupportedTranslation(
            code: "hook-shell",
            message: "This native hook requires a caller-confirmed POSIX-compatible hook shell."
        )
    }
    return argv.map(posixQuote).joined(separator: " ")
}

private func posixQuote(_ value: String) -> String {
    let safe = Set("_@%+=:,./-".utf8)
    if !value.isEmpty, value.utf8.allSatisfy({
        asciiLetter($0) || asciiNumber($0) || safe.contains($0)
    }) {
        return value
    }
    return "'\(value.replacingOccurrences(of: "'", with: "'\"'\"'"))'"
}

private func hookEventName(_ agent: Agent, _ event: HookEvent) -> String {
    switch (agent, event) {
    case (.claude, .sessionStart), (.codex, .sessionStart): "SessionStart"
    case (.claude, .beforeTool), (.codex, .beforeTool): "PreToolUse"
    case (.claude, .afterTool), (.codex, .afterTool): "PostToolUse"
    case (.claude, .prompt), (.codex, .prompt): "UserPromptSubmit"
    case (.claude, .stop), (.codex, .stop): "Stop"
    case (.cursor, .sessionStart), (.copilot, .sessionStart): "sessionStart"
    case (.cursor, .beforeTool), (.copilot, .beforeTool): "preToolUse"
    case (.cursor, .afterTool), (.copilot, .afterTool): "postToolUse"
    case (.cursor, .prompt): "beforeSubmitPrompt"
    case (.copilot, .prompt): "userPromptSubmitted"
    case (.cursor, .stop): "stop"
    case (.copilot, .stop): "agentStop"
    default: ""
    }
}

private func generatedID(_ values: [String]) -> String {
    let json = "[" + values.map(asciiJSONString).joined(separator: ",") + "]"
    return SHA256.hash(data: Data(json.utf8)).hexadecimal
}

private func fileURL(_ path: ResourcePath, platform: Platform) throws -> String {
    var text = path.displayString
    if platform == .windows { text = text.replacingOccurrences(of: "\\", with: "/") }
    let prefix: String
    if platform == .windows, text.hasPrefix("//") {
        prefix = "file:"
    } else if platform == .windows,
              text.utf8.count > 1,
              text.utf8.dropFirst().first == 58,
              text.utf8.first.map(asciiLetter) == true
    {
        prefix = "file:///"
    } else if text.hasPrefix("/") {
        prefix = "file://"
    } else {
        throw FlyrailValueError("hook import path must be absolute")
    }
    let allowed = Set("/-._~".utf8)
    let encoded = text.utf8.enumerated().map { index, byte -> String in
        let driveColon = platform == .windows && index == 1 && byte == 58
        if asciiLetter(byte) || asciiNumber(byte) || allowed.contains(byte) || driveColon {
            return String(UnicodeScalar(byte))
        }
        return String(format: "%%%02X", byte)
    }.joined()
    return prefix + encoded
}

// swiftlint:disable:next cyclomatic_complexity function_body_length
package func hookArtifacts(
    bundle: FlyrailBundle,
    context: RenderContext,
    assets: [String: ResourcePath]
) throws -> HookTranslation {
    let hooks = bundle.artifacts.compactMap { artifact -> HookArtifact? in
        if case let .hook(hook) = artifact { hook } else { nil }
    }
    guard !hooks.isEmpty else { return HookTranslation(artifacts: [], dependencies: [], notices: []) }
    for hook in hooks {
        try supportedHook(hook, context: context)
    }
    for event in HookEvent.allCases {
        let selected = hooks.filter { $0.event == event }
        guard selected.count <= hookHandlerLimit,
              selected.map(\.timeoutMilliseconds).reduce(0, +) <= hookEventTimeoutLimitMilliseconds
        else {
            throw UnsupportedTranslation(
                code: "hook-event-budget",
                message: "Each event permits at most 64 handlers and 300000 ms total declared timeout."
            )
        }
    }
    let agent = context.audience.agent
    let embedded = agent == .opencode || agent == .pi
    guard let runtime = context.selectedHookRuntime, embedded || runtime.node != nil else {
        throw UnsupportedTranslation(
            code: "hook-runtime",
            message: "Native hooks require an explicit Node 24+ executable."
        )
    }
    let nativeDirectory: ResourcePath
    if context.target.scope == .user {
        nativeDirectory = try userDirectory(context)
    } else {
        let suffix = switch agent {
        case .claude: ".claude"
        case .codex: ".codex"
        case .cursor: ".cursor"
        case .copilot: ".github"
        case .opencode: ".opencode"
        case .pi: ".pi"
        case .droid:
            throw UnsupportedTranslation(code: "agent-unsupported", message: untranslatableMessage(agent))
        }
        nativeDirectory = try appendPath(context.root, suffix)
    }
    let base = try appendPath(
        context.selectedAssetRoot ?? appendPath(context.root, ".flyrail-assets"),
        "\(bundle.identity.id)/.hooks/\(agent.rawValue)"
    )
    guard !base.displayString.split(separator: "/").contains(where: {
        $0.lowercased() == "node_modules"
    }) else {
        throw UnsupportedTranslation(
            code: "hook-node-modules",
            message: "Node cannot load stripped TypeScript from a node_modules asset root."
        )
    }
    let handlers = try hooks.map { hook in
        let argv = try hook.command.argv.map { try translatedAsset($0, assets: assets) }
        let cwd: ResourcePath = switch hook.command.cwd {
        case let .some(.text(relative)):
            try appendPath(context.selectedExecutionRoot ?? context.root, relative)
        case let .some(value):
            try ResourcePath(translatedAsset(value, assets: assets))
        case .none:
            context.selectedExecutionRoot ?? context.root
        }
        return try HookHandler(
            id: hook.id,
            event: hook.event,
            argv: argv,
            environment: hook.command.env.map { entry in
                let value: HookEnvironment = switch entry.value {
                case let .text(text): .text(text)
                case let .reference(reference): .reference(reference.name)
                }
                return HookEnvironmentEntry(name: entry.name, value: value)
            },
            cwd: cwd,
            timeoutMilliseconds: hook.timeoutMilliseconds,
            outcomes: hook.outcomes,
            tools: hook.tools
        )
    }
    let config = try HookConfig(host: agent.rawValue, handlers: handlers)
    let configJSON = asciiJSON(Data(config.data))
    var entries = runtime.resources
    let configSource = "import type { Config } from './runner.mts';\n" +
        "const config: Config = JSON.parse(\(asciiJSONString(configJSON)));\n" +
        "export default config;\n"
    try entries.append(BundleEntry(path: "config.mts", data: Data(configSource.utf8)))
    var revisionBytes = Data()
    entries.compactMap(\.data).forEach { revisionBytes.append($0) }
    let revision = try appendPath(base, "revisions/\(SHA256.hash(data: revisionBytes).hexadecimal)")
    let runtimeID = generatedID([bundle.identity.id, agent.rawValue, "runtime"])
    let launcherID = generatedID([bundle.identity.id, agent.rawValue, "launcher"])
    let versionID = generatedID([bundle.identity.id, agent.rawValue, "version"])
    let authored = Set(bundle.artifacts.map(\.id) + bundle.assets.map(\.id))
    guard ![runtimeID, launcherID, versionID].contains(where: authored.contains) else {
        throw UnsupportedTranslation(
            code: "hook-generated-id",
            message: "An authored identifier collides with a generated hook identifier."
        )
    }
    var artifacts = [try RenderedArtifact(
        id: runtimeID,
        family: .hooks,
        destination: revision,
        content: .tree(TreeContent(entries: entries))
    )]
    let hookIDs = Set(hooks.map(\.id))
    var dependencies = try bundle.dependencies
        .filter { hookIDs.contains($0.dependent) }
        .map { try Dependency(dependent: runtimeID, required: $0.required) }
    let bridge = asciiJSONString(try fileURL(appendPath(revision, "bridge.mts"), platform: context.platform))
    let configURL = asciiJSONString(try fileURL(appendPath(revision, "config.mts"), platform: context.platform))
    if embedded {
        let launcher = try appendPath(
            nativeDirectory,
            "\(agent == .opencode ? "plugins" : "extensions")/flyrail-\(generatedID([bundle.identity.id])).ts"
        )
        let code = if agent == .opencode {
            "import { openCode, type OpenCodeInput } from \(bridge);\n" +
                "import config from \(configURL);\n" +
                "export default async function(host: OpenCodeInput) {\n  return openCode(config, host);\n}\n"
        } else {
            "import { registerPi, type Pi } from \(bridge);\n" +
                "import config from \(configURL);\n" +
                "export default function(pi: Pi) { registerPi(config, pi); }\n"
        }
        for hook in hooks {
            try artifacts.append(RenderedArtifact(
                id: hook.id,
                family: .hooks,
                destination: launcher,
                content: .file(FileContent(data: Data(code.utf8)))
            ))
            try dependencies.append(Dependency(dependent: hook.id, required: runtimeID))
        }
    } else {
        let launcher = try appendPath(base, "entry.mjs")
        let code = "try { const { native } = await import(\(bridge)); " +
            "const { default: config } = await import(\(configURL)); " +
            "await native(config, process.argv[2]); } catch { " +
            "process.stderr.write('flyrail-hook:bootstrap\\n'); process.stdout.write('{}\\n'); }\n"
        try artifacts.append(RenderedArtifact(
            id: launcherID,
            family: .hooks,
            destination: launcher,
            content: .file(FileContent(data: Data(code.utf8)))
        ))
        try dependencies.append(Dependency(dependent: launcherID, required: runtimeID))
        let node = runtime.node?.displayString ?? ""
        if agent == .claude, node.contains("${") || launcher.displayString.contains("${") {
            throw UnsupportedTranslation(
                code: "hook-literal-interpolation",
                message: "Claude launcher paths cannot contain host placeholder syntax."
            )
        }
        if context.platform == .windows,
           node.lowercased().hasSuffix(".cmd") || node.lowercased().hasSuffix(".bat")
        {
            throw UnsupportedTranslation(
                code: "hook-executable",
                message: "Native hooks require a real executable, not a Windows command shim."
            )
        }
        let events = Set(hooks.map(\.event)).sorted { utf8Less($0.rawValue, $1.rawValue) }
        var native: [HookEvent: SemanticValue] = [:]
        for event in events {
            let argv = [node, launcher.displayString, event.rawValue]
            let entry: SemanticValue
            switch agent {
            case .copilot:
                entry = try semanticObject([
                    ("type", .scalar(.text("command"))),
                    ("exec", .scalar(.text(node))),
                    ("args", .array(argv.dropFirst().map { .scalar(.text($0)) })),
                    ("timeoutSec", .scalar(.integer(310))),
                ])
            case .claude:
                entry = try semanticObject([("hooks", .array([semanticObject([
                    ("type", .scalar(.text("command"))),
                    ("command", .scalar(.text(node))),
                    ("args", .array(argv.dropFirst().map { .scalar(.text($0)) })),
                    ("timeout", .scalar(.integer(310))),
                ])]))])
            case .codex:
                entry = try semanticObject([("hooks", .array([semanticObject([
                    ("type", .scalar(.text("command"))),
                    ("command", .scalar(.text(try shell(argv, context: context)))),
                    ("timeout", .scalar(.integer(310))),
                ])]))])
            case .cursor:
                entry = try semanticObject([
                    ("command", .scalar(.text(try shell(argv, context: context)))),
                    ("timeout", .scalar(.integer(310))),
                ])
            default:
                throw FlyrailValueError("invalid native hook host")
            }
            native[event] = entry
        }
        for hook in hooks {
            guard let value = native[hook.event] else { throw FlyrailValueError("missing hook event") }
            let destination: ResourcePath
            let content: Content
            let schemas: [DocumentSchema]
            if agent == .copilot {
                destination = try appendPath(
                    nativeDirectory,
                    "hooks/flyrail-\(generatedID([bundle.identity.id])).json"
                )
                let body = copilotHookJSON(events: events, node: node, launcher: launcher)
                content = .file(FileContent(data: Data((body + "\n").utf8)))
                schemas = []
            } else {
                let member: Member
                if agent == .cursor,
                   case let .object(items) = value,
                   case let .scalar(.text(command))? = items.first(where: { $0.0 == "command" })?.1
                {
                    member = try Member(identity: .scalar(.text(command)), key: ["command"])
                } else {
                    member = try Member(identity: value)
                }
                let format: DocumentFormat = agent == .cursor ? .json : .jsonc
                content = .structured(try StructuredContent(
                    format: format,
                    selector: Selector(parts: [
                        .key(Key("hooks")),
                        .key(Key(hookEventName(agent, hook.event))),
                        .member(member),
                    ]),
                    value: value
                ))
                destination = try appendPath(
                    nativeDirectory,
                    agent == .claude ? "settings.json" : "hooks.json"
                )
                schemas = agent == .cursor
                    ? [try DocumentSchema(format: .json, key: "version", value: .integer(1))]
                    : []
            }
            try artifacts.append(RenderedArtifact(
                id: hook.id,
                family: .hooks,
                destination: destination,
                content: content,
                schemaRequirements: schemas
            ))
            try dependencies.append(Dependency(
                dependent: hook.id,
                required: launcherID,
                mode: .stableReference
            ))
        }
    }
    dependencies = Array(Set(dependencies)).sorted {
        $0.dependent == $1.dependent
            ? utf8Less($0.required, $1.required)
            : utf8Less($0.dependent, $1.dependent)
    }
    guard let noticeID = hooks.first?.id else { throw FlyrailValueError("hook translation requires a hook") }
    return try HookTranslation(
        artifacts: artifacts,
        dependencies: dependencies,
        notices: [
            Notice(
                artifactID: noticeID,
                kind: .prerequisite,
                message: "Requires Node 24+ for native/Pi bridges, Bun 1.4.2+ for OpenCode; host APIs documented in the hook protocol.",
                code: "hook-runtime-prerequisite"
            ),
            Notice(
                artifactID: noticeID,
                kind: .activation,
                message: "Installation only writes assets/configuration. Host trust, reload and actual activation remain host-controlled.",
                code: "hook-activation"
            ),
        ]
    )
}

private func semanticObject(_ values: [(String, SemanticValue)]) throws -> SemanticValue {
    try SemanticValue(object: values)
}

private func copilotHookJSON(events: [HookEvent], node: String, launcher: ResourcePath) -> String {
    let fields = events.map { event in
        let entry = "{\"type\":\"command\",\"exec\":\(asciiJSONString(node))," +
            "\"args\":[\(asciiJSONString(launcher.displayString)),\(asciiJSONString(event.rawValue))]," +
            "\"timeoutSec\":310}"
        return "\(asciiJSONString(hookEventName(.copilot, event))):[\(entry)]"
    }.joined(separator: ",")
    return "{\"version\":1,\"hooks\":{\(fields)}}"
}

private func asciiJSON(_ data: Data) -> String {
    let text = String(decoding: data, as: UTF8.self)
    return text.unicodeScalars.map { scalar -> String in
        if scalar.value < 128 { return String(scalar) }
        if scalar.value <= 0xFFFF { return String(format: "\\u%04x", scalar.value) }
        let value = scalar.value - 0x10000
        return String(format: "\\u%04x\\u%04x", 0xD800 + value >> 10, 0xDC00 + value & 0x3FF)
    }.joined()
}

private func asciiJSONString(_ value: String) -> String {
    var output = "\""
    for scalar in value.unicodeScalars {
        switch scalar.value {
        case 0x22: output += "\\\""
        case 0x5C: output += "\\\\"
        case 0x08: output += "\\b"
        case 0x0C: output += "\\f"
        case 0x0A: output += "\\n"
        case 0x0D: output += "\\r"
        case 0x09: output += "\\t"
        case 0 ... 0x1F: output += String(format: "\\u%04x", scalar.value)
        case 0x20 ... 0x7F: output.append(Character(String(scalar)))
        case 0x80 ... 0xFFFF: output += String(format: "\\u%04x", scalar.value)
        default:
            let value = scalar.value - 0x10000
            output += String(format: "\\u%04x\\u%04x", 0xD800 + value >> 10, 0xDC00 + value & 0x3FF)
        }
    }
    return output + "\""
}

private extension Digest {
    var hexadecimal: String {
        map { String(format: "%02x", $0) }.joined()
    }
}

private func asciiLetter(_ value: UInt8) -> Bool {
    (65 ... 90).contains(value) || (97 ... 122).contains(value)
}

private func asciiNumber(_ value: UInt8) -> Bool {
    (48 ... 57).contains(value)
}
