import Foundation

public let translatableAgents: [Agent] = [.claude, .codex, .opencode, .pi, .cursor, .copilot]

public struct RenderContext: Sendable, Hashable {
    public let target: Target
    public let platform: Platform
    public let surface: Surface
    public let audience: Audience
    public let selectedExecutionRoot: ResourcePath?
    public let selectedInstructionPath: ResourcePath?
    public let selectedMCPPath: ResourcePath?
    public let selectedNativeRoot: ResourcePath?
    public let selectedAssetRoot: ResourcePath?
    public let selectedHookRuntime: HookRuntime?

    public init(target: Target, platform: Platform, surface: Surface = .cli) throws {
        guard let agent = target.agent, target.scope != .directory else {
            throw FlyrailValueError("render context requires a project or user agent Target")
        }
        _ = try validateRoute(target.root)
        _ = try validateRoute(target.home ?? target.anchor)
        self.target = target
        self.platform = platform
        self.surface = surface
        audience = try Audience(agent: agent, scope: target.scope, surface: surface, platform: platform)
        selectedExecutionRoot = nil
        selectedInstructionPath = nil
        selectedMCPPath = nil
        selectedNativeRoot = nil
        selectedAssetRoot = nil
        selectedHookRuntime = nil
    }

    fileprivate init(
        copying value: Self,
        executionRoot: ResourcePath?? = nil,
        instructionPath: ResourcePath?? = nil,
        mcpPath: ResourcePath?? = nil,
        nativeRoot: ResourcePath?? = nil,
        assetRoot: ResourcePath?? = nil,
        hookRuntime: HookRuntime?? = nil,
    ) throws {
        target = value.target
        platform = value.platform
        surface = value.surface
        audience = value.audience
        selectedExecutionRoot = try (executionRoot ?? value.selectedExecutionRoot).map(validateRoute)
        selectedInstructionPath = try (instructionPath ?? value.selectedInstructionPath).map(validateRoute)
        selectedMCPPath = try (mcpPath ?? value.selectedMCPPath).map(validateRoute)
        selectedNativeRoot = try (nativeRoot ?? value.selectedNativeRoot).map(validateRoute)
        selectedAssetRoot = try (assetRoot ?? value.selectedAssetRoot).map(validateRoute)
        selectedHookRuntime = hookRuntime ?? value.selectedHookRuntime
    }

    public func executionRoot(_ value: ResourcePath) throws -> Self {
        try Self(copying: self, executionRoot: value)
    }

    public func instructionPath(_ value: ResourcePath) throws -> Self {
        try Self(copying: self, instructionPath: value)
    }

    public func mcpPath(_ value: ResourcePath) throws -> Self {
        try Self(copying: self, mcpPath: value)
    }

    public func nativeRoot(_ value: ResourcePath) throws -> Self {
        try Self(copying: self, nativeRoot: value)
    }

    public func assetRoot(_ value: ResourcePath) throws -> Self {
        try Self(copying: self, assetRoot: value)
    }

    public func hookRuntime(_ value: HookRuntime) throws -> Self {
        try Self(copying: self, hookRuntime: value)
    }

    public var root: ResourcePath {
        target.home ?? target.anchor
    }

    public var skills: ResourcePath {
        get throws {
            if surface == .vscode, target.scope == .user {
                return try appendPath(root, ".copilot/skills")
            }
            return target.root
        }
    }

    public var routingContext: [(String, String)] {
        get throws {
            var result = try [
                ("agent", audience.agent.rawValue),
                ("scope", target.scope.rawValue),
                ("surface", surface.rawValue),
                ("platform", platform.rawValue),
                ("root", root.displayString),
                ("skills", skills.displayString),
                ("execution", (selectedExecutionRoot ?? root).displayString),
                ("instructions", selectedInstructionPath?.displayString ?? ""),
                ("mcp", selectedMCPPath?.displayString ?? ""),
                ("native", (selectedNativeRoot ?? root).displayString),
                ("assets", (selectedAssetRoot ?? appendPath(root, ".flyrail-assets")).displayString),
                ("hook-node", selectedHookRuntime?.node?.displayString ?? ""),
                ("hook-shell", selectedHookRuntime?.shell?.rawValue ?? ""),
            ]
            result += target.environment.map { ("env:\($0.0)", $0.1) }
            return result.sorted { $0.0 == $1.0 ? utf8Less($0.1, $1.1) : utf8Less($0.0, $1.0) }
        }
    }

