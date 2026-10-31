import Foundation

package enum LifecyclePlanning {
    package static func overlaps(_ left: ResourcePath, _ right: ResourcePath) throws -> Bool {
        let first = try NativeFilesystem.components(left).map { portablePathKey(String(decoding: $0, as: UTF8.self)) }
        let second = try NativeFilesystem.components(right).map { portablePathKey(String(decoding: $0, as: UTF8.self)) }
        return zip(first, second).allSatisfy(==)
    }

    package static func overlapsManagement(_ path: ResourcePath, destination: ResourcePath) throws -> Bool {
        let parent = try NativeFilesystem.components(destination).dropLast().map(portableKey)
        let names = try NativeFilesystem.components(path)
        let selected = names.map(portableKey)
        if selected.count <= parent.count {
            return parent.starts(with: selected)
        }
        return selected.starts(with: parent) &&
            NativeTransactionDirectory.isTransactionName(String(decoding: names[parent.count], as: UTF8.self))
    }

    private static func portableKey(_ name: [UInt8]) -> String {
        portablePathKey(String(decoding: name, as: UTF8.self))
    }

    package static func references(_ rendered: RenderedBundle, index: Index?, target: InstallationTarget,
                                   bundle: FlyrailBundle?,
                                   group: NativeTransactionPlanning.Group) throws -> [ResourceRef]
    {
        let stores = try NativeTransactionDirectory.storePaths(target.store)
        let sources = bundle?.sourceRoots ?? []
        for source in sources where try stores.contains(where: { try overlaps(source, $0) }) {
            throw FlyrailValueError("state store overlaps bundle source")
        }
        let resources = destinationOrder(Array(Set(Array(group.artifacts.keys) + (index?.resources ?? []))))
        guard Set(resources.map(\.destination)).count == resources.count else {
            throw FlyrailValueError("resource kinds disagree at the same destination")
        }
        for resource in resources {
            if try stores.contains(where: {
                try overlaps(resource.destination, $0) || overlapsManagement($0, destination: resource.destination)
            }) {
                throw FlyrailValueError("state store overlaps a resource or authority")
            }
            for source in sources {
                if try overlapsManagement(source, destination: resource.destination) {
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
        let index = try InstallationIndex.read(bundleID: bundleID, target: target)
        let group = try NativeTransactionPlanning.group(rendered)
        let refs = try references(rendered, index: index, target: target, bundle: bundle, group: group)
        var plans = try refs.map { resource in
            let state = try ResourceAdmission.observe(resource, store: target.store)
            let plan = try NativeResourcePlanner.plan(
                resource, state: state, store: target.store, bundleID: bundleID, bundle: bundle,
                rendered: rendered,
                artifacts: (group.artifacts[resource] ?? []).sorted { utf8Less($0.id, $1.id) },
                acquisition: acquisition, replaceModified: replaceModified,
                requirements: group.requirements, stableRequirements: group.stableRequirements,
            )
            if state.receipt == nil, InstallationIndex.expectsReceipt(index, resource: resource) {
                return try copy(plan, error: missingReceipt(resource))
            }
            return plan
        }
        if plans.allSatisfy({ $0.error == nil }) {
            plans = try NativeTransactionPlanning.order(plans, rendered: rendered, byID: group.byID)
        }
        let error: TargetError? = rendered.supported ? nil :
            TargetError(code: .unsupported, message: "rendered target has unsupported requirements")
        return try LifecyclePreview(
            bundleID: bundleID, bundle: bundle, rendered: rendered, target: target, acquisition: acquisition,
            replaceModified: replaceModified, index: index,
            generation: NativeTransactionPlanning.generation(bundle, rendered: rendered, group: group),
            resources: plans, error: error, artifactsByResource: group.artifacts,
        )
    }

    package static func missingReceipt(_ resource: ResourceRef) -> TargetError {
        nativeFailure(.invalidState, "recorded ownership receipt is missing; restore metadata before retrying",
                      resource.destination)
    }

    package static func copy(_ plan: ResourcePlan, ancestors: [Ancestor]? = nil,
                             error: TargetError? = nil) throws -> ResourcePlan
    {
        try ResourcePlan(resource: plan.resource, before: plan.before, after: plan.after, previous: plan.previous,
                         receipt: plan.receipt, ancestors: ancestors ?? plan.ancestors,
                         transactionDirectory: plan.transactionDirectory, recoveryPaths: plan.recoveryPaths,
                         requireExisting: plan.requireExisting, error: error ?? plan.error)
    }

    package static func fresh(_ preview: LifecyclePreview, created: [Ancestor]) throws -> LifecyclePreview {
        let store = preview.target.store
        try StateRecords.requireIndex(
            store, key: InstallationIndex.key(bundleID: preview.bundleID, target: preview.target),
            expected: preview.index,
        )
        let plans = try preview.resources.map { plan in
            let destination = plan.resource.destination
            try NativeObservation.requireRevision(destination, plan.before)
            let stored = try StateRecords.raw(store, key: StateKey.resource(destination, .receipt))
            guard try stored == plan.previous.map({ try StateCodec.encode($0) }) else {
                throw nativeFailure(.concurrentChange, "preview receipt changed", destination)
            }
            let recorded = try [ResourceRecord.preparation, .journal].map {
                try StateRecords.raw(store, key: StateKey.resource(destination, $0)) != nil
            }.contains(true)
            if recorded == plan.recoveryPaths.isEmpty {
                throw nativeFailure(.concurrentChange, "preview recovery changed", destination)
            }
            let staging = try appendPath(plan.transactionDirectory, "staging")
            let current = try ResourceTransaction.planAncestors(
                destination: destination, directory: plan.transactionDirectory,
            )
            let approved = try plan.ancestors + created.filter { item in
                try (within(destination, item.path) || within(staging, item.path)) && !plan.ancestors.contains(item)
            }
            guard current.allSatisfy(approved.contains), approved.allSatisfy(current.contains) else {
                throw nativeFailure(.concurrentChange, "preview ancestor changed", staging)
            }
            return try copy(plan, ancestors: current)
        }
        return try LifecyclePreview(
            bundleID: preview.bundleID, bundle: preview.bundle, rendered: preview.rendered, target: preview.target,
            acquisition: preview.acquisition, replaceModified: preview.replaceModified, index: preview.index,
            generation: preview.generation,
            resources: plans, error: preview.error, artifactsByResource: preview.artifactsByResource,
        )
    }

    private static func within(_ path: ResourcePath, _ ancestor: ResourcePath) throws -> Bool {
        try NativeFilesystem.components(path).starts(with: NativeFilesystem.components(ancestor))
    }
}
