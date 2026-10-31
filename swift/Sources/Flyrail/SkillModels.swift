import CryptoKit
import Foundation

public struct InventoryEntry: Sendable, Hashable {
    public let path: String
    public let size: UInt64?
    public let sha256: String?
    public let executable: Bool
    public var isDirectory: Bool {
        size == nil
    }

    public init(path: String, size: UInt64? = nil, sha256: String? = nil, executable: Bool = false) throws {
        try validateRelativePath(path, label: "inventory path")
        self.path = path
        self.size = size
        self.sha256 = sha256
        self.executable = executable
    }

    public static func == (left: Self, right: Self) -> Bool {
        exactTextEqual(left.path, right.path) && left.size == right.size &&
            left.sha256.map { Data($0.utf8) } == right.sha256.map { Data($0.utf8) } && left.executable == right
            .executable
    }

    public func hash(into hasher: inout Hasher) {
        hashExactText(path, into: &hasher)
        hasher.combine(size)
        hasher.combine(sha256.map { Data($0.utf8) })
        hasher.combine(executable)
    }
}

public func inventory(_ entries: [BundleEntry]) throws -> [InventoryEntry] {
    try entries.sorted { utf8Less($0.path, $1.path) }.map {
        try InventoryEntry(path: $0.path, size: $0.data.map { UInt64($0.count) },
                           sha256: $0.data.map { SHA256.hash(data: $0).map { String(format: "%02x", $0) }.joined() },
                           executable: $0.executable)
    }
}

public enum ObservationState: String, Sendable, Hashable {
    case absent, installed, unknown
    case recoveryNeeded = "recovery_needed"
}

public enum ModificationKind: String, Sendable, Hashable {
    case missing, added
    case typeChanged = "type_changed"
    case contentChanged = "content_changed"
    case executableChanged = "executable_changed"
}

public struct Installation: Sendable, Hashable {
    public let bundleID: String
    public let version: String
    public let contentDigest: String
    public let transactionID: String
    public let entries: [InventoryEntry]

    public init(bundleID: String, version: String, contentDigest: String, transactionID: String,
                entries: [InventoryEntry]) throws
    {
        try validateIdentifier(bundleID, label: "bundle id")
        try validateVersion(version)
        self.bundleID = bundleID
        self.version = version
        self.contentDigest = contentDigest
        self.transactionID = transactionID
        self.entries = entries
    }

    private var textIdentity: [Data] {
        [bundleID, version, contentDigest, transactionID].map { Data($0.utf8) }
    }

    public static func == (left: Self, right: Self) -> Bool {
        left.textIdentity == right.textIdentity && left.entries == right.entries
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(textIdentity)
        hasher.combine(entries)
    }
}

public struct Modification: Sendable, Hashable {
    public let bundleID: String
    public let path: String
    public let kind: ModificationKind

    public init(bundleID: String, path: String, kind: ModificationKind) throws {
        try validateIdentifier(bundleID, label: "bundle id")
        self.bundleID = bundleID
        self.path = path
        self.kind = kind
    }

    public static func == (left: Self, right: Self) -> Bool {
        exactTextEqual(left.bundleID, right.bundleID) && exactTextEqual(left.path, right.path) && left.kind == right
            .kind
    }

    public func hash(into hasher: inout Hasher) {
        hashExactText(bundleID, into: &hasher)
        hashExactText(path, into: &hasher)
        hasher.combine(kind)
    }
}

public struct Conflict: Sendable, Hashable {
    public let path: String
    public let owner: String?
    public init(path: String, owner: String? = nil) {
        self.path = path; self.owner = owner
    }

    public static func == (left: Self, right: Self) -> Bool {
        exactTextEqual(left.path, right.path) && left.owner.map { Data($0.utf8) } == right.owner.map { Data($0.utf8) }
    }

    public func hash(into hasher: inout Hasher) {
        hashExactText(path, into: &hasher)
        hasher.combine(owner.map { Data($0.utf8) })
    }
}

public struct Observation: Sendable, Hashable {
    public let state: ObservationState
    public let installed: Installation?
    public let installations: [Installation]
    public let versionMatches: Bool?
    public let recordedContentMatches: Bool?
    public let contentMatches: Bool?
    public let modifications: [Modification]
    public let conflicts: [Conflict]
    public let error: TargetError?
    public let recoveryPaths: [ResourcePath]

    public init(state: ObservationState, installed: Installation? = nil, installations: [Installation] = [],
                versionMatches: Bool? = nil, recordedContentMatches: Bool? = nil, contentMatches: Bool? = nil,
                modifications: [Modification] = [], conflicts: [Conflict] = [], error: TargetError? = nil,
                recoveryPaths: [ResourcePath] = [])
    {
        self.state = state
        self.installed = installed
        self.installations = installations
        self.versionMatches = versionMatches
        self.recordedContentMatches = recordedContentMatches
        self.contentMatches = contentMatches
        self.modifications = modifications
        self.conflicts = conflicts
        self.error = error
        self.recoveryPaths = recoveryPaths
    }

    public var isCurrent: Bool {
        state == .installed && error == nil && installed != nil && versionMatches == true &&
            recordedContentMatches == true && contentMatches == true && conflicts.isEmpty &&
            modifications.allSatisfy { $0.bundleID != installed?.bundleID }
    }

    package static func unknown(_ error: TargetError) -> Self {
        Self(state: .unknown, error: error)
    }
}

public struct TargetInspection<T: Sendable>: Sendable {
    public let target: T
    public let root: ResourcePath
    public let aliasOf: Int?
    public let observation: Observation
    public init(target: T, root: ResourcePath, aliasOf: Int? = nil, observation: Observation) {
        self.target = target
        self.root = root
        self.aliasOf = aliasOf
        self.observation = observation
    }
}

extension TargetInspection: Equatable where T: Equatable {}
extension TargetInspection: Hashable where T: Hashable {}

public struct TargetResult<T: Sendable>: Sendable {
    public let target: T
    public let root: ResourcePath
    public let aliasOf: Int?
    public let status: OperationStatus
    public let observation: Observation
    public let error: TargetError?
    public let recoveryPaths: [ResourcePath]

    public init(target: T, root: ResourcePath, aliasOf: Int? = nil, status: OperationStatus,
                observation: Observation, error: TargetError? = nil, recoveryPaths: [ResourcePath] = [])
    {
        self.target = target
        self.root = root
        self.aliasOf = aliasOf
        self.status = status
        self.observation = observation
        self.error = error
        self.recoveryPaths = recoveryPaths
    }
}

extension TargetResult: Equatable where T: Equatable {}
extension TargetResult: Hashable where T: Hashable {}

public struct SkillOptions: Sendable, Hashable {
    public let replaceModified: Bool
    public let lockTimeout: Duration
    public init(replaceModified: Bool = false, lockTimeout: Duration = .zero) {
        self.replaceModified = replaceModified
        self.lockTimeout = lockTimeout
    }
}
