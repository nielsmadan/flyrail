import CryptoKit
import Foundation

public struct ArtifactTranslation: Sendable, Hashable {
    public let artifact: RenderedArtifact?
    public let notices: [Notice]

    public var supported: Bool {
        artifact != nil && !notices.contains { $0.kind == .unsupported }
    }
}

private func translationID(_ values: [String]) -> String {
    let escaped = values.map { value in
        var text = "\""
        for scalar in value.unicodeScalars {
            switch scalar.value {
            case 0x22: text += "\\\""
            case 0x5C: text += "\\\\"
            case 0 ... 0x1F: text += String(format: "\\u%04x", scalar.value)
            default: text.append(Character(String(scalar)))
            }
        }
        return text + "\""
    }.joined(separator: ",")
    return SHA256.hash(data: Data("[\(escaped)]".utf8))
        .map { String(format: "%02x", $0) }
        .joined()
}

private func unsupportedTranslation(
    artifactID: String,
    error: UnsupportedTranslation,
) throws -> ArtifactTranslation {
    try ArtifactTranslation(
        artifact: nil,
        notices: [Notice(
            artifactID: artifactID,
            kind: .unsupported,
            message: error.message,
            code: error.code,
        )],
    )
}

private func activationNotices(
    artifactID: String,
    family: Family,
    context: RenderContext,
) throws -> [Notice] {
    var result: [Notice] = []
    let agent = context.audience.agent
    if family == .instructions {
        try result.append(Notice(
            artifactID: artifactID,
            kind: .activation,
            message: "Instruction discovery and loading are controlled by the host and current session.",
            code: "instruction-loading",
        ))
        if agent == .codex {
            try result.append(Notice(
                artifactID: artifactID,
                kind: .activation,
                message: "AGENTS.override.md takes precedence; instructions default to a 32 KiB limit. Start a new session after changes and check host discovery.",
                code: "codex-instruction-discovery",
            ))
        }
        if context.surface == .vscode, context.target.scope == .user {
            try result.append(Notice(
                artifactID: artifactID,
                kind: .activation,
                message: "User instructions target VS Code Agent Host; local extension-host sessions may use different discovery settings.",
                code: "vscode-agent-host-instructions",
            ))
        }
    }
    if family == .mcp {
        try result.append(Notice(
            artifactID: artifactID,
            kind: .activation,
            message: "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            code: "mcp-activation",
        ))
        if agent == .pi {
            try result.append(Notice(
                artifactID: artifactID,
                kind: .prerequisite,
                message: "Requires externally installed pi-mcp-adapter; emitted schema targets v2.32.1.",
                code: "pi-mcp-adapter-v2-32-1",
            ))
        }
        if context.surface == .vscode {
            try result.append(Notice(
                artifactID: artifactID,
                kind: .activation,
                message: "VS Code forwards these servers to Agent Host, which does not read this servers document directly.",
                code: "vscode-mcp-forwarding",
            ))
        }
    }
    return result
}

