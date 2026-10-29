import Foundation

package struct ParsedConfiguration {
    let identity: BundleIdentity
    let artifacts: [Artifact]
    let assets: [SupportAsset]
    let dependencies: [Dependency]
}

package func readConfigurationManifest(
    _ data: Data,
    source: some BundleSource,
) throws -> ParsedConfiguration {
    let raw = try parseManifestJSON(data)
    let fields = try configObject(
        raw,
        required: ["schema_version", "id", "version", "artifacts"],
        optional: ["assets", "dependencies"],
    )
    guard configValue(fields, "schema_version")?.signedInteger == 2 else {
        throw FlyrailValueError("configuration schema_version must be integer 2")
    }
    let identity = try BundleIdentity(
        id: configString(configValue(fields, "id"), label: "bundle id"),
        version: configString(configValue(fields, "version"), label: "version"),
    )
    let artifacts = try configList(configValue(fields, "artifacts")).map {
        try parseArtifact($0, source: source)
    }
    let assets = try configList(configValue(fields, "assets")).map {
        try parseAsset($0, source: source)
    }
    let dependencies = try configList(configValue(fields, "dependencies")).map(parseDependency)
    return ParsedConfiguration(
        identity: identity,
        artifacts: artifacts,
        assets: assets,
        dependencies: dependencies,
    )
}

private func parseArtifact(_ raw: JSONValue, source: some BundleSource) throws -> Artifact {
    let header = try configObject(
        raw,
        required: ["kind", "id"],
        optional: [
            "name", "path", "executables", "text", "transport", "event", "command",
            "timeout_ms", "outcomes", "tools", "protocol_version", "family", "audience",
            "destination", "content",
        ],
    )
    let id = try configString(configValue(header, "id"), label: "artifact id")
    switch try configString(configValue(header, "kind"), label: "artifact kind") {
    case "skill":
        let fields = try configObject(
            raw,
            required: ["kind", "id", "name", "path"],
            optional: ["executables"],
        )
        return try .skill(SkillArtifact(
            id: id,
            name: configString(configValue(fields, "name"), label: "skill name"),
            content: parseTree(fields, source: source),
        ))
    case "instruction":
        let fields = try configObject(raw, required: ["kind", "id"], optional: ["text", "path"])
        return try .instruction(InstructionArtifact(
            id: id,
            text: parseText(fields, source: source),
        ))
    case "mcp":
        let fields = try configObject(
            raw,
            required: ["kind", "id", "name", "transport"],
            optional: [],
        )
        guard let transport = configValue(fields, "transport") else {
            throw FlyrailValueError("MCP artifacts require a transport")
        }
        return try .mcp(MCPArtifact(
            id: id,
            name: configString(configValue(fields, "name"), label: "MCP server name"),
            transport: parseTransport(transport),
        ))
    case "hook":
        return try .hook(parseHookArtifact(raw, id: id))
    case "native":
        return try .native(parseNativeArtifact(raw, id: id, source: source))
    default:
        throw FlyrailValueError("unknown artifact kind")
    }
}

private func parseHookArtifact(_ raw: JSONValue, id: String) throws -> HookArtifact {
    let fields = try configObject(
        raw,
        required: ["kind", "id", "event", "command", "protocol_version"],
        optional: ["timeout_ms", "outcomes", "tools"],
    )
    guard configValue(fields, "protocol_version")?.signedInteger == 1 else {
        throw FlyrailValueError("hook protocol_version must be integer 1")
    }
    let timeout: Int
    if let rawTimeout = configValue(fields, "timeout_ms") {
        guard let integer = rawTimeout.signedInteger, let value = Int(exactly: integer) else {
            throw FlyrailValueError("hook timeout must be an integer")
        }
        timeout = value
    } else {
        timeout = 10000
    }
    guard let rawCommand = configValue(fields, "command") else {
        throw FlyrailValueError("hooks require a command")
    }
    let outcomes: [HookOutcome] = if let rawOutcomes = configValue(fields, "outcomes") {
        try configList(rawOutcomes).map {
            let name = try configString($0, label: "hook outcome")
            guard let value = HookOutcome(rawValue: name) else {
                throw FlyrailValueError("'\(name)' is not a valid HookOutcome")
            }
            return value
        }
    } else {
        [.continue]
    }
    let tools = try configList(configValue(fields, "tools")).map {
        try configString($0, label: "hook tool")
    }
    let eventName = try configString(configValue(fields, "event"), label: "hook event")
    guard let event = HookEvent(rawValue: eventName) else {
        throw FlyrailValueError("'\(eventName)' is not a valid HookEvent")
    }
    return try HookArtifact(
        id: id,
        event: event,
        command: parseCommand(rawCommand),
        timeoutMilliseconds: timeout,
        outcomes: outcomes,
        tools: tools,
    )
}

