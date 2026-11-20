import CryptoKit
import Foundation

private func loadoutID(_ values: String...) -> String {
    SHA256.hash(data: Data(values.joined(separator: "\0").utf8)).hexadecimal
}

private func moduleMapping(_ agent: Agent) -> (folder: String, prefix: String)? {
    switch agent {
    case .opencode: ("plugins", ".config/opencode/plugins/")
    case .pi: ("extensions", ".pi/agent/extensions/")
    default: nil
    }
}

package func loadoutRequiredSlices(_ bundle: FlyrailBundle, harnesses: [Agent]) -> [LoadoutSlice] {
    bundle.artifacts.flatMap { artifact in
        if case let .native(item) = artifact {
            return [LoadoutSlice(agent: item.audience.agent, category: "module-config")]
        }
        return harnesses.map { agent in
            let category = switch artifact {
            case .skill: "skills"
            case .instruction: "instructions"
            case .mcp: "mcp"
            case .hook: moduleMapping(agent) == nil ? "hooks" : "module-config"
            case .native: "module-config"
            }
            return LoadoutSlice(agent: agent, category: category)
        }
    }
}

package func loadoutHooksSupported(_ bundle: FlyrailBundle, harnesses: [Agent]) -> Bool {
    !harnesses.contains(.droid) || !bundle.artifacts.contains { item in
        if case .hook = item {
            return true
        }
        return false
    }
}

private func loadoutFiltered(_ path: String) -> Bool {
    let components = path.split(separator: "/")
    let forbidden = [
        ".git",
        ".hg",
        "node_modules",
        ".venv",
        "__pycache__",
        ".mypy_cache",
        ".ruff_cache",
        ".pytest_cache",
    ]
    return components.contains { forbidden.contains(String($0)) } ||
        components.last.map { [".DS_Store", ".loadout-bundle.toml"].contains(String($0)) } == true ||
        [".pyc", ".pyo", ".swp"].contains { path.hasSuffix($0) }
}

private func loadoutStructured(_ keys: [String], _ value: SemanticValue, member: Bool = false) throws -> Content {
    var path = try keys.map { try SelectorPart.key(Key($0)) }
    if member {
        try path.append(.member(Member(identity: value)))
    }
    return try .structured(StructuredContent(format: .toml, selector: Selector(parts: path), value: value))
}

