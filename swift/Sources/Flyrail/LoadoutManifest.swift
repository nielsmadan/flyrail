import Foundation

package typealias LoadoutTable = [String: SemanticValue]

private let loadoutCategories: Set<String> = [
    "agents", "defaults", "hooks", "instructions", "kits", "mcp", "module-config",
    "permissions", "plugins", "settings", "skills",
]
private let automaticSlices: Set<String> = [
    "agents", "mcp", "mcp-permissions", "module-config", "permissions", "skills",
]
private let namedTables: Set<String> = ["instructions", "permissions", "skills"]
private let harnessExtras: Set<String> = ["preserve", "settings", "substitute"]

package func loadoutAgent(_ name: String) -> Agent? {
    Agent(rawValue: name).flatMap { $0 == .copilot ? nil : $0 }
}

package func loadoutProfileFile(_ root: ResourcePath, _ profile: String) throws -> ResourcePath {
    guard !profile.isEmpty, profile != ".", profile != "..",
          !profile.contains("/"), !profile.contains("\\"), !profile.utf8.contains(0)
    else {
        throw LoadoutDiagnostic("invalid-profile", "Profile must name a sibling manifest", root)
    }
    return try appendPath(root, profile == "default" ? "loadout.toml" : profile + ".toml")
}

package func loadoutText(_ value: SemanticValue?) -> String? {
    if case let .scalar(.text(text)) = value {
        return text
    }
    return nil
}

package func loadoutStrings(
    _ value: SemanticValue?, _ path: ResourcePath, _ label: String, code: String = "invalid-manifest",
) throws -> [String] {
    if case let .array(items) = value {
        let strings = items.compactMap { loadoutText($0) }
        if strings.count == items.count {
            return strings
        }
    }
    throw LoadoutDiagnostic(code, "\(label) must be an array of strings", path)
}

package func loadoutTable(
    _ value: SemanticValue?, _ path: ResourcePath, _ label: String,
) throws -> LoadoutTable {
    guard let value else { return [:] }
    guard case let .object(items) = value else {
        throw LoadoutDiagnostic("invalid-manifest", "\(label) must be a table", path)
    }
    var output: LoadoutTable = [:]
    for (key, value) in items {
        guard output[key] == nil else {
            throw LoadoutDiagnostic("unsupported-manifest", "Canonically equivalent keys in \(label)", path)
        }
        output[key] = value
    }
    return output
}

private func loadoutObject(_ value: LoadoutTable) -> SemanticValue {
    .object(value.sorted { utf8Less($0.key, $1.key) }.map { ($0.key, $0.value) })
}

private func loadoutChain(
    _ root: ResourcePath, _ profile: String, _ inputs: inout LoadoutInputs,
) throws -> [(ResourcePath, LoadoutTable)] {
    let marker = try loadoutProfileFile(root, "default")
    guard let base = try inputs.table(marker, code: "invalid-manifest", within: root) else {
        throw LoadoutDiagnostic("invalid-manifest", "Loadout source has no loadout.toml", marker)
    }
    var path = try loadoutProfileFile(root, profile)
    var data: LoadoutTable
    if path == marker {
        data = base
    } else if let selected = try inputs.table(path, code: "invalid-profile", within: root) {
        data = selected
    } else {
        path = marker
        data = base
    }
    var result: [(ResourcePath, LoadoutTable)] = []
    var seen: Set<ResourcePath> = []
    while true {
        let physical = try inputs.physical(path)
        guard parentPath(physical) == root else {
            throw LoadoutDiagnostic("escaped-path", "Profile must resolve to a sibling manifest", path)
        }
        guard seen.insert(physical).inserted else {
            throw LoadoutDiagnostic("profile-cycle", "Loadout profile extends cycle", path)
        }
        result.append((physical, data))
        guard let parent = data["extends"] else { return result }
        guard let name = loadoutText(parent) else {
            throw LoadoutDiagnostic("invalid-profile", "Profile extends must name a sibling manifest", path)
        }
        path = try loadoutProfileFile(root, name)
        guard let previous = try inputs.table(path, code: "invalid-profile", within: root) else {
            throw LoadoutDiagnostic("invalid-profile", "Missing parent profile \(name)", path)
        }
        data = previous
    }
}