private func parseNativeArtifact(
    _ raw: JSONValue,
    id: String,
    source: some BundleSource,
) throws -> NativeArtifact {
    let fields = try configObject(
        raw,
        required: ["kind", "id", "family", "audience", "destination", "content"],
        optional: [],
    )
    guard let rawAudience = configValue(fields, "audience"),
          let rawContent = configValue(fields, "content")
    else {
        throw FlyrailValueError("native artifacts require content")
    }
    return try NativeArtifact(
        id: id,
        family: parseEnum(
            configValue(fields, "family"),
            label: "native family",
            as: Family.self,
        ),
        audience: parseAudience(rawAudience),
        destination: sourcePath(configValue(fields, "destination")),
        content: parseContent(rawContent, source: source),
    )
}

private func parseAsset(_ raw: JSONValue, source: some BundleSource) throws -> SupportAsset {
    let fields = try configObject(
        raw,
        required: ["id", "family", "path"],
        optional: ["executables"],
    )
    return try SupportAsset(
        id: configString(configValue(fields, "id"), label: "asset id"),
        family: parseEnum(configValue(fields, "family"), label: "asset family", as: Family.self),
        content: parseTree(fields, source: source),
    )
}

private func parseDependency(_ raw: JSONValue) throws -> Dependency {
    let fields = try configObject(raw, required: ["dependent", "required"], optional: ["mode"])
    let mode: DependencyMode = if let rawMode = configValue(fields, "mode") {
        try parseEnum(rawMode, label: "dependency mode", as: DependencyMode.self)
    } else {
        .revision
    }
    return try Dependency(
        dependent: configString(configValue(fields, "dependent"), label: "dependent"),
        required: configString(configValue(fields, "required"), label: "required"),
        mode: mode,
    )
}

private func parseAudience(_ raw: JSONValue) throws -> Audience {
    let fields = try configObject(
        raw,
        required: ["agent", "scope"],
        optional: ["surface", "platform"],
    )
    let surface: Surface = if let value = configValue(fields, "surface") {
        try parseEnum(value, label: "surface", as: Surface.self)
    } else {
        .cli
    }
    let platform: Platform? = if let value = configValue(fields, "platform") {
        try parseEnum(value, label: "platform", as: Platform.self)
    } else {
        nil
    }
    return try Audience(
        agent: parseEnum(configValue(fields, "agent"), label: "agent", as: Agent.self),
        scope: parseEnum(configValue(fields, "scope"), label: "scope", as: TargetScope.self),
        surface: surface,
        platform: platform,
    )
}

private func parseTransport(_ raw: JSONValue) throws -> MCPTransport {
    let header = try configObject(
        raw,
        required: ["kind"],
        optional: ["command", "url", "bearer_token_env"],
    )
    switch try configString(configValue(header, "kind"), label: "MCP transport") {
    case "stdio":
        let fields = try configObject(raw, required: ["kind", "command"], optional: [])
        guard let command = configValue(fields, "command") else {
            throw FlyrailValueError("stdio transports require a command")
        }
        return try .stdio(parseCommand(command))
    case "http":
        let fields = try configObject(
            raw,
            required: ["kind", "url"],
            optional: ["bearer_token_env"],
        )
        let token = try configValue(fields, "bearer_token_env").map {
            try EnvRef(configString($0, label: "bearer environment reference"))
        }
        return try .http(HttpTransport(
            url: configString(configValue(fields, "url"), label: "MCP URL"),
            bearerToken: token,
        ))
    default:
        throw FlyrailValueError("unknown MCP transport")
    }
}

private func parseCommand(_ raw: JSONValue) throws -> Command {
    let fields = try configObject(raw, required: ["argv"], optional: ["env", "cwd"])
    let argv = try configList(configValue(fields, "argv")).map(parseArgument)
    let environment: [EnvironmentEntry]
    if let rawEnvironment = configValue(fields, "env") {
        guard case let .object(items) = rawEnvironment else {
            throw FlyrailValueError("command env must be a JSON object")
        }
        environment = try items.map { name, rawValue in
            let value: EnvironmentValue
            if case .object = rawValue {
                let reference = try configObject(rawValue, required: ["env"], optional: [])
                value = try .reference(EnvRef(configString(
                    configValue(reference, "env"),
                    label: "environment reference",
                )))
            } else {
                value = try .text(configString(rawValue, label: "environment value"))
            }
            return EnvironmentEntry(name: name, value: value)
        }
    } else {
        environment = []
    }
    return try Command(
        argv: argv,
        env: environment,
        cwd: configValue(fields, "cwd").map(parseArgument),
    )
}

