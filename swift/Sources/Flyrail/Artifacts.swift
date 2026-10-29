import Foundation

public enum Agent: String, Sendable, CaseIterable, Hashable {
    case claude
    case codex
    case opencode
    case pi
    case cursor
    case copilot
    case droid
}

public enum Platform: String, Sendable, Hashable {
    case macOS = "macos"
    case linux
    case windows
}

public enum Surface: String, Sendable, Hashable {
    case cli
    case ide
    case vscode
}

public enum TargetScope: String, Sendable, Hashable {
    case directory
    case user
    case project
}

public enum Family: String, Sendable, Hashable {
    case skills
    case instructions
    case mcp
    case hooks
}

public struct Audience: Sendable, Hashable {
    public let agent: Agent
    public let scope: TargetScope
    public let surface: Surface
    public let platform: Platform?

    public init(
        agent: Agent,
        scope: TargetScope,
        surface: Surface = .cli,
        platform: Platform? = nil,
    ) throws {
        guard scope != .directory else {
            throw FlyrailValueError("native audiences require project or user scope")
        }
        guard surface != .ide || agent == .cursor else {
            throw FlyrailValueError("IDE surface is only defined for Cursor")
        }
        guard surface != .vscode || agent == .copilot else {
            throw FlyrailValueError("VSCode surface is only defined for Copilot")
        }
        self.agent = agent
        self.scope = scope
        self.surface = surface
        self.platform = platform
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "Audience",
            fields: [
                ("agent", .scalar(.text(agent.rawValue))),
                ("platform", platform.map { .scalar(.text($0.rawValue)) } ?? .scalar(.null)),
                ("scope", .scalar(.text(scope.rawValue))),
                ("surface", .scalar(.text(surface.rawValue))),
            ],
        )
    }
}

public struct EnvRef: Sendable, Hashable {
    public let name: String

    public init(_ name: String) throws {
        let bytes = Array(name.utf8)
        guard let first = bytes.first,
              first == 95 || (65 ... 90).contains(first) || (97 ... 122).contains(first),
              bytes.dropFirst().allSatisfy({
                  $0 == 95 || (65 ... 90).contains($0) || (97 ... 122).contains($0) || (48 ... 57).contains($0)
              })
        else {
            throw FlyrailValueError("invalid environment reference")
        }
        self.name = name
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(type: "EnvRef", fields: [("name", .scalar(.text(name)))])
    }
}

public struct AssetRef: Sendable, Hashable {
    public let assetID: String
    public let path: String?

    public init(assetID: String, path: String? = nil) throws {
        try validateIdentifier(assetID, label: "referenced asset id")
        if let path {
            try validateRelativePath(path, label: "asset reference path")
        }
        self.assetID = assetID
        self.path = path
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "AssetRef",
            fields: [
                ("asset_id", .scalar(.text(assetID))),
                ("path", path.map { .scalar(.text($0)) } ?? .scalar(.null)),
            ],
        )
    }
}

public enum CommandArgument: Sendable, Hashable {
    case text(String)
    case asset(AssetRef)

    package func modelValue() throws -> SemanticValue {
        switch self {
        case let .text(value): .scalar(.text(value))
        case let .asset(value): try value.modelValue()
        }
    }
}

public enum EnvironmentValue: Sendable, Hashable {
    case text(String)
    case reference(EnvRef)

    package func modelValue() throws -> SemanticValue {
        switch self {
        case let .text(value): .scalar(.text(value))
        case let .reference(value): try value.modelValue()
        }
    }
}

public struct EnvironmentEntry: Sendable, Hashable {
    public let name: String
    public let value: EnvironmentValue

    public init(name: String, value: EnvironmentValue) {
        self.name = name
        self.value = value
    }
}

public struct Command: Sendable, Hashable {
    public let argv: [CommandArgument]
    public let env: [EnvironmentEntry]
    public let cwd: CommandArgument?