public func translateInstructionArtifact(
    bundleID: String,
    artifact: InstructionArtifact,
    context: RenderContext,
) throws -> ArtifactTranslation {
    guard isTranslatable(context.audience.agent) else {
        return try unsupportedTranslation(
            artifactID: artifact.id,
            error: UnsupportedTranslation(
                code: "agent-unsupported",
                message: untranslatableMessage(context.audience.agent),
            ),
        )
    }
    guard var path = try instructionDestinationOptional(for: context) else {
        return try unsupportedTranslation(
            artifactID: artifact.id,
            error: UnsupportedTranslation(
                code: "instruction-destination",
                message: "Cursor user rules have no verified filesystem preset.",
            ),
        )
    }
    let marker = translationID([bundleID, artifact.id])
    let content: Content
    if context.surface == .vscode, context.target.scope == .user {
        if context.selectedInstructionPath == nil {
            path = try appendPath(path, "\(marker).instructions.md")
        }
        guard path.displayString.hasSuffix(".instructions.md") else {
            return try unsupportedTranslation(
                artifactID: artifact.id,
                error: UnsupportedTranslation(
                    code: "instruction-extension",
                    message: "VS Code user instructions require an .instructions.md file.",
                ),
            )
        }
        content = .file(FileContent(data: Data("---\napplyTo: \"**\"\n---\n\n\(artifact.text)".utf8)))
    } else {
        if path.displayString.hasSuffix(".mdc") || path.displayString.hasSuffix(".instructions.md") {
            return try unsupportedTranslation(
                artifactID: artifact.id,
                error: UnsupportedTranslation(
                    code: "native-instruction-required",
                    message: "Use native content for authored modular rule metadata.",
                ),
            )
        }
        content = try .section(SectionContent(marker: marker, text: artifact.text))
    }
    return try ArtifactTranslation(
        artifact: RenderedArtifact(
            id: artifact.id,
            family: .instructions,
            destination: path,
            content: content,
        ),
        notices: activationNotices(artifactID: artifact.id, family: .instructions, context: context),
    )
}

public func translateMCPArtifact(
    artifact: MCPArtifact,
    context: RenderContext,
    assetDestinations: [String: ResourcePath] = [:],
) throws -> ArtifactTranslation {
    guard isTranslatable(context.audience.agent) else {
        return try unsupportedTranslation(
            artifactID: artifact.id,
            error: UnsupportedTranslation(
                code: "agent-unsupported",
                message: untranslatableMessage(context.audience.agent),
            ),
        )
    }
    guard let path = try mcpDestinationOptional(for: context) else {
        let error = context.surface == .vscode
            ? UnsupportedTranslation(
                code: "mcp-profile-path",
                message: "VS Code user MCP requires an explicit profile mcp.json path.",
            )
            : UnsupportedTranslation(
                code: "mcp-explicit-path",
                message: "Relocated Claude user MCP requires an explicit mcp_path.",
            )
        return try unsupportedTranslation(artifactID: artifact.id, error: error)
    }
    if context.surface == .vscode, !path.displayString.hasSuffix("/mcp.json") {
        return try unsupportedTranslation(
            artifactID: artifact.id,
            error: UnsupportedTranslation(
                code: "mcp-profile-schema",
                message: "VS Code routes require mcp.json with the servers schema; use the CLI surface for Agent Host native mcpServers files.",
            ),
        )
    }
    do {
        let payload = try mcpContent(
            artifact,
            context: context,
            path: path,
            assets: assetDestinations,
        )
        return try ArtifactTranslation(
            artifact: RenderedArtifact(
                id: artifact.id,
                family: .mcp,
                destination: path,
                content: .structured(payload),
            ),
            notices: activationNotices(artifactID: artifact.id, family: .mcp, context: context),
        )
    } catch let error as UnsupportedTranslation {
        return try unsupportedTranslation(artifactID: artifact.id, error: error)
    }
}

public func assembleTranslations(
    translations: [ArtifactTranslation],
    dependencies: [Dependency],
    contexts: [RenderContext],
    routingContext: [(String, String)],
) throws -> RenderedBundle {
    let artifacts = translations.compactMap(\.artifact)
    var notices = translations.flatMap(\.notices)
    notices += try collisionNotices(artifacts)
    notices += try copilotPrecedenceNotices(artifacts, contexts: contexts)
    let blocked = notices.contains { $0.kind == .unsupported }
    return try RenderedBundle(
        artifacts: blocked ? [] : artifacts,
        dependencies: blocked ? [] : dependencies,
        notices: notices,
        routingContext: routingContext,
    )
}