private func parseArgument(_ raw: JSONValue) throws -> CommandArgument {
    guard case .object = raw else {
        return try .text(configString(raw, label: "command argument"))
    }
    let fields = try configObject(raw, required: ["asset"], optional: ["path"])
    return try .asset(AssetRef(
        assetID: configString(configValue(fields, "asset"), label: "referenced asset id"),
        path: configValue(fields, "path").map { try sourcePath($0) },
    ))
}

private func parseContent(_ raw: JSONValue, source: some BundleSource) throws -> Content {
    _ = try configObject(
        raw,
        required: ["kind"],
        optional: [
            "path", "text", "mode", "executables", "marker", "format", "selector",
            "value", "semantic_value", "placement", "boundaries",
        ],
    )
    guard case let .object(header) = raw else {
        throw FlyrailValueError("manifest records must be JSON objects")
    }
    switch try configString(configValue(header, "kind"), label: "content kind") {
    case "file":
        let fields = try configObject(raw, required: ["kind", "path"], optional: ["mode"])
        let mode: FileMode = if let value = configValue(fields, "mode") {
            try parseEnum(value, label: "file mode", as: FileMode.self)
        } else {
            .private
        }
        return try .file(FileContent(
            data: source.readFile(path: sourcePath(configValue(fields, "path"))),
            mode: mode,
        ))
    case "tree":
        let fields = try configObject(
            raw,
            required: ["kind", "path"],
            optional: ["executables"],
        )
        return try .tree(parseTree(fields, source: source))
    case "section":
        let fields = try configObject(
            raw,
            required: ["kind", "marker"],
            optional: ["text", "path", "boundaries"],
        )
        let boundaries = try configValue(fields, "boundaries").map { rawBoundaries in
            let values = try configObject(rawBoundaries, required: ["start", "end"], optional: [])
            return try SectionBoundaries(
                start: configString(configValue(values, "start"), label: "start boundary"),
                end: configString(configValue(values, "end"), label: "end boundary"),
            )
        }
        return try .section(SectionContent(
            marker: configString(configValue(fields, "marker"), label: "section marker"),
            text: parseText(fields, source: source),
            boundaries: boundaries,
        ))
    case "structured":
        let fields = try configObject(
            raw,
            required: ["kind", "format", "selector"],
            optional: ["value", "semantic_value", "placement"],
        )
        let placement = try configValue(fields, "placement").map(parsePlacement)
        guard let rawSelector = configValue(fields, "selector") else {
            throw FlyrailValueError("structured content requires a selector")
        }
        return try .structured(StructuredContent(
            format: parseEnum(
                configValue(fields, "format"),
                label: "document format",
                as: DocumentFormat.self,
            ),
            selector: parseSelector(rawSelector),
            value: configSemanticValue(fields, plain: "value", typed: "semantic_value"),
            placement: placement,
        ))
    default:
        throw FlyrailValueError("unknown native content kind")
    }
}

private func parseTree(
    _ fields: [(String, JSONValue)],
    source: some BundleSource,
) throws -> TreeContent {
    let executables = try configList(configValue(fields, "executables")).map(sourcePath)
    guard Set(executables).count == executables.count else {
        throw FlyrailValueError("duplicate executable path")
    }
    let path = try sourcePath(configValue(fields, "path"))
    let entries = try source.readTree(path: path, executables: executables)
    let files = Set(entries.filter { !$0.isDirectory }.map(\.path))
    guard executables.allSatisfy(files.contains) else {
        throw FlyrailValueError("every executable must name an existing file")
    }
    return try TreeContent(entries: entries)
}

private func parseText(
    _ fields: [(String, JSONValue)],
    source: some BundleSource,
) throws -> String {
    let inline = configValue(fields, "text")
    let path = configValue(fields, "path")
    guard (inline != nil) != (path != nil) else {
        throw FlyrailValueError("provide exactly one of text or path")
    }
    if let inline {
        return try configString(inline, label: "text")
    }
    let data = try source.readFile(path: sourcePath(path))
    guard let text = String(data: data, encoding: .utf8) else {
        throw FlyrailValueError("text is not valid UTF-8")
    }
    return text
}

private func parseSelector(_ raw: JSONValue) throws -> Selector {
    try Selector(parts: configList(raw).map { value in
        if case let .string(name) = value {
            return try .key(Key(name))
        }
        return try .member(parseMember(value))
    })
}

