import Foundation

public enum Ownership: String, Sendable, Hashable {
    case file
    case tree
    case subtree
    case section
    case structured
}

public enum ClaimSelector: Sendable, Hashable {
    case whole
    case subtree(String)
    case section(SectionBoundaries)
    case structured(Selector)

    public static func == (left: Self, right: Self) -> Bool {
        switch (left, right) {
        case (.whole, .whole):
            true
        case let (.subtree(left), .subtree(right)):
            exactTextEqual(left, right)
        case let (.section(left), .section(right)):
            left == right
        case let (.structured(left), .structured(right)):
            left == right
        default:
            false
        }
    }

    public func hash(into hasher: inout Hasher) {
        switch self {
        case .whole:
            hasher.combine(0)
        case let .subtree(value):
            hasher.combine(1)
            hashExactText(value, into: &hasher)
        case let .section(value):
            hasher.combine(2)
            hasher.combine(value)
        case let .structured(value):
            hasher.combine(3)
            hasher.combine(value)
        }
    }
}

public struct Claim: Sendable, Hashable {
    public let kind: Ownership
    public let selector: ClaimSelector

    public init(kind: Ownership, selector: ClaimSelector) throws {
        switch (kind, selector) {
        case (.file, .whole), (.tree, .whole), (.structured, .structured),
             (.section, .section):
            break
        case let (.section, .subtree(marker)):
            self.kind = kind
            self.selector = try .section(SectionBoundaries(marker: marker))
            return
        case let (.subtree, .subtree(path)):
            try validateRelativePath(path, label: "subtree selector")
        case (.file, _), (.tree, _):
            throw FlyrailValueError("whole-resource claims cannot have selectors")
        case (.structured, _):
            throw FlyrailValueError("structured claims require a Selector")
        case (.section, _):
            throw FlyrailValueError("section claims require SectionBoundaries or a marker name")
        case (.subtree, _):
            throw FlyrailValueError("subtree claims require a string selector")
        }
        self.kind = kind
        self.selector = selector
    }

    public func overlaps(_ other: Claim) -> Bool {
        if kind == .file || kind == .tree || other.kind == .file || other.kind == .tree {
            return true
        }
        guard kind == other.kind else {
            return true
        }
        switch (selector, other.selector) {
        case let (.section(left), .section(right)):
            return [
                exactTextEqual(left.start, right.start),
                exactTextEqual(left.start, right.end),
                exactTextEqual(left.end, right.start),
                exactTextEqual(left.end, right.end),
            ].contains(true)
        case let (.structured(left), .structured(right)):
            for (leftPart, rightPart) in zip(left.parts, right.parts) {
                if leftPart == rightPart {
                    continue
                }
                switch (leftPart, rightPart) {
                case (.key, .key):
                    return false
                case let (.member(leftMember), .member(rightMember)):
                    return !exactStringArraysEqual(leftMember.key, rightMember.key) ||
                        leftMember.identity == rightMember.identity
                default:
                    return true
                }
            }
            return true
        case let (.subtree(left), .subtree(right)):
            let leftKey = portablePathKey(left)
            let rightKey = portablePathKey(right)
            return leftKey == rightKey ||
                leftKey.starts(with: rightKey + "/") ||
                rightKey.starts(with: leftKey + "/")
        default:
            return true
        }
    }
}

public enum Acquisition: String, Sendable, Hashable {
    case conflict
    case adopt
    case takeover
}

public enum Disposition: String, Sendable, Hashable {
    case created
    case takenOver = "taken-over"
    case adopted
}

public enum EditStatus: String, Sendable, Hashable {
    case created
    case takenOver = "taken-over"
    case adopted
    case current
    case updated
    case removed
    case modified
    case conflict
}

public enum SnapshotValue: Sendable, Hashable {
    case bytes(Data)
    case typed(SemanticValue)
}