    public func installation(store: any StateStore,
                             context: [(String, String)]? = nil) throws -> InstallationTarget
    {
        let routing = try routingContext
        let identity: Set = ["agent", "scope", "surface", "root"]
        return try InstallationTarget(store: store, context: context ?? routing.filter { identity.contains($0.0) },
                                      routingContext: routing)
    }
}

public struct Capability: Sendable, Hashable {
    public let family: Family
    public let portable: Bool
    public let native: Bool
    public let limitations: [String]
    public let prerequisites: [String]

    public init(
        family: Family,
        portable: Bool,
        native: Bool,
        limitations: [String] = [],
        prerequisites: [String] = [],
    ) throws {
        guard limitations.allSatisfy({ !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }),
              prerequisites.allSatisfy({ !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty })
        else {
            throw FlyrailValueError("capability details must be nonblank strings")
        }
        self.family = family
        self.portable = portable
        self.native = native
        self.limitations = limitations
        self.prerequisites = prerequisites
    }

    fileprivate init(
        raw family: Family,
        portable: Bool,
        native: Bool,
        limitations: [String] = [],
        prerequisites: [String] = [],
    ) {
        self.family = family
        self.portable = portable
        self.native = native
        self.limitations = limitations
        self.prerequisites = prerequisites
    }

    public static func == (left: Self, right: Self) -> Bool {
        left.family == right.family &&
            left.portable == right.portable &&
            left.native == right.native &&
            exactTextsEqual(left.limitations, right.limitations) &&
            exactTextsEqual(left.prerequisites, right.prerequisites)
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(family)
        hasher.combine(portable)
        hasher.combine(native)
        hashExactTexts(limitations, into: &hasher)
        hashExactTexts(prerequisites, into: &hasher)
    }
}

public func capabilities(for context: RenderContext) -> Set<Capability> {
    guard isTranslatable(context.audience.agent) else {
        return Set(Family.allCases.map {
            Capability(raw: $0, portable: false, native: false)
        })
    }
    return Set([
        Capability(raw: .skills, portable: true, native: true),
        Capability(
            raw: .instructions,
            portable: (try? instructionDestinationOptional(for: context)) != nil,
            native: true,
            limitations: ["Discovery, overrides, byte limits and session loading remain host-controlled."],
        ),
        Capability(
            raw: .mcp,
            portable: (try? mcpDestinationOptional(for: context)) != nil,
            native: true,
            limitations: ["Transport, interpolation, environment references and cwd are checked per request."],
            prerequisites: context.audience.agent == .pi ? ["pi-mcp-adapter@2.32.1"] : [],
        ),
        Capability(
            raw: .hooks,
            portable: context.surface != .vscode && context.selectedHookRuntime != nil,
            native: true,
            limitations: ["Event/outcome and native shell support are validated for each request."],
            prerequisites: ["Node 24+; explicit executable for native hosts. See the hook protocol matrix."],
        ),
    ])
}

public func instructionDestination(for context: RenderContext) throws -> ResourcePath {
    guard let result = try instructionDestinationOptional(for: context) else {
        throw FlyrailValueError("no verified instruction destination")
    }
    return result
}

public func mcpDestination(for context: RenderContext) throws -> ResourcePath {
    guard let result = try mcpDestinationOptional(for: context) else {
        throw FlyrailValueError("no verified MCP destination")
    }
    return result
}

