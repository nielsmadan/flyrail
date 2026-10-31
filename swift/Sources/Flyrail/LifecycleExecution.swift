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

    package static func locks(bundleID: String, target: InstallationTarget, expected: Index?,
                              resources: [ResourceRef], timeout: Duration) throws -> NativeTransactionLocks
    {
        try NativeTransactionLocks(target: target, bundleID: bundleID, resources: resources, timeout: timeout) {
            guard try InstallationIndex.read(bundleID: bundleID, target: target) == expected else {
                throw TargetError(code: .concurrentChange, message: "index changed while acquiring locks",
                                  path: target.store.location)
            }
        }
    }

    package static func run(_ preview: LifecyclePreview, locks: NativeTransactionLocks,
                            seam: NativeTransactionSeam = .none) throws -> InstallationResult
    {
        try requireApplicable(preview)
        let fresh = try LifecyclePlanning.fresh(preview, created: locks.created)
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
        let completed = try Index(bundleID: fresh.bundleID, contextDigest: fresh.target.contextDigest,
                                  current: fresh.generation)
        let outcome = try NativeTransactionExecution.execute(locks, plans: fresh.resources, previous: fresh.index,
                                                             pending: pending, completed: completed, seam: seam)
        return try InstallationResult(status: outcome.status,
                                      observation: InstallationInspection.observe(
                                          bundleID: fresh.bundleID,
                                          target: fresh.target,
                                      ),
                                      resources: outcome.resources, error: outcome.error,
                                      notices: fresh.rendered.notices)
    }

    package static func requireReceipts(_ index: Index?, resources: [ResourceRef], store: any StateStore) throws {
        for resource in resources {
            if InstallationIndex.expectsReceipt(index, resource: resource),
               try ResourceAdmission.storedReceipt(resource, store: store) == nil
            {
                throw LifecyclePlanning.missingReceipt(resource)
            }
        }
    }

    package static func recover(bundleID: String, target: InstallationTarget,
                                locks: NativeTransactionLocks,
                                seam: NativeTransactionSeam = .none) throws -> InstallationResult
    {
        let store = target.store
        let index = try InstallationIndex.read(bundleID: bundleID, target: target)
        try requireReceipts(index, resources: locks.resources, store: store)
        var results: [ResourceResult] = []
        for resource in locks.resources {
            let pending: Bool
            do {
                pending = try !ResourceTransaction.pendingRecovery(resource, store: store).isEmpty
                try ResourceTransaction.recover(resource, store: store, seam: seam)
            } catch let crash as NativeSimulatedCrash {
                throw crash
            } catch {
                let detail = NativeTransactionExecution.detail(error)
                try results.append(ResourceResult(
                    resource: resource, status: .incomplete, error: detail,
                    recoveryPaths: NativeTransactionExecution.recoveryPaths(store, resource, directory: nil),
                ))
                return try InstallationResult(status: .incomplete,
                                              observation: InstallationInspection.observe(
                                                  bundleID: bundleID,
                                                  target: target,
                                              ),
                                              resources: results, error: detail)
            }
            try results.append(ResourceResult(resource: resource, status: pending ? .applied : .unchanged))
        }
        let changed = results.contains { $0.status == .applied }
        return try InstallationResult(status: changed ? .applied : .unchanged,
                                      observation: InstallationInspection.observe(
                                          bundleID: bundleID,
                                          target: target,
                                      ),
                                      resources: results)
    }

    // swiftlint:disable:next function_parameter_count
    package static func synchronize(bundleID: String, bundle: FlyrailBundle?, rendered: RenderedBundle,
                                    target: InstallationTarget, acquisition: Acquisition,
                                    replaceModified: Bool, lockTimeout: Duration,
                                    seam: NativeTransactionSeam = .none) throws -> InstallationResult
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
            let index = try InstallationIndex.read(bundleID: bundleID, target: target)
            let group = try NativeTransactionPlanning.group(rendered)
            let references = try LifecyclePlanning.references(
                rendered,
                index: index,
                target: target,
                bundle: bundle,
                group: group,
            )
            let locks = try locks(bundleID: bundleID, target: target, expected: index,
                                  resources: references, timeout: lockTimeout)
            defer { locks.release() }
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
        } catch let crash as NativeSimulatedCrash {
            throw crash
        } catch {
            return InstallationInspection.failed(bundleID, target: target, error: error)
        }
    }
}