    public init(
        argv: [CommandArgument],
        env: [EnvironmentEntry] = [],
        cwd: CommandArgument? = nil,
    ) throws {
        guard !argv.isEmpty else {
            throw FlyrailValueError("argv requires a command")
        }
        for argument in argv {
            if case let .text(text) = argument, text.contains("\0") {
                throw FlyrailValueError("command arguments cannot contain NUL")
            }
        }
        if case let .text(first) = argv[0], first.isEmpty {
            throw FlyrailValueError("command cannot be empty")
        }
        var names: Set<String> = []
        for entry in env {
            _ = try EnvRef(entry.name)
            guard names.insert(entry.name).inserted else {
                throw FlyrailValueError("duplicate environment name")
            }
            if case let .text(text) = entry.value, text.contains("\0") {
                throw FlyrailValueError("environment values cannot contain NUL")
            }
        }
        if case let .text(path)? = cwd {
            try validateRelativePath(path, label: "command cwd")
        }
        self.argv = argv
        self.env = env.sorted { utf8Less($0.name, $1.name) }
        self.cwd = cwd
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "Command",
            fields: [
                ("argv", .array(argv.map { try $0.modelValue() })),
                ("cwd", cwd.map { try $0.modelValue() } ?? .scalar(.null)),
                ("env", .array(env.map {
                    try .array([.scalar(.text($0.name)), $0.value.modelValue()])
                })),
            ],
        )
    }
}

public struct HttpTransport: Sendable, Hashable {
    public let url: String
    public let bearerToken: EnvRef?

    public init(url: String, bearerToken: EnvRef? = nil) throws {
        try validateHTTPURL(url)
        self.url = url
        self.bearerToken = bearerToken
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "HttpTransport",
            fields: [
                ("bearer_token", bearerToken.map { try $0.modelValue() } ?? .scalar(.null)),
                ("url", .scalar(.text(url))),
            ],
        )
    }
}

public enum MCPTransport: Sendable, Hashable {
    case stdio(Command)
    case http(HttpTransport)

    package func modelValue() throws -> SemanticValue {
        switch self {
        case let .stdio(value): try value.modelValue()
        case let .http(value): try value.modelValue()
        }
    }
}

public struct InstructionArtifact: Sendable, Hashable {
    public let id: String
    public let text: String

    public init(id: String, text: String) throws {
        try validateIdentifier(id, label: "artifact id")
        guard !text.contains("\0") else {
            throw FlyrailValueError("instruction text cannot contain NUL")
        }
        self.id = id
        self.text = text
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "InstructionArtifact",
            fields: [("id", .scalar(.text(id))), ("text", .scalar(.text(text)))],
        )
    }
}

public struct MCPArtifact: Sendable, Hashable {
    public let id: String
    public let name: String
    public let transport: MCPTransport

    public init(id: String, name: String, transport: MCPTransport) throws {
        try validateIdentifier(id, label: "artifact id")
        try validateIdentifier(name, label: "MCP server name")
        self.id = id
        self.name = name
        self.transport = transport
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "McpArtifact",
            fields: [
                ("id", .scalar(.text(id))),
                ("name", .scalar(.text(name))),
                ("transport", transport.modelValue()),
            ],
        )
    }
}

public enum HookEvent: String, Sendable, Hashable, CaseIterable {
    case sessionStart = "session-start"
    case beforeTool = "before-tool"
    case afterTool = "after-tool"
    case prompt
    case stop
}

public enum HookOutcome: String, Sendable, Hashable, CaseIterable {
    case `continue`
    case block
    case ask
    case modifyInput = "modify-input"
    case context
}

public struct HookArtifact: Sendable, Hashable {
    public let id: String
    public let event: HookEvent
    public let command: Command
    public let timeoutMilliseconds: Int
    public let outcomes: [HookOutcome]
    public let tools: [String]
    public let protocolVersion: Int

    public init(
        id: String,
        event: HookEvent,
        command: Command,
        timeoutMilliseconds: Int = 10000,
        outcomes: [HookOutcome] = [.continue],
        tools: [String] = [],
    ) throws {
        try validateIdentifier(id, label: "artifact id")
        guard (1 ... 300_000).contains(timeoutMilliseconds) else {
            throw FlyrailValueError("hook timeout must be 1-300000 integer milliseconds")
        }
        guard !outcomes.isEmpty else {
            throw FlyrailValueError("hook requires declared HookOutcome values")
        }
        guard Set(outcomes).count == outcomes.count else {
            throw FlyrailValueError("hook outcomes must be unique")
        }
        guard tools.allSatisfy({ !$0.isEmpty && !$0.contains("\0") }) else {
            throw FlyrailValueError("tool names must be nonempty and cannot contain NUL")
        }
        guard Set(tools.map { Array($0.utf8) }).count == tools.count else {
            throw FlyrailValueError("tool names must be unique")
        }
        guard tools.isEmpty || event == .beforeTool || event == .afterTool else {
            throw FlyrailValueError("tool selection requires a tool event")
        }
        self.id = id
        self.event = event
        self.command = command
        self.timeoutMilliseconds = timeoutMilliseconds
        self.outcomes = outcomes.sorted { utf8Less($0.rawValue, $1.rawValue) }
        self.tools = tools.sorted(by: utf8Less)
        protocolVersion = 1
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "HookArtifact",
            fields: [
                ("command", command.modelValue()),
                ("event", .scalar(.text(event.rawValue))),
                ("id", .scalar(.text(id))),
                ("outcomes", .array(outcomes.map { .scalar(.text($0.rawValue)) })),
                ("protocol_version", .scalar(.integer(Int64(protocolVersion)))),
                ("timeout_ms", .scalar(.integer(Int64(timeoutMilliseconds)))),
                ("tools", .array(tools.map { .scalar(.text($0)) })),
            ],
        )
    }
}

