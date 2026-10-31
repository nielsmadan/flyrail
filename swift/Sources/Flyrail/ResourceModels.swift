import Foundation

public struct Count: Sendable, Hashable, Comparable, ExpressibleByIntegerLiteral {
    public let decimal: String

    public init(_ value: UInt64) {
        decimal = String(value)
    }

    public init(integerLiteral value: UInt64) {
        self.init(value)
    }

    public init(decimal: String) throws {
        guard !decimal.isEmpty, decimal.utf8.count <= 4300,
              decimal.utf8.allSatisfy({ (48 ... 57).contains($0) }),
              decimal == "0" || decimal.first != "0"
        else {
            throw FlyrailValueError("invalid unsigned state count")
        }
        self.decimal = decimal
    }

    public var uint64: UInt64? {
        UInt64(decimal)
    }

    public var uint32: UInt32? {
        UInt32(decimal)
    }

    public static func < (left: Self, right: Self) -> Bool {
        left.decimal.utf8.count == right.decimal.utf8.count
            ? utf8Less(left.decimal, right.decimal)
            : left.decimal.utf8.count < right.decimal.utf8.count
    }
}

public enum ResourceKind: String, Sendable, Hashable {
    case document
    case tree
    case skillContainer = "skill-container"
}

public struct Ancestor: Sendable, Hashable {
    public let path: ResourcePath
    public let device: Count
    public let inode: Count
    public let mode: Count
    public let uid: Count
    public let gid: Count
    public let security: Data

    public init(
        path: ResourcePath, device: Count, inode: Count, mode: Count,
        uid: Count, gid: Count, security: Data = Data(),
    ) throws {
        _ = try statePathText(path)
        guard mode <= 0o7777 else { throw FlyrailValueError("invalid ancestor metadata") }
        self.path = path
        self.device = device
        self.inode = inode
        self.mode = mode
        self.uid = uid
        self.gid = gid
        self.security = security
    }
}

public struct Node: Sendable, Hashable {
    public let path: String
    public let data: Data?
    public let mode: Count
    public let uid: Count
    public let gid: Count
    public let device: Count
    public let inode: Count
    public let security: Data

    public init(
        path: String, data: Data?, mode: Count, uid: Count, gid: Count,
        device: Count, inode: Count, security: Data = Data(),
    ) throws {
        if !path.isEmpty {
            try validateRelativePath(path, label: "revision path")
        }
        guard mode <= 0o777 else { throw FlyrailValueError("invalid revision metadata") }
        self.path = path
        self.data = data
        self.mode = mode
        self.uid = uid
        self.gid = gid
        self.device = device
        self.inode = inode
        self.security = security
    }

    public static func == (left: Self, right: Self) -> Bool {
        exactTextEqual(left.path, right.path) && left.data == right.data && left.mode == right.mode &&
            left.uid == right.uid && left.gid == right.gid && left.device == right.device &&
            left.inode == right.inode && left.security == right.security
    }

    public func hash(into hasher: inout Hasher) {
        hashExactText(path, into: &hasher)
        hasher.combine(data)
        hasher.combine(mode)
        hasher.combine(uid)
        hasher.combine(gid)
        hasher.combine(device)
        hasher.combine(inode)
        hasher.combine(security)
    }
}

public struct Revision: Sendable, Hashable {
    public let nodes: [Node]

    public init(nodes: [Node] = []) throws {
        let paths = nodes.map { Array($0.path.utf8) }
        guard Set(paths).count == paths.count, nodes.first?.path.isEmpty != false else {
            throw FlyrailValueError("revision must have exactly one root")
        }
        let byPath = Dictionary(uniqueKeysWithValues: zip(paths, nodes))
        for node in nodes.dropFirst() {
            let bytes = Array(node.path.utf8)
            let parent = bytes.lastIndex(of: 47).map { Array(bytes[..<$0]) } ?? []
            guard let parentNode = byPath[parent], parentNode.data == nil else {
                throw FlyrailValueError("revision parents must be directories")
            }
        }
        guard paths == paths.sorted(by: { $0.lexicographicallyPrecedes($1) }) else {
            throw FlyrailValueError("revision entries must be ordered")
        }
        self.nodes = nodes
    }