// swiftlint:disable:next function_body_length
public func render(_ bundle: FlyrailBundle, for context: RenderContext) throws -> RenderedBundle {
    if !isTranslatable(context.audience.agent) {
        let selected = bundle.artifacts.filter {
            guard case let .native(native) = $0 else { return true }
            return nativeAudienceMatches(native.audience, context: context)
        }
        let notices = try selected.map {
            try Notice(
                artifactID: $0.id,
                kind: .unsupported,
                message: untranslatableMessage(context.audience.agent),
                code: "agent-unsupported",
            )
        }
        return try RenderedBundle(
            artifacts: [],
            notices: notices,
            routingContext: context.routingContext,
        )
    }
    let (assetArtifacts, assets) = try supportAssetArtifacts(bundle: bundle, context: context)
    var artifacts: [RenderedArtifact] = []
    var notices: [Notice] = []
    var translatedNotices: [Notice] = []
    var hookDependencies: [Dependency] = []
    do {
        let output = try hookArtifacts(bundle: bundle, context: context, assets: assets)
        artifacts += output.artifacts
        hookDependencies += output.dependencies
        notices += output.notices
    } catch let error as UnsupportedTranslation {
        for artifact in bundle.artifacts {
            if case let .hook(hook) = artifact {
                try notices.append(Notice(
                    artifactID: hook.id,
                    kind: .unsupported,
                    message: error.message,
                    code: error.code,
                ))
            }
        }
    }
    var selected: Set<String> = []
    for artifact in bundle.artifacts {
        if case let .native(native) = artifact,
           try translateNativeArtifact(native, context: context) == nil
        {
            continue
        }
        selected.insert(artifact.id)
        let translation: ArtifactTranslation
        switch artifact {
        case let .skill(skill):
            translation = try ArtifactTranslation(
                artifact: RenderedArtifact(
                    id: skill.id,
                    family: .skills,
                    destination: context.skills,
                    content: .tree(skill.content),
                    subtree: skill.name,
                ),
                notices: [],
            )
        case let .instruction(instruction):
            translation = try translateInstructionArtifact(
                bundleID: bundle.identity.id,
                artifact: instruction,
                context: context,
            )
        case let .mcp(mcp):
            translation = try translateMCPArtifact(
                artifact: mcp,
                context: context,
                assetDestinations: assets,
            )
        case .hook:
            continue
        case let .native(native):
            translation = try ArtifactTranslation(
                artifact: translateNativeArtifact(native, context: context),
                notices: activationNotices(
                    artifactID: native.id,
                    family: native.family,
                    context: context,
                ),
            )
        }
        if let output = translation.artifact {
            artifacts.append(output)
        }
        for notice in translation.notices {
            if notice.kind == .unsupported {
                notices.append(notice)
            } else {
                translatedNotices.append(notice)
            }
        }
    }
    var pending = selected
    while !pending.isEmpty {
        let required = Set(bundle.dependencies.filter {
            pending.contains($0.dependent)
        }.map(\.required))
        pending = required.subtracting(selected)
        selected.formUnion(required)
    }
    artifacts += assetArtifacts.filter { selected.contains($0.id) }
    let emitted = Set(artifacts.map(\.id))
    for name in selected.subtracting(emitted).sorted(by: utf8Less)
        where !notices.contains(where: { $0.artifactID == name && $0.kind == .unsupported })
    {
        try notices.append(Notice(
            artifactID: name,
            kind: .unsupported,
            message: "A selected dependency is unavailable for this audience.",
            code: "dependency-audience",
        ))
    }
    notices += translatedNotices
    notices += try collisionNotices(artifacts)
    notices += try copilotPrecedenceNotices(artifacts, contexts: [context])
    let blocked = notices.contains { $0.kind == .unsupported }
    let dependencies = bundle.dependencies.filter { emitted.contains($0.dependent) } + hookDependencies
    return try RenderedBundle(
        artifacts: blocked ? [] : artifacts,
        dependencies: blocked ? [] : dependencies,
        notices: notices,
        routingContext: context.routingContext,
    )
}

private func nativeAudienceMatches(_ audience: Audience, context: RenderContext) -> Bool {
    audience.agent == context.audience.agent &&
        audience.scope == context.target.scope &&
        audience.surface == context.surface &&
        (audience.platform == nil || audience.platform == context.platform)
}