private struct ProfileMerge {
    private(set) var data: LoadoutTable = [:]
    private(set) var provenance: [[String]: ResourcePath] = [:]

    private mutating func forget(_ path: [String]) {
        provenance = provenance.filter { !$0.key.starts(with: path) }
    }

    private mutating func remove(_ raw: SemanticValue, file: ResourcePath) throws {
        let paths = try loadoutStrings(raw, file, "remove", code: "invalid-profile")
        var seen: [[String]] = []
        for target in paths {
            let invalid = LoadoutDiagnostic(
                "invalid-profile",
                "Invalid or overlapping profile remove path \(target)",
                file,
            )
            guard let parts = try? tomlHeaderPath(target), let leaf = parts.last,
                  !seen.contains(where: { $0.starts(with: parts) || parts.starts(with: $0) })
            else { throw invalid }
            var table = data
            var parents: [(LoadoutTable, String)] = []
            for key in parts.dropLast() {
                guard case .object = table[key] else { throw invalid }
                parents.append((table, key))
                table = try loadoutTable(table[key], file, "remove path")
            }
            guard table.removeValue(forKey: leaf) != nil else { throw invalid }
            for (parent, key) in parents.reversed() {
                var previous = parent
                previous[key] = loadoutObject(table)
                table = previous
            }
            data = table
            forget(parts)
            seen.append(parts)
        }
    }

    mutating func add(_ layer: LoadoutTable, file: ResourcePath) throws {
        if let raw = layer["remove"] {
            guard layer["extends"] != nil else {
                throw LoadoutDiagnostic("invalid-profile", "Profile remove requires extends", file)
            }
            try remove(raw, file: file)
        }
        for (key, value) in layer where key != "extends" && key != "remove" {
            guard case .object = value else {
                if case .object = data[key] {
                    forget([key])
                }
                data[key] = value
                provenance[[key]] = file
                continue
            }
            var result = (try? loadoutTable(data[key], file, key)) ?? [:]
            for (name, raw) in try loadoutTable(value, file, key) {
                if namedTables.contains(key), case .object = raw {
                    var target = (try? loadoutTable(result[name], file, name)) ?? [:]
                    for (field, value) in try loadoutTable(raw, file, name) {
                        if case .object = target[field] {
                            forget([key, name, field])
                        }
                        provenance[[key, name, field]] = file
                        target[field] = value
                    }
                    result[name] = loadoutObject(target)
                } else {
                    if case .object = result[name] {
                        forget([key, name])
                    }
                    provenance[[key, name]] = file
                    result[name] = raw
                }
            }
            data[key] = loadoutObject(result)
        }
    }
}

private func loadoutSources(
    _ value: SemanticValue?, root: ResourcePath, home: ResourcePath, file: ResourcePath, inputs: inout LoadoutInputs,
) throws -> [LoadoutSource] {
    guard case let .array(entries) = value, !entries.isEmpty else {
        throw LoadoutDiagnostic("invalid-manifest", "Declare at least one [[source]]", file)
    }
    var sources: [LoadoutSource] = []
    var used: Set<String> = []
    for entry in entries {
        let table = try loadoutTable(entry, file, "source")
        guard Set(table.keys).isSubset(of: ["name", "path", "use", "overrides"]) else {
            throw LoadoutDiagnostic("unsupported-manifest", "Unknown source declaration field", file)
        }
        guard let name = loadoutText(table["name"]), !name.contains("/"),
              (try? validateRelativePath(name, label: "Loadout source name")) != nil,
              used.insert(portablePathKey(name)).inserted,
              let spelling = loadoutText(table["path"]),
              let logical = try? loadoutSourcePath(spelling, home: home, relativeTo: root)
        else {
            throw LoadoutDiagnostic(
                "invalid-source",
                "Source needs a unique single-component name and valid path",
                file,
            )
        }
        let path = try inputs.directory(logical)
        let uses = try table["use"].map {
            try Set(loadoutStrings($0, file, "\(name).use", code: "invalid-source"))
        } ?? loadoutCategories
        guard !uses.isEmpty, uses.isSubset(of: loadoutCategories),
              !sources.contains(where: { $0.path == path && !$0.uses.isDisjoint(with: uses) })
        else {
            throw LoadoutDiagnostic("invalid-source", "Invalid source use or aliased source directory", file)
        }
        if uses.contains("kits") {
            let (templates, _) = try inputs.read(appendPath(path, "templates"), code: "invalid-source", within: path)
            if templates.identity != nil {
                throw LoadoutDiagnostic("unsupported-manifest", "Rename templates/ to kits/ in this source", path)
            }
        }
        let overrides = try loadoutOverrides(table["overrides"], uses: uses, path: file)
        sources.append(LoadoutSource(name: name, path: path, uses: uses, declaredIn: file, overrides: overrides))
    }
    return sources
}