public struct NativeArtifact: Sendable, Hashable {
    public let id: String
    public let family: Family
    public let audience: Audience
    public let destination: String
    public let content: Content

    public init(
        id: String,
        family: Family,
        audience: Audience,
        destination: String,
        content: Content,
    ) throws {
        try validateIdentifier(id, label: "artifact id")
        try validateRelativePath(destination, label: "native destination")
        self.id = id
        self.family = family
        self.audience = audience
        self.destination = destination
        self.content = content
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "NativeArtifact",
            fields: [
                ("audience", audience.modelValue()),
                ("content", content.modelValue()),
                ("destination", .scalar(.text(destination))),
                ("family", .scalar(.text(family.rawValue))),
                ("id", .scalar(.text(id))),
            ],
        )
    }
}

public struct SupportAsset: Sendable, Hashable {
    public let id: String
    public let family: Family
    public let content: TreeContent

    public init(id: String, family: Family, content: TreeContent) throws {
        try validateIdentifier(id, label: "asset id")
        self.id = id
        self.family = family
        self.content = content
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "SupportAsset",
            fields: [
                ("content", content.modelValue()),
                ("family", .scalar(.text(family.rawValue))),
                ("id", .scalar(.text(id))),
            ],
        )
    }
}

public enum DependencyMode: String, Sendable, Hashable {
    case revision
    case stableReference = "stable-reference"
}

public struct Dependency: Sendable, Hashable {
    public let dependent: String
    public let required: String
    public let mode: DependencyMode

    public init(dependent: String, required: String, mode: DependencyMode = .revision) throws {
        try validateIdentifier(dependent, label: "dependent artifact")
        try validateIdentifier(required, label: "required artifact")
        guard !exactTextEqual(dependent, required) else {
            throw FlyrailValueError("an artifact cannot depend on itself")
        }
        self.dependent = dependent
        self.required = required
        self.mode = mode
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "Dependency",
            fields: [
                ("dependent", .scalar(.text(dependent))),
                ("mode", .scalar(.text(mode.rawValue))),
                ("required", .scalar(.text(required))),
            ],
        )
    }
}

private func validateHTTPURL(_ value: String) throws {
    let absolute = FlyrailValueError("MCP URL must be an absolute HTTP or HTTPS URL")
    guard let separator = value.firstIndex(of: ":") else { throw absolute }
    let scheme = value[..<separator].lowercased()
    guard scheme == "http" || scheme == "https" else { throw absolute }
    let rest = value[value.index(after: separator)...]
    guard rest.hasPrefix("//") else { throw absolute }
    let authorityStart = rest.index(rest.startIndex, offsetBy: 2)
    let authorityEnd = rest[authorityStart...].firstIndex(where: { "/?#".contains($0) }) ?? rest.endIndex
    let authority = String(rest[authorityStart ..< authorityEnd])
    guard !authority.isEmpty else { throw absolute }
    try validateNetlocNormalization(authority)
    if authority.contains("@") {
        throw FlyrailValueError("MCP URL cannot embed credentials or a fragment")
    }
    if let hash = value.firstIndex(of: "#"), value.index(after: hash) < value.endIndex {
        throw FlyrailValueError("MCP URL cannot embed credentials or a fragment")
    }
    guard !value.unicodeScalars.contains(where: {
        $0.value < 32 || CharacterSet.whitespacesAndNewlines.contains($0)
    }) else {
        throw FlyrailValueError("MCP URL cannot contain whitespace or control characters")
    }
    let hostPort: Substring
    if authority.hasPrefix("[") {
        guard let close = authority.firstIndex(of: "]"),
              close > authority.startIndex,
              !authority[authority.index(after: close)...].contains("["),
              !authority[authority.index(after: close)...].contains("]")
        else {
            throw FlyrailValueError("Invalid IPv6 URL")
        }
        let suffix = authority[authority.index(after: close)...]
        guard suffix.isEmpty || suffix.first == ":" else {
            throw FlyrailValueError("Invalid IPv6 URL")
        }
        try validateBracketedHost(String(authority[authority.index(after: authority.startIndex) ..< close]))
        hostPort = suffix
    } else {
        guard !authority.contains("["), !authority.contains("]") else {
            throw FlyrailValueError("Invalid IPv6 URL")
        }
        hostPort = authority[...]
    }
    if let colon = hostPort.lastIndex(of: ":") {
        let port = hostPort[hostPort.index(after: colon)...]
        if !port.isEmpty {
            guard port.utf8.allSatisfy({ (48 ... 57).contains($0) }),
                  let number = Int(port),
                  (1 ... 65535).contains(number)
            else {
                throw FlyrailValueError("MCP URL port must be 1-65535")
            }
        }
    }
    let host = authority.hasPrefix("[")
        ? authority.dropFirst().prefix(while: { $0 != "]" })
        : authority.prefix(while: { $0 != ":" })
    guard !host.isEmpty else { throw absolute }
}

