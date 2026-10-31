import Foundation

public enum ErrorCode: String, Sendable, Hashable {
    case busy
    case conflict
    case modified
    case updateRequired = "update_required"
    case unsupported
    case ioError = "io_error"
    case unsafePath = "unsafe_path"
    case invalidState = "invalid_state"
    case concurrentChange = "concurrent_change"
    case recoveryNeeded = "recovery_needed"
}

public enum OperationStatus: String, Sendable, Hashable {
    case applied
    case unchanged
    case failed
    case partial
    case incomplete
}

public struct TargetError: Error, Sendable, Hashable {
    public let code: ErrorCode
    public let message: String
    public let path: ResourcePath?
    public let errno: Int32?

    public init(code: ErrorCode, message: String, path: ResourcePath? = nil, errno: Int32? = nil) {
        self.code = code
        self.message = message
        self.path = path
        self.errno = errno
    }

    public static func == (left: Self, right: Self) -> Bool {
        left.code == right.code && exactTextEqual(left.message, right.message) &&
            left.path == right.path && left.errno == right.errno
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(code)
        hashExactText(message, into: &hasher)
        hasher.combine(path)
        hasher.combine(errno)
    }
}

public struct InstallationTarget: Sendable, Hashable {
    public let store: any StateStore
    public let context: [(String, String)]
    public let routingContext: [(String, String)]

    public init(
        store: any StateStore, context: [(String, String)] = [], routingContext: [(String, String)] = [],
    ) throws {
        self.store = store
        self.context = try stateContext(context)
        self.routingContext = try stateContext(routingContext)
    }

    public var contextDigest: String {
        get throws {
            try StateCodec.digest(.array(context.map { .array([.string($0.0), .string($0.1)]) }))
        }
    }

    public static func == (left: Self, right: Self) -> Bool {
        ObjectIdentifier(left.store) == ObjectIdentifier(right.store) &&
            exactContextEqual(left.context, right.context) &&
            exactContextEqual(left.routingContext, right.routingContext)
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(ObjectIdentifier(store))
        for pairs in [context, routingContext] {
            hasher.combine(pairs.count)
            for pair in pairs {
                hashExactText(pair.0, into: &hasher)
                hashExactText(pair.1, into: &hasher)
            }
        }
    }
}

package func stateContext(_ pairs: [(String, String)]) throws -> [(String, String)] {
    guard Set(pairs.map { Array($0.0.utf8) }).count == pairs.count else {
        throw FlyrailValueError("target context keys must be unique")
    }
    return pairs.sorted { utf8Less($0.0, $1.0) }
}

private func exactContextEqual(_ left: [(String, String)], _ right: [(String, String)]) -> Bool {
    left.count == right.count && zip(left, right).allSatisfy {
        exactTextEqual($0.0.0, $0.1.0) && exactTextEqual($0.0.1, $0.1.1)
    }
}

public struct ClaimObservation: Sendable, Hashable {
    public let claimID: String
    public let bundleID: String
    public let artifactID: String
    public let version: String
    public let bundleDigest: String
    public let renderDigest: String
    public let status: EditStatus

    public init(
        claimID: String, bundleID: String, artifactID: String, version: String,
        bundleDigest: String, renderDigest: String, status: EditStatus,
    ) throws {
        try hexadecimalIdentity(claimID)
        try validateIdentifier(bundleID, label: "bundle id")
        try validateIdentifier(artifactID, label: "artifact id")
        try validateVersion(version)
        try hexadecimalIdentity(bundleDigest)
        try hexadecimalIdentity(renderDigest)
        self.claimID = claimID
        self.bundleID = bundleID
        self.artifactID = artifactID
        self.version = version
        self.bundleDigest = bundleDigest
        self.renderDigest = renderDigest
        self.status = status
    }

    public static func == (left: Self, right: Self) -> Bool {
        left.claimID == right.claimID && left.bundleID == right.bundleID && left.artifactID == right.artifactID &&
            exactTextEqual(left.version, right.version) && left.bundleDigest == right.bundleDigest &&
            left.renderDigest == right.renderDigest && left.status == right.status
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(claimID)
        hasher.combine(bundleID)
        hasher.combine(artifactID)
        hashExactText(version, into: &hasher)
        hasher.combine(bundleDigest)
        hasher.combine(renderDigest)
        hasher.combine(status)
    }
}

public struct ResourceObservation: Sendable, Hashable {
    public let destination: ResourcePath
    public let claims: [ClaimObservation]
    public let revisionDigest: String?
    public let error: TargetError?
    public let recoveryPaths: [ResourcePath]

    public init(
        destination: ResourcePath, claims: [ClaimObservation] = [], revisionDigest: String? = nil,
        error: TargetError? = nil, recoveryPaths: [ResourcePath] = [],
    ) throws {
        try requireResourcePath(destination)
        if let revisionDigest {
            try hexadecimalIdentity(revisionDigest)
        }
        try recoveryPaths.forEach(requireResourcePath)
        self.destination = destination
        self.claims = claims
        self.revisionDigest = revisionDigest
        self.error = error
        self.recoveryPaths = recoveryPaths
    }
}

public struct InstallationObservation: Sendable, Hashable {
    public let bundleID: String
    public let target: InstallationTarget
    public let resources: [ResourceObservation]
    public let isCurrent: Bool
    public let pending: Bool
    public let version: String?
    public let bundleDigest: String?
    public let renderDigest: String?
    public let error: TargetError?

    package init(failedBundleID: String, target: InstallationTarget, resources: [ResourceObservation] = [],
                 error: TargetError)
    {
        bundleID = failedBundleID
        self.target = target
        self.resources = resources
        isCurrent = false
        pending = false
        version = nil
        bundleDigest = nil
        renderDigest = nil
        self.error = error
    }

