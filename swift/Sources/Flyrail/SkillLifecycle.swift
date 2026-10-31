import Foundation

public func install<T: SkillTarget>(_ bundle: FlyrailBundle, targets: [T], store: any StateStore,
                                    lockTimeout: Duration = .zero) -> [TargetResult<T>]
{
    skillOperation(bundle: bundle, bundleID: bundle.identity.id, targets: targets, store: store, operation: .install,
                   options: SkillOptions(lockTimeout: lockTimeout))
}

public func update<T: SkillTarget>(_ bundle: FlyrailBundle, targets: [T], store: any StateStore,
                                   options: SkillOptions = SkillOptions()) -> [TargetResult<T>]
{
    skillOperation(bundle: bundle, bundleID: bundle.identity.id, targets: targets, store: store, operation: .update,
                   options: options)
}

public func uninstall<T: SkillTarget>(bundleID: String, targets: [T], store: any StateStore,
                                      options: SkillOptions = SkillOptions()) -> [TargetResult<T>]
{
    skillOperation(bundle: nil, bundleID: bundleID, targets: targets, store: store, operation: .uninstall,
                   options: options)
}

private enum SkillOperation { case install, update, uninstall }

// swiftlint:disable:next function_parameter_count
private func skillOperation<T: SkillTarget>(bundle: FlyrailBundle?, bundleID: String, targets: [T],
                                            store: any StateStore, operation: SkillOperation,
                                            options: SkillOptions) -> [TargetResult<T>]
{
    var results: [TargetResult<T>] = []
    var known: [ResourcePath: Int] = [:]
    for target in targets {
        var root = target.root
        var prior: Observation?
        do {
            if let bundle {
                try SkillInspection.validate(bundle)
            }
            let prepared = try SkillInspection.target(target, bundleID: bundleID, store: store)
            root = prepared.0
            if let alias = known[root] {
                let previous = results[alias]
                results.append(TargetResult(target: target, root: root, aliasOf: alias, status: previous.status,
                                            observation: previous.observation, error: previous.error,
                                            recoveryPaths: previous.recoveryPaths))
                continue
            }
            known[root] = results.count
            let rendered = try bundle.map { try SkillInspection.render($0, root: root) }
            if let bundle, let rendered {
                try SkillInspection.validateReferences(bundle, rendered: rendered, target: prepared.1)
            }
            let observed = try inspectInstallation(bundleID: bundleID, target: prepared.1)
            prior = try SkillInspection.observation(observed, bundle: bundle, rendered: rendered)
            if operation == .install, let bundle, observed.version != nil,
               observed.bundleDigest != bundle.contentDigest ||
               !exactTextEqual(observed.version ?? "", bundle.identity.version)
            {
                throw TargetError(code: .updateRequired, message: "owned revision requires explicit update", path: root)
            }
            let result: InstallationResult = if let bundle, let rendered {
                try sync(bundle: bundle, rendered: rendered, target: prepared.1,
                         replaceModified: options.replaceModified, lockTimeout: options.lockTimeout)
            } else {
                remove(bundleID: bundleID, target: prepared.1,
                       replaceModified: options.replaceModified, lockTimeout: options.lockTimeout)
            }
            let observation = try SkillInspection.observation(result.observation, bundle: bundle, rendered: rendered)
            results.append(TargetResult(target: target, root: root, status: result.status, observation: observation,
                                        error: result.error, recoveryPaths: result.resources.flatMap(\.recoveryPaths)))
        } catch {
            let error = NativeTransactionExecution.detail(error)
            results.append(TargetResult(target: target, root: root, status: .failed,
                                        observation: prior ?? .unknown(error), error: error))
        }
    }
    return results
}
