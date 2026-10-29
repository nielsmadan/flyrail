import Foundation

package enum NativeTransactionDriver {
    // swiftlint:disable:next function_parameter_count
    package static func write(
        bundle: FlyrailBundle?, bundleID: String, rendered: RenderedBundle,
        target: InstallationTarget, acquisition: Acquisition, replaceModified: Bool,
        timeout: Duration, seam: NativeTransactionSeam = .none
    ) throws -> InstallationResult {
        let group = try NativeTransactionPlanning.group(rendered)
        let locks = try NativeTransactionLocks(target: target, bundleID: bundleID,
                                               resources: Array(group.artifacts.keys), timeout: timeout)
        _ = try NativeIndexRecovery.recover(locks, seam: seam)
        let index = try NativeAdmission.observeIndex(target, bundleID: bundleID)
        let plans = try locks.resources.map { resource in
            try NativeResourcePlanner.plan(
                resource, observation: NativeAdmission.observe(resource, lock: locks.guardFor(resource)),
                bundleID: bundleID, bundle: bundle, rendered: rendered,
                artifacts: group.artifacts[resource] ?? [], acquisition: acquisition, replaceModified: replaceModified,
                requirements: group.requirements, stableRequirements: group.stableRequirements
            )
        }
        if let error = plans.compactMap(\.error).first {
            return try InstallationResult(status: .failed, observation: inspect(bundleID, target: target), error: error)
        }
        let generation = try NativeTransactionPlanning.generation(bundle, rendered: rendered, group: group)
        let unchanged = plans.allSatisfy { $0.before == $0.after && $0.previous == $0.receipt }
        if unchanged, (index.current == nil && bundle == nil) ||
            (index.current?.current == generation && index.current?.pending == nil &&
                index.current?.previous == nil && index.current?.residual.isEmpty == true)
        {
            return try InstallationResult(status: .unchanged, observation: inspect(bundleID, target: target))
        }
        let pending = try Index(
            bundleID: bundleID, contextDigest: target.contextDigest, current: index.current?.current,
            pending: generation, previous: index.current?.current, residual: locks.resources
        )
        let ordered = try NativeTransactionPlanning.order(plans, rendered: rendered, byID: group.byID)
        let outcome = try NativeTransactionExecution.execute(
            locks, plans: ordered, pending: pending, completed: NativeIndexRecovery.completion(pending),
            indexSnapshot: index.snapshot, seam: seam
        )
        return try InstallationResult(status: outcome.status, observation: observe(bundleID, target: target),
                                      resources: outcome.resources, error: outcome.error)
    }

    package static func recover(
        _ bundleID: String, target: InstallationTarget, timeout: Duration,
        seam: NativeTransactionSeam = .none
    ) throws -> InstallationResult {
        let initial = try NativeAdmission.observeIndex(target, bundleID: bundleID)
        if initial.current == nil, initial.prepared == nil {
            return try InstallationResult(status: .unchanged, observation: inspect(bundleID, target: target))
        }
        let locks = try NativeTransactionLocks(target: target, bundleID: bundleID, resources: [], timeout: timeout)
        let results = try NativeIndexRecovery.recover(locks, seam: seam)
        let changed = initial.prepared != nil || results.contains { $0.status == .applied }
        return try InstallationResult(status: changed ? .applied : .unchanged,
                                      observation: inspect(bundleID, target: target), resources: results)
    }

    package static func inspect(_ bundleID: String, target: InstallationTarget) throws -> InstallationObservation {
        let index = try NativeAdmission.observeIndex(target, bundleID: bundleID)
        let references = Set([index.current, index.prepared].compactMap(\.self).flatMap(\.resources)).sorted {
            $0.stateRoot.platformBytes.lexicographicallyPrecedes($1.stateRoot.platformBytes)
        }
        let resources = try references.map { resource -> ResourceObservation in
            do {
                try NativeIndexRecovery.requireReceipts(index.current, prepared: index.prepared, resources: [resource])
                let observation = try NativeAdmission.observe(resource)
                return try ResourceObservation(
                    destination: resource.destination, stateRoot: resource.stateRoot,
                    claims: observation.receipt.map {
                        try NativeResourcePlanner.claimObservations($0, revision: observation.revision)
                    } ?? [],
                    revisionDigest: observation.revision.contentDigest, recoveryPaths: observation.recovery
                )
            } catch {
                return try ResourceObservation(destination: resource.destination, stateRoot: resource.stateRoot,
                                               error: NativeTransactionExecution.detail(error))
            }
        }
        let current = index.current?.current
        let pending = index.prepared != nil || index.current?.pending != nil || index.current?.previous != nil ||
            index.current?.residual.isEmpty == false
        let actual = resources.flatMap { resource in
            resource.claims.filter { $0.bundleID == bundleID }.map {
                ClaimRequirementUnchecked(destination: resource.destination, claimID: $0.claimID)
            }
        }
        let expected = current?.membership.flatMap { member in
            member.claimIDs.map { ClaimRequirementUnchecked(destination: member.resource.destination, claimID: $0) }
        } ?? []
        let intact = resources.allSatisfy { resource in
            resource.error == nil && resource.recoveryPaths.isEmpty && resource.claims
                .filter { $0.bundleID == bundleID }.allSatisfy {
                    $0.status == .current && $0.version.mapUTF8 == current?.version.mapUTF8 &&
                        $0.bundleDigest == current?.bundleDigest && $0.renderDigest == current?.renderDigest
                }
        }
        return try InstallationObservation(
            bundleID: bundleID, target: target, resources: resources,
            isCurrent: current != nil && !pending && intact && Set(actual) == Set(expected), pending: pending,
            version: current?.version, bundleDigest: current?.bundleDigest, renderDigest: current?.renderDigest
        )
    }

    package static func observe(_ bundleID: String, target: InstallationTarget) throws -> InstallationObservation {
        do {
            return try inspect(bundleID, target: target)
        } catch {
            return try InstallationObservation(
                bundleID: bundleID, target: target, resources: [], isCurrent: false, pending: false,
                error: NativeTransactionExecution.detail(error)
            )
        }
    }

    private struct ClaimRequirementUnchecked: Hashable {
        let destination: ResourcePath
        let claimID: String
    }
}

private extension String {
    var mapUTF8: [UInt8] { Array(utf8) }
}