private func validateNetlocNormalization(_ netloc: String) throws {
    guard netloc.unicodeScalars.contains(where: { $0.value > 127 }) else { return }
    let stripped = String(netloc.filter { !"@:#?".contains($0) })
    let normalized = stripped.precomposedStringWithCompatibilityMapping
    if !exactTextEqual(stripped, normalized),
       normalized.contains(where: { "/?#@:".contains($0) })
    {
        throw FlyrailValueError(
            "netloc '\(netloc)' contains invalid characters under NFKC normalization",
        )
    }
}

private func validateBracketedHost(_ host: String) throws {
    guard !host.isEmpty, !host.contains("["), !host.contains("]") else {
        throw FlyrailValueError("Invalid IPv6 URL")
    }
    if host.hasPrefix("v") {
        let parts = host.dropFirst().split(separator: ".", maxSplits: 1, omittingEmptySubsequences: false)
        guard parts.count == 2,
              !parts[0].isEmpty,
              parts[0].utf8.allSatisfy(\.isHexDigit),
              !parts[1].isEmpty
        else {
            throw FlyrailValueError("IPvFuture address is invalid")
        }
        return
    }
    try validateIPv6Address(host)
}

private func validateIPv6Address(_ value: String) throws {
    let zoneParts = value.split(separator: "%", maxSplits: 1, omittingEmptySubsequences: false)
    guard zoneParts.count == 1 || !zoneParts[1].isEmpty else {
        throw FlyrailValueError("'\(value)' does not appear to be an IPv4 or IPv6 address")
    }
    let address = String(zoneParts[0])
    let compressionParts = address.components(separatedBy: "::")
    guard compressionParts.count <= 2 else {
        throw FlyrailValueError("'\(value)' does not appear to be an IPv4 or IPv6 address")
    }
    let groups = try compressionParts.reduce(0) { try $0 + countIPv6Groups($1) }
    let valid = compressionParts.count == 2 ? groups < 8 : groups == 8
    guard valid else {
        throw FlyrailValueError("'\(value)' does not appear to be an IPv4 or IPv6 address")
    }
}

private func countIPv6Groups(_ value: String) throws -> Int {
    guard !value.isEmpty else { return 0 }
    let parts = value.split(separator: ":", omittingEmptySubsequences: false)
    var count = 0
    for (index, part) in parts.enumerated() {
        if part.contains(".") {
            guard index == parts.count - 1, validIPv4Tail(part) else {
                throw FlyrailValueError("bracketed host is not a valid IPv6 address")
            }
            count += 2
        } else {
            guard (1 ... 4).contains(part.count), part.utf8.allSatisfy(\.isHexDigit) else {
                throw FlyrailValueError("bracketed host is not a valid IPv6 address")
            }
            count += 1
        }
    }
    return count
}

private func validIPv4Tail(_ value: Substring) -> Bool {
    let parts = value.split(separator: ".", omittingEmptySubsequences: false)
    return parts.count == 4 && parts.allSatisfy {
        !$0.isEmpty &&
            ($0.count == 1 || !$0.hasPrefix("0")) &&
            $0.utf8.allSatisfy { (48 ... 57).contains($0) } &&
            UInt16($0).map { $0 <= 255 } == true
    }
}

private extension UInt8 {
    var isHexDigit: Bool {
        (48 ... 57).contains(self) || (65 ... 70).contains(self) || (97 ... 102).contains(self)
    }
}