private func parseMember(_ raw: JSONValue) throws -> Member {
    let fields = try configObject(
        raw,
        required: [],
        optional: ["identity", "semantic_identity", "key"],
    )
    let key = try configList(configValue(fields, "key")).map {
        try configString($0, label: "member key")
    }
    return try Member(
        identity: configSemanticValue(fields, plain: "identity", typed: "semantic_identity"),
        key: key,
    )
}

private func parsePlacement(_ raw: JSONValue) throws -> Placement {
    let fields = try configObject(raw, required: ["position"], optional: ["anchor"])
    return try Placement(
        position: parseEnum(
            configValue(fields, "position"),
            label: "placement position",
            as: Position.self,
        ),
        anchor: configValue(fields, "anchor").map(parseMember),
    )
}

private func configSemanticValue(
    _ fields: [(String, JSONValue)],
    plain: String,
    typed: String,
) throws -> SemanticValue {
    let rawPlain = configValue(fields, plain)
    let rawTyped = configValue(fields, typed)
    guard (rawPlain != nil) != (rawTyped != nil) else {
        throw FlyrailValueError("provide exactly one of \(plain) or \(typed)")
    }
    if let rawPlain {
        return try semanticValue(from: rawPlain)
    }
    guard let rawTyped else {
        throw FlyrailValueError("provide exactly one of \(plain) or \(typed)")
    }
    return try SemanticValue(semanticBytes: JSONWriter.canonicalData(rawTyped))
}

private func semanticValue(from raw: JSONValue) throws -> SemanticValue {
    switch raw {
    case .null:
        .scalar(.null)
    case let .boolean(value):
        .scalar(.boolean(value))
    case let .number(token):
        if let integer = raw.signedInteger {
            .scalar(.integer(integer))
        } else if !token.contains("."), !token.contains("e"), !token.contains("E") {
            throw FlyrailValueError("integers must fit signed 64 bits")
        } else if let number = Double(token), number.isFinite {
            try .scalar(.float(Binary64(number)))
        } else {
            throw FlyrailValueError("floats must be finite")
        }
    case let .string(value):
        .scalar(.text(value))
    case let .array(values):
        try .array(values.map(semanticValue))
    case let .object(items):
        try SemanticValue(object: items.map { try ($0.0, semanticValue(from: $0.1)) })
    }
}

private func sourcePath(_ raw: JSONValue?) throws -> String {
    let path = try configString(raw, label: "source path")
    try validateRelativePath(path, label: "source path")
    return path
}

private func parseManifestJSON(_ data: Data) throws -> JSONValue {
    guard !data.starts(with: [0xEF, 0xBB, 0xBF]) else {
        throw FlyrailValueError("manifest is not valid UTF-8")
    }
    do {
        return try JSONParser.parse(data)
    } catch let error as FlyrailValueError where error.message == "JSON must be UTF-8" {
        throw FlyrailValueError("manifest is not valid UTF-8")
    }
}

private func configObject(
    _ raw: JSONValue,
    required: [String],
    optional: [String],
) throws -> [(String, JSONValue)] {
    guard case let .object(items) = raw else {
        throw FlyrailValueError("manifest records must be JSON objects")
    }
    let allowed = required + optional
    guard items.allSatisfy({ item in allowed.contains(where: { exactTextEqual($0, item.0) }) }),
          required.allSatisfy({ name in items.contains(where: { exactTextEqual($0.0, name) }) })
    else {
        throw FlyrailValueError(
            "expected fields \(configFieldList(required)); optional fields \(configFieldList(optional))",
        )
    }
    return items
}

private func configList(_ raw: JSONValue?) throws -> [JSONValue] {
    guard let raw else { return [] }
    guard case let .array(values) = raw else {
        throw FlyrailValueError("expected a JSON array")
    }
    return values
}

private func configValue(_ fields: [(String, JSONValue)], _ name: String) -> JSONValue? {
    fields.first(where: { exactTextEqual($0.0, name) })?.1
}

private func configString(_ raw: JSONValue?, label: String) throws -> String {
    guard case let .string(value)? = raw else {
        throw FlyrailValueError("\(label) must be a string")
    }
    return value
}

private func parseEnum<T: RawRepresentable>(
    _ raw: JSONValue?,
    label: String,
    as _: T.Type,
) throws -> T where T.RawValue == String {
    let value = try configString(raw, label: label)
    guard let result = T(rawValue: value) else {
        throw FlyrailValueError("'\(value)' is not a valid \(T.self)")
    }
    return result
}

private func configFieldList(_ fields: [String]) -> String {
    "[" + fields.sorted().map { "'\($0)'" }.joined(separator: ", ") + "]"
}
