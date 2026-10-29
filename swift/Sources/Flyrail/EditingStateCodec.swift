import Foundation

package protocol StateEnumeration: StateCodable, RawRepresentable where RawValue == String {
    static var stateTag: String { get }
}

package extension StateEnumeration {
    func stateRecord() -> JSONValue {
        StateCodec.enumeration(Self.stateTag, rawValue)
    }

    static func fromState(_ value: JSONValue) throws -> Self {
        let text = try StateCodec.enumText(value, tag: stateTag)
        guard let result = Self(rawValue: text) else {
            throw StateCodec.invalidEnumValue(.string(text), tag: stateTag)
        }
        return result
    }
}

extension Ownership: StateEnumeration { package static let stateTag = "Ownership" }
extension Disposition: StateEnumeration { package static let stateTag = "Disposition" }
extension DocumentFormat: StateEnumeration { package static let stateTag = "DocumentFormat" }
extension ResourceKind: StateEnumeration { package static let stateTag = "ResourceKind" }

extension Key: StateCodable {
    package func stateRecord() -> JSONValue {
        StateCodec.record("Key", [("name", .string(name))])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        try Self(StateFields(value, tag: "Key", names: ["name"]).text("name"))
    }
}

extension Member: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("Member", [
            ("identity", StateCodec.semantic(identity)), ("key", .array(key.map(JSONValue.string))),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "Member", names: ["identity", "key"])
        return try Self(identity: fields.semantic("identity"), key: fields.array("key").map(stateText))
    }
}

extension Selector: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("Selector", [
            ("parts", .array(parts.map { part in
                switch part {
                case let .key(key): key.stateRecord()
                case let .member(member): try member.stateRecord()
                }
            })),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "Selector", names: ["parts"])
        return try Self(parts: fields.array("parts").map { part in
            switch stateRecordTag(part) {
            case "Key": try .key(Key.fromState(part))
            case "Member": try .member(Member.fromState(part))
            default: throw FlyrailValueError("invalid state record")
            }
        })
    }
}

extension SectionBoundaries: StateCodable {
    package func stateRecord() -> JSONValue {
        StateCodec.record("SectionBoundaries", [("start", .string(start)), ("end", .string(end))])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "SectionBoundaries", names: ["start", "end"])
        return try Self(start: fields.text("start"), end: fields.text("end"))
    }
}

extension Claim: StateCodable {
    package func stateRecord() throws -> JSONValue {
        let value: JSONValue = switch selector {
        case .whole: .null
        case let .subtree(path): .string(path)
        case let .section(boundaries): boundaries.stateRecord()
        case let .structured(selector): try selector.stateRecord()
        }
        return StateCodec.record("Claim", [("kind", kind.stateRecord()), ("selector", value)])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "Claim", names: ["kind", "selector"])
        let kind: Ownership = try fields.decode("kind")
        let raw = try fields.raw("selector")
        let selector: ClaimSelector
        switch raw {
        case .null: selector = .whole
        case let .string(path): selector = .subtree(path)
        default:
            switch stateRecordTag(raw) {
            case "SectionBoundaries": selector = try .section(SectionBoundaries.fromState(raw))
            case "Selector": selector = try .structured(Selector.fromState(raw))
            default:
                if kind == .file || kind == .tree {
                    throw FlyrailValueError("whole-resource claims cannot have selectors")
                }
                throw FlyrailValueError("invalid state record")
            }
        }
        return try Self(kind: kind, selector: selector)
    }
}

extension DocumentSchema: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("DocumentSchema", [
            ("format", format.stateRecord()), ("key", .string(key)),
            ("value", StateCodec.semantic(.scalar(value))),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "DocumentSchema", names: ["format", "key", "value"])
        guard case .object = try fields.raw("format") else {
            throw FlyrailValueError("document schema scaffolds require JSON or JSONC")
        }
        guard case let .scalar(scalar) = try fields.semantic("value") else {
            throw FlyrailValueError("invalid state record")
        }
        return try Self(format: fields.decode("format"), key: fields.text("key"), value: scalar)
    }
}

extension ArrayPosition: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("ArrayPosition", [
            ("previous", StateCodec.optional(previous)), ("following", StateCodec.optional(following)),
            ("prefix", StateCodec.bytes(prefix)),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "ArrayPosition", names: ["previous", "following", "prefix"])
        return try Self(
            previous: fields.optional("previous"), following: fields.optional("following"),
            prefix: fields.bytes("prefix"),
        )
    }
}

