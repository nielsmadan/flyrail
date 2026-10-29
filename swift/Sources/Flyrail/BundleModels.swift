import CryptoKit
import Foundation

public struct BundleEntry: Sendable, Hashable {
    public let path: String
    public let data: Data?
    public let executable: Bool

    public init(path: String, data: Data? = nil, executable: Bool = false) throws {
        try validateRelativePath(path, label: "entry path")
        guard data != nil || !executable else {
            throw FlyrailValueError("a directory cannot have executable file intent")
        }
        self.path = path
        self.data = data
        self.executable = executable
    }

    public var isDirectory: Bool {
        data == nil
    }

    public static func == (left: Self, right: Self) -> Bool {
        exactTextEqual(left.path, right.path) &&
            left.data == right.data &&
            left.executable == right.executable
    }

    public func hash(into hasher: inout Hasher) {
        path.utf8.forEach { hasher.combine($0) }
        hasher.combine(data)
        hasher.combine(executable)
    }
}

public struct SkillSpec: Sendable, Hashable {
    public let name: String
    public let path: String
    public let executables: [String]

    public init(name: String, path: String, executables: [String] = []) throws {
        try validateIdentifier(name, label: "skill name")
        try validateRelativePath(path, label: "skill path")
        guard path.split(separator: "/").last.map(String.init)
            .map({ exactTextEqual($0, name) }) == true
        else {
            throw FlyrailValueError("skill name must match its containing directory")
        }
        var seen: Set<String> = []
        for executable in executables {
            try validateRelativePath(executable, label: "executable path")
            guard seen.insert(portablePathKey(executable)).inserted else {
                throw FlyrailValueError("executable paths must be unique across portable filesystems")
            }
        }
        self.name = name
        self.path = path
        self.executables = executables
    }

    public static func == (left: Self, right: Self) -> Bool {
        exactTextEqual(left.name, right.name) &&
            exactTextEqual(left.path, right.path) &&
            exactStringArraysEqual(left.executables, right.executables)
    }

    public func hash(into hasher: inout Hasher) {
        name.utf8.forEach { hasher.combine($0) }
        path.utf8.forEach { hasher.combine($0) }
        for executable in executables {
            executable.utf8.forEach { hasher.combine($0) }
        }
    }
}

public struct TreeContent: Sendable, Hashable {
    public let entries: [BundleEntry]

    public init(entries: [BundleEntry]) throws {
        self.entries = try snapshotEntries(entries)
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "TreeContent",
            fields: [
                ("entries", .array(entries.map { try $0.modelValue() })),
            ]
        )
    }
}

public struct SkillArtifact: Sendable, Hashable {
    public let id: String
    public let name: String
    public let content: TreeContent

    public init(id: String, name: String, content: TreeContent) throws {
        try validateIdentifier(id, label: "artifact id")
        try validateIdentifier(name, label: "skill name")
        guard content.entries.contains(where: {
            exactTextEqual($0.path, "SKILL.md") && !$0.isDirectory
        }) else {
            throw FlyrailValueError("skill must contain a SKILL.md file")
        }
        self.id = id
        self.name = name
        self.content = content
    }

    public static func == (left: Self, right: Self) -> Bool {
        exactTextEqual(left.id, right.id) &&
            exactTextEqual(left.name, right.name) &&
            left.content == right.content
    }

    public func hash(into hasher: inout Hasher) {
        id.utf8.forEach { hasher.combine($0) }
        name.utf8.forEach { hasher.combine($0) }
        hasher.combine(content)
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "SkillArtifact",
            fields: [
                ("content", content.modelValue()),
                ("id", .scalar(.text(id))),
                ("name", .scalar(.text(name))),
            ]
        )
    }
}

public enum Artifact: Sendable, Hashable {
    case skill(SkillArtifact)
    case instruction(InstructionArtifact)
    case mcp(MCPArtifact)
    case hook(HookArtifact)
    case native(NativeArtifact)

    public var id: String {
        switch self {
        case let .skill(value): value.id
        case let .instruction(value): value.id
        case let .mcp(value): value.id
        case let .hook(value): value.id
        case let .native(value): value.id
        }
    }