    public var data: Data? {
        nodes.first?.data
    }

    public var contentDigest: String {
        get throws {
            try StateCodec.digest(.array(nodes.map { node in
                try .array([.string(node.path), node.data.map(StateCodec.bytes) ?? .null, .number(node.mode.decimal)])
            }))
        }
    }
}

public struct ResourceRef: Sendable, Hashable {
    public let destination: ResourcePath
    public let kind: ResourceKind
    public let schemaVersion: Int

    public init(destination: ResourcePath, kind: ResourceKind, schemaVersion: Int = 2) throws {
        guard schemaVersion == 2 else { throw FlyrailValueError("authority schema must be 2") }
        try requireResourcePath(destination)
        self.destination = destination
        self.kind = kind
        self.schemaVersion = schemaVersion
    }

    public var recordedAuthority: ResourceAuthority {
        get throws { try ResourceAuthority(recordedDestination: destination) }
    }
}

public struct Preparation: Sendable, Hashable {
    public let resource: ResourceRef
    public let transactionID: String
    public let directory: ResourcePath
    public let schemaVersion: Int

    public init(resource: ResourceRef, transactionID: String, directory: ResourcePath, schemaVersion: Int = 2) throws {
        try hexadecimalIdentity(transactionID, length: 32)
        try Self.requireConcrete(directory)
        guard schemaVersion == 2 else { throw FlyrailValueError("preparation schema must be 2") }
        self.resource = resource
        self.transactionID = transactionID
        self.directory = directory
        self.schemaVersion = schemaVersion
    }

    private static func requireConcrete(_ path: ResourcePath) throws {
        let parts = path.platformBytes.split(separator: 47, omittingEmptySubsequences: false)
        guard path.platformBytes.first == 47, !parts.contains(where: { $0.elementsEqual([46, 46]) }) else {
            throw FlyrailValueError("expected an absolute concrete path")
        }
        guard parts.contains(where: { !$0.isEmpty && !$0.elementsEqual([46]) }) else {
            throw FlyrailValueError("filesystem roots are not resources")
        }
    }
}

package func destinationOrder(_ resources: [ResourceRef]) -> [ResourceRef] {
    resources.sorted { $0.destination.platformBytes.lexicographicallyPrecedes($1.destination.platformBytes) }
}

public struct ClaimRequirement: Sendable, Hashable {
    public let destination: ResourcePath
    public let claimID: String

    public init(destination: ResourcePath, claimID: String) throws {
        try requireResourcePath(destination)
        try hexadecimalIdentity(claimID)
        self.destination = destination
        self.claimID = claimID
    }
}

public struct OwnedClaim: Sendable, Hashable {
    public let claim: Claim
    public let claimID: String
    public let bundleID: String
    public let artifactID: String
    public let version: String
    public let bundleDigest: String
    public let renderDigest: String
    public let selection: OwnedSelection?
    public let installed: Revision?
    public let disposition: Disposition
    public let baseline: Revision?
    public let requirements: [ClaimRequirement]
    public let stableRequirements: [ClaimRequirement]
    public let schemaRequirements: [DocumentSchema]

    public init(
        claim: Claim, claimID: String, bundleID: String, artifactID: String, version: String,
        bundleDigest: String, renderDigest: String, selection: OwnedSelection? = nil,
        installed: Revision? = nil, disposition: Disposition = .created, baseline: Revision? = nil,
        requirements: [ClaimRequirement] = [], stableRequirements: [ClaimRequirement] = [],
        schemaRequirements: [DocumentSchema] = [],
    ) throws {
        try hexadecimalIdentity(claimID)
        try validateIdentifier(bundleID, label: "owner")
        try validateIdentifier(artifactID, label: "artifact")
        try validateVersion(version)
        try hexadecimalIdentity(bundleDigest)
        try hexadecimalIdentity(renderDigest)
        try validateRequirements(requirements, stableRequirements, schemaRequirements, selection)
        try validateOwnership(claim, selection, installed, disposition, baseline)
        self.claim = claim
        self.claimID = claimID
        self.bundleID = bundleID
        self.artifactID = artifactID
        self.version = version
        self.bundleDigest = bundleDigest
        self.renderDigest = renderDigest
        self.selection = selection
        self.installed = installed
        self.disposition = disposition
        self.baseline = baseline
        self.requirements = requirements
        self.stableRequirements = stableRequirements
        self.schemaRequirements = schemaRequirements
    }
}

