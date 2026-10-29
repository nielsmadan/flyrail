import Foundation

package enum LifecycleExecution {
    package static func requireApplicable(_ preview: LifecyclePreview) throws {
        if let error = preview.error ?? preview.resources.compactMap(\.error).first {
            throw error
        }
        guard preview.resources.allSatisfy(\.recoveryPaths.isEmpty) else {
            throw TargetError(code: .recoveryNeeded, message: "preview requires recovery and re-observation")
        }
    }

    package static func locks(bundleID: String, target: InstallationTarget, observed: NativeIndexObservation,
                              resources: [ResourceRef], timeout: Duration) throws -> NativeTransactionLocks
    {
        try NativeTransactionLocks(target: target, bundleID: bundleID, resources: resources,
                                   timeout: timeout)
        { current in
            guard current.current == observed.current, current.prepared == observed.prepared else {
                throw nativeFailure(.concurrentChange, "index changed while acquiring locks", target.indexRoot)
            }
        }
    }

    package static func run(_ preview: LifecyclePreview, locks: NativeTransactionLocks,
                            seam: NativeTransactionSeam = .none) throws -> InstallationResult
    {
        try requireApplicable(preview)
        let fresh = try LifecyclePlanning.fresh(preview, initial: false, created: locks.created)
        let unchanged = fresh.resources.allSatisfy { $0.before == $0.after && $0.previous == $0.receipt }
        if unchanged, (fresh.index?.current == fresh.generation && fresh.index?.pending == nil &&
            fresh.index?.residual.isEmpty == true) || (fresh.index == nil && fresh.bundle == nil)
        {
            return try InstallationResult(status: .unchanged,
                                          observation: InstallationInspection.observe(
                                              bundleID: fresh.bundleID,
                                              target: fresh.target,
                                          ),
                                          notices: fresh.rendered.notices)
        }
        let pending = try Index(bundleID: fresh.bundleID, contextDigest: fresh.target.contextDigest,
                                current: fresh.index?.current, pending: fresh.generation,
                                previous: fresh.index?.current,
                                residual: fresh.resources.map(\.resource))
        let completed = try NativeIndexRecovery.completion(pending)
        let snapshot = try NativeObservation.snapshot(fresh.target.indexRoot, privateStorage: true)
        guard snapshot.revision == fresh.indexRevision else {
            throw nativeFailure(.concurrentChange, "preview metadata changed", fresh.target.indexRoot)
        }
        let outcome = try NativeTransactionExecution.execute(locks, plans: fresh.resources, pending: pending,
                                                             completed: completed, indexSnapshot: snapshot, seam: seam)
        return try InstallationResult(status: outcome.status,
                                      observation: InstallationInspection.observe(
                                          bundleID: fresh.bundleID,
                                          target: fresh.target,
                                      ),
                                      resources: outcome.resources, error: outcome.error,
                                      notices: fresh.rendered.notices)
    }

    package static func recover(bundleID: String, target: InstallationTarget,
                                locks: NativeTransactionLocks,
                                seam: NativeTransactionSeam = .none) throws -> InstallationResult
    {
        do {
            let resources = try NativeIndexRecovery.recover(locks, seam: seam, retainingResults: true)
            let changed = locks.observation.prepared != nil || resources.contains { $0.status == .applied }
            return try InstallationResult(status: changed ? .applied : .unchanged,
                                          observation: InstallationInspection.observe(
                                              bundleID: bundleID,
                                              target: target,
                                          ),
                                          resources: resources)
        } catch let failure as NativeIndexRecoveryFailure {
            return try InstallationResult(status: .incomplete,
                                          observation: InstallationInspection.observe(
                                              bundleID: bundleID,
                                              target: target,
                                          ),
                                          resources: failure.resources, error: failure.error)
        }
    }

    // swiftlint:disable:next function_parameter_count
    package static func synchronize(bundleID: String, bundle: FlyrailBundle?, rendered: RenderedBundle,
                                    target: InstallationTarget, acquisition: Acquisition,
                                    replaceModified: Bool, lockTimeout: Duration,
                                    seam: NativeTransactionSeam = .none) -> InstallationResult
    {
        do {
            guard rendered.supported else {
                return InstallationInspection.failed(bundleID,
                                                     target: target,
                                                     error: TargetError(
                                                         code: .unsupported,
                                                         message: "rendered target has unsupported requirements",
                                                     ),
                                                     notices: rendered.notices)
            }
            let observed = try NativeAdmission.observeIndex(target, bundleID: bundleID)
            let index = try LifecyclePlanning.visible(observed, target: target, bundleID: bundleID)
            let group = try NativeTransactionPlanning.group(rendered)
            let references = try LifecyclePlanning.references(
                rendered,
                index: index,
                target: target,
                bundle: bundle,
                group: group,
            )
            let locks = try locks(bundleID: bundleID, target: target, observed: observed,
                                  resources: references, timeout: lockTimeout)
            let recovered = try recover(bundleID: bundleID, target: target, locks: locks, seam: seam)
            if recovered.error != nil {
                return recovered
            }
            let preview = try LifecyclePlanning.build(
                bundleID: bundleID,
                bundle: bundle,
                rendered: rendered,
                target: target,
                acquisition: acquisition,
                replaceModified: replaceModified,
            )
            return try run(preview, locks: locks, seam: seam)
        } catch {
            return InstallationInspection.failed(bundleID, target: target, error: error)
        }
    }
}