public struct ArrayPosition: Sendable, Hashable {
    public let previous: Member?
    public let following: Member?
    public let prefix: Data

    public init(previous: Member? = nil, following: Member? = nil, prefix: Data = Data()) throws {
        guard previous == nil || previous != following else {
            throw FlyrailValueError("array position anchors must be distinct")
        }
        self.previous = previous
        self.following = following
        self.prefix = prefix
    }
}

public struct SelectionSnapshot: Sendable, Hashable {
    public let value: SnapshotValue
    public let syntax: Data
    public let ownedSyntax: Data
    public let position: ArrayPosition?
    public let layout: Data

    public init(
        value: SnapshotValue,
        syntax: Data,
        ownedSyntax: Data,
        position: ArrayPosition? = nil
    ) {
        self.value = value
        self.syntax = syntax
        self.ownedSyntax = ownedSyntax
        self.position = position
        layout = Data()
    }

    public init(
        value: SnapshotValue, syntax: Data, ownedSyntax: Data,
        position: ArrayPosition? = nil, layout: Data
    ) throws {
        self.value = value
        self.syntax = syntax
        self.ownedSyntax = ownedSyntax
        self.position = position
        self.layout = layout
        try validateTOMLLayout(self)
    }
}

public struct OwnedSelection: Sendable, Hashable {
    public let claim: Claim
    public let format: DocumentFormat?
    public let installed: SelectionSnapshot
    public let disposition: Disposition
    public let baseline: SelectionSnapshot?
    public let separator: Data

    public init(
        claim: Claim,
        format: DocumentFormat?,
        installed: SelectionSnapshot,
        disposition: Disposition,
        baseline: SelectionSnapshot? = nil,
        separator: Data = Data()
    ) throws {
        let validClaim = (claim.kind == .section && format == nil) ||
            (claim.kind == .structured && format != nil)
        guard validClaim else {
            throw FlyrailValueError("shared claim and document format disagree")
        }
        guard (disposition == .takenOver) == (baseline != nil) else {
            throw FlyrailValueError("only takeover ownership has a baseline")
        }
        for snapshot in [installed, baseline].compactMap(\.self) {
            let containsBytes = if case .bytes = snapshot.value { true } else { false }
            guard containsBytes == (format == nil) else {
                throw FlyrailValueError("selection snapshot and document format disagree")
            }
            guard snapshot.layout.isEmpty || format == .toml else {
                throw FlyrailValueError("restoration layout requires a TOML claim")
            }
            if case let .structured(selector) = claim.selector {
                try validateTOMLLayout(snapshot, selector: selector)
            }
        }
        guard format == nil || separator.isEmpty else {
            throw FlyrailValueError("section separators require a text claim")
        }
        self.claim = claim
        self.format = format
        self.installed = installed
        self.disposition = disposition
        self.baseline = baseline
        self.separator = separator
    }
}

public struct CreatedContainer: Sendable, Hashable {
    public let selector: Selector
    public let emptySyntax: Data

    public init(selector: Selector, emptySyntax: Data) {
        self.selector = selector
        self.emptySyntax = emptySyntax
    }
}

public struct DocumentProvenance: Sendable, Hashable {
    public let emptyDocument: Data?
    public let containers: [CreatedContainer]
    public let trailingSeparators: [String]
    public let schemaFields: [OwnedSelection]
    public let schemaEmptyDocument: Data?

    public init() {
        emptyDocument = nil
        containers = []
        trailingSeparators = []
        schemaFields = []
        schemaEmptyDocument = nil
    }