private func validateRequirements(
    _ requirements: [ClaimRequirement], _ stable: [ClaimRequirement],
    _ schemas: [DocumentSchema], _ selection: OwnedSelection?,
) throws {
    guard Set(requirements).count == requirements.count else {
        throw FlyrailValueError("duplicate claim requirement")
    }
    guard Set(stable).count == stable.count, Set(stable).isSubset(of: Set(requirements)) else {
        throw FlyrailValueError("stable requirements must be a unique subset of requirements")
    }
    guard Set(schemas.map { Array($0.key.utf8) }).count == schemas.count else {
        throw FlyrailValueError("duplicate claim schema requirement")
    }
    guard schemas.isEmpty || (selection != nil && schemas.allSatisfy { $0.format == selection?.format }) else {
        throw FlyrailValueError("schema requirements need matching document selections")
    }
}

private func validateOwnership(
    _ claim: Claim, _ selection: OwnedSelection?, _ installed: Revision?,
    _ disposition: Disposition, _ baseline: Revision?,
) throws {
    if claim.kind == .section || claim.kind == .structured {
        guard selection?.claim == claim else {
            throw FlyrailValueError("shared claims require matching selection ownership")
        }
        guard installed == nil, baseline == nil else {
            throw FlyrailValueError("shared claims cannot own complete revisions")
        }
        guard disposition == selection?.disposition else {
            throw FlyrailValueError("selection disposition disagrees")
        }
    } else {
        guard selection == nil, let installed else {
            throw FlyrailValueError("whole claims require an installed revision")
        }
        guard !installed.nodes.isEmpty else { throw FlyrailValueError("owned revisions cannot be absent") }
        guard (disposition == .takenOver) == (baseline != nil) else {
            throw FlyrailValueError("only takeover has a baseline")
        }
        guard baseline?.nodes.isEmpty != true else { throw FlyrailValueError("takeover needs an original revision") }
    }
}

public struct Receipt: Sendable, Hashable {
    public let resource: ResourceRef
    public let transactionID: String
    public let claims: [OwnedClaim]
    public let provenance: DocumentProvenance
    public let createdDirectories: [Node]
    public let schemaVersion: Int

    public init(
        resource: ResourceRef, transactionID: String, claims: [OwnedClaim] = [],
        provenance: DocumentProvenance = DocumentProvenance(), createdDirectories: [Node] = [],
        schemaVersion: Int = 2,
    ) throws {
        guard schemaVersion == 2 else { throw FlyrailValueError("receipt schema must be 2") }
        try hexadecimalIdentity(transactionID, length: 32)
        guard createdDirectories.isEmpty || resource.kind == .skillContainer else {
            throw FlyrailValueError("directory provenance requires a skill container")
        }
        guard createdDirectories.allSatisfy({ $0.data == nil }),
              Set(createdDirectories.map { Array($0.path.utf8) }).count == createdDirectories.count
        else {
            throw FlyrailValueError("created directory evidence must contain distinct directories")
        }
        try validateReceiptClaims(claims, resource)
        self.resource = resource
        self.transactionID = transactionID
        self.claims = claims
        self.provenance = provenance
        self.createdDirectories = createdDirectories
        self.schemaVersion = schemaVersion
    }
}

private func validateReceiptClaims(_ claims: [OwnedClaim], _ resource: ResourceRef) throws {
    let allowed: Set<Ownership> = switch resource.kind {
    case .document: [.file, .section, .structured]
    case .tree: [.tree]
    case .skillContainer: [.subtree]
    }
    for (index, owned) in claims.enumerated() {
        guard try owned.claimID == owned.claim.identity(destination: resource.destination) else {
            throw FlyrailValueError("claim identity disagrees with physical destination")
        }
        guard !claims.prefix(index).contains(where: { owned.claim.overlaps($0.claim) }) else {
            throw FlyrailValueError("receipt claims overlap")
        }
        guard allowed.contains(owned.claim.kind) else {
            throw FlyrailValueError("claim kind disagrees with resource kind")
        }
    }
    let schemas = Set(claims.flatMap(\.schemaRequirements))
    guard Set(schemas.map { Array($0.key.utf8) }).count == schemas.count else {
        throw FlyrailValueError("receipt has incompatible schema requirements")
    }
    for schema in schemas {
        let claim = try contentClaim(.structured(schema.content))
        guard !claims.contains(where: { claim.overlaps($0.claim) }) else {
            throw FlyrailValueError("schema requirement overlaps app ownership")
        }
    }
}

