import Foundation

package enum NativeTransactionPlanning {
    package struct Group {
        package let artifacts: [ResourceRef: [RenderedArtifact]]
        package let byID: [String: ResourceRef]
        package let requirements: [String: [ClaimRequirement]]
        package let stableRequirements: [String: [ClaimRequirement]]
    }

    package static func group(_ rendered: RenderedBundle) throws -> Group {
        let byID = try Dictionary(uniqueKeysWithValues: rendered.artifacts.map {
            try ($0.id, NativeResourcePlanner.reference($0))
        })
        let artifacts = Dictionary(uniqueKeysWithValues: rendered.artifacts.map { ($0.id, $0) })
        var requirements = Dictionary(uniqueKeysWithValues: rendered.artifacts.map { ($0.id, Set<ClaimRequirement>()) })
        var stable = requirements
        for edge in rendered.dependencies {
            guard let resource = byID[edge.required], let artifact = artifacts[edge.required] else {
                throw FlyrailValueError("dependency references an unknown artifact")
            }
            let pair = try ClaimRequirement(
                destination: resource.destination,
                claimID: NativeResourcePlanner.claim(artifact).identity(destination: resource.destination)
            )
            requirements[edge.dependent, default: []].insert(pair)
            if edge.mode == .stableReference { stable[edge.dependent, default: []].insert(pair) }
        }
        var grouped: [ResourceRef: [RenderedArtifact]] = [:]
        for artifact in rendered.artifacts {
            guard let resource = byID[artifact.id] else { throw FlyrailValueError("resource resolution is missing") }
            let claim = try NativeResourcePlanner.claim(artifact)
            if let previous = try grouped[resource]?.first(where: { try NativeResourcePlanner.claim($0) == claim }) {
                guard artifact.content == previous.content, artifact.family == previous.family,
                      artifact.schemaRequirements == previous.schemaRequirements,
                      stable[artifact.id] == stable[previous.id]
                else { throw FlyrailValueError("physical aliases have incompatible desired content") }
                requirements[previous.id, default: []].formUnion(requirements[artifact.id] ?? [])
                let merged = try RenderedArtifact(
                    id: previous.id, family: previous.family, destination: previous.destination,
                    content: previous.content, subtree: previous.subtree,
                    requireExisting: previous.requireExisting || artifact.requireExisting,
                    schemaRequirements: previous.schemaRequirements
                )
                grouped[resource] = grouped[resource]?.map { $0.id == previous.id ? merged : $0 }
            } else { grouped[resource, default: []].append(artifact) }
        }
        func ordered(_ pairs: Set<ClaimRequirement>) -> [ClaimRequirement] {
            pairs.sorted {
                $0.destination == $1.destination ? utf8Less($0.claimID, $1.claimID) :
                    $0.destination.platformBytes.lexicographicallyPrecedes($1.destination.platformBytes)
            }
        }
        return Group(artifacts: grouped, byID: byID, requirements: requirements.mapValues(ordered),
                     stableRequirements: stable.mapValues(ordered))
    }

    private struct Edge: Hashable {
        let dependent: ResourceRef
        let required: ResourceRef
    }

    package static func order(
        _ plans: [ResourcePlan], rendered: RenderedBundle, byID: [String: ResourceRef]
    ) throws -> [ResourcePlan] {
        var edges = Set(rendered.dependencies.compactMap { edge -> Edge? in
            guard let dependent = byID[edge.dependent], let required = byID[edge.required], dependent != required else {
                return nil
            }
            return Edge(dependent: dependent, required: required)
        })
        let byDestination = Dictionary(uniqueKeysWithValues: plans.map { ($0.resource.destination, $0) })
        for plan in plans {
            for old in plan.previous?.claims ?? [] {
                let new = plan.receipt.claims.first { $0.claimID == old.claimID }
                for pair in old.requirements {
                    guard let required = byDestination[pair.destination], required.resource != plan.resource else {
                        continue
                    }
                    let oldAsset = required.previous?.claims.first { $0.claimID == pair.claimID }
                    let newAsset = required.receipt.claims.first { $0.claimID == pair.claimID }
                    if let oldAsset, let newAsset, oldAsset.installed != newAsset.installed,
                       !old.stableRequirements.contains(pair)
                    {
                        throw FlyrailValueError("referenced asset changes require a distinct revision destination")
                    }
                    if oldAsset != nil, newAsset == nil {
                        guard new?.requirements.contains(pair) != true else {
                            throw FlyrailValueError("cannot retire an asset with an active reference")
                        }
                        edges.insert(Edge(dependent: required.resource, required: plan.resource))
                    }
                }
            }
        }
        var pending = Dictionary(uniqueKeysWithValues: plans.map { ($0.resource, $0) })
        var ordered: [ResourcePlan] = []
        while !pending.isEmpty {
            let ready = pending.keys.filter { resource in
                !edges.contains { $0.dependent == resource && pending[$0.required] != nil }
            }.sorted { $0.stateRoot.platformBytes.lexicographicallyPrecedes($1.stateRoot.platformBytes) }
            guard !ready.isEmpty else { throw FlyrailValueError("cyclic resource dependencies") }
            for resource in ready {
                if let plan = pending.removeValue(forKey: resource) { ordered.append(plan) }
            }
        }
        return ordered
    }

    package static func generation(_ bundle: FlyrailBundle?, rendered: RenderedBundle,
                                   group: Group) throws -> Generation?
    {
        guard let bundle else { return nil }
        let membership = try group.artifacts.keys.sorted {
            $0.stateRoot.platformBytes.lexicographicallyPrecedes($1.stateRoot.platformBytes)
        }.map { resource in
            try GenerationMember(resource: resource, claimIDs: (group.artifacts[resource] ?? []).map {
                try NativeResourcePlanner.claim($0).identity(destination: resource.destination)
            }.sorted(by: utf8Less))
        }
        return try Generation(version: bundle.identity.version, bundleDigest: bundle.contentDigest,
                              renderDigest: rendered.renderDigest, membership: membership)
    }
}