private func loadoutOverrides(
    _ value: SemanticValue?, uses: Set<String>, path: ResourcePath,
) throws -> [String: Set<String>] {
    let table = try loadoutTable(value, path, "overrides")
    var result: [String: Set<String>] = [:]
    for (category, raw) in table {
        guard uses.contains(category), ["skills", "agents", "module-config"].contains(category) else {
            throw LoadoutDiagnostic("invalid-source", "Unsupported source override category", path)
        }
        let values = try loadoutStrings(raw, path, category, code: "invalid-source")
        let items = Set(values)
        guard items.count == values.count else {
            throw LoadoutDiagnostic("invalid-source", "Duplicate source override", path)
        }
        for item in items {
            let parts = item.split(separator: "/")
            let scoped = category == "module-config"
            guard (try? validateRelativePath(item, label: "source override")) != nil,
                  !parts.contains(".git"),
                  scoped ? parts.count >= 2 && parts.first.flatMap({ loadoutAgent(String($0)) }) != nil : parts
                      .count == 1
            else {
                throw LoadoutDiagnostic("invalid-source", "Invalid source override \(item)", path)
            }
        }
        result[category] = items
    }
    return result
}

package struct LoadoutSlice: Sendable, Hashable {
    let agent: Agent
    let category: String
}

package struct LoadoutManifest: Sendable {
    let chain: [ResourcePath]
    let sources: [LoadoutSource]
    let harnesses: [LoadoutHarness]
    let selectors: [LoadoutSelector]
    let policies: [String: Set<ResourcePath>]
    let customRoutes: Set<LoadoutSlice>

    static func read(
        root: ResourcePath, profile: String, home: ResourcePath, inputs: inout LoadoutInputs,
    ) throws -> Self {
        let layers = try loadoutChain(root, profile, &inputs)
        guard let (file, first) = layers.first else {
            throw LoadoutDiagnostic("invalid-profile", "Profile chain is empty", root)
        }
        var merge = ProfileMerge()
        for (path, layer) in layers.reversed() {
            try merge.add(layer, file: path)
        }
        var targets = LoadoutTargets(file: file, merged: merge)
        let named = try targets.legacy(profile)
        let requested = try loadoutProfileFile(root, profile)
        guard profile == "default" ||
            (profile != "loadout" && first["extends"] != nil && inputs.observedFile(requested)) || named
        else {
            throw LoadoutDiagnostic("invalid-profile", "Unknown Loadout profile \(profile)", file)
        }
        try targets.modern()
        let harnesses = targets.harnesses.keys.sorted { $0.rawValue < $1.rawValue }.compactMap { agent in
            targets.harnesses[agent].flatMap { $0.isEmpty ? nil : LoadoutHarness(agent: agent, slices: $0) }
        }
        guard !harnesses.isEmpty else {
            throw LoadoutDiagnostic("invalid-manifest", "No active Loadout targets in this profile", file)
        }
        let sources = try loadoutSources(merge.data["source"], root: root, home: home,
                                         file: merge.provenance[["source"]] ?? file, inputs: &inputs)
        let policies = try targets.policies()
        return Self(chain: layers.map(\.0), sources: sources, harnesses: harnesses,
                    selectors: targets.selectors, policies: policies, customRoutes: targets.custom)
    }
}

private func legacySlice(_ renderer: String) -> LoadoutSlice? {
    switch renderer {
    case "codex-config": return LoadoutSlice(agent: .codex, category: "mcp")
    case "codex-settings": return LoadoutSlice(agent: .codex, category: "defaults")
    case "claude-mcp-permissions": return LoadoutSlice(agent: .claude, category: "mcp-permissions")
    default: break
    }
    for (suffix, category) in [("-servers", "mcp"), ("-hooks", "hooks"), ("-plugins", "plugins"), ("", "permissions")]
        where renderer.hasSuffix(suffix)
    {
        if let agent = loadoutAgent(String(renderer.dropLast(suffix.count))) {
            return LoadoutSlice(agent: agent, category: category)
        }
    }
    return nil
}

