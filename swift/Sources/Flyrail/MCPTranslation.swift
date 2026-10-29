import Foundation

package struct UnsupportedTranslation: Error, Sendable, Equatable {
    let code: String
    let message: String
}

private func unsupported<T>(_ code: String, _ message: String) throws -> T {
    throw UnsupportedTranslation(code: code, message: message)
}

private func scalar(_ value: String) -> SemanticValue {
    .scalar(.text(value))
}

private func object(_ values: [(String, SemanticValue)]) throws -> SemanticValue {
    try SemanticValue(object: values)
}

private func hasCopilotEnvironmentSubstitution(_ value: String) -> Bool {
    let bytes = Array(value.utf8)
    return bytes.indices.dropLast().contains { index in
        bytes[index] == 36 &&
            (bytes[index + 1] == 95 ||
                (65 ... 90).contains(bytes[index + 1]) ||
                (97 ... 122).contains(bytes[index + 1]))
    }
}

private func literal(
    _ value: String,
    context: RenderContext,
    field: String,
) throws -> String {
    let agent = context.audience.agent
    let substitution = switch agent {
    case .opencode: value.contains("{env:") || value.contains("{file:")
    case .pi: value.contains("${") || value.contains("$env:") || value.contains("{env:")
    case .codex: false
    default: value.contains("${")
    }
    let copilotCLI = agent == .copilot && context.surface == .cli
    if substitution || (copilotCLI && field == "env" && hasCopilotEnvironmentSubstitution(value)) {
        if copilotCLI, field != "env" {
            let label = field == "argv" ? "argv" : field
            return try unsupported(
                "literal-interpolation-unverified",
                "Literal \(label) substitution-like syntax is unverified for Copilot CLI.",
            )
        }
        return try unsupported(
            "literal-interpolation",
            "\(field) contains host interpolation syntax; use native content.",
        )
    }
    return value
}

private func reference(
    _ value: EnvRef,
    context: RenderContext,
    field: String,
) throws -> String {
    let agent = context.audience.agent
    if agent == .claude || (agent == .copilot && context.surface == .cli && field == "env") {
        return "${\(value.name)}"
    }
    if agent == .opencode {
        return "{env:\(value.name)}"
    }
    if agent == .cursor || context.surface == .vscode {
        return "${env:\(value.name)}"
    }
    return try unsupported(
        "portable-env-reference-unverified",
        "This host cannot represent this environment reference.",
    )
}

package func translatedAsset(
    _ value: CommandArgument,
    assets: [String: ResourcePath],
) throws -> String {
    switch value {
    case let .text(text):
        return text
    case let .asset(reference):
        guard let root = assets[reference.assetID] else {
            throw FlyrailValueError("unknown translated asset reference")
        }
        if let path = reference.path {
            return try appendPath(root, path).displayString
        }
        return root.displayString
    }
}