    package func modelValue() throws -> SemanticValue {
        switch self {
        case let .skill(skill):
            try skill.modelValue()
        case let .instruction(value):
            try value.modelValue()
        case let .mcp(value):
            try value.modelValue()
        case let .hook(value):
            try value.modelValue()
        case let .native(value):
            try value.modelValue()
        }
    }
}

public struct FlyrailBundle: Sendable, Hashable {
    public let identity: BundleIdentity
    public let skills: [SkillSpec]
    public let entries: [BundleEntry]
    public let artifacts: [Artifact]
    public let assets: [SupportAsset]
    public let dependencies: [Dependency]
    public let contentDigest: String
    public let sourceRoots: [ResourcePath]
    public let schemaVersion: UInt8

    package init(
        identity: BundleIdentity,
        skills: [SkillSpec],
        entries: [BundleEntry],
        sourceRoots: [ResourcePath]
    ) throws {
        self.identity = identity
        self.skills = skills
        self.entries = entries
        self.sourceRoots = sourceRoots
        artifacts = try skills.map { spec in
            let prefix = Array("\(spec.name)/".utf8)
            let content = try TreeContent(entries: entries.compactMap { entry in
                let bytes = Array(entry.path.utf8)
                guard bytes.starts(with: prefix) else {
                    return nil
                }
                return try BundleEntry(
                    path: String(decoding: bytes.dropFirst(prefix.count), as: UTF8.self),
                    data: entry.data,
                    executable: entry.executable
                )
            })
            return try .skill(SkillArtifact(id: spec.name, name: spec.name, content: content))
        }
        assets = []
        dependencies = []
        contentDigest = bundleContentDigest(for: entries)
        schemaVersion = 1
    }

    package init(
        identity: BundleIdentity,
        artifacts: [Artifact],
        assets: [SupportAsset],
        dependencies: [Dependency],
        sourceRoots: [ResourcePath]
    ) throws {
        let validated = try validateConfiguration(
            artifacts: artifacts,
            assets: assets,
            dependencies: dependencies
        )
        self.identity = identity
        self.artifacts = validated.artifacts
        self.assets = validated.assets
        self.dependencies = validated.dependencies
        self.sourceRoots = sourceRoots
        schemaVersion = 2
        skills = try validated.artifacts.compactMap { artifact in
            guard case let .skill(skill) = artifact else { return nil }
            return try SkillSpec(
                name: skill.name,
                path: skill.name,
                executables: skill.content.entries.filter(\.executable).map(\.path)
            )
        }
        var selected: [BundleEntry] = []
        for artifact in validated.artifacts {
            guard case let .skill(skill) = artifact else { continue }
            try selected.append(BundleEntry(path: skill.name))
            for entry in skill.content.entries {
                try selected.append(BundleEntry(
                    path: "\(skill.name)/\(entry.path)",
                    data: entry.data,
                    executable: entry.executable
                ))
            }
        }
        entries = selected.sorted { utf8Less($0.path, $1.path) }
        contentDigest = try ModelEncoding.digest(
            domain: "flyrail-config-v2",
            values: [
                .array(self.artifacts.map { try $0.modelValue() }),
                .array(self.assets.map { try $0.modelValue() }),
                .array(self.dependencies.map { try $0.modelValue() }),
            ]
        )
    }

    package func differentialCanonicalValue() throws -> SemanticValue {
        try .array([
            .array(artifacts.map { try $0.modelValue() }),
            .array(assets.map { try $0.modelValue() }),
            .array(dependencies.map { try $0.modelValue() }),
        ])
    }
}

