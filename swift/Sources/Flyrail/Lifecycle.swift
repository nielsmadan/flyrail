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
                                    generation: nil, resources: [], error: error)
    }
}

public func applyPreview(_ preview: LifecyclePreview, lockTimeout: Duration = .zero) -> InstallationResult {
    if let error = preview.error {
        return InstallationInspection.failed(preview.bundleID, target: preview.target, error: error,
                                             notices: preview.rendered.notices)
    }
    guard preview.rendered.supported else {
        return InstallationInspection.failed(preview.bundleID,
                                             target: preview.target,
                                             error: TargetError(code: .unsupported,
                                                                message: "rendered target has unsupported requirements"),
                                             notices: preview.rendered.notices)
    }
    do {
        _ = try LifecyclePlanning.fresh(preview, created: [])
        let locks = try LifecycleExecution.locks(bundleID: preview.bundleID, target: preview.target,
                                                 expected: preview.index, resources: preview.resources.map(\.resource),
                                                 timeout: lockTimeout)
        defer { locks.release() }
        return try LifecycleExecution.run(preview, locks: locks)
    } catch {
        return InstallationInspection.failed(preview.bundleID, target: preview.target, error: error)
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
    do {
        return try LifecycleExecution.synchronize(
            bundleID: bundle.identity.id,
            bundle: bundle,
            rendered: rendered,
            target: target,
            acquisition: acquisition,
            replaceModified: replaceModified,
            lockTimeout: lockTimeout,
        )
    } catch {
        return InstallationInspection.failed(bundle.identity.id, target: target, error: error)
    }
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
        guard let index = try InstallationIndex.read(bundleID: bundleID, target: target) else {
            return try InstallationResult(status: .unchanged,
                                          observation: InstallationInspection.observe(
                                              bundleID: bundleID,
                                              target: target,
                                          ))
        }
        let references = try LifecyclePlanning.references(.empty, index: index, target: target, bundle: nil,
                                                          group: NativeTransactionPlanning.group(.empty))
        let locks = try LifecycleExecution.locks(bundleID: bundleID, target: target, expected: index,
                                                 resources: references, timeout: lockTimeout)
        defer { locks.release() }
        return try LifecycleExecution.recover(bundleID: bundleID, target: target, locks: locks)
    } catch {
        return InstallationInspection.failed(bundleID, target: target, error: error)
    }
}
