import Foundation

package enum LifecyclePlanning {
    package static func visible(_ observation: NativeIndexObservation, target: InstallationTarget,
                                bundleID: String) throws -> Index?
    {
        guard let prepared = observation.prepared else { return observation.current }
        return try Index(
            bundleID: bundleID, contextDigest: target.contextDigest,
            current: observation.current?.current, pending: observation.current?.pending,
            previous: observation.current?.previous,
            residual: Set((observation.current?.resources ?? []) + prepared.resources).sorted {
                $0.stateRoot.platformBytes.lexicographicallyPrecedes($1.stateRoot.platformBytes)
            },
        )
    }

    package static func overlaps(_ left: ResourcePath, _ right: ResourcePath) throws -> Bool {
        let first = try NativeFilesystem.components(left).map { portablePathKey(String(decoding: $0, as: UTF8.self)) }
        let second = try NativeFilesystem.components(right).map { portablePathKey(String(decoding: $0, as: UTF8.self)) }
        return zip(first, second).allSatisfy(==)
    }

    package static func references(_ rendered: RenderedBundle, index: Index?, target: InstallationTarget,
                                   bundle: FlyrailBundle?,
                                   group: NativeTransactionPlanning.Group) throws -> [ResourceRef]
    {
        let sources = bundle?.sourceRoots ?? []
        for source in sources where try overlaps(source, target.indexRoot) {
            throw FlyrailValueError("installation index overlaps bundle source")
        }
        let resources = Set(Array(group.artifacts.keys) + (index?.resources ?? [])).sorted {
            $0.stateRoot.platformBytes.lexicographicallyPrecedes($1.stateRoot.platformBytes)
        }
        guard Set(resources.map(\.destination)).count == resources.count else {
            throw FlyrailValueError("resource kinds disagree at the same destination")
        }
        for resource in resources {
            guard try !overlaps(resource.destination, target.indexRoot),
                  try !overlaps(resource.stateRoot, target.indexRoot)
            else {
                throw FlyrailValueError("installation index overlaps a resource or authority")
            }
            for source in sources {
                if try overlaps(source, resource.stateRoot) {
                    throw FlyrailValueError("resource authority overlaps bundle source")
                }
                for artifact in rendered.artifacts where group.byID[artifact.id] == resource {
                    let output = try artifact.subtree.map { try appendPath(resource.destination, $0) }
                        ?? resource.destination
                    if try overlaps(source, output) {
                        throw FlyrailValueError("resource output overlaps bundle source")
                    }
                }
            }
        }
        return resources
    }

    // swiftlint:disable:next function_parameter_count
    package static func build(bundleID: String, bundle: FlyrailBundle?, rendered: RenderedBundle,
                              target: InstallationTarget, acquisition: Acquisition,
                              replaceModified: Bool) throws -> LifecyclePreview
    {
        let observed = try NativeAdmission.observeIndex(target, bundleID: bundleID)
        let index = try visible(observed, target: target, bundleID: bundleID)
        let group = try NativeTransactionPlanning.group(rendered)
        let refs = try references(rendered, index: index, target: target, bundle: bundle, group: group)
        var plans = try refs.map { resource in
            let observation = try NativeAdmission.observe(resource)
            let plan = try NativeResourcePlanner.plan(
                resource, observation: observation, bundleID: bundleID, bundle: bundle, rendered: rendered,
                artifacts: (group.artifacts[resource] ?? []).sorted { utf8Less($0.id, $1.id) },
                acquisition: acquisition, replaceModified: replaceModified,
                requirements: group.requirements, stableRequirements: group.stableRequirements,
            )
            if try observation.receipt == nil &&
                NativeIndexRecovery.expectsReceipt(observed.current, prepared: observed.prepared, resource: resource)
            {
                return try copy(plan, error: missingReceipt(resource))
            }
            return plan
        }
        if plans.allSatisfy({ $0.error == nil }) {
            plans = try NativeTransactionPlanning.order(plans, rendered: rendered, byID: group.byID)
        }
        var error: TargetError? = rendered.supported ? nil :
            TargetError(code: .unsupported, message: "rendered target has unsupported requirements")
        if observed.prepared != nil {
            error = try nativeFailure(.recoveryNeeded, "installation index has prepared metadata",
                                      appendPath(target.indexRoot, "index.json.next"))
        }
        let revision = try NativeObservation.snapshot(target.indexRoot, privateStorage: true).revision
        guard revision == observed.snapshot.revision else {
            throw nativeFailure(.concurrentChange, "preview metadata changed", target.indexRoot)
        }
        return try LifecyclePreview(
            bundleID: bundleID, bundle: bundle, rendered: rendered, target: target, acquisition: acquisition,
            replaceModified: replaceModified, index: index, indexRevision: revision,
            indexAncestors: NativeObservation.ancestors(appendPath(target.indexRoot, "index.json")),
            generation: NativeTransactionPlanning.generation(bundle, rendered: rendered, group: group),
            resources: plans, error: error, artifactsByResource: group.artifacts,
        )
    }

    package static func missingReceipt(_ resource: ResourceRef) throws -> TargetError {
        try nativeFailure(.invalidState, "recorded ownership receipt is missing; restore metadata before retrying",
                          appendPath(resource.stateRoot, "receipt.json"))
    }

    package static func copy(_ plan: ResourcePlan, state: Revision? = nil, ancestors: [Ancestor]? = nil,
                             error: TargetError? = nil) throws -> ResourcePlan
    {
        try ResourcePlan(resource: plan.resource, before: plan.before, after: plan.after, previous: plan.previous,
                         receipt: plan.receipt, ancestors: ancestors ?? plan.ancestors,
                         stateRevision: state ?? plan.stateRevision, recoveryPaths: plan.recoveryPaths,
                         requireExisting: plan.requireExisting, error: error ?? plan.error)
    }

    package static func stateMatches(_ path: ResourcePath, expected: Revision, initial: Bool) throws -> Revision {
        let current = try NativeObservation.snapshot(path, privateStorage: true, resourcePayloads: true).revision
        if initial {
            guard current == expected else {
                throw nativeFailure(.concurrentChange, "preview metadata changed", path)
            }
        } else {
            let old = Dictionary(uniqueKeysWithValues: expected.nodes.map { (Data($0.path.utf8), $0) })
            let new = Dictionary(uniqueKeysWithValues: current.nodes.map { (Data($0.path.utf8), $0) })
            guard old.allSatisfy({ new[$0.key] == $0.value }),
                  new.allSatisfy({ old[$0.key] != nil || ["", "lock", "authority.json"].contains($0.value.path) })
            else {
                throw nativeFailure(.concurrentChange, "preview metadata changed", path)
            }
        }
        return current
    }

    package static func checkedAncestors(_ path: ResourcePath, expected: [Ancestor],
                                         created: [Ancestor]) throws -> [Ancestor]
    {
        var approved = expected
        let parts = try NativeFilesystem.components(path)
        for ancestor in created where !approved.contains(ancestor) {
            let prefix = try NativeFilesystem.components(ancestor.path)
            if prefix.count < parts.count, parts.starts(with: prefix) {
                approved.append(ancestor)
            }
        }
        approved.sort { $0.path.platformBytes.count < $1.path.platformBytes.count }
        guard try NativeObservation.ancestors(path) == approved else {
            throw nativeFailure(.concurrentChange, "preview ancestor changed", path)
        }
        return approved
    }

    package static func fresh(_ preview: LifecyclePreview, initial: Bool,
                              created: [Ancestor] = []) throws -> LifecyclePreview
    {
        guard let expected = preview.indexRevision else {
            throw nativeFailure(.invalidState, "preview metadata was not observed", preview.target.indexRoot)
        }
        let revision = try stateMatches(preview.target.indexRoot, expected: expected, initial: initial)
        let ancestors = try checkedAncestors(appendPath(preview.target.indexRoot, "index.json"),
                                             expected: preview.indexAncestors, created: created)
        let plans = try preview.resources.map { plan in
            try NativeObservation.requireRevision(plan.resource.destination, plan.before)
            return try copy(plan, state: stateMatches(plan.resource.stateRoot, expected: plan.stateRevision,
                                                      initial: initial),
                            ancestors: checkedAncestors(appendPath(plan.resource.stateRoot, "transaction.json"),
                                                        expected: plan.ancestors, created: created))
        }
        return try LifecyclePreview(
            bundleID: preview.bundleID, bundle: preview.bundle, rendered: preview.rendered, target: preview.target,
            acquisition: preview.acquisition, replaceModified: preview.replaceModified, index: preview.index,
            indexRevision: revision, indexAncestors: ancestors, generation: preview.generation,
            resources: plans, error: preview.error, artifactsByResource: preview.artifactsByResource,
        )
    }
}