public struct GenerationMember: Sendable, Hashable {
    public let resource: ResourceRef
    public let claimIDs: [String]

    public init(resource: ResourceRef, claimIDs: [String]) throws {
        try claimIDs.forEach { try hexadecimalIdentity($0) }
        guard Set(claimIDs).count == claimIDs.count else {
            throw FlyrailValueError("duplicate membership claim")
        }
        self.resource = resource
        self.claimIDs = claimIDs
    }
}

public struct Generation: Sendable, Hashable {
    public let version: String
    public let bundleDigest: String
    public let renderDigest: String
    public let membership: [GenerationMember]

    public init(
        version: String, bundleDigest: String, renderDigest: String, membership: [GenerationMember],
    ) throws {
        try validateVersion(version)
        try hexadecimalIdentity(bundleDigest)
        try hexadecimalIdentity(renderDigest)
        guard Set(membership.map(\.resource.destination)).count == membership.count else {
            throw FlyrailValueError("duplicate generation resource")
        }
        self.version = version
        self.bundleDigest = bundleDigest
        self.renderDigest = renderDigest
        self.membership = membership
    }
}

public struct Index: Sendable, Hashable {
    public let bundleID: String
    public let contextDigest: String
    public let current: Generation?
    public let pending: Generation?
    public let previous: Generation?
    public let residual: [ResourceRef]
    public let schemaVersion: Int

    public init(
        bundleID: String, contextDigest: String, current: Generation? = nil, pending: Generation? = nil,
        previous: Generation? = nil, residual: [ResourceRef] = [], schemaVersion: Int = 2,
    ) throws {
        try validateIdentifier(bundleID, label: "index owner")
        try hexadecimalIdentity(contextDigest)
        guard schemaVersion == 2 else { throw FlyrailValueError("index schema must be 2") }
        self.bundleID = bundleID
        self.contextDigest = contextDigest
        self.current = current
        self.pending = pending
        self.previous = previous
        self.residual = residual
        self.schemaVersion = schemaVersion
        guard Set(resources.map(\.destination)).count == resources.count,
              Set(residual).count == residual.count
        else {
            throw FlyrailValueError("index has conflicting resource kinds")
        }
    }

    public var resources: [ResourceRef] {
        destinationOrder(Array(Set(
            residual + [current, pending, previous].compactMap(\.self).flatMap { $0.membership.map(\.resource) },
        )))
    }
}

public struct Journal: Sendable, Hashable {
    public let resource: ResourceRef
    public let previous: Receipt?
    public let receipt: Receipt
    public let before: Revision
    public let staged: Revision
    public let published: Revision
    public let backup: Revision
    public let ancestors: [Ancestor]
    public let schemaVersion: Int

    public init(
        resource: ResourceRef, previous: Receipt?, receipt: Receipt, before: Revision, staged: Revision,
        published: Revision, backup: Revision, ancestors: [Ancestor], schemaVersion: Int = 2,
    ) throws {
        guard receipt.resource == resource, previous == nil || previous?.resource == resource else {
            throw FlyrailValueError("journal receipt resources disagree")
        }
        guard schemaVersion == 2 else { throw FlyrailValueError("journal schema must be 2") }
        guard try before.contentDigest == backup.contentDigest,
              try staged.contentDigest == published.contentDigest
        else {
            throw FlyrailValueError("journal private revisions must retain complete content")
        }
        self.resource = resource
        self.previous = previous
        self.receipt = receipt
        self.before = before
        self.staged = staged
        self.published = published
        self.backup = backup
        self.ancestors = ancestors
        self.schemaVersion = schemaVersion
    }
}