private func validateConfiguration(
    artifacts: [Artifact],
    assets: [SupportAsset],
    dependencies: [Dependency]
) throws -> (artifacts: [Artifact], assets: [SupportAsset], dependencies: [Dependency]) {
    guard !artifacts.isEmpty else {
        throw FlyrailValueError("a configuration bundle requires at least one artifact")
    }
    var ids: Set<String> = []
    for id in artifacts.map(\.id) + assets.map(\.id) {
        guard ids.insert(id).inserted else {
            throw FlyrailValueError("artifact and asset ids must be unique")
        }
    }
    let skillNames = artifacts.compactMap {
        if case let .skill(skill) = $0 { skill.name } else { nil }
    }
    guard Set(skillNames).count == skillNames.count else {
        throw FlyrailValueError("duplicate skill name")
    }
    try validateDependencies(names: ids, dependencies: dependencies)
    for asset in assets where !dependencies.contains(where: { exactTextEqual($0.required, asset.id) }) {
        throw FlyrailValueError("support assets must have an explicit dependent")
    }
    for artifact in artifacts {
        guard let command = artifactCommand(artifact) else { continue }
        for (index, argument) in command.argv.enumerated() {
            guard case let .asset(reference) = argument else { continue }
            let entry = try validateAssetReference(
                artifactID: artifact.id,
                reference: reference,
                assets: assets,
                dependencies: dependencies
            )
            if index == 0, entry?.isDirectory != false {
                throw FlyrailValueError("asset command must reference a file")
            }
        }
        if case let .asset(reference)? = command.cwd {
            let entry = try validateAssetReference(
                artifactID: artifact.id,
                reference: reference,
                assets: assets,
                dependencies: dependencies
            )
            if let entry, !entry.isDirectory {
                throw FlyrailValueError("asset cwd must reference a directory")
            }
        }
    }
    return (
        artifacts.sorted { utf8Less($0.id, $1.id) },
        assets.sorted { utf8Less($0.id, $1.id) },
        dependencies.sorted {
            exactTextEqual($0.dependent, $1.dependent)
                ? utf8Less($0.required, $1.required)
                : utf8Less($0.dependent, $1.dependent)
        }
    )
}

private func validateDependencies(names: Set<String>, dependencies: [Dependency]) throws {
    var pairs: Set<String> = []
    for edge in dependencies {
        guard pairs.insert("\(edge.dependent)\0\(edge.required)").inserted else {
            throw FlyrailValueError("duplicate dependency")
        }
        guard names.contains(edge.dependent), names.contains(edge.required) else {
            throw FlyrailValueError("dependency references an unknown artifact or asset")
        }
    }
    var pending = names
    while !pending.isEmpty {
        let ready = pending.filter { name in
            !dependencies.contains {
                exactTextEqual($0.dependent, name) && pending.contains($0.required)
            }
        }
        guard !ready.isEmpty else {
            throw FlyrailValueError("cyclic artifact dependencies")
        }
        pending.subtract(ready)
    }
}

private func artifactCommand(_ artifact: Artifact) -> Command? {
    switch artifact {
    case let .hook(value):
        value.command
    case let .mcp(value):
        if case let .stdio(command) = value.transport { command } else { nil }
    default:
        nil
    }
}

private func validateAssetReference(
    artifactID: String,
    reference: AssetRef,
    assets: [SupportAsset],
    dependencies: [Dependency]
) throws -> BundleEntry? {
    guard let asset = assets.first(where: { exactTextEqual($0.id, reference.assetID) }) else {
        throw FlyrailValueError("asset reference names an unknown support asset")
    }
    guard dependencies.contains(where: {
        exactTextEqual($0.dependent, artifactID) &&
            exactTextEqual($0.required, reference.assetID) &&
            $0.mode == .revision
    }) else {
        throw FlyrailValueError("asset reference requires a direct dependency")
    }
    guard let path = reference.path else { return nil }
    guard let entry = asset.content.entries.first(where: { exactTextEqual($0.path, path) }) else {
        throw FlyrailValueError("asset reference path does not exist in snapshot")
    }
    return entry
}

package struct BundleLimits: Sendable, Hashable {
    package let maximumArchiveBytes: UInt64
    package let maximumMemberCount: UInt64
    package let maximumMemberBytes: UInt64
    package let maximumTotalBytes: UInt64
    package let maximumNameBytes: UInt64
    package let maximumExpansionRatio: UInt64

    package static let `default` = Self(
        maximumArchiveBytes: 256 * 1024 * 1024,
        maximumMemberCount: 100_000,
        maximumMemberBytes: 64 * 1024 * 1024,
        maximumTotalBytes: 512 * 1024 * 1024,
        maximumNameBytes: 65535,
        maximumExpansionRatio: 1000
    )
}