    public init(
        emptyDocument: Data?,
        containers: [CreatedContainer] = [],
        trailingSeparators: [String] = [],
        schemaFields: [OwnedSelection] = [],
        schemaEmptyDocument: Data? = nil
    ) throws {
        guard Set(containers.map(\.selector)).count == containers.count else {
            throw FlyrailValueError("duplicate container provenance")
        }
        guard trailingSeparators.allSatisfy({
            $0.utf8.count == 64 && $0.utf8.allSatisfy { byte in
                (48 ... 57).contains(byte) || (97 ... 102).contains(byte)
            }
        }) else {
            throw FlyrailValueError("separator evidence must be SHA-256 digests")
        }
        guard schemaEmptyDocument?.allSatisfy({ [9, 10, 11, 12, 13, 32].contains($0) }) != false else {
            throw FlyrailValueError("schema empty-document evidence must be whitespace bytes")
        }
        guard schemaFields.allSatisfy({ field in
            guard field.disposition == .created,
                  field.format == .json || field.format == .jsonc,
                  case let .structured(selector) = field.claim.selector,
                  selector.parts.count == 1,
                  case .key? = selector.parts.first,
                  case .typed(.scalar) = field.installed.value
            else {
                return false
            }
            return true
        }) else {
            throw FlyrailValueError("schema provenance requires created top-level scalar fields")
        }
        guard Set(schemaFields.map(\.claim)).count == schemaFields.count else {
            throw FlyrailValueError("duplicate schema provenance")
        }
        self.emptyDocument = emptyDocument
        self.containers = containers
        self.trailingSeparators = trailingSeparators
        self.schemaFields = schemaFields
        self.schemaEmptyDocument = schemaEmptyDocument
    }
}

public struct DocumentRequest: Sendable, Hashable {
    public let claim: Claim
    public let content: Content?
    public let acquisition: Acquisition

    public init(
        claim: Claim,
        content: Content?,
        acquisition: Acquisition = .conflict
    ) throws {
        guard claim.kind == .section || claim.kind == .structured else {
            throw FlyrailValueError("document requests require section or structured claims")
        }
        if let content {
            guard try claim == contentClaim(content) else {
                throw FlyrailValueError("requested content must address its physical claim")
            }
        } else if acquisition != .conflict {
            throw FlyrailValueError("retirement does not acquire ownership")
        }
        self.claim = claim
        self.content = content
        self.acquisition = acquisition
    }

    public static func set(
        _ content: Content,
        acquisition: Acquisition = .conflict
    ) throws -> Self {
        try Self(claim: contentClaim(content), content: content, acquisition: acquisition)
    }

    public static func retire(_ claim: Claim) throws -> Self {
        try Self(claim: claim, content: nil)
    }
}

public struct EditObservation: Sendable, Hashable {
    public let claim: Claim
    public let status: EditStatus
    public let current: SelectionSnapshot?
    public let message: String

    public init(
        claim: Claim,
        status: EditStatus,
        current: SelectionSnapshot?,
        message: String = ""
    ) {
        self.claim = claim
        self.status = status
        self.current = current
        self.message = message
    }

    public static func == (left: Self, right: Self) -> Bool {
        left.claim == right.claim &&
            left.status == right.status &&
            left.current == right.current &&
            exactTextEqual(left.message, right.message)
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(claim)
        hasher.combine(status)
        hasher.combine(current)
        hashExactText(message, into: &hasher)
    }
}

public struct DocumentEdit: Sendable, Hashable {
    public let before: Data?
    public let after: Data?
    public let ownership: [OwnedSelection]
    public let provenance: DocumentProvenance
    public let observations: [EditObservation]

    public var applicable: Bool {
        observations.allSatisfy { $0.status != .conflict && $0.status != .modified }
    }
}

public func contentClaim(_ content: Content) throws -> Claim {
    switch content {
    case .file, .tree:
        throw FlyrailValueError("document content must be a section or structured selection")
    case let .section(section):
        try Claim(kind: .section, selector: .section(section.selector))
    case let .structured(structured):
        try Claim(kind: .structured, selector: .structured(structured.selector))
    }
}

private func exactStringArraysEqual(_ left: [String], _ right: [String]) -> Bool {
    left.count == right.count && zip(left, right).allSatisfy(exactTextEqual)
}