private func legacyPaths(
    _ block: LoadoutTable, _ file: ResourcePath, _ claimed: inout Set<String>,
) throws -> Int {
    let destinations = try block["destinations"].map { try loadoutStrings($0, file, "destinations") } ?? []
    guard !destinations.contains(where: { $0.isEmpty || $0.utf8.contains(0) }) else {
        throw LoadoutDiagnostic("invalid-manifest", "Invalid legacy destination", file)
    }
    if let output = block["output"] {
        guard let value = loadoutText(output), (try? validateRelativePath(value, label: "legacy output")) != nil,
              claimed.insert(value).inserted
        else {
            throw LoadoutDiagnostic("invalid-manifest", "Legacy output must be a unique relative path", file)
        }
        return destinations.count + 1
    }
    guard !destinations.isEmpty else {
        throw LoadoutDiagnostic("invalid-manifest", "Legacy target needs destinations or output", file)
    }
    return destinations.count
}

private func substitutions(_ value: SemanticValue?, _ file: ResourcePath) throws -> [String: String] {
    try loadoutTable(value, file, "substitute").mapValues { entry in
        guard let name = loadoutText(entry) else {
            throw LoadoutDiagnostic("invalid-manifest", "Substitute targets must be strings", file)
        }
        return name
    }
}

private func declaredSlices(_ agent: Agent) -> Set<String> {
    var offered = automaticSlices.union(["hooks", "instructions"])
    if agent != .claude {
        offered.remove("mcp-permissions")
    }
    if agent == .codex {
        offered.remove("module-config"); offered.insert("defaults")
    }
    if [.claude, .codex, .droid, .pi].contains(agent) {
        offered.insert("plugins")
    }
    return offered
}

private func settingsReaders(_ agent: Agent) -> Set<String> {
    switch agent {
    case .claude: ["permissions", "hooks", "plugins"]
    case .cursor: ["permissions"]
    case .droid: ["permissions", "plugins"]
    case .opencode: ["permissions", "mcp"]
    case .pi: ["plugins"]
    default: []
    }
}

private struct LoadoutTargets {
    let file: ResourcePath
    let merged: ProfileMerge
    var harnesses: [Agent: Set<String>] = [:]
    var selectors: [LoadoutSelector] = []
    var custom: Set<LoadoutSlice> = []

    mutating func legacy(_ profile: String) throws -> Bool {
        var named = false
        var claimed: Set<String> = []
        for category in ["instructions", "permissions"] {
            for (name, entry) in try loadoutTable(merged.data[category], file, category)
                .sorted(by: { $0.key < $1.key })
            {
                let block = try loadoutTable(entry, file, name)
                let destinations = try legacyPaths(block, file, &claimed)
                let targetProfile = loadoutText(block["profile"])
                if block["profile"] != nil, targetProfile == nil {
                    throw LoadoutDiagnostic("invalid-manifest", "Legacy profile must be a string", file)
                }
                named = named || targetProfile == profile
                let selected = targetProfile == nil || targetProfile == profile
                if category == "instructions" {
                    guard let agent = loadoutAgent(name) else {
                        throw LoadoutDiagnostic(
                            "unsupported-manifest",
                            "Unknown legacy instruction harness \(name)",
                            file,
                        )
                    }
                    let values = try loadoutStrings(block["order"], file, "order")
                    let swaps = try substitutions(block["substitute"], file)
                    if selected {
                        let key = [category, name, "order"]
                        harnesses[agent, default: []].insert(category)
                        selectors.append(LoadoutSelector(
                            agent: agent, category: category, manifest: merged.provenance[key] ?? file,
                            key: key, entries: values, substitutions: swaps, array: true, destinations: destinations,
                            allProfiles: targetProfile == nil,
                        ))
                    }
                } else {
                    guard let name = loadoutText(block["render"]), let slice = legacySlice(name),
                          block["rules"] == nil
                    else {
                        throw LoadoutDiagnostic("unsupported-manifest", "Unsupported legacy permission renderer", file)
                    }
                    if selected {
                        harnesses[slice.agent, default: []].insert(slice.category)
                        custom.insert(slice)
                    }
                }
            }
        }
        return named
    }