package struct BundleEntryBudget {
    private let limits: BundleLimits
    private var count: UInt64 = 0
    private var totalBytes: UInt64 = 0

    package init(limits: BundleLimits) {
        self.limits = limits
    }

    package mutating func reserve(path: String, dataBytes: UInt64?) throws {
        guard UInt64(path.utf8.count) <= limits.maximumNameBytes else {
            throw FlyrailValueError("bundle entry name exceeds the byte limit")
        }
        let nextCount = count.addingReportingOverflow(1)
        guard !nextCount.overflow, nextCount.partialValue <= limits.maximumMemberCount else {
            throw FlyrailValueError("bundle source exceeds the entry count limit")
        }
        if let dataBytes {
            guard dataBytes <= limits.maximumMemberBytes else {
                throw FlyrailValueError("bundle entry exceeds the byte limit")
            }
            let nextTotal = totalBytes.addingReportingOverflow(dataBytes)
            guard !nextTotal.overflow, nextTotal.partialValue <= limits.maximumTotalBytes else {
                throw FlyrailValueError("bundle source exceeds the total byte limit")
            }
            totalBytes = nextTotal.partialValue
        }
        count = nextCount.partialValue
    }
}

package func snapshotEntries(_ input: [BundleEntry]) throws -> [BundleEntry] {
    var entries: [[UInt8]: BundleEntry] = [:]
    var spellings: [String: String] = [:]
    for entry in input {
        let bytes = Array(entry.path.utf8)
        guard entries[bytes] == nil else {
            throw FlyrailValueError("duplicate entry path")
        }
        for path in ancestors(entry.path) {
            let key = portablePathKey(path)
            if let previous = spellings[key], !exactTextEqual(previous, path) {
                throw FlyrailValueError("portable entry path collision")
            }
            spellings[key] = path
        }
        entries[bytes] = entry
    }
    for entry in input {
        let paths = ancestors(entry.path)
        for path in paths.dropLast() {
            let bytes = Array(path.utf8)
            if let existing = entries[bytes] {
                guard existing.isDirectory else {
                    throw FlyrailValueError("entry file/directory collision")
                }
            } else {
                entries[bytes] = try BundleEntry(path: path)
            }
        }
    }
    return entries.values.sorted { utf8Less($0.path, $1.path) }
}

package func validateBundleEntryLimits(_ entries: [BundleEntry], limits: BundleLimits) throws {
    var budget = BundleEntryBudget(limits: limits)
    for entry in entries {
        try budget.reserve(path: entry.path, dataBytes: entry.data.map { UInt64($0.count) })
    }
}

package func contentFraming(for entries: [BundleEntry]) -> Data {
    let ordered = entries.sorted { utf8Less($0.path, $1.path) }
    var output = Data("flyrail-content-v1".utf8)
    output.append(0)
    output.appendBigEndian(UInt64(ordered.count))
    for entry in ordered {
        output.append(entry.isDirectory ? 68 : 70)
        let path = Data(entry.path.utf8)
        output.appendBigEndian(UInt64(path.count))
        output.append(path)
        if let data = entry.data {
            output.append(entry.executable ? 1 : 0)
            output.appendBigEndian(UInt64(data.count))
            output.append(data)
        }
    }
    return output
}

package func bundleContentDigest(for entries: [BundleEntry]) -> String {
    SHA256.hash(data: contentFraming(for: entries))
        .map { String(format: "%02x", $0) }
        .joined()
}

private func exactStringArraysEqual(_ left: [String], _ right: [String]) -> Bool {
    left.count == right.count && zip(left, right).allSatisfy(exactTextEqual)
}

private extension BundleEntry {
    func modelValue() throws -> SemanticValue {
        let encodedData: SemanticValue = if let data {
            try ModelEncoding.bytes(data)
        } else {
            .scalar(.null)
        }
        return try ModelEncoding.record(
            type: "BundleEntry",
            fields: [
                ("data", encodedData),
                ("executable", .scalar(.boolean(executable))),
                ("path", .scalar(.text(path))),
            ]
        )
    }
}

private extension Data {
    mutating func appendBigEndian(_ value: UInt64) {
        var encoded = value.bigEndian
        Swift.withUnsafeBytes(of: &encoded) { append(contentsOf: $0) }
    }
}
