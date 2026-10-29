import Foundation

extension Ancestor: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("Ancestor", [
            ("path", StateCodec.path(path)), ("device", .number(device.decimal)),
            ("inode", .number(inode.decimal)), ("mode", .number(mode.decimal)),
            ("uid", .number(uid.decimal)), ("gid", .number(gid.decimal)), ("security", StateCodec.bytes(security)),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(
            value, tag: "Ancestor", names: ["path", "device", "inode", "mode", "uid", "gid", "security"],
        )
        let message = "invalid ancestor metadata"
        return try Self(
            path: fields.path("path", message: "ancestor path must be absolute"),
            device: fields.count("device", message: message),
            inode: fields.count("inode", message: message), mode: fields.count("mode", message: message),
            uid: fields.count("uid", message: message), gid: fields.count("gid", message: message),
            security: fields.bytes("security"),
        )
    }
}

extension Node: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("Node", [
            ("path", .string(path)), ("data", data.map(StateCodec.bytes) ?? .null),
            ("mode", .number(mode.decimal)), ("uid", .number(uid.decimal)), ("gid", .number(gid.decimal)),
            ("device", .number(device.decimal)), ("inode", .number(inode.decimal)),
            ("security", StateCodec.bytes(security)),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(
            value, tag: "Node", names: ["path", "data", "mode", "uid", "gid", "device", "inode", "security"],
        )
        let message = "invalid revision metadata"
        return try Self(
            path: fields.text("path"), data: fields.optionalBytes("data"), mode: fields.count("mode", message: message),
            uid: fields.count("uid", message: message), gid: fields.count("gid", message: message),
            device: fields.count("device", message: message), inode: fields.count("inode", message: message),
            security: fields.bytes("security"),
        )
    }
}

extension Revision: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("Revision", [("nodes", StateCodec.sequence(nodes))])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        try Self(nodes: StateFields(value, tag: "Revision", names: ["nodes"]).sequence("nodes"))
    }
}

extension ResourceRef: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("ResourceRef", [
            ("destination", StateCodec.path(destination)), ("kind", kind.stateRecord()),
            ("state_root", StateCodec.path(stateRoot)), ("schema_version", .number(String(schemaVersion))),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(
            value, tag: "ResourceRef", names: ["destination", "kind", "state_root", "schema_version"],
        )
        return try Self(
            destination: fields.path("destination", message: "expected an absolute concrete path"),
            kind: fields.decode("kind"),
            stateRoot: fields.path("state_root", message: "resource state path disagrees with destination"),
            schemaVersion: fields.schema("authority schema must be 2"),
        )
    }
}

extension ClaimRequirement: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try .array([StateCodec.path(destination), .string(claimID)])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        guard case let .array(items) = value, items.count == 2 else {
            throw FlyrailValueError("invalid state record")
        }
        return try Self(destination: StateCodec.decodePath(items[0]), claimID: stateText(items[1]))
    }
}

extension OwnedClaim: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("OwnedClaim", [
            ("claim", claim.stateRecord()), ("claim_id", .string(claimID)), ("bundle_id", .string(bundleID)),
            ("artifact_id", .string(artifactID)), ("version", .string(version)),
            ("bundle_digest", .string(bundleDigest)),
            ("render_digest", .string(renderDigest)), ("selection", StateCodec.optional(selection)),
            ("installed", StateCodec.optional(installed)), ("disposition", disposition.stateRecord()),
            ("baseline", StateCodec.optional(baseline)), ("requirements", StateCodec.sequence(requirements)),
            ("stable_requirements", StateCodec.sequence(stableRequirements)),
            ("schema_requirements", StateCodec.sequence(schemaRequirements)),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "OwnedClaim", names: [
            "claim", "claim_id", "bundle_id", "artifact_id", "version", "bundle_digest", "render_digest",
            "selection", "installed", "disposition", "baseline", "requirements", "stable_requirements",
            "schema_requirements",
        ])
        return try Self(
            claim: fields.decode("claim"), claimID: fields.text("claim_id"), bundleID: fields.text("bundle_id"),
            artifactID: fields.text("artifact_id"), version: fields.text("version"),
            bundleDigest: fields.text("bundle_digest"),
            renderDigest: fields.text("render_digest"), selection: fields.optional("selection"),
            installed: fields.optional("installed"), disposition: fields.decode("disposition"),
            baseline: fields.optional("baseline"), requirements: fields.sequence("requirements"),
            stableRequirements: fields.sequence("stable_requirements"),
            schemaRequirements: fields.sequence("schema_requirements"),
        )
    }
}

extension Receipt: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("Receipt", [
            ("resource", resource.stateRecord()), ("transaction_id", .string(transactionID)),
            ("claims", StateCodec.sequence(claims)), ("provenance", provenance.stateRecord()),
            ("created_directories", StateCodec.sequence(createdDirectories)),
            ("schema_version", .number(String(schemaVersion))),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "Receipt", names: [
            "resource", "transaction_id", "claims", "provenance", "created_directories", "schema_version",
        ])
        return try Self(
            resource: fields.decode("resource"), transactionID: fields.text("transaction_id"),
            claims: fields.sequence("claims"), provenance: fields.decode("provenance"),
            createdDirectories: fields.sequence("created_directories"),
            schemaVersion: fields.schema("receipt schema must be 2"),
        )
    }
}