    mutating func modern() throws {
        let common = try loadoutTable(merged.data["all"], file, "all")
        for (name, value) in merged.data.sorted(by: { $0.key < $1.key }) {
            if let agent = loadoutAgent(name) {
                try harness(agent, declared: loadoutTable(value, file, name), common: common)
            } else if !namedTables.contains(name), name != "all", name != "source" {
                throw LoadoutDiagnostic("unsupported-manifest", "Unknown top-level manifest key \(name)", file)
            }
        }
    }

    private mutating func harness(_ agent: Agent, declared: LoadoutTable, common: LoadoutTable) throws {
        let offered = declaredSlices(agent)
        guard Set(declared.keys).isSubset(of: offered.union(harnessExtras)),
              agent != .codex || (declared["settings"] == nil && declared["preserve"] == nil)
        else {
            throw LoadoutDiagnostic("invalid-manifest", "Unsupported fields in [\(agent.rawValue)]", file)
        }
        let block = common.merging(declared) { _, owned in owned }.filter {
            offered.contains($0.key) || harnessExtras.contains($0.key)
        }
        var active = automaticSlices.intersection(offered).subtracting(block.keys)
        for (category, value) in block.sorted(by: { $0.key < $1.key }) {
            try validateSlice(category, value: value, agent: agent)
            guard offered.contains(category), value != .scalar(.boolean(false)) else { continue }
            active.insert(category)
            if ["instructions", "hooks"].contains(category) {
                let single = category == "hooks" ? loadoutText(value) : nil
                let items = try single.map { [$0] } ?? loadoutStrings(value, file, category)
                let key = [declared[category] == nil ? "all" : agent.rawValue, category]
                let swaps = try category == "instructions" ? substitutions(block["substitute"], file) : [:]
                selectors.append(LoadoutSelector(
                    agent: agent, category: category, manifest: merged.provenance[key] ?? file,
                    key: key, entries: items, substitutions: swaps, array: single == nil, destinations: 1,
                    allProfiles: false,
                ))
            }
        }
        _ = try substitutions(block["substitute"], file)
        guard declared["settings"] == nil || !active.isDisjoint(with: settingsReaders(agent)) else {
            throw LoadoutDiagnostic("invalid-manifest", "\(agent.rawValue).settings needs a slice that reads it", file)
        }
        harnesses[agent, default: []].formUnion(active)
    }

    private func validateSlice(_ category: String, value: SemanticValue, agent: Agent) throws {
        if automaticSlices.contains(category) {
            if case .scalar(.boolean) = value {
                return
            }
            guard ["mcp", "mcp-permissions", "permissions"].contains(category), value == .array([]) else {
                throw LoadoutDiagnostic(
                    "invalid-manifest",
                    "\(agent.rawValue).\(category) must be enabled or disabled",
                    file,
                )
            }
        } else if category == "preserve" || (["settings", "defaults", "plugins"].contains(category) &&
            loadoutText(value) == nil && value != .scalar(.boolean(false)))
        {
            _ = try loadoutStrings(value, file, category)
        }
    }

    func policies() throws -> [String: Set<ResourcePath>] {
        var result: [String: Set<ResourcePath>] = [:]
        for (name, entry) in try loadoutTable(merged.data["skills"], file, "skills") {
            let block = try loadoutTable(entry, file, "skills." + name)
            let agents = try loadoutStrings(block["agents"], file, "skill agents")
            let render = block["render"] ?? .scalar(.boolean(true))
            guard Set(block.keys).isSubset(of: ["agents", "render"]), case .scalar(.boolean) = render,
                  !["", ".", ".."].contains(name), !name.contains("/"), !name.contains("\\"), !name.utf8.contains(0),
                  Set(agents).count == agents.count,
                  agents.allSatisfy({ name in
                      loadoutAgent(name).flatMap { harnesses[$0] }?.contains("skills") == true
                  })
            else {
                throw LoadoutDiagnostic("invalid-manifest", "Invalid skill policy for \(name)", file)
            }
            result[name] = Set(block.keys.compactMap { merged.provenance[["skills", name, $0]] })
        }
        return result
    }
}