public func renderMany(
    _ bundle: FlyrailBundle,
    for contexts: [RenderContext],
) throws -> RenderedBundle {
    guard !contexts.isEmpty else {
        throw FlyrailValueError("render_many requires at least one explicit context")
    }
    var unique: [String: (String, RenderedBundle)] = [:]
    for context in contexts {
        let rendered = try render(bundle, for: context)
        let route = routeJSON(rendered.routingContext)
        unique[route] = (route, rendered)
    }
    var artifacts: [RenderedArtifact] = []
    var dependencies: [Dependency] = []
    var notices: [Notice] = []
    var routes: [(String, String)] = []
    for key in unique.keys.sorted(by: utf8Less) {
        guard let (route, rendered) = unique[key] else { continue }
        routes.append((translationID([route]), route))
        let names = Dictionary(uniqueKeysWithValues: rendered.artifacts.map {
            ($0.id, translationID([route, $0.id]))
        })
        for artifact in rendered.artifacts {
            guard let id = names[artifact.id] else {
                throw FlyrailValueError("missing route artifact identifier")
            }
            try artifacts.append(RenderedArtifact(
                id: id,
                family: artifact.family,
                destination: artifact.destination,
                content: artifact.content,
                subtree: artifact.subtree,
                requireExisting: artifact.requireExisting,
                schemaRequirements: artifact.schemaRequirements,
            ))
        }
        for edge in rendered.dependencies {
            guard let dependent = names[edge.dependent], let required = names[edge.required] else {
                throw FlyrailValueError("missing route dependency identifier")
            }
            try dependencies.append(Dependency(
                dependent: dependent,
                required: required,
                mode: edge.mode,
            ))
        }
        for notice in rendered.notices {
            try notices.append(Notice(
                artifactID: translationID([route, notice.artifactID]),
                kind: notice.kind,
                message: notice.message,
                code: notice.code,
            ))
        }
    }
    notices += try collisionNotices(artifacts)
    notices += try copilotPrecedenceNotices(artifacts, contexts: contexts)
    let blocked = notices.contains { $0.kind == .unsupported }
    return try RenderedBundle(
        artifacts: blocked ? [] : artifacts,
        dependencies: blocked ? [] : dependencies,
        notices: notices,
        routingContext: routes,
    )
}

private func routeJSON(_ context: [(String, String)]) -> String {
    let values = context.map { pair in
        "[\(jsonString(pair.0)), \(jsonString(pair.1))]"
    }
    return "[\(values.joined(separator: ", "))]"
}

private func jsonString(_ value: String) -> String {
    var result = "\""
    for scalar in value.unicodeScalars {
        switch scalar.value {
        case 0x22: result += "\\\""
        case 0x5C: result += "\\\\"
        case 0x08: result += "\\b"
        case 0x0C: result += "\\f"
        case 0x0A: result += "\\n"
        case 0x0D: result += "\\r"
        case 0x09: result += "\\t"
        case 0 ... 0x1F: result += String(format: "\\u%04x", scalar.value)
        case 0x20 ... 0x7F: result.append(Character(String(scalar)))
        case 0x80 ... 0xFFFF: result += String(format: "\\u%04x", scalar.value)
        default:
            let value = scalar.value - 0x10000
            result += String(format: "\\u%04x\\u%04x", 0xD800 + value >> 10, 0xDC00 + value & 0x3FF)
        }
    }
    return result + "\""
}