package struct LoadoutSourceRender {
    let bundle: FlyrailBundle
    let destination: LoadoutDestination
    let options: LoadoutOptions
    private(set) var inputs = LoadoutInputs()
    private(set) var impacts: [LoadoutImpact] = []
    private var output: [RenderedArtifact] = []
    private var dependencies: Set<Dependency> = []
    private var notices: [Notice] = []
    private var assets: [String: ResourcePath] = [:]

    init(bundle: FlyrailBundle, destination: LoadoutDestination, options: LoadoutOptions) {
        self.bundle = bundle
        self.destination = destination
        self.options = options
    }

    private var activeFile: ResourcePath {
        destination.setup.profileChain.first ?? destination.setup.root
    }

    private func problem(_ code: String, _ message: String, _ path: ResourcePath? = nil) -> LoadoutDiagnostic {
        LoadoutDiagnostic(code, message, path ?? activeFile)
    }

    private func gate() throws {
        for agent in destination.harnesses {
            guard agent != .copilot else { throw problem(
                "unsupported-harness",
                "Copilot is outside the Loadout harness set",
            ) }
            guard destination.setup.harnesses.contains(where: { $0.agent == agent }) else {
                throw problem("disabled-harness", "\(agent.rawValue) is absent from this profile")
            }
        }
        if !loadoutHooksSupported(bundle, harnesses: destination.harnesses) {
            throw problem("unsupported-hook", "Droid hooks have no verified Loadout mapping")
        }
        for item in loadoutRequiredSlices(bundle, harnesses: destination.harnesses) {
            let source = destination.source
            guard source.uses.contains(item.category) else {
                throw problem("source-use", "Source \(source.name) does not offer \(item.category)", source.declaredIn)
            }
            guard destination.harnesses.contains(item.agent) else {
                throw problem(
                    "native-audience",
                    "Native artifact targets \(item.agent.rawValue) outside this selection",
                )
            }
            guard destination.setup.hasSlice(item) else {
                throw problem("disabled-slice", "\(item.agent.rawValue).\(item.category) is disabled in this profile")
            }
            guard !destination.setup.manifest.customRoutes.contains(item) else {
                throw problem(
                    "unsupported-slice",
                    "\(item.agent.rawValue).\(item.category) uses a custom legacy renderer",
                )
            }
        }
    }

    private func context(_ agent: Agent) throws -> RenderContext {
        let target = try Target.user(agent: agent, home: destination.setup.home)
        var context = try RenderContext(target: target, platform: options.platform)
            .assetRoot(appendPath(destination.source.path, ".flyrail-assets"))
        if let path = options.executionRoot {
            context = try context.executionRoot(path)
        }
        if let runtime = options.hookRuntime {
            context = try context.hookRuntime(runtime)
        }
        return context
    }

    private func safe(_ path: ResourcePath) throws -> ResourcePath {
        guard loadoutContains(destination.source.path, path),
              try NativeTransactionDirectory.physicalLocation(path) == path
        else {
            throw problem("escaped-path", "Source destination must stay within its declared root without aliases", path)
        }
        return path
    }

    private mutating func impact(
        _ category: String, _ path: ResourcePath, agents: [Agent], inherited: Bool = false,
    ) {
        let shared = inherited || agents.contains { !destination.harnesses.contains($0) }
        let effect = LoadoutImpact(category: category, path: path,
                                   harnesses: Set(agents).sorted { $0.rawValue < $1.rawValue }, shared: shared)
        if !impacts.contains(effect) {
            impacts.append(effect)
        }
    }

    private mutating func fileSeen(_ source: LoadoutSource, _ relative: String) throws -> Bool {
        try inputs.file(appendPath(source.path, relative), code: "invalid-source", within: source.path) != nil
    }

    private mutating func write(
        _ name: String, _ family: Family, _ path: ResourcePath, _ content: Content, subtree: String? = nil,
    ) throws {
        try output.append(RenderedArtifact(
            id: name,
            family: family,
            destination: path,
            content: content,
            subtree: subtree,
        ))
    }

    private mutating func depends(_ name: String, on required: String, mode: DependencyMode = .revision) throws {
        try dependencies.insert(Dependency(dependent: name, required: required, mode: mode))
    }

    private mutating func skill(_ artifact: SkillArtifact) throws {
        let name = artifact.name
        let root = try safe(appendPath(destination.source.path, "skills"))
        guard !loadoutFiltered(name), !artifact.content.entries.contains(where: { loadoutFiltered($0.path) }) else {
            throw problem("skill-filter", "Loadout excludes part of skill \(name)", root)
        }
        for source in destination.setup.sources where source.uses.contains("skills") {
            if source.overrides["skills"]?.contains(name) == true {
                throw problem("source-collision", "An override controls skill \(name)", source.declaredIn)
            }
            let relative = "skills/\(name)/SKILL.md"
            if try fileSeen(source, "skills/\(name).local/SKILL.md") ||
                (source != destination.source && fileSeen(source, relative))
            {
                throw try problem(
                    "source-collision",
                    "Skill \(name) also resolves from \(source.name)",
                    appendPath(source.path, relative),
                )
            }
        }
        let paths = destination.setup.manifest.policies[name] ?? [activeFile]
        guard paths.count == 1, let file = paths.first else {
            throw problem("skill-policy", "Split declarations control skill \(name)")
        }
        impact("skills", file, agents: destination.harnesses, inherited: file != activeFile)
        let policy = loadoutID("skill-policy", artifact.id)
        try write(policy, .skills, file, loadoutStructured(["skills", name], .object([
            ("agents", .array(destination.harnesses.map { .scalar(.text($0.rawValue)) })),
            ("render", .scalar(.boolean(false))),
        ])))
        try write(artifact.id, .skills, root, .tree(artifact.content), subtree: name)
        try depends(artifact.id, on: policy)
    }

    private func routes(_ category: String) -> [[LoadoutSelector]] {
        let groups = Dictionary(grouping: destination.setup.selectors.filter { $0.category == category }) {
            [$0.manifest.displayString] + $0.key
        }
        return groups.keys.sorted { $0.lexicographicallyPrecedes($1) }.compactMap { key in
            groups[key].flatMap { group in group.contains { destination.harnesses.contains($0.agent) } ? group : nil }
        }
    }

    private mutating func entry(
        _ group: [LoadoutSelector], _ name: String, _ family: Family, requires: String,
    ) throws {
        guard let route = group.first else { throw problem("disabled-slice", "No selector for this fragment") }
        let entry = destination.source.name + "/" + name
        let path = route.manifest
        impact(route.category, path, agents: group.map(\.agent),
               inherited: path != activeFile || group.contains { $0.destinations > 1 || $0.allProfiles })
        guard route.array else {
            throw problem("selector-not-array", "Write the fragment selector as an array before adding a member", path)
        }
        guard !group.contains(where: { selector in
            selector.substitutions[entry] != nil || selector.entries.contains {
                $0 != entry && [name, entry].contains(selector.substitutions[$0] ?? $0)
            }
        }) else {
            throw problem("selector-shadowed", "An existing selector or substitution shadows \(entry)", path)
        }
        let keys = ["selector", path.displayString] + route.key + [entry]
        let identifier = SHA256.hash(data: Data(keys.joined(separator: "\0").utf8)).hexadecimal
        try write(identifier, family, path, loadoutStructured(route.key, .scalar(.text(entry)), member: true))
        try depends(identifier, on: requires, mode: .stableReference)
    }

    private mutating func fragment(_ category: String, _ relative: String) throws -> ResourcePath {
        let path = try safe(appendPath(destination.source.path, relative))
        for source in destination.setup.sources where source != destination.source && source.uses.contains(category) {
            if try fileSeen(source, relative) {
                throw try problem(
                    "source-collision",
                    "Fragment also resolves from \(source.name)",
                    appendPath(source.path, relative),
                )
            }
        }
        return path
    }

    private mutating func instruction(_ item: InstructionArtifact) throws {
        let name = "flyrail-" + loadoutID("instructions", bundle.identity.id, item.id)
        let path = try fragment("instructions", "instructions/\(name).md")
        let groups = routes("instructions")
        try checkUnselected("instructions", name, groups)
        try write(item.id, .instructions, path, .file(FileContent(data: Data(item.text.utf8))))
        for group in groups {
            try entry(group, name, .instructions, requires: item.id)
        }
    }

    private func checkUnselected(_ category: String, _ name: String, _ groups: [[LoadoutSelector]]) throws {
        let covered = Set(groups.flatMap(\.self))
        let qualified = destination.source.name + "/" + name
        for selector in destination.setup.selectors
            where selector.category == category && !covered.contains(selector)
        {
            if selector.entries.contains(where: {
                [name, qualified].contains(selector.substitutions[$0] ?? $0)
            }) {
                throw problem(
                    "selector-shadowed",
                    "An unselected \(category) target refers to \(qualified)",
                    selector.manifest,
                )
            }
        }
    }

    private mutating func serverTables() throws -> [String: [String]] {
        var tables: [String: [String]] = [:]
        for source in destination.setup.sources where source.uses.contains("mcp") {
            let path = try appendPath(source.path, "mcp.toml")
            let table = try inputs.table(path, code: "invalid-mcp", within: source.path) ?? [:]
            let names = table.keys.sorted(by: utf8Less)
            for name in names {
                guard let fields = try? loadoutTable(table[name], path, name),
                      let transport = loadoutText(fields["transport"])
                else { throw problem("invalid-mcp", "Invalid Loadout MCP definition \(name)", path) }
                let key = transport == "http" ? "url" : "command"
                guard ["stdio", "http"].contains(transport), let text = loadoutText(fields[key]),
                      !text.isEmpty, validMCPFields(fields, transport, path), Set(fields.keys).isSubset(of: [
                          "transport",
                          "url",
                          "auth_env_var",
                          "command",
                          "args",
                          "env",
                          "headers",
                      ])
                else {
                    throw problem("invalid-mcp", "Invalid Loadout MCP definition \(name)", path)
                }
            }
            tables[source.name] = names
        }
        return tables
    }

    private func validMCPFields(_ fields: LoadoutTable, _ transport: String, _ path: ResourcePath) -> Bool {
        guard let headers = try? loadoutTable(fields["headers"], path, "headers"),
              let environment = try? loadoutTable(fields["env"], path, "env"),
              headers.isEmpty || transport == "http"
        else { return false }
        let auth = loadoutText(fields["auth_env_var"])
        guard fields["auth_env_var"] == nil || auth != nil,
              environment.values.allSatisfy({ loadoutText($0) != nil }),
              headers.allSatisfy({ key, value in
                  guard let text = loadoutText(value), !text.contains("$"), !text.contains("{"), !text.hasPrefix("!")
                  else { return false }
                  return !(auth?.isEmpty == false && key.lowercased() == "authorization")
              })
        else { return false }
        return fields["args"].map { (try? loadoutStrings($0, path, "args")) != nil } ?? true
    }

    private mutating func checkMCPPermissions(_ servers: [String]) throws {
        let scoped = destination.setup.harnesses.filter {
            [.opencode, .pi].contains($0.agent) && $0.slices.contains("permissions")
        }
        guard !scoped.isEmpty else { return }
        for source in destination.setup.sources where source.uses.contains("permissions") {
            let path = try appendPath(source.path, "permissions.toml")
            let table = try inputs.table(path, code: "invalid-permissions", within: source.path) ?? [:]
            guard let rules = try? loadoutTable(table["mcp"], path, "mcp"),
                  Set(rules.keys).isSubset(of: ["allow", "deny", "ask"])
            else { throw problem("invalid-permissions", "Expected MCP permission decision lists", path) }
            for decision in rules.keys.sorted(by: utf8Less) {
                for rule in try loadoutStrings(rules[decision], path, decision, code: "invalid-permissions") {
                    let parts = rule.split(separator: "/", maxSplits: 1, omittingEmptySubsequences: false)
                    guard parts.count == 2, !parts[0].isEmpty, !parts[1].isEmpty else {
                        throw problem("invalid-permissions", "MCP permissions must use server/tool entries", path)
                    }
                    if scoped.contains(where: { target in
                        servers.contains {
                            loadoutPermissionOverlap(String(parts[0]), String(parts[1]), $0, pi: target.agent == .pi)
                        }
                    }) {
                        throw problem(
                            "mcp-permission-overlap",
                            "MCP permission \(rule) overlaps a proposed server namespace",
                            path,
                        )
                    }
                }
            }
        }
    }

    private mutating func mcp(_ item: MCPArtifact, tables: [String: [String]]) throws {
        let path = try safe(appendPath(destination.source.path, "mcp.toml"))
        let agents = destination.setup.harnesses.filter { $0.slices.contains("mcp") }.map(\.agent)
        impact("mcp", path, agents: agents)
        guard !agents.contains(where: {
            destination.setup.manifest.customRoutes.contains(LoadoutSlice(agent: $0, category: "mcp"))
        }) else {
            throw problem("unsupported-slice", "MCP also reaches a custom legacy renderer", path)
        }
        for source in destination.setup.sources where tables[source.name]?.contains(where: {
            ($0 != item.name || source != destination.source) &&
                (portablePathKey($0) == portablePathKey(item.name) ||
                    (agents.contains(.pi) && $0.replacingOccurrences(of: "-", with: "_") ==
                        item.name.replacingOccurrences(of: "-", with: "_")))
        }) == true {
            throw try problem(
                "source-collision",
                "MCP name \(item.name) collides in \(source.name)",
                appendPath(source.path, "mcp.toml"),
            )
        }
        let value = try server(item, agents.map(context))
        try write(item.id, .mcp, path, loadoutStructured([item.name], value))
    }

    private func server(_ item: MCPArtifact, _ contexts: [RenderContext]) throws -> SemanticValue {
        switch item.transport {
        case let .http(transport):
            for context in contexts {
                _ = try literal(transport.url, context: context, field: "url")
            }
            var fields: [(String, SemanticValue)] = [
                ("transport", .scalar(.text("http"))), ("url", .scalar(.text(transport.url))),
            ]
            if let token = transport.bearerToken {
                fields.append(("auth_env_var", .scalar(.text(token.name))))
            }
            return try SemanticValue(object: fields)
        case let .stdio(command):
            guard command.cwd == nil else {
                throw problem("mcp-cwd", "Loadout cannot preserve the cwd of MCP server \(item.name)")
            }
            let argv = try command.argv.map { try translatedAsset($0, assets: assets) }
            guard let program = argv.first else { throw problem("mcp-command", "Expected an MCP command") }
            let variables = try command.env.map { entry in
                guard case let .text(value) = entry.value else {
                    throw problem(
                        "mcp-env-reference",
                        "Loadout cannot preserve stdio environment references for \(item.name)",
                    )
                }
                return (entry.name, value)
            }
            for context in contexts {
                for arg in argv {
                    _ = try literal(arg, context: context, field: "argv")
                }
                for (_, value) in variables {
                    _ = try literal(value, context: context, field: "env")
                }
                if context.audience.agent == .pi, (argv + variables.map(\.1)).contains(where: { $0.hasPrefix("!") }) {
                    throw problem("mcp-literal", "Pi could interpret a literal MCP value")
                }
            }
            return .object([
                ("transport", .scalar(.text("stdio"))), ("command", .scalar(.text(program))),
                ("args", .array(argv.dropFirst().map { .scalar(.text($0)) })),
                ("env", .object(variables.map { ($0.0, .scalar(.text($0.1))) })),
            ])
        }
    }

    private mutating func modulePath(_ agent: Agent, _ relative: String) throws -> ResourcePath {
        let tail = "\(agent.rawValue)/\(relative)"
        for source in destination.setup.sources where source.uses.contains("module-config") {
            if source.overrides["module-config"]?.contains(tail) == true {
                throw problem("source-collision", "An override controls \(tail)", source.declaredIn)
            }
        }
        return try fragment("module-config", "module-config/" + tail)
    }

    private mutating func native(_ item: NativeArtifact) throws {
        let audience = item.audience
        guard let mapping = moduleMapping(audience.agent), item.destination.hasPrefix(mapping.prefix),
              audience.scope == .user, audience.surface == .cli,
              audience.platform == nil || audience.platform == options.platform, case .file = item.content
        else {
            throw problem("native-path", "No Loadout source mapping for native artifact \(item.id)")
        }
        let relative = String(item.destination.dropFirst(mapping.prefix.count))
        guard !relative.contains("/"), [".js", ".mjs", ".ts"].contains(where: { relative.hasSuffix($0) }),
              !["loadout-hooks.js", "loadout-hooks.ts"].contains(relative), !loadoutFiltered(relative)
        else {
            throw problem("native-path", "No Loadout source mapping for native artifact \(item.id)")
        }
        let path = try modulePath(audience.agent, mapping.folder + "/" + relative)
        impact("module-config", path, agents: [audience.agent])
        try write(item.id, item.family, path, item.content)
    }

    private mutating func hookFragment(
        _ agent: Agent, _ authored: [RenderedArtifact], _ hooks: [String: HookArtifact],
    ) throws -> String {
        let name = "flyrail-" + loadoutID("hooks", bundle.identity.id, agent.rawValue)
        let path = try fragment("hooks", "hooks/\(name).json")
        var events: LoadoutTable = [:]
        for item in authored {
            guard case let .structured(content) = item.content, let hook = hooks[item.id] else {
                throw problem("hook-source", "Expected a structured hook registration", path)
            }
            var value = content.value
            if agent == .cursor {
                guard case let .object(fields) = value else { throw problem(
                    "hook-source",
                    "Expected Cursor hook object",
                    path,
                ) }
                value = .object([("hooks", .array([.object(fields + [("type", .scalar(.text("command")))])]))])
            }
            events[hookEventName(.codex, hook.event)] = .array([value])
        }
        let value = try SemanticValue(object: events.map { ($0.key, $0.value) })
        let data = try encodeJSONValue(value) + Data([10])
        let identifier = loadoutID("hook-fragment", bundle.identity.id, agent.rawValue)
        try write(identifier, .hooks, path, .file(FileContent(data: data)))
        let selected = routes("hooks").filter { group in group.contains { $0.agent == agent } }
        try checkUnselected("hooks", name, selected)
        for group in selected {
            guard !group.contains(where: { $0.agent != agent }) else {
                throw problem(
                    "hook-shared-selector",
                    "A shared hook selector cannot preserve separate harness ABIs. Declare separate arrays.",
                )
            }
            try entry(group, name, .hooks, requires: identifier)
        }
        return identifier
    }

    private mutating func hookModule(_ agent: Agent, _ folder: String,
                                     _ authored: [RenderedArtifact]) throws -> String
    {
        guard let item = authored.first, let name = item.destination.displayString.split(separator: "/").last else {
            throw problem("hook-source", "No generated bridge")
        }
        let path = try modulePath(agent, folder + "/" + name)
        impact("module-config", path, agents: [agent])
        let identifier = loadoutID("hook-module", bundle.identity.id, agent.rawValue)
        try write(identifier, .hooks, path, item.content)
        return identifier
    }

    private mutating func renderHooks() throws -> [String: [String]] {
        let hooks: [String: HookArtifact] = Dictionary(uniqueKeysWithValues: bundle.artifacts.compactMap {
            if case let .hook(hook) = $0 {
                return (hook.id, hook)
            }
            return nil
        })
        guard !hooks.isEmpty else { return [:] }
        var aliases: [String: [String]] = [:]
        for agent in destination.harnesses {
            let translation = try hookArtifacts(
                bundle: bundle,
                context: context(agent),
                assets: assets,
                revisionLauncher: true,
            )
            let authored = translation.artifacts.filter { hooks[$0.id] != nil }
            let support = translation.artifacts.filter { hooks[$0.id] == nil }
            for asset in support {
                _ = try safe(asset.destination)
            }
            output += support
            let name: String = if let module = moduleMapping(agent) {
                try hookModule(agent, module.folder, authored)
            } else {
                try hookFragment(agent, authored, hooks)
            }
            for item in authored {
                aliases[item.id, default: []].append(name)
            }
            for edge in translation.dependencies {
                if hooks[edge.dependent] != nil {
                    try depends(name, on: edge.required, mode: edge.mode)
                } else if hooks[edge.required] == nil {
                    dependencies.insert(edge)
                }
            }
            notices += translation.notices
        }
        return aliases
    }

    mutating func render(prefix: String) throws -> RenderedBundle {
        do { return try build(prefix: prefix) } catch let issue as UnsupportedTranslation {
            throw problem(issue.code, issue.message)
        }
    }

    private mutating func build(prefix: String) throws -> RenderedBundle {
        try gate()
        guard let agent = destination.harnesses.first else { throw problem("disabled-harness", "No selected harness") }
        let (assets, paths) = try supportAssetArtifacts(bundle: bundle, context: context(agent))
        for item in assets {
            _ = try safe(item.destination)
        }
        self.assets = paths
        let names = bundle.artifacts.compactMap { item -> String? in
            if case let .mcp(mcp) = item {
                return mcp.name
            }
            return nil
        }
        guard Set(names).count == names.count else {
            throw problem("source-collision", "A bundle may declare each MCP server once")
        }
        let servers = try names.isEmpty ? [:] : serverTables()
        if !names.isEmpty {
            try checkMCPPermissions(names)
        }
        for artifact in bundle.artifacts {
            switch artifact {
            case let .skill(item): try skill(item)
            case let .instruction(item): try instruction(item)
            case let .mcp(item): try mcp(item, tables: servers)
            case let .native(item): try native(item)
            case .hook: break
            }
        }
        let aliases = try renderHooks()
        for edge in bundle.dependencies {
            for dependent in aliases[edge.dependent] ?? [edge.dependent] {
                for required in aliases[edge.required] ?? [edge.required] where dependent != required {
                    try depends(dependent, on: required, mode: edge.mode)
                }
            }
        }
        if !destination.acceptSharedScope, let item = impacts.first(where: \.shared) {
            throw problem(
                "shared-scope",
                "Approve the shared source and selector scope shown in the preview",
                item.path,
            )
        }
        try notices.append(loadoutSyncNotice(bundle.identity.id))
        func qualify(_ id: String) -> String {
            prefix + loadoutID("loadout", id).prefix(31)
        }
        let artifacts = try (output + assets).map { item in
            try RenderedArtifact(id: qualify(item.id), family: item.family,
                                 destination: item.destination, content: item.content, subtree: item.subtree,
                                 requireExisting: item.requireExisting, schemaRequirements: item.schemaRequirements)
        }
        let edges = try dependencies.map { edge in
            try Dependency(dependent: qualify(edge.dependent),
                           required: qualify(edge.required), mode: edge.mode)
        }
        return try RenderedBundle(artifacts: artifacts, dependencies: edges, notices: notices)
    }
}

private func loadoutPermissionOverlap(_ other: String, _ tool: String, _ server: String, pi: Bool) -> Bool {
    func encoded(_ text: String) -> String {
        String(text.unicodeScalars.map { character in
            let code = character.value
            let ascii = (48 ... 57).contains(code) || (65 ... 90).contains(code) || (97 ... 122).contains(code)
            return ascii || "_*?".unicodeScalars.contains(character) || (!pi && character == "-")
                ? Character(character) : "_"
        })
    }
    let separator = pi ? "__" : "_"
    let prefix = encoded(server) + separator
    let namespace = encoded(other) + separator
    guard !other.contains("*"), !other.contains("?") else { return false }
    if prefix == namespace {
        return !exactTextEqual(other, server)
    }
    let pattern = namespace + encoded(tool)
    for (actual, expected) in zip(pattern, prefix) {
        if actual == "*" {
            return true
        }
        if actual != "?", actual != expected {
            return false
        }
    }
    return pattern.utf8.count > prefix.utf8.count
}
