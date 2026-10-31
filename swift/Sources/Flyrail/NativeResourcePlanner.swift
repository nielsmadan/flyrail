import Darwin
import Foundation

package enum NativeResourcePlanner {
    package static func reference(_ artifact: RenderedArtifact) throws -> ResourceRef {
        let kind: ResourceKind = if artifact.subtree != nil {
            .skillContainer
        } else if case .tree = artifact.content {
            .tree
        } else {
            .document
        }
        return try NativeAuthority.reference(artifact.destination, kind: kind)
    }

    package static func claim(_ artifact: RenderedArtifact) throws -> Claim {
        if let subtree = artifact.subtree {
            return try Claim(kind: .subtree, selector: .subtree(subtree))
        }
        switch artifact.content {
        case .file: return try Claim(kind: .file, selector: .whole)
        case .tree: return try Claim(kind: .tree, selector: .whole)
        case .section, .structured: return try contentClaim(artifact.content)
        }
    }

    package static func node(_ path: String, data: Data?, mode: UInt64) throws -> Node {
        try Node(
            path: path, data: data, mode: Count(mode),
            uid: Count(UInt64(geteuid())), gid: Count(UInt64(getegid())), device: 0, inode: 0,
        )
    }

    package static func rebase(_ node: Node, path: String, data: Data? = nil,
                               replaceData: Bool = false) throws -> Node
    {
        try Node(
            path: path, data: replaceData ? data : node.data, mode: node.mode,
            uid: node.uid, gid: node.gid, device: node.device, inode: node.inode, security: node.security,
        )
    }

    package static func desired(_ content: Content) throws -> Revision {
        switch content {
        case let .file(file):
            let mode: UInt64 = switch file.mode {
            case .private: 0o600
            case .readable: 0o644
            case .executable: 0o755
            }
            return try Revision(nodes: [node("", data: file.data, mode: mode)])
        case let .tree(tree):
            return try Revision(nodes: [node("", data: nil, mode: 0o755)] + tree.entries.map {
                try node($0.path, data: $0.data, mode: $0.isDirectory || $0.executable ? 0o755 : 0o644)
            })
        case .section, .structured:
            throw FlyrailValueError("whole resource requires file or tree content")
        }
    }

    package static func intact(_ actual: Revision, _ expected: Revision) -> Bool {
        actual.nodes.count == expected.nodes.count && zip(actual.nodes, expected.nodes).allSatisfy {
            exactTextEqual($0.path, $1.path) && $0.data == $1.data && $0.mode == $1.mode
        }
    }

    package static func selected(_ revision: Revision, claim: Claim) throws -> Revision {
        guard case let .subtree(selector) = claim.selector else { return revision }
        let prefix = selector + "/"
        return try Revision(nodes: revision.nodes.filter {
            exactTextEqual($0.path, selector) || $0.path.utf8.starts(with: prefix.utf8)
        }
        .map {
            try rebase($0, path: exactTextEqual($0.path, selector) ? "" : String($0.path.dropFirst(prefix.count)))
        })
    }

    package static func replaceSelection(_ revision: Revision, claim: Claim, content: Revision) throws -> Revision {
        guard case let .subtree(selector) = claim.selector else { return content }
        let prefix = selector + "/"
        var nodes = revision.nodes.filter {
            !exactTextEqual($0.path, selector) && !$0.path.utf8.starts(with: prefix.utf8)
        }
        if !content.nodes.isEmpty, nodes.isEmpty {
            try nodes.append(node("", data: nil, mode: 0o755))
        }
        try nodes += content.nodes.map { try rebase($0, path: $0.path.isEmpty ? selector : prefix + $0.path) }
        if !content.nodes.isEmpty {
            let parts = selector.split(separator: "/")
            for count in 1 ..< parts.count {
                let parent = parts.prefix(count).joined(separator: "/")
                if !nodes.contains(where: { exactTextEqual($0.path, parent) }) {
                    try nodes.append(node(parent, data: nil, mode: 0o755))
                }
            }
        }
        return try Revision(nodes: nodes.sorted { utf8Less($0.path, $1.path) })
    }

    package static func claimObservations(_ receipt: Receipt, revision: Revision) throws -> [ClaimObservation] {
        let ownership = receipt.claims.compactMap(\.selection)
        let selections = try observeSelections(source: revision.data, ownership: ownership)
        return try receipt.claims.map { owned in
            let status: EditStatus = if let installed = owned.installed {
                try intact(selected(revision, claim: owned.claim), installed) ? .current : .modified
            } else {
                selections.first(where: { $0.claim == owned.claim })?.status == .current
                    ? .current : .modified
            }
            let schemasMatch = try owned.schemaRequirements.allSatisfy {
                try matchesSchema(source: revision.data, requirement: $0)
            }
            return try ClaimObservation(
                claimID: owned.claimID, bundleID: owned.bundleID, artifactID: owned.artifactID,
                version: owned.version, bundleDigest: owned.bundleDigest, renderDigest: owned.renderDigest,
                status: schemasMatch ? status : .modified,
            )
        }
    }

    // swiftlint:disable:next function_parameter_count
    package static func plan(
        _ resource: ResourceRef, state: ResourceState, store: any StateStore, bundleID: String,
        bundle: FlyrailBundle?, rendered: RenderedBundle, artifacts: [RenderedArtifact],
        acquisition: Acquisition, replaceModified: Bool,
        requirements: [String: [ClaimRequirement]] = [:], stableRequirements: [String: [ClaimRequirement]] = [:],
        transactionID: String = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased(),
    ) throws -> ResourcePlan {
        let before = state.revision
        let previous = state.receipt
        guard previous == nil || previous?.resource == resource else {
            throw nativeFailure(.invalidState, "receipt resource disagrees", resource.destination)
        }
        let claims = previous?.claims ?? []
        let own = Dictionary(uniqueKeysWithValues: claims.filter { $0.bundleID == bundleID }.map { ($0.claim, $0) })
        let wanted = try artifacts.map { try (claim($0), $0) }
        guard Set(wanted.map(\.0)).count == wanted.count else {
            throw FlyrailValueError("rendered artifacts duplicate a physical claim")
        }
        let desired = Dictionary(uniqueKeysWithValues: wanted)
        var error: TargetError?
        if desired.keys.contains(where: { desired in
            claims.contains { desired.overlaps($0.claim) && ($0.bundleID != bundleID || desired != $0.claim) }
        }) {
            error = nativeFailure(.conflict, "claim overlaps recorded ownership", resource.destination)
        }
        let required = artifacts.contains(where: \.requireExisting)
        if required, before.nodes.isEmpty {
            error = nativeFailure(.conflict, "resource must already exist", resource.destination)
        }
        if !before.nodes.isEmpty, (before.data != nil) != (resource.kind == .document) {
            error = nativeFailure(
                .unsafePath,
                "resource kind disagrees with existing destination",
                resource.destination,
            )
        }
        let foreign = claims.filter { $0.bundleID != bundleID }
        let shared = resource.kind == .document &&
            (Array(own.keys) + Array(desired.keys)).contains { $0.kind == .section || $0.kind == .structured }
        var after = before
        var result = foreign
        var provenance = previous?.provenance ?? DocumentProvenance()
        if error == nil {
            if shared {
                let edit = try sharedEdit(
                    before, destination: resource.destination, own: own, claims: claims,
                    desired: desired, acquisition: acquisition,
                    replaceModified: replaceModified, provenance: provenance,
                    schemas: foreign.flatMap(\.schemaRequirements) + artifacts.flatMap(\.schemaRequirements),
                )
                if let issue = edit.observations.first(where: { $0.status == .modified || $0.status == .conflict }) {
                    error = nativeFailure(
                        issue.status == .modified ? .modified : .conflict,
                        issue.message.isEmpty ? "owned selection cannot be changed" : issue.message,
                        resource.destination,
                    )
                } else {
                    provenance = edit.provenance
                    if let data = edit.after {
                        after = try Revision(nodes: [before.nodes.first.map {
                            try rebase($0, path: "", data: data, replaceData: true)
                        } ?? node("", data: data, mode: 0o600)])
                    } else {
                        after = try Revision()
                    }
                    for (claim, artifact) in wanted {
                        guard let selection = edit.ownership.first(where: { $0.claim == claim }) else {
                            throw FlyrailValueError("planned selection ownership is missing")
                        }
                        try result.append(owned(
                            artifact, resource: resource, bundle: bundle, rendered: rendered, selection: selection,
                            disposition: selection.disposition, requirements: requirements,
                            stableRequirements: stableRequirements,
                        ))
                    }
                }
            } else {
                let planned = try wholePlan(
                    before, resource: resource, own: own, desired: desired, bundle: bundle, rendered: rendered,
                    acquisition: acquisition, replaceModified: replaceModified,
                    requirements: requirements, stableRequirements: stableRequirements,
                )
                after = planned.after
                result += planned.claims
                error = planned.error
            }
        }
        var created = previous?.createdDirectories ?? []
        if resource.kind == .skillContainer, error == nil {
            (after, created) = try containerProvenance(before, after: after, previous: previous, claims: result)
        }
        let receipt = try error == nil ? Receipt(
            resource: resource, transactionID: transactionID,
            claims: result.sorted { utf8Less($0.claimID, $1.claimID) },
            provenance: provenance, createdDirectories: created,
        ) : (previous ?? Receipt(resource: resource, transactionID: transactionID))
        let planned = try transactionReceipt(receipt, previous: previous, unchanged: before == after)
        let directory = try NativeTransactionDirectory.choose(
            store: store, destination: resource.destination, transactionID: planned.transactionID,
        )
        return try ResourcePlan(
            resource: resource, before: before, after: error == nil ? after : before,
            previous: previous, receipt: planned,
            ancestors: ResourceTransaction.planAncestors(destination: resource.destination, directory: directory),
            transactionDirectory: directory, recoveryPaths: state.recoveryPaths,
            requireExisting: required, error: error,
        )
    }

    package static func replacingTransaction(_ receipt: Receipt, id: String) throws -> Receipt {
        try Receipt(resource: receipt.resource, transactionID: id, claims: receipt.claims,
                    provenance: receipt.provenance, createdDirectories: receipt.createdDirectories)
    }

    private static func transactionReceipt(_ receipt: Receipt, previous: Receipt?, unchanged: Bool) throws -> Receipt {
        guard let previous else { return receipt }
        if unchanged, try replacingTransaction(receipt, id: previous.transactionID) == previous {
            return previous
        }
        guard receipt.transactionID == previous.transactionID else { return receipt }
        return try replacingTransaction(
            receipt, id: UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased(),
        )
    }

    // swiftlint:disable:next function_parameter_count
    private static func sharedEdit(
        _ before: Revision, destination: ResourcePath, own: [Claim: OwnedClaim], claims: [OwnedClaim],
        desired: [Claim: RenderedArtifact], acquisition: Acquisition, replaceModified: Bool,
        provenance: DocumentProvenance, schemas: [DocumentSchema],
    ) throws -> DocumentEdit {
        var requests = try own.keys.filter { desired[$0] == nil }.sorted {
            try $0.identity(destination: destination) < $1.identity(destination: destination)
        }.map { try DocumentRequest.retire($0) }
        for artifact in desired.values.sorted(by: { utf8Less($0.id, $1.id) }) {
            switch artifact.content {
            case .section, .structured: try requests.append(.set(artifact.content, acquisition: acquisition))
            case .file, .tree: throw FlyrailValueError("whole and partial claims cannot share a document")
            }
        }
        var ownership = claims.compactMap(\.selection)
        let edit = try editSchemaDocument(source: before.data, requests: requests, ownership: ownership,
                                          provenance: provenance, activeSchemas: schemas, validate: !desired.isEmpty)
        guard replaceModified, !edit.applicable else { return edit }
        ownership = try ownership.map { item in
            guard let current = edit.observations.first(where: {
                $0.claim == item.claim && $0.status == .modified
            })?.current else { return item }
            return try OwnedSelection(
                claim: item.claim, format: item.format, installed: current, disposition: item.disposition,
                baseline: item.baseline, separator: item.separator,
            )
        }
        return try editSchemaDocument(source: before.data, requests: requests, ownership: ownership,
                                      provenance: provenance, activeSchemas: schemas, validate: !desired.isEmpty)
    }

    private struct WholePlan {
        let after: Revision
        let claims: [OwnedClaim]
        let error: TargetError?
    }

    // swiftlint:disable:next function_parameter_count
    private static func wholePlan(
        _ before: Revision, resource: ResourceRef, own: [Claim: OwnedClaim],
        desired: [Claim: RenderedArtifact], bundle: FlyrailBundle?, rendered: RenderedBundle,
        acquisition: Acquisition, replaceModified: Bool,
        requirements: [String: [ClaimRequirement]], stableRequirements: [String: [ClaimRequirement]],
    ) throws -> WholePlan {
        let ordered = try Set(own.keys).union(desired.keys).map {
            try ($0.identity(destination: resource.destination), $0)
        }.sorted { utf8Less($0.0, $1.0) }
        var after = before
        var claims: [OwnedClaim] = []
        for (_, claim) in ordered {
            let old = own[claim]
            let actual = try selected(before, claim: claim)
            if let installed = old?.installed, !intact(actual, installed), !replaceModified {
                return WholePlan(after: before, claims: [], error: nativeFailure(
                    .modified, "owned resource has local modifications", resource.destination,
                ))
            }
            let replacement: Revision
            if let artifact = desired[claim] {
                replacement = try self.desired(artifact.content)
                var disposition = old?.disposition ?? .created
                var baseline = old?.baseline
                if old == nil, !actual.nodes.isEmpty {
                    switch acquisition {
                    case .conflict:
                        return WholePlan(after: before, claims: [], error: nativeFailure(
                            .conflict, "unowned content already exists", resource.destination,
                        ))
                    case .adopt:
                        guard intact(actual, replacement) else {
                            return WholePlan(after: before, claims: [], error: nativeFailure(
                                .conflict, "adoption requires matching content", resource.destination,
                            ))
                        }
                        disposition = .adopted
                    case .takeover:
                        disposition = .takenOver
                        baseline = actual
                    }
                }
                try claims.append(owned(
                    artifact, resource: resource, bundle: bundle, rendered: rendered, installed: replacement,
                    disposition: disposition, baseline: baseline,
                    requirements: requirements, stableRequirements: stableRequirements,
                ))
            } else {
                replacement = try old?.baseline ?? Revision()
            }
            if !intact(actual, replacement) {
                after = try replaceSelection(after, claim: claim, content: replacement)
            }
        }
        return WholePlan(after: after, claims: claims, error: nil)
    }

    // swiftlint:disable:next function_parameter_count
    private static func owned(
        _ artifact: RenderedArtifact, resource: ResourceRef, bundle: FlyrailBundle?, rendered: RenderedBundle,
        selection: OwnedSelection? = nil, installed: Revision? = nil,
        disposition: Disposition, baseline: Revision? = nil,
        requirements: [String: [ClaimRequirement]], stableRequirements: [String: [ClaimRequirement]],
    ) throws -> OwnedClaim {
        guard let bundle else { throw FlyrailValueError("desired content requires a bundle") }
        let claim = try claim(artifact)
        return try OwnedClaim(
            claim: claim, claimID: claim.identity(destination: resource.destination),
            bundleID: bundle.identity.id, artifactID: artifact.id, version: bundle.identity.version,
            bundleDigest: bundle.contentDigest, renderDigest: rendered.renderDigest,
            selection: selection, installed: installed, disposition: disposition, baseline: baseline,
            requirements: requirements[artifact.id] ?? [], stableRequirements: stableRequirements[artifact.id] ?? [],
            schemaRequirements: artifact.schemaRequirements,
        )
    }

    package static func containerProvenance(
        _ before: Revision, after: Revision, previous: Receipt?, claims: [OwnedClaim],
    ) throws -> (Revision, [Node]) {
        let existing = Dictionary(uniqueKeysWithValues: before.nodes.map { (Data($0.path.utf8), $0) })
        var created = Dictionary(uniqueKeysWithValues: (previous?.createdDirectories ?? [])
            .filter { existing[Data($0.path.utf8)] == $0 }.map { (Data($0.path.utf8), $0) })
        let selectors = claims.compactMap { owned -> String? in
            guard case let .subtree(path) = owned.claim.selector else { return nil }
            return path
        }
        var nodes = Dictionary(uniqueKeysWithValues: after.nodes.map { (Data($0.path.utf8), $0) })
        func needed(_ path: String) -> Bool {
            selectors.contains { path.isEmpty || $0.utf8.starts(with: (path + "/").utf8) }
        }
        for node in after.nodes where node.data == nil && existing[Data(node.path.utf8)] == nil && needed(node.path) {
            created[Data(node.path.utf8)] = node
        }
        var retained: [Node] = []
        for marker in created.values.sorted(by: { utf8Less($1.path, $0.path) }) {
            if needed(marker.path) {
                retained.append(marker)
            } else if !nodes.values.contains(where: {
                !exactTextEqual($0.path, marker.path) &&
                    (marker.path.isEmpty || $0.path.utf8.starts(with: (marker.path + "/").utf8))
            }) {
                nodes.removeValue(forKey: Data(marker.path.utf8))
            }
        }
        return try (Revision(nodes: nodes.values.sorted { utf8Less($0.path, $1.path) }),
                    retained.sorted { utf8Less($0.path, $1.path) })
    }
}
