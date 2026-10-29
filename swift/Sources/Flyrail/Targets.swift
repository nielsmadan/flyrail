import Foundation
import SystemPackage

public struct Target: Sendable, Hashable {
    public let root: ResourcePath
    public let agent: Agent?
    public let scope: TargetScope
    public let home: ResourcePath?
    package let anchor: ResourcePath
    package let environment: [(String, String)]

    private struct Routing {
        let home: ResourcePath?
        let environment: [(String, String)]
    }

    public static func directory(_ root: ResourcePath) throws -> Self {
        try create(anchor: root, suffix: [], agent: nil, scope: .directory)
    }

    public static func project(agent: Agent, root: ResourcePath) throws -> Self {
        try create(
            anchor: root,
            suffix: projectSuffix(agent),
            agent: agent,
            scope: .project,
        )
    }

    public static func user(agent: Agent) throws -> Self {
        let environment = ProcessInfo.processInfo.environment
        let home: ResourcePath = if let configured = environment["HOME"], let selected = try? ResourcePath(configured) {
            selected
        } else {
            try ResourcePath(FileManager.default.homeDirectoryForCurrentUser.path)
        }
        return try user(agent: agent, home: home, environment: environment)
    }

    public static func user(agent: Agent, home: ResourcePath) throws -> Self {
        try user(agent: agent, home: home, environment: [:])
    }

    public static func user(
        agent: Agent,
        home: ResourcePath,
        environment: [String: String],
    ) throws -> Self {
        try requireAbsoluteTarget(home)
        let relocation = relocation(agent).flatMap { variable, suffix in
            environment[variable].flatMap { $0.isEmpty ? nil : ($0, suffix) }
        }
        let anchor: ResourcePath
        let suffix: [String]
        if let relocation {
            anchor = try relocatedPath(relocation.0, home: home)
            suffix = relocation.1
        } else {
            anchor = home
            suffix = userSuffix(agent)
        }
        let retained = environment
            .filter { retainedEnvironmentKeys.contains($0.key) }
            .sorted { utf8Less($0.key, $1.key) }
        return try create(
            anchor: anchor,
            suffix: suffix,
            agent: agent,
            scope: .user,
            routing: Routing(home: home, environment: retained),
        )
    }

    package var routingContext: [(String, String)] {
        var result = [
            ("agent", agent?.rawValue ?? ""),
            ("scope", scope.rawValue),
            ("home", home?.displayString ?? ""),
        ]
        result += environment.map { ("env:\($0.0)", $0.1) }
        return result
    }

    private static func create(
        anchor: ResourcePath,
        suffix: [String],
        agent: Agent?,
        scope: TargetScope,
        routing: Routing = Routing(home: nil, environment: []),
    ) throws -> Self {
        try requireAbsoluteTarget(anchor)
        let root = try appendPath(anchor, components: suffix)
        guard root.displayString != "/" else {
            throw FlyrailValueError("a filesystem root cannot be a skill container")
        }
        return Self(
            root: root,
            agent: agent,
            scope: scope,
            home: routing.home,
            anchor: anchor,
            environment: routing.environment,
        )
    }

    public static func == (left: Self, right: Self) -> Bool {
        left.root == right.root &&
            left.agent == right.agent &&
            left.scope == right.scope &&
            left.anchor == right.anchor
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(root)
        hasher.combine(agent)
        hasher.combine(scope)
        hasher.combine(anchor)
    }
}

private let retainedEnvironmentKeys = Set([
    "HOME",
    "USERPROFILE",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "XDG_CONFIG_HOME",
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "PI_CODING_AGENT_DIR",
    "COPILOT_HOME",
])

private func projectSuffix(_ agent: Agent) -> [String] {
    switch agent {
    case .claude: [".claude", "skills"]
    case .codex: [".agents", "skills"]
    case .opencode: [".opencode", "skills"]
    case .pi: [".pi", "skills"]
    case .cursor: [".cursor", "skills"]
    case .copilot: [".github", "skills"]
    case .droid: [".factory", "skills"]
    }
}

private func userSuffix(_ agent: Agent) -> [String] {
    switch agent {
    case .claude: [".claude", "skills"]
    case .codex: [".agents", "skills"]
    case .opencode: [".config", "opencode", "skills"]
    case .pi: [".pi", "agent", "skills"]
    case .cursor: [".cursor", "skills"]
    case .copilot: [".copilot", "skills"]
    case .droid: [".factory", "skills"]
    }
}

private func relocation(_ agent: Agent) -> (String, [String])? {
    switch agent {
    case .claude: ("CLAUDE_CONFIG_DIR", ["skills"])
    case .opencode: ("XDG_CONFIG_HOME", ["opencode", "skills"])
    case .pi: ("PI_CODING_AGENT_DIR", ["skills"])
    case .copilot: ("COPILOT_HOME", ["skills"])
    case .codex, .cursor, .droid: nil
    }
}

package func appendPath(_ root: ResourcePath, _ relative: String) throws -> ResourcePath {
    try appendPath(root, components: relative.split(separator: "/", omittingEmptySubsequences: true).map(String.init))
}

package func appendPath(_ root: ResourcePath, components: [String]) throws -> ResourcePath {
    guard components.allSatisfy({
        !$0.isEmpty && $0 != "." && $0 != ".." && !$0.contains("/") && !$0.contains("\0")
    }) else {
        throw FlyrailValueError("render routes must be unambiguous paths without NUL")
    }
    var path = root.filePath
    components.forEach { path.append($0) }
    return ResourcePath(filePath: path, doubleSlashRoot: root.doubleSlashRoot)
}

package func validateRoute(_ path: ResourcePath) throws -> ResourcePath {
    guard path.filePath.root != nil else {
        throw FlyrailValueError("render routes must be absolute Paths")
    }
    guard !path.filePath.components.contains(where: { $0.kind == .parentDirectory }) else {
        throw FlyrailValueError("render routes must be unambiguous paths without NUL")
    }
    return path
}

package func relocatedPath(_ value: String, home: ResourcePath) throws -> ResourcePath {
    if value == "~" {
        return home
    }
    if value.hasPrefix("~/") {
        return try appendPath(home, String(value.dropFirst(2)))
    }
    let path = try ResourcePath(value)
    guard value.hasPrefix("/") else {
        throw FlyrailValueError("environment paths must be absolute or start with ~/")
    }
    return try validateRoute(path)
}

private func requireAbsoluteTarget(_ path: ResourcePath) throws {
    let value = path.displayString
    guard !value.isEmpty, value.hasPrefix("/") else {
        throw FlyrailValueError("target roots must be nonempty absolute paths without NUL")
    }
}
