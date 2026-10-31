import Foundation

package struct NativeTransactionOutcome {
    package let resources: [ResourceResult]
    package let error: TargetError?

    package var status: OperationStatus {
        if error == nil {
            return .applied
        }
        if resources.contains(where: { $0.status == .applied }) {
            return .partial
        }
        return resources.contains(where: { $0.status == .incomplete }) ? .incomplete : .failed
    }
}

package enum NativeTransactionExecution {
    package static func detail(_ error: Error) -> TargetError {
        if let error = error as? TargetError {
            return error
        }
        if let error = error as? StoreError {
            return error.targetError
        }
        if let error = error as? FlyrailValueError {
            return TargetError(code: .invalidState, message: error.message)
        }
        return TargetError(code: .ioError, message: String(describing: error))
    }

    package static func recoveryPaths(
        _ store: any StateStore, _ resource: ResourceRef, directory: ResourcePath?,
    ) -> [ResourcePath] {
        if let paths = try? ResourceTransaction.pendingRecovery(resource, store: store) {
            return paths
        }
        guard let directory else { return [] }
        let preparation = try? StateRecords.preparation(store, resource.destination)
        return preparation?.directory == directory ? [directory] : []
    }

    package static func execute(
        _ locks: NativeTransactionLocks, plans: [ResourcePlan], previous: Index?, pending: Index, completed: Index,
        seam: NativeTransactionSeam = .none,
    ) throws -> NativeTransactionOutcome {
        let store = locks.target.store
        let key = try InstallationIndex.key(bundleID: locks.bundleID, target: locks.target)
        guard try Index(bundleID: pending.bundleID, contextDigest: pending.contextDigest, current: pending.pending) ==
            completed,
            plans.count == locks.resources.count, Set(plans.map(\.resource)) == Set(locks.resources),
            pending.bundleID == locks.bundleID, try pending.contextDigest == locks.target.contextDigest,
            Set(pending.residual) == Set(locks.resources),
            Set(pending.resources) == Set(locks.resources)
        else {
            throw StateRecords.failure(store, .invalidState, "installation index completion disagrees with preparation")
        }
        guard previous?.current == pending.current, pending.previous == pending.current else {
            throw StateRecords.failure(store, .invalidState, "installation index preparation disagrees")
        }
        try validateMembership(plans, pending: pending, store: store)
        _ = try StateCodec.encode(pending)
        _ = try StateCodec.encode(completed)
        for plan in plans {
            try NativeTransactionBudget.validate(plan)
        }
        try StateRecords.write(store, key: key, value: pending, expected: previous)
        var results: [ResourceResult] = []
        for plan in plans {
            if plan.previous == plan.receipt, plan.before == plan.after {
                try results.append(ResourceResult(resource: plan.resource, status: .unchanged))
                continue
            }
            var journal: Journal?
            var guarded = false
            var committed = false
            do {
                try seam("resource-precondition")
                try seam("index-require")
                try StateRecords.requireIndex(store, key: key, expected: pending)
                try seam("guard-lookup")
                guard locks.resources.contains(plan.resource) else {
                    throw nativeFailure(.concurrentChange, "resource lock disagrees", plan.resource.destination)
                }
                guarded = true
                let prepared = try ResourceTransaction.prepare(plan, store: store, seam: seam)
                journal = prepared
                try StateRecords.requireIndex(store, key: key, expected: pending)
                try ResourceTransaction.publish(prepared, store: store, seam: seam) { committed = true }
                try ResourceTransaction.cleanup(prepared, store: store, seam: seam)
            } catch let crash as NativeSimulatedCrash {
                throw crash
            } catch {
                if !committed, let journal {
                    committed = (try? StateRecords.receipt(store, plan.resource.destination)) == journal.receipt
                }
                let failure = try guarded ? recovered(detail(error), plan.resource, store: store, seam: seam) :
                    detail(error)
                let paths = recoveryPaths(store, plan.resource, directory: plan.transactionDirectory)
                try results.append(ResourceResult(
                    resource: plan.resource, status: committed ? .applied : (paths.isEmpty ? .failed : .incomplete),
                    error: failure, recoveryPaths: paths,
                ))
                return NativeTransactionOutcome(resources: results, error: failure)
            }
            try results.append(ResourceResult(resource: plan.resource, status: .applied))
        }
        do {
            try seam("index-completion")
            try StateRecords.write(store, key: key, value: completed, expected: pending)
        } catch let crash as NativeSimulatedCrash {
            throw crash
        } catch { return NativeTransactionOutcome(resources: results, error: detail(error)) }
        return NativeTransactionOutcome(resources: results, error: nil)
    }

    private static func recovered(
        _ failure: TargetError, _ resource: ResourceRef, store: any StateStore, seam: NativeTransactionSeam,
    ) throws -> TargetError {
        do {
            try ResourceTransaction.recover(resource, store: store, seam: seam)
            return failure
        } catch let crash as NativeSimulatedCrash {
            throw crash
        } catch {
            return TargetError(
                code: failure.code, message: failure.message + "; recovery: " + detail(error).message,
                path: failure.path, errno: failure.errno,
            )
        }
    }

    private static func validateMembership(_ plans: [ResourcePlan], pending: Index, store: any StateStore) throws {
        let members = Dictionary(uniqueKeysWithValues: (pending.pending?.membership ?? []).map {
            ($0.resource, Set($0.claimIDs))
        })
        for plan in plans {
            let owned = plan.receipt.claims.filter { $0.bundleID == pending.bundleID }
            guard Set(owned.map(\.claimID)) == (members[plan.resource] ?? []),
                  owned.allSatisfy({
                      exactTextEqual($0.version, pending.pending?.version ?? "") &&
                          $0.bundleDigest == pending.pending?.bundleDigest &&
                          $0.renderDigest == pending.pending?.renderDigest
                  })
            else { throw StateRecords.failure(store, .invalidState, "planned receipt disagrees with membership") }
        }
        guard Set(members.keys).isSubset(of: Set(plans.map(\.resource))) else {
            throw StateRecords.failure(store, .invalidState, "membership has an unplanned resource")
        }
    }
}