private func translateStdio(
    _ command: Command,
    context: RenderContext,
    assets: [String: ResourcePath],
) throws -> SemanticValue {
    let agent = context.audience.agent
    if context.platform == .windows {
        let names = command.env.map { $0.name.lowercased() }
        guard Set(names).count == names.count else {
            return try unsupported(
                "windows-env-collision",
                "Environment names collide under Windows case-insensitive lookup.",
            )
        }
    }
    let argv = try command.argv.map { try translatedAsset($0, assets: assets) }
    for (index, argument) in argv.enumerated() where agent != .pi || index != 0 {
        _ = try literal(argument, context: context, field: "argv")
    }
    var environment: [(String, SemanticValue)] = []
    var forwarded: [SemanticValue] = []
    for entry in command.env {
        switch entry.value {
        case let .reference(value) where agent == .codex:
            guard entry.name == value.name else {
                return try unsupported(
                    "environment-alias",
                    "Codex env_vars forwards a variable under its own name.",
                )
            }
            forwarded.append(scalar(entry.name))
        case let .reference(value):
            try environment.append((
                entry.name,
                scalar(reference(value, context: context, field: "env")),
            ))
        case let .text(text):
            if agent != .pi {
                _ = try literal(text, context: context, field: "env")
            }
            environment.append((entry.name, scalar(text)))
        }
    }
    var record: [(String, SemanticValue)]
    if agent == .opencode {
        record = try [
            ("type", scalar("local")),
            ("command", .array(argv.map(scalar))),
            ("environment", object(environment)),
        ]
    } else {
        guard let commandName = argv.first else { throw FlyrailValueError("argv requires a command") }
        record = try [
            ("command", scalar(commandName)),
            ("args", .array(argv.dropFirst().map(scalar))),
            ("env", object(environment)),
        ]
    }
    if agent == .cursor || context.surface == .vscode {
        record.append(("type", scalar("stdio")))
    }
    if agent == .pi {
        record.append(("literalEnv", .scalar(.boolean(true))))
    }
    if !forwarded.isEmpty {
        record.append(("env_vars", .array(forwarded)))
    }
    if let cwd = command.cwd {
        guard agent != .claude, agent != .cursor else {
            return try unsupported("command-cwd", "The selected MCP schema cannot honor cwd.")
        }
        let translated: String = switch cwd {
        case .asset:
            try translatedAsset(cwd, assets: assets)
        case let .text(relative):
            try appendPath(context.selectedExecutionRoot ?? context.root, relative).displayString
        }
        _ = try literal(translated, context: context, field: "cwd")
        record.append(("cwd", scalar(translated)))
    }
    return try object(record)
}

private func translateHTTP(_ transport: HttpTransport, context: RenderContext) throws -> SemanticValue {
    let agent = context.audience.agent
    _ = try literal(transport.url, context: context, field: "url")
    var result: [(String, SemanticValue)] = [("url", scalar(transport.url))]
    if agent == .opencode {
        result.append(("type", scalar("remote")))
    } else if agent != .codex, agent != .pi {
        result.append(("type", scalar("http")))
    }
    if let token = transport.bearerToken {
        switch agent {
        case .codex:
            result.append(("bearer_token_env_var", scalar(token.name)))
        case .pi:
            result.append(("auth", scalar("bearer")))
            result.append(("bearerTokenEnv", scalar(token.name)))
        case .copilot where context.surface == .cli:
            return try unsupported(
                "portable-auth-env-unverified",
                "Copilot CLI bearer environment syntax is unverified.",
            )
        default:
            let value = try reference(token, context: context, field: "headers")
            try result.append((
                "headers",
                object([("Authorization", scalar("Bearer \(value)"))]),
            ))
        }
    }
    return try object(result)
}

package func mcpContent(
    _ artifact: MCPArtifact,
    context: RenderContext,
    path: ResourcePath,
    assets: [String: ResourcePath],
) throws -> StructuredContent {
    let agent = context.audience.agent
    if agent == .claude, ["workspace", "claude-in-chrome", "computer-use"].contains(artifact.name) {
        return try unsupported("reserved-server-name", "Claude reserves this MCP server name.")
    }
    let value = switch artifact.transport {
    case let .stdio(command): try translateStdio(command, context: context, assets: assets)
    case let .http(transport): try translateHTTP(transport, context: context)
    }
    let format: DocumentFormat = if agent == .codex {
        .toml
    } else if agent == .opencode || context.surface == .vscode || path.displayString.hasSuffix(".jsonc") {
        .jsonc
    } else {
        .json
    }
    let key = if agent == .codex {
        "mcp_servers"
    } else if agent == .opencode {
        "mcp"
    } else if context.surface == .vscode {
        "servers"
    } else {
        "mcpServers"
    }
    return try StructuredContent(
        format: format,
        selector: Selector(parts: [.key(Key(key)), .key(Key(artifact.name))]),
        value: value,
    )
}
