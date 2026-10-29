import Foundation

public func preview(bundle: FlyrailBundle, rendered: RenderedBundle, target: InstallationTarget,
                    acquisition: Acquisition = .conflict, replaceModified: Bool = false) throws -> LifecyclePreview
{
    try lifecyclePreview(bundleID: bundle.identity.id, bundle: bundle, rendered: rendered, target: target,
                         acquisition: acquisition, replaceModified: replaceModified)
}

public func previewRemoval(bundleID: String, target: InstallationTarget,
                           replaceModified: Bool = false) throws -> LifecyclePreview
{
    try validateIdentifier(bundleID, label: "bundle id")
    return try lifecyclePreview(bundleID: bundleID, bundle: nil, rendered: .empty, target: target,
                                acquisition: .conflict, replaceModified: replaceModified)
}

// swiftlint:disable:next function_parameter_count
private func lifecyclePreview(bundleID: String, bundle: FlyrailBundle?, rendered: RenderedBundle,
                              target: InstallationTarget, acquisition: Acquisition,
                              replaceModified: Bool) throws -> LifecyclePreview
{
    do {
        return try LifecyclePlanning.build(bundleID: bundleID, bundle: bundle, rendered: rendered, target: target,
                                           acquisition: acquisition, replaceModified: replaceModified)
    } catch let error as TargetError {
        return try LifecyclePreview(bundleID: bundleID, bundle: bundle, rendered: rendered, target: target,
                                    acquisition: acquisition, replaceModified: replaceModified, index: nil,
                                    indexRevision: nil, indexAncestors: [], generation: nil, resources: [],
                                    error: error)
    }
}

public func applyPreview(_ preview: LifecyclePreview, lockTimeout: Duration = .zero) -> InstallationResult {
    do {
        try LifecycleExecution.requireApplicable(preview)
        _ = try LifecyclePlanning.fresh(preview, initial: true)
        let locks = try NativeTransactionLocks(target: preview.target, bundleID: preview.bundleID,
                                               resources: preview.resources.map(\.resource), timeout: lockTimeout,
                                               allowRecovery: false)
        { observed in
            guard observed.current == preview.index, observed.prepared == nil else {
                throw nativeFailure(.concurrentChange, "index changed while acquiring locks", preview.target.indexRoot)
            }
        }
        return try LifecycleExecution.run(preview, locks: locks)
    } catch {
        return InstallationInspection.failed(preview.bundleID, target: preview.target, error: error,
                                             notices: preview.error == nil ? [] : preview.rendered.notices)
    }
}

public func sync(bundle: FlyrailBundle, rendered: RenderedBundle, target: InstallationTarget,
                 acquisition: Acquisition = .conflict, replaceModified: Bool = false,
                 lockTimeout: Duration = .zero) throws -> InstallationResult
{
    do {
        let group = try NativeTransactionPlanning.group(rendered)
        _ = try LifecyclePlanning.references(rendered, index: nil, target: target, bundle: bundle, group: group)
    } catch let error as TargetError {
        return InstallationInspection.failed(
            bundle.identity.id,
            target: target,
            error: error,
            notices: rendered.notices,
        )
    }
    return LifecycleExecution.synchronize(
        bundleID: bundle.identity.id,
        bundle: bundle,
        rendered: rendered,
        target: target,
        acquisition: acquisition,
        replaceModified: replaceModified,
        lockTimeout: lockTimeout,
    )
}

public func remove(bundleID: String, target: InstallationTarget, replaceModified: Bool = false,
                   lockTimeout: Duration = .zero) -> InstallationResult
{
    do {
        try validateIdentifier(bundleID, label: "bundle id")
        return try LifecycleExecution.synchronize(bundleID: bundleID, bundle: nil, rendered: .empty, target: target,
                                                  acquisition: .conflict, replaceModified: replaceModified,
                                                  lockTimeout: lockTimeout)
    } catch {
        return InstallationResult(status: .failed,
                                  observation: InstallationObservation(failedBundleID: bundleID, target: target,
                                                                       error: NativeTransactionExecution.detail(error)),
                                  error: NativeTransactionExecution.detail(error))
    }
}

public func recoverInstallation(bundleID: String, target: InstallationTarget,
                                lockTimeout: Duration = .zero) -> InstallationResult
{
    do {
        try validateIdentifier(bundleID, label: "bundle id")
    } catch {
        return InstallationResult(status: .failed,
                                  observation: InstallationObservation(failedBundleID: bundleID, target: target,
                                                                       error: NativeTransactionExecution.detail(error)),
                                  error: NativeTransactionExecution.detail(error))
    }
    do {
        let observed = try NativeAdmission.observeIndex(target, bundleID: bundleID)
        guard let index = try LifecyclePlanning.visible(observed, target: target, bundleID: bundleID) else {
            return try InstallationResult(status: .unchanged,
                                          observation: InstallationInspection.observe(
                                              bundleID: bundleID,
                                              target: target,
                                          ))
        }
        let locks = try LifecycleExecution.locks(bundleID: bundleID, target: target, observed: observed,
                                                 resources: index.resources, timeout: lockTimeout)
        return try LifecycleExecution.recover(bundleID: bundleID, target: target, locks: locks)
    } catch {
        return InstallationInspection.failed(bundleID, target: target, error: error)
    }
}