package func instructionDestinationOptional(for context: RenderContext) throws -> ResourcePath? {
    if let path = context.selectedInstructionPath {
        return path
    }
    guard isTranslatable(context.audience.agent) else { return nil }
    if context.target.scope == .project {
        let path: String? = switch context.audience.agent {
        case .claude: "CLAUDE.md"
        case .codex, .opencode, .pi, .cursor: "AGENTS.md"
        case .copilot: ".github/copilot-instructions.md"
        case .droid: nil
        }
        guard let path else { return nil }
        return try appendPath(context.root, path)
    }
    guard context.audience.agent != .cursor else { return nil }
    var directory = try userDirectory(context)
    if context.surface == .vscode {
        directory = try appendPath(directory, "instructions")
        return directory
    }
    let path: String? = switch context.audience.agent {
    case .claude: "CLAUDE.md"
    case .codex, .opencode, .pi: "AGENTS.md"
    case .copilot: "copilot-instructions.md"
    case .cursor, .droid: nil
    }
    guard let path else { return nil }
    return try appendPath(directory, path)
}

package func mcpDestinationOptional(for context: RenderContext) throws -> ResourcePath? {
    if let path = context.selectedMCPPath {
        return path
    }
    guard isTranslatable(context.audience.agent) else { return nil }
    if context.target.scope == .project {
        if context.surface == .vscode {
            return try appendPath(context.root, ".vscode/mcp.json")
        }
        let path: String? = switch context.audience.agent {
        case .claude, .copilot: ".mcp.json"
        case .codex: ".codex/config.toml"
        case .opencode: "opencode.json"
        case .pi: ".pi/mcp.json"
        case .cursor: ".cursor/mcp.json"
        case .droid: nil
        }
        guard let path else { return nil }
        return try appendPath(context.root, path)
    }
    if context.surface == .vscode {
        return nil
    }
    if context.audience.agent == .claude {
        return environmentValue(context, "CLAUDE_CONFIG_DIR") == nil
            ? try appendPath(context.root, ".claude.json")
            : nil
    }
    if context.audience.agent == .opencode,
       let configured = environmentValue(context, "OPENCODE_CONFIG")
    {
        return try relocatedPath(configured, home: context.root)
    }
    let path: String? = switch context.audience.agent {
    case .codex: "config.toml"
    case .opencode: "opencode.json"
    case .pi, .cursor: "mcp.json"
    case .copilot: "mcp-config.json"
    case .claude, .droid: nil
    }
    guard let path else { return nil }
    return try appendPath(userDirectory(context), path)
}

package func userDirectory(_ context: RenderContext) throws -> ResourcePath {
    let agent = context.audience.agent
    let selection: (String, String) = switch agent {
    case .claude: ("CLAUDE_CONFIG_DIR", ".claude")
    case .codex: ("CODEX_HOME", ".codex")
    case .opencode: ("XDG_CONFIG_HOME", ".config")
    case .pi: ("PI_CODING_AGENT_DIR", ".pi/agent")
    case .cursor: ("", ".cursor")
    case .copilot: ("COPILOT_HOME", ".copilot")
    case .droid: throw FlyrailValueError(untranslatableMessage(agent))
    }
    let variable = context.surface == .vscode ? "" : selection.0
    var root = if let value = environmentValue(context, variable) {
        try relocatedPath(value, home: context.root)
    } else {
        try appendPath(context.root, selection.1)
    }
    if agent == .opencode {
        root = try appendPath(root, "opencode")
    }
    return root
}

package func isTranslatable(_ agent: Agent) -> Bool {
    translatableAgents.contains(agent)
}

package func untranslatableMessage(_ agent: Agent) -> String {
    "Flyrail detects \(agent.rawValue) but has no verified configuration translation."
}

private func environmentValue(_ context: RenderContext, _ key: String) -> String? {
    context.target.environment.first { $0.0 == key && !$0.1.isEmpty }?.1
}

extension Family: CaseIterable {
    public static var allCases: [Family] {
        [.skills, .instructions, .mcp, .hooks]
    }
}

private func exactTextsEqual(_ left: [String], _ right: [String]) -> Bool {
    left.count == right.count && zip(left, right).allSatisfy(exactTextEqual)
}

private func hashExactTexts(_ values: [String], into hasher: inout Hasher) {
    hasher.combine(values.count)
    values.forEach { hashExactText($0, into: &hasher) }
}
