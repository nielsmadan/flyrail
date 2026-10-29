import Foundation

public enum NoticeKind: String, Sendable, Hashable {
    case prerequisite
    case activation
    case unsupported
}

public struct Notice: Sendable, Hashable {
    public let artifactID: String
    public let kind: NoticeKind
    public let message: String
    public let code: String

    public init(
        artifactID: String,
        kind: NoticeKind,
        message: String,
        code: String = "",
    ) throws {
        try validateIdentifier(artifactID, label: "notice artifact id")
        guard !message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw FlyrailValueError("notice message must be nonblank")
        }
        if !code.isEmpty {
            try validateIdentifier(code, label: "notice code")
        }
        self.artifactID = artifactID
        self.kind = kind
        self.message = message
        self.code = code
    }

    public static func == (left: Self, right: Self) -> Bool {
        exactTextEqual(left.artifactID, right.artifactID) &&
            left.kind == right.kind &&
            exactTextEqual(left.message, right.message) &&
            exactTextEqual(left.code, right.code)
    }

    public func hash(into hasher: inout Hasher) {
        hashExactText(artifactID, into: &hasher)
        hasher.combine(kind)
        hashExactText(message, into: &hasher)
        hashExactText(code, into: &hasher)
    }
}

public struct RenderedArtifact: Sendable, Hashable {
    public let id: String
    public let family: Family
    public let destination: ResourcePath
    public let content: Content
    public let subtree: String?
    public let requireExisting: Bool
    public let schemaRequirements: [DocumentSchema]

    public init(
        id: String,
        family: Family,
        destination: ResourcePath,
        content: Content,
        subtree: String? = nil,
        requireExisting: Bool = false,
        schemaRequirements: [DocumentSchema] = [],
    ) throws {
        try validateIdentifier(id, label: "rendered artifact id")
        _ = try validateRoute(destination)
        guard destination.displayString != "/" else {
            throw FlyrailValueError("rendered destination must be an absolute Path")
        }
        guard Set(schemaRequirements.map(\.key)).count == schemaRequirements.count else {
            throw FlyrailValueError("duplicate document schema requirement")
        }
        if !schemaRequirements.isEmpty {
            guard case let .structured(structured) = content,
                  schemaRequirements.allSatisfy({ $0.format == structured.format })
            else {
                throw FlyrailValueError("document schema requirements need matching structured content")
            }
        }
        if let subtree {
            try validateRelativePath(subtree, label: "rendered subtree")
            guard case .tree = content else {
                throw FlyrailValueError("subtree claims require tree content")
            }
        }
        self.id = id
        self.family = family
        self.destination = destination
        self.content = content
        self.subtree = subtree
        self.requireExisting = requireExisting
        self.schemaRequirements = schemaRequirements
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "RenderedArtifact",
            fields: [
                ("content", content.modelValue()),
                ("destination", .scalar(.text(destination.displayString))),
                ("family", .scalar(.text(family.rawValue))),
                ("id", .scalar(.text(id))),
                ("require_existing", .scalar(.boolean(requireExisting))),
                ("schema_requirements", .array(schemaRequirements.map { try $0.modelValue() })),
                ("subtree", subtree.map { .scalar(.text($0)) } ?? .scalar(.null)),
            ],
        )
    }
}

public struct RenderedBundle: Sendable, Hashable {
    public let artifacts: [RenderedArtifact]
    public let dependencies: [Dependency]
    public let notices: [Notice]
    public let routingContext: [(String, String)]
    public let renderDigest: String

    public var supported: Bool {
        !notices.contains { $0.kind == .unsupported }
    }

    public init(
        artifacts: [RenderedArtifact],
        dependencies: [Dependency] = [],
        notices: [Notice] = [],
        routingContext: [(String, String)] = [],
    ) throws {
        guard Set(artifacts.map(\.id)).count == artifacts.count else {
            throw FlyrailValueError("rendered artifact ids must be unique")
        }
        _ = try dependencyOrder(artifacts: artifacts, dependencies: dependencies)
        let byID = Dictionary(uniqueKeysWithValues: artifacts.map { ($0.id, $0) })
        for edge in dependencies where edge.mode == .stableReference {
            guard let required = byID[edge.required], case .file = required.content else {
                throw FlyrailValueError("stable references require a whole-file routing entry")
            }
            guard !dependencies.contains(where: {
                $0.dependent == edge.required && $0.mode != .revision
            }) else {
                throw FlyrailValueError("stable routing entries require revision-bound dependencies")
            }
        }
        guard Set(routingContext.map(\.0)).count == routingContext.count else {
            throw FlyrailValueError("render routing context keys must be unique")
        }
        self.artifacts = artifacts.sorted { utf8Less($0.id, $1.id) }
        self.dependencies = dependencies.sorted {
            $0.dependent == $1.dependent
                ? utf8Less($0.required, $1.required)
                : utf8Less($0.dependent, $1.dependent)
        }
        self.notices = notices
        self.routingContext = routingContext.sorted {
            $0.0 == $1.0 ? utf8Less($0.1, $1.1) : utf8Less($0.0, $1.0)
        }
        renderDigest = try ModelEncoding.digest(
            domain: "flyrail-render-v1",
            values: [
                .array(self.artifacts.map { try $0.modelValue() }),
                .array(self.dependencies.map { try $0.modelValue() }),
            ],
        )
    }

    public static var empty: Self {
        get throws { try Self(artifacts: []) }
    }

    public static func == (left: Self, right: Self) -> Bool {
        left.artifacts == right.artifacts &&
            left.dependencies == right.dependencies &&
            left.notices == right.notices &&
            routeEqual(left.routingContext, right.routingContext) &&
            left.renderDigest == right.renderDigest
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(artifacts)
        hasher.combine(dependencies)
        hasher.combine(notices)
        for pair in routingContext {
            hashExactText(pair.0, into: &hasher)
            hashExactText(pair.1, into: &hasher)
        }
        hashExactText(renderDigest, into: &hasher)
    }
}

public func dependencyOrder(
    artifacts: [RenderedArtifact],
    dependencies: [Dependency],
) throws -> [RenderedArtifact] {
    let names = artifacts.map(\.id)
    guard Set(names).count == names.count else {
        throw FlyrailValueError("rendered artifact ids must be unique")
    }
    var pairs: Set<String> = []
    for edge in dependencies {
        guard pairs.insert("\(edge.dependent)\0\(edge.required)").inserted else {
            throw FlyrailValueError("duplicate dependency")
        }
        guard names.contains(edge.dependent), names.contains(edge.required) else {
            throw FlyrailValueError("dependency references an unknown artifact or asset")
        }
    }
    var pending = Set(names)
    var result: [RenderedArtifact] = []
    let byID = Dictionary(uniqueKeysWithValues: artifacts.map { ($0.id, $0) })
    while !pending.isEmpty {
        let ready = pending.filter { name in
            !dependencies.contains { $0.dependent == name && pending.contains($0.required) }
        }.sorted(by: utf8Less)
        guard !ready.isEmpty else {
            throw FlyrailValueError("cyclic artifact dependencies")
        }
        result += ready.compactMap { byID[$0] }
        pending.subtract(ready)
    }
    return result
}

private func routeEqual(_ left: [(String, String)], _ right: [(String, String)]) -> Bool {
    left.count == right.count && zip(left, right).allSatisfy {
        exactTextEqual($0.0.0, $0.1.0) && exactTextEqual($0.0.1, $0.1.1)
    }
}
