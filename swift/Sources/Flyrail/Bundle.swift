import Foundation

public extension FlyrailBundle {
    init(directory: ResourcePath) throws {
        let source = try DirectoryBundleSource(root: directory)
        do {
            let loaded = try Self.load(source: source, sourceRoots: [source.canonicalRoot])
            try source.finish()
            self = loaded
        } catch {
            source.close()
            throw error
        }
        source.close()
    }

    init(archive: ResourcePath, root: ResourcePath) throws {
        let resource = try resourceString(root)
        try validateRelativePath(resource, label: "archive resource")
        let snapshot = try snapshotArchiveFile(archive, limit: BundleLimits.default.maximumArchiveBytes)
        let parsed = try ZIPArchive(data: snapshot.data, limits: .default)
        let source = try ZIPBundleSource(archive: parsed, root: resource)
        self = try Self.load(source: source, sourceRoots: [snapshot.canonicalPath])
    }

    init(
        manifest: Data,
        entries: [BundleEntry],
        sourceRoots: [ResourcePath] = [],
    ) throws {
        let source = try MemoryBundleSource(manifest: manifest, entries: entries)
        self = try Self.load(source: source, sourceRoots: sourceRoots)
    }

    init(
        identity: BundleIdentity,
        artifacts: [Artifact],
        assets: [SupportAsset] = [],
        dependencies: [Dependency] = [],
    ) throws {
        try self.init(
            identity: identity,
            artifacts: artifacts,
            assets: assets,
            dependencies: dependencies,
            sourceRoots: [],
        )
    }

    private static func load(
        source: some BundleSource,
        sourceRoots: [ResourcePath],
    ) throws -> Self {
        let manifest = try source.manifest()
        let raw = try JSONParser.parse(manifest)
        if case let .object(fields) = raw,
           isConfigurationSchema(fields.first(where: {
               exactTextEqual($0.0, "schema_version")
           })?.1)
        {
            let parsed = try readConfigurationManifest(manifest, source: source)
            return try Self(
                identity: parsed.identity,
                artifacts: parsed.artifacts,
                assets: parsed.assets,
                dependencies: parsed.dependencies,
                sourceRoots: sourceRoots,
            )
        }
        let (identity, specs) = try readBundleManifest(manifest)
        var selected: [BundleEntry] = []
        var budget = BundleEntryBudget(limits: .default)
        for spec in specs {
            try selected.append(contentsOf: source.tree(spec: spec, budget: &budget))
        }
        let entries = try snapshotEntries(selected)
        let files = Set(entries.filter { !$0.isDirectory }.map { Array($0.path.utf8) })
        for spec in specs {
            guard files.contains(Array("\(spec.name)/SKILL.md".utf8)) else {
                throw FlyrailValueError("skill must contain a SKILL.md file: \(spec.name)")
            }
            for executable in spec.executables {
                guard files.contains(Array("\(spec.name)/\(executable)".utf8)) else {
                    throw FlyrailValueError("every executable must name an existing file: \(spec.name)")
                }
            }
        }
        return try Self(
            identity: identity,
            skills: specs,
            entries: entries,
            sourceRoots: sourceRoots,
        )
    }
}

private func isConfigurationSchema(_ value: JSONValue?) -> Bool {
    guard case let .number(token)? = value, let number = Double(token) else {
        return false
    }
    return number.bitPattern == 2.0.bitPattern
}

package protocol BundleSource {
    func manifest() throws -> Data
    func tree(spec: SkillSpec, budget: inout BundleEntryBudget) throws -> [BundleEntry]
    func readFile(path: String) throws -> Data
    func readTree(path: String, executables: [String]) throws -> [BundleEntry]
}

package struct MemoryBundleSource: BundleSource {
    private let manifestData: Data
    private let entries: [BundleEntry]

    package init(manifest: Data, entries: [BundleEntry]) throws {
        manifestData = manifest
        try validateBundleEntryLimits(entries, limits: .default)
        self.entries = try snapshotEntries(entries)
    }

    package func manifest() -> Data {
        manifestData
    }

    package func tree(spec: SkillSpec, budget: inout BundleEntryBudget) throws -> [BundleEntry] {
        guard entries.contains(where: {
            exactTextEqual($0.path, spec.path) && $0.isDirectory
        }) else {
            throw FlyrailValueError("skill source must be an existing directory: \(spec.path)")
        }
        let installedRoot = try BundleEntry(path: spec.name)
        try budget.reserve(path: installedRoot.path, dataBytes: nil)
        var result = [installedRoot]
        let prefix = Array("\(spec.path)/".utf8)
        for entry in entries where Array(entry.path.utf8).starts(with: prefix) {
            let relative = String(decoding: entry.path.utf8.dropFirst(prefix.count), as: UTF8.self)
            let installed = try BundleEntry(
                path: "\(spec.name)/\(relative)",
                data: entry.data,
                executable: spec.executables.contains(where: { exactTextEqual($0, relative) }),
            )
            try budget.reserve(
                path: installed.path,
                dataBytes: installed.data.map { UInt64($0.count) },
            )
            result.append(installed)
        }
        return result
    }

    package func readFile(path: String) throws -> Data {
        guard let entry = entries.first(where: { exactTextEqual($0.path, path) }),
              let data = entry.data
        else {
            throw FlyrailValueError("expected a source file: \(path)")
        }
        return data
    }

    package func readTree(path: String, executables: [String]) throws -> [BundleEntry] {
        guard entries.contains(where: { exactTextEqual($0.path, path) && $0.isDirectory }) else {
            throw FlyrailValueError("skill source must be an existing directory: \(path)")
        }
        var result: [BundleEntry] = []
        let prefix = Array("\(path)/".utf8)
        for entry in entries where Array(entry.path.utf8).starts(with: prefix) {
            let relative = String(decoding: entry.path.utf8.dropFirst(prefix.count), as: UTF8.self)
            try result.append(BundleEntry(
                path: relative,
                data: entry.data,
                executable: executables.contains(where: { exactTextEqual($0, relative) }),
            ))
        }
        return try snapshotEntries(result)
    }
}

private func resourceString(_ path: ResourcePath) throws -> String {
    guard let value = String(bytes: path.platformBytes, encoding: .utf8) else {
        throw FlyrailValueError("archive resource must contain Unicode scalar values")
    }
    return value
}