    public init(
        bundleID: String, target: InstallationTarget, resources: [ResourceObservation], isCurrent: Bool,
        pending: Bool, version: String? = nil, bundleDigest: String? = nil, renderDigest: String? = nil,
        error: TargetError? = nil,
    ) throws {
        try validateIdentifier(bundleID, label: "bundle id")
        if let version {
            try validateVersion(version)
        }
        try [bundleDigest, renderDigest].compactMap(\.self).forEach { try hexadecimalIdentity($0) }
        self.bundleID = bundleID
        self.target = target
        self.resources = resources
        self.isCurrent = isCurrent
        self.pending = pending
        self.version = version
        self.bundleDigest = bundleDigest
        self.renderDigest = renderDigest
        self.error = error
    }

    public func matches(_ bundle: FlyrailBundle, _ rendered: RenderedBundle) -> Bool {
        isCurrent && !pending && error == nil && rendered.supported &&
            exactTextEqual(bundleID, bundle.identity.id) &&
            version.map { exactTextEqual($0, bundle.identity.version) } == true &&
            bundleDigest == bundle.contentDigest && renderDigest == rendered.renderDigest
    }

    public static func == (left: Self, right: Self) -> Bool {
        left.bundleID == right.bundleID && left.target == right.target && left.resources == right.resources &&
            left.isCurrent == right.isCurrent && left.pending == right.pending &&
            left.version.map { Array($0.utf8) } == right.version.map { Array($0.utf8) } &&
            left.bundleDigest == right.bundleDigest && left.renderDigest == right.renderDigest && left.error == right
            .error
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(bundleID)
        hasher.combine(target)
        hasher.combine(resources)
        hasher.combine(isCurrent)
        hasher.combine(pending)
        hasher.combine(version.map { Array($0.utf8) })
        hasher.combine(bundleDigest)
        hasher.combine(renderDigest)
        hasher.combine(error)
    }
}

public struct ResourceResult: Sendable, Hashable {
    public let resource: ResourceRef
    public let status: OperationStatus
    public let error: TargetError?
    public let recoveryPaths: [ResourcePath]

    public init(
        resource: ResourceRef, status: OperationStatus, error: TargetError? = nil, recoveryPaths: [ResourcePath] = [],
    ) throws {
        try recoveryPaths.forEach(requireResourcePath)
        self.resource = resource
        self.status = status
        self.error = error
        self.recoveryPaths = recoveryPaths
    }
}

public struct InstallationResult: Sendable, Hashable {
    public let status: OperationStatus
    public let observation: InstallationObservation
    public let resources: [ResourceResult]
    public let error: TargetError?
    public let notices: [Notice]

    public init(
        status: OperationStatus, observation: InstallationObservation, resources: [ResourceResult] = [],
        error: TargetError? = nil, notices: [Notice] = [],
    ) {
        self.status = status
        self.observation = observation
        self.resources = resources
        self.error = error
        self.notices = notices
    }
}

public struct ResourcePlan: Sendable, Hashable {
    public let resource: ResourceRef
    public let before: Revision
    public let after: Revision
    public let previous: Receipt?
    public let receipt: Receipt
    public let ancestors: [Ancestor]
    public let transactionDirectory: ResourcePath
    public let recoveryPaths: [ResourcePath]
    public let requireExisting: Bool
    public let error: TargetError?

    public init(
        resource: ResourceRef, before: Revision, after: Revision, previous: Receipt?, receipt: Receipt,
        ancestors: [Ancestor], transactionDirectory: ResourcePath, recoveryPaths: [ResourcePath],
        requireExisting: Bool = false, error: TargetError? = nil,
    ) throws {
        guard receipt.resource == resource, previous == nil || previous?.resource == resource else {
            throw FlyrailValueError("planned receipts disagree with resource")
        }
        try requireResourcePath(transactionDirectory)
        try recoveryPaths.forEach(requireResourcePath)
        self.resource = resource
        self.before = before
        self.after = after
        self.previous = previous
        self.receipt = receipt
        self.ancestors = ancestors
        self.transactionDirectory = transactionDirectory
        self.recoveryPaths = recoveryPaths
        self.requireExisting = requireExisting
        self.error = error
    }
}

public struct LifecyclePreview: Sendable, Hashable {
    public let bundleID: String
    public let bundle: FlyrailBundle?
    public let rendered: RenderedBundle
    public let target: InstallationTarget
    public let acquisition: Acquisition
    public let replaceModified: Bool
    public let index: Index?
    public let generation: Generation?
    public let resources: [ResourcePlan]
    public let error: TargetError?
    package let artifactsByResource: [ResourceRef: [RenderedArtifact]]

    package init(
        bundleID: String, bundle: FlyrailBundle?, rendered: RenderedBundle, target: InstallationTarget,
        acquisition: Acquisition, replaceModified: Bool, index: Index?, generation: Generation?,
        resources: [ResourcePlan], error: TargetError? = nil,
        artifactsByResource: [ResourceRef: [RenderedArtifact]] = [:],
    ) throws {
        try validateIdentifier(bundleID, label: "bundle id")
        guard bundle == nil || bundle?.identity.id == bundleID else {
            throw FlyrailValueError("preview bundle identity disagrees")
        }
        self.bundleID = bundleID
        self.bundle = bundle
        self.rendered = rendered
        self.target = target
        self.acquisition = acquisition
        self.replaceModified = replaceModified
        self.index = index
        self.generation = generation
        self.resources = resources
        self.error = error
        self.artifactsByResource = artifactsByResource
    }

    public var applicable: Bool {
        error == nil && resources.allSatisfy { $0.error == nil && $0.recoveryPaths.isEmpty }
    }
}