extension GenerationMember: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try .array([resource.stateRecord(), .array(claimIDs.map(JSONValue.string))])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        guard case let .array(items) = value else {
            throw FlyrailValueError("invalid state record")
        }
        guard items.count >= 2 else {
            throw FlyrailValueError("not enough values to unpack (expected 2, got \(items.count))")
        }
        guard items.count == 2 else {
            throw FlyrailValueError("too many values to unpack (expected 2)")
        }
        guard case let .array(ids) = items[1] else { throw FlyrailValueError("invalid state record") }
        return try Self(
            resource: ResourceRef.fromState(items[0]),
            claimIDs: ids.map { try stateText($0, message: "invalid hexadecimal identity") },
        )
    }
}

extension Generation: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("Generation", [
            ("version", .string(version)), ("bundle_digest", .string(bundleDigest)),
            ("render_digest", .string(renderDigest)),
            ("membership", StateCodec.sequence(membership)),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "Generation", names: [
            "version", "bundle_digest", "render_digest", "membership",
        ])
        return try Self(
            version: fields.text("version"),
            bundleDigest: fields.text("bundle_digest", message: "invalid hexadecimal identity"),
            renderDigest: fields.text("render_digest", message: "invalid hexadecimal identity"),
            membership: fields.sequence("membership"),
        )
    }
}

extension Index: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("Index", [
            ("bundle_id", .string(bundleID)), ("context_digest", .string(contextDigest)),
            ("current", StateCodec.optional(current)), ("pending", StateCodec.optional(pending)),
            ("previous", StateCodec.optional(previous)), ("residual", StateCodec.sequence(residual)),
            ("schema_version", .number(String(schemaVersion))),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "Index", names: [
            "bundle_id", "context_digest", "current", "pending", "previous", "residual", "schema_version",
        ])
        return try Self(
            bundleID: fields.text("bundle_id"), contextDigest: fields.text("context_digest"),
            current: fields.optional("current"), pending: fields.optional("pending"),
            previous: fields.optional("previous"),
            residual: fields.sequence("residual"), schemaVersion: fields.schema("index schema must be 2"),
        )
    }
}

extension Journal: StateCodable {
    package func stateRecord() throws -> JSONValue {
        try StateCodec.record("Journal", [
            ("resource", resource.stateRecord()), ("previous", StateCodec.optional(previous)),
            ("receipt", receipt.stateRecord()), ("before", before.stateRecord()), ("staged", staged.stateRecord()),
            ("published", published.stateRecord()), ("backup", backup.stateRecord()),
            ("ancestors", StateCodec.sequence(ancestors)), ("schema_version", .number(String(schemaVersion))),
        ])
    }

    package static func fromState(_ value: JSONValue) throws -> Self {
        let fields = try StateFields(value, tag: "Journal", names: [
            "resource", "previous", "receipt", "before", "staged", "published", "backup", "ancestors", "schema_version",
        ])
        return try Self(
            resource: fields.decode("resource"), previous: fields.optional("previous"),
            receipt: fields.decode("receipt"),
            before: fields.decode("before"), staged: fields.decode("staged"), published: fields.decode("published"),
            backup: fields.decode("backup"), ancestors: fields.sequence("ancestors"),
            schemaVersion: fields.schema("journal schema must be 2"),
        )
    }
}

public extension OwnedClaim {
    static func == (left: Self, right: Self) -> Bool {
        left.claim == right.claim && left.claimID == right.claimID && left.bundleID == right.bundleID &&
            left.artifactID == right.artifactID && exactTextEqual(left.version, right.version) &&
            left.bundleDigest == right.bundleDigest && left.renderDigest == right.renderDigest &&
            left.selection == right.selection && left.installed == right.installed &&
            left.disposition == right.disposition && left.baseline == right.baseline &&
            left.requirements == right.requirements && left.stableRequirements == right.stableRequirements &&
            left.schemaRequirements == right.schemaRequirements
    }

    func hash(into hasher: inout Hasher) {
        hasher.combine(claim)
        hasher.combine(claimID)
        hasher.combine(bundleID)
        hasher.combine(artifactID)
        hashExactText(version, into: &hasher)
        hasher.combine(bundleDigest)
        hasher.combine(renderDigest)
        hasher.combine(selection)
        hasher.combine(installed)
        hasher.combine(disposition)
        hasher.combine(baseline)
        hasher.combine(requirements)
        hasher.combine(stableRequirements)
        hasher.combine(schemaRequirements)
    }
}

public extension Generation {
    static func == (left: Self, right: Self) -> Bool {
        exactTextEqual(left.version, right.version) && left.bundleDigest == right.bundleDigest &&
            left.renderDigest == right.renderDigest && left.membership == right.membership
    }

    func hash(into hasher: inout Hasher) {
        hashExactText(version, into: &hasher)
        hasher.combine(bundleDigest)
        hasher.combine(renderDigest)
        hasher.combine(membership)
    }
}