extension SelectionSnapshot: StateCodable {
    package func stateRecord() throws -> JSONValue {
        let encoded: JSONValue = switch value {
        case let .bytes(data): try StateCodec.bytes(data)
        case let .typed(value): try StateCodec.semantic(value)
        }
        return try StateCodec.record("SelectionSnapshot", [
            ("value", encoded), ("syntax", StateCodec.bytes(syntax)),
            ("owned_syntax", StateCodec.bytes(ownedSyntax)), ("position", StateCodec.optional(position)),
            ("layout", StateCodec.bytes(layout)),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(
            value, tag: "SelectionSnapshot", names: ["value", "syntax", "owned_syntax", "position", "layout"],
        )
        let raw = try fields.raw("value")
        let snapshot: SnapshotValue = if case let .object(items) = raw, items.count == 1, items[0].0 == "bytes" {
            try .bytes(StateCodec.decodeBytes(raw))
        } else {
            try .typed(StateCodec.decodeSemantic(raw))
        }
        return try Self(
            value: snapshot, syntax: fields.bytes("syntax"), ownedSyntax: fields.bytes("owned_syntax"),
            position: fields.optional("position"), layout: fields.bytes("layout"),
        )
    }
}

extension OwnedSelection: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("OwnedSelection", [
            ("claim", claim.stateRecord()), ("format", StateCodec.optional(format)),
            ("installed", installed.stateRecord()), ("disposition", disposition.stateRecord()),
            ("baseline", StateCodec.optional(baseline)), ("separator", StateCodec.bytes(separator)),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(
            value, tag: "OwnedSelection",
            names: ["claim", "format", "installed", "disposition", "baseline", "separator"],
        )
        return try Self(
            claim: fields.decode("claim"), format: fields.optional("format"), installed: fields.decode("installed"),
            disposition: fields.decode("disposition"), baseline: fields.optional("baseline"),
            separator: fields.bytes("separator"),
        )
    }
}

extension CreatedContainer: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("CreatedContainer", [
            ("selector", selector.stateRecord()), ("empty_syntax", StateCodec.bytes(emptySyntax)),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "CreatedContainer", names: ["selector", "empty_syntax"])
        return try Self(selector: fields.decode("selector"), emptySyntax: fields.bytes("empty_syntax"))
    }
}

extension DocumentProvenance: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("DocumentProvenance", [
            ("empty_document", emptyDocument.map(StateCodec.bytes) ?? .null),
            ("containers", StateCodec.sequence(containers)),
            ("trailing_separators", .array(trailingSeparators.map(JSONValue.string))),
            ("schema_fields", StateCodec.sequence(schemaFields)),
            ("schema_empty_document", schemaEmptyDocument.map(StateCodec.bytes) ?? .null),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(
            value, tag: "DocumentProvenance",
            names: ["empty_document", "containers", "trailing_separators", "schema_fields", "schema_empty_document"],
        )
        let schemaEmptyDocument = try fields.raw("schema_empty_document")
        guard schemaEmptyDocument == .null || schemaEmptyDocument.isStateByteRecord else {
            throw FlyrailValueError("schema empty-document evidence must be whitespace bytes")
        }
        return try Self(
            emptyDocument: fields.optionalBytes("empty_document"), containers: fields.sequence("containers"),
            trailingSeparators: fields.array("trailing_separators").map {
                try stateText($0, message: "separator evidence must be SHA-256 digests")
            },
            schemaFields: fields.sequence("schema_fields"),
            schemaEmptyDocument: fields.optionalBytes("schema_empty_document"),
        )
    }
}

package func stateRecordTag(_ value: JSONValue) -> String? {
    guard case let .object(items) = value,
          case let .string(tag) = items.first(where: { $0.0 == "type" })?.1
    else { return nil }
    return tag
}

package func stateText(_ value: JSONValue) throws -> String {
    try stateText(value, message: "invalid state record")
}

package func stateText(_ value: JSONValue, message: String) throws -> String {
    guard case let .string(text) = value else { throw FlyrailValueError(message) }
    return text
}

private extension JSONValue {
    var isStateByteRecord: Bool {
        guard case let .object(items) = self, items.count == 1, items[0].0 == "bytes" else { return false }
        return true
    }
}
