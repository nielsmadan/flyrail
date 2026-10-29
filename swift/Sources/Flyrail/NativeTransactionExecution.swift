import Foundation

package struct NativeTransactionOutcome {
    package let resources: [ResourceResult]
    package let error: TargetError?

    package var status: OperationStatus {
        if error == nil { return .applied }
        if resources.contains(where: { $0.status == .applied }) { return .partial }
        return resources.contains(where: { $0.status == .incomplete }) ? .incomplete : .failed
    }
}

package enum NativeTransactionExecution {
    package static func detail(_ error: Error) -> TargetError {
        if let error = error as? TargetError { return error }
        if let error = error as? FlyrailValueError { return TargetError(code: .invalidState, message: error.message) }
        return TargetError(code: .ioError, message: String(describing: error))
    }

    private static func preflight(_ plan: ResourcePlan, lock: NativeDirectoryLock) throws {
        if let error = plan.error { throw error }
        guard plan.recoveryPaths.isEmpty else {
            throw nativeFailure(
                .recoveryNeeded,
                "preview requires recovery and re-observation",
                plan.resource.stateRoot
            )
        }
        try lock.validate()
        let current = try NativeAdmission.observe(plan.resource, lock: lock)
        guard current.revision == plan.before, current.receipt == plan.previous,
              current.state.revision == plan.stateRevision, current.recovery == plan.recoveryPaths,
              try NativeObservation.ancestors(
                  appendPath(plan.resource.stateRoot, "transaction.json")
              ) == plan.ancestors
        else {
            throw nativeFailure(.concurrentChange, "complete resource preconditions changed", plan.resource.destination)
        }
    }

    package static func publishIndex(
        _ value: Index, locks: NativeTransactionLocks, expected: NativeResourceSnapshot,
        phase: String, seam: NativeTransactionSeam, beforePublication: () throws -> Void = {}
    ) throws -> NativeResourceSnapshot {
        try locks.validate()
        try NativeObservation.require(locks.target.indexRoot, expected)
        let path = try appendPath(locks.target.indexRoot, "index.json")
        try NativeTransactionIO.atomic(
            path, value: value, phase: phase, seam: seam,
            beforePublication: {
                try locks.validate()
                try beforePublication()
            }
        )
        return try NativeObservation.snapshot(locks.target.indexRoot, privateStorage: true)
    }

    package static func execute(
        _ locks: NativeTransactionLocks, plans: [ResourcePlan], pending: Index, completed: Index,
        indexSnapshot: NativeResourceSnapshot, seam: NativeTransactionSeam = .none
    ) throws -> NativeTransactionOutcome {
        guard try NativeIndexRecovery.completion(pending) == completed,
              plans.count == locks.resources.count, Set(plans.map(\.resource)) == Set(locks.resources),
              pending.bundleID == locks.bundleID, try pending.contextDigest == locks.target.contextDigest,
              Set(pending.residual) == Set(locks.resources),
              Set(pending.resources) == Set(locks.resources)
        else {
            throw nativeFailure(.invalidState, "installation index completion disagrees with preparation",
                                locks.target.indexRoot)
        }
        try locks.validate()
        try NativeObservation.require(locks.target.indexRoot, indexSnapshot)
        let current = try NativeAdmission.observeIndex(locks.target, bundleID: locks.bundleID)
        guard current.prepared == nil, current.current?.current == pending.current,
              pending.previous == pending.current
        else { throw nativeFailure(.invalidState, "installation index preparation disagrees", locks.target.indexRoot) }
        try validateMembership(plans, pending: pending, indexRoot: locks.target.indexRoot)
        _ = try StateCodec.encode(pending)
        _ = try StateCodec.encode(completed)
        for plan in plans {
            try preflight(plan, lock: locks.guardFor(plan.resource))
            try NativeTransactionBudget.validate(plan)
        }
        let pendingSnapshot = try publishIndex(
            pending, locks: locks, expected: indexSnapshot, phase: "index-pending", seam: seam
        )
        var results: [ResourceResult] = []
        for plan in plans {
            let lock = try locks.guardFor(plan.resource)
            var journal: Journal?
            var committed = false
            do {
                try seam("resource-precondition")
                try seam("index-require")
                try NativeObservation.require(locks.target.indexRoot, pendingSnapshot)
                try seam("index-ancestor")
                try locks.index.validate()
                try seam("guard-lookup")
                try preflight(plan, lock: lock)
                if plan.previous == plan.receipt, plan.before == plan.after {
                    try results.append(ResourceResult(resource: plan.resource, status: .unchanged))
                    continue
                }
                journal = try NativeTransaction.prepare(plan, lock: lock, seam: seam)
                try NativeObservation.require(locks.target.indexRoot, pendingSnapshot)
                if let journal {
                    try NativeTransaction.publish(journal, lock: lock, seam: seam) { committed = true }
                    try NativeTransaction.cleanup(journal, lock: lock, seam: seam)
                }
                try results.append(ResourceResult(resource: plan.resource, status: .applied))
            } catch {
                var failure = detail(error)
                if !committed, let journal {
                    committed = (try? NativeObservation.read(
                        appendPath(plan.resource.stateRoot, "receipt.json"), as: Receipt.self, tag: "Receipt"
                    )) == journal.receipt
                }
                do {
                    try NativeTransaction.recover(plan.resource, lock: lock, seam: seam)
                } catch {
                    failure = TargetError(
                        code: failure.code, message: failure.message + "; recovery: " + detail(error).message,
                        path: failure.path, errno: failure.errno
                    )
                }
                let paths = (try? NativeAdmission.validateState(plan.resource).1) ?? [plan.resource.stateRoot]
                try results.append(ResourceResult(
                    resource: plan.resource, status: committed ? .applied : (paths.isEmpty ? .failed : .incomplete),
                    error: failure, recoveryPaths: paths
                ))
                return NativeTransactionOutcome(resources: results, error: failure)
            }
        }
        do {
            try seam("index-completion")
            _ = try publishIndex(
                completed, locks: locks, expected: pendingSnapshot, phase: "index-completion", seam: seam
            ) {
                try requireCompleted(plans)
            }
        } catch { return NativeTransactionOutcome(resources: results, error: detail(error)) }
        return NativeTransactionOutcome(resources: results, error: nil)
    }

    private static func validateMembership(_ plans: [ResourcePlan], pending: Index, indexRoot: ResourcePath) throws {
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
            else { throw nativeFailure(.invalidState, "planned receipt disagrees with membership", indexRoot) }
        }
        guard Set(members.keys).isSubset(of: Set(plans.map(\.resource))) else {
            throw nativeFailure(.invalidState, "membership has an unplanned resource", indexRoot)
        }
    }

    private static func requireCompleted(_ plans: [ResourcePlan]) throws {
        for plan in plans {
            let resource = plan.resource
            let (_, recovery) = try NativeAdmission.validateState(resource)
            let current = try NativeObservation.read(
                appendPath(resource.stateRoot, "receipt.json"), as: Receipt.self, tag: "Receipt"
            )
            guard recovery.isEmpty, let current, current.resource == resource,
                  current.transactionID == plan.receipt.transactionID,
                  current.claims == plan.receipt.claims, current.provenance == plan.receipt.provenance,
                  current.createdDirectories.map(\.path) == plan.receipt.createdDirectories.map(\.path)
            else { throw nativeFailure(.concurrentChange, "resource receipt changed before index completion",
                                       resource.stateRoot) }
        }
    }
}