private func collisionNotices(_ artifacts: [RenderedArtifact]) throws -> [Notice] {
    var selected: [RenderedArtifact] = []
    var notices: [Notice] = []
    for artifact in artifacts {
        for previous in selected where artifact.destination == previous.destination {
            if case let .structured(left) = artifact.content,
               case let .structured(right) = previous.content,
               left.format != right.format
            {
                try notices.append(Notice(
                    artifactID: artifact.id,
                    kind: .unsupported,
                    message: "Incompatible document formats from \(previous.id) and \(artifact.id) at \(artifact.destination.displayString).",
                    code: "document-format-collision",
                ))
                continue
            }
            if claimsOverlap(artifact, previous),
               claimsDiffer(artifact, previous)
            {
                try notices.append(Notice(
                    artifactID: artifact.id,
                    kind: .unsupported,
                    message: "Incompatible claims from \(previous.id) and \(artifact.id) at \(artifact.destination.displayString).",
                    code: "destination-collision",
                ))
            }
        }
        selected.append(artifact)
    }
    return notices
}

private func claimsOverlap(_ left: RenderedArtifact, _ right: RenderedArtifact) -> Bool {
    switch (left.content, right.content) {
    case (.file, _), (_, .file):
        true
    case let (.section(left), .section(right)):
        (try? left.selector) == (try? right.selector)
    case let (.structured(left), .structured(right)):
        selectorPrefix(left.selector.parts, right.selector.parts) ||
            selectorPrefix(right.selector.parts, left.selector.parts)
    case (.tree, .tree):
        switch (left.subtree, right.subtree) {
        case (.none, _), (_, .none): true
        case let (.some(left), .some(right)):
            left == right || left.hasPrefix("\(right)/") || right.hasPrefix("\(left)/")
        }
    default:
        true
    }
}

private func claimsDiffer(_ left: RenderedArtifact, _ right: RenderedArtifact) -> Bool {
    left.content != right.content ||
        left.family != right.family ||
        left.schemaRequirements != right.schemaRequirements ||
        left.subtree != right.subtree
}

private func selectorPrefix(_ left: [SelectorPart], _ right: [SelectorPart]) -> Bool {
    left.count <= right.count && zip(left, right).allSatisfy(==)
}

private func copilotPrecedenceNotices(
    _ artifacts: [RenderedArtifact],
    contexts: [RenderContext],
) throws -> [Notice] {
    let roots = Set(contexts.filter {
        $0.audience.agent == .copilot &&
            $0.surface == .cli &&
            $0.target.scope == .project
    }.map(\.root))
    var notices: [Notice] = []
    for root in roots {
        let highPath = try appendPath(root, ".mcp.json")
        let lowPath = try appendPath(root, ".github/mcp.json")
        let higher = artifacts.filter { $0.destination == highPath }
        let lower = artifacts.filter { $0.destination == lowPath }
        for artifact in lower {
            for previous in higher {
                guard case let .structured(content) = artifact.content,
                      case let .structured(previousContent) = previous.content,
                      content.selector.parts.count == 2,
                      previousContent.selector.parts.count == 2,
                      case let .key(first) = content.selector.parts[0],
                      case let .key(previousFirst) = previousContent.selector.parts[0],
                      first.name == "mcpServers",
                      previousFirst.name == "mcpServers",
                      case let .key(server) = content.selector.parts[1],
                      case .key = previousContent.selector.parts[1]
                else {
                    try notices.append(precedenceUnverified(artifact, previous: previous))
                    continue
                }
                if content.selector == previousContent.selector,
                   content.value != previousContent.value
                {
                    try notices.append(Notice(
                        artifactID: artifact.id,
                        kind: .unsupported,
                        message: "Copilot CLI selects \(previous.destination.displayString) over \(artifact.destination.displayString) for server \(server.name); the selected records disagree.",
                        code: "copilot-mcp-precedence-collision",
                    ))
                }
            }
        }
    }
    return notices
}

private func precedenceUnverified(
    _ artifact: RenderedArtifact,
    previous: RenderedArtifact,
) throws -> Notice {
    try Notice(
        artifactID: artifact.id,
        kind: .activation,
        message: "Copilot CLI gives \(previous.destination.displayString) precedence over \(artifact.destination.displayString); native content requires caller review of the effective server definitions.",
        code: "copilot-mcp-precedence-unverified",
    )
}
