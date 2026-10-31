import Foundation

package enum InstallationInspection {
    private struct Membership: Hashable {
        let destination: ResourcePath
        let claimID: String
    }

    package static func observe(bundleID: String, target: InstallationTarget) throws -> InstallationObservation {
        var resources: [ResourceObservation] = []
        do {
            let index = try InstallationIndex.read(bundleID: bundleID, target: target)
            for resource in index?.resources ?? [] {
                do {
                    try LifecycleExecution.requireReceipts(index, resources: [resource], store: target.store)
                    let current = try ResourceAdmission.observe(resource, store: target.store)
                    try resources.append(ResourceObservation(
                        destination: resource.destination,
                        claims: current.receipt.map {
                            try NativeResourcePlanner.claimObservations($0, revision: current.revision)
                        } ?? [],
                        revisionDigest: current.revision.contentDigest, recoveryPaths: current.recoveryPaths,
                    ))
                } catch {
                    try resources.append(ResourceObservation(
                        destination: resource.destination,
                        error: NativeTransactionExecution.detail(error),
                    ))
                }
            }
            let generation = index?.current
            let pending = InstallationIndex.isPending(index)
            let actual = Set(resources.flatMap { resource in
                resource.claims.filter { $0.bundleID == bundleID }.map {
                    Membership(destination: resource.destination, claimID: $0.claimID)
                }
            })
            let expected = Set((generation?.membership ?? []).flatMap { member in
                member.claimIDs.map { Membership(destination: member.resource.destination, claimID: $0) }
            })
            let intact = resources.allSatisfy { resource in
                resource.error == nil && resource.recoveryPaths.isEmpty &&
                    resource.claims.filter { $0.bundleID == bundleID }.allSatisfy {
                        $0.status == .current && exactTextEqual($0.version, generation?.version ?? "") &&
                            $0.bundleDigest == generation?.bundleDigest && $0.renderDigest == generation?.renderDigest
                    }
            }
            return try InstallationObservation(bundleID: bundleID, target: target, resources: resources,
                                               isCurrent: generation != nil && !pending && intact && actual == expected,
                                               pending: pending, version: generation?.version,
                                               bundleDigest: generation?.bundleDigest,
                                               renderDigest: generation?.renderDigest)
        } catch {
            return InstallationObservation(failedBundleID: bundleID, target: target, resources: resources,
                                           error: NativeTransactionExecution.detail(error))
        }
    }

    package static func failed(_ bundleID: String, target: InstallationTarget, error: Error,
                               notices: [Notice] = []) -> InstallationResult
    {
        let detail = NativeTransactionExecution.detail(error)
        let observation = (try? observe(bundleID: bundleID, target: target)) ??
            InstallationObservation(failedBundleID: bundleID, target: target, error: detail)
        return InstallationResult(status: .failed, observation: observation, error: detail, notices: notices)
    }
}

public func inspectInstallation(bundleID: String, target: InstallationTarget) throws -> InstallationObservation {
    try validateIdentifier(bundleID, label: "bundle id")
    return try InstallationInspection.observe(bundleID: bundleID, target: target)
}
