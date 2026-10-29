import Foundation

package final class NativeTransactionLocks {
    package let target: InstallationTarget
    package let bundleID: String
    package let index: NativeDirectoryLock
    package let resources: [ResourceRef]
    package let guards: [NativeDirectoryLock]
    package let created: [Ancestor]
    package let observation: NativeIndexObservation

    package init(
        target: InstallationTarget, bundleID: String, resources requested: [ResourceRef], timeout: Duration,
        allowRecovery: Bool = true,
        validateIndex: (NativeIndexObservation) throws -> Void = { _ in },
    ) throws {
        self.target = target
        self.bundleID = bundleID
        index = try NativeAdmission.reserveIndex(target, timeout: timeout)
        observation = try NativeAdmission.observeIndex(target, bundleID: bundleID)
        try validateIndex(observation)
        if let prepared = observation.prepared {
            do {
                _ = try NativeIndexRecovery.promotes(observation.current, prepared: prepared)
            } catch let error as FlyrailValueError {
                throw try nativeFailure(.recoveryNeeded, error.message, appendPath(target.indexRoot, "index.json.next"))
            }
        }
        resources = Set(requested + [observation.current, observation.prepared].compactMap(\.self).flatMap(\.resources))
            .sorted { $0.stateRoot.platformBytes.lexicographicallyPrecedes($1.stateRoot.platformBytes) }
        guard Set(resources.map(\.destination)).count == resources.count else {
            throw nativeFailure(.invalidState, "resource kinds disagree at the same destination", target.indexRoot)
        }
        try NativeAdmission.validateIndexSeparation(target, resources: resources)
        try NativeIndexRecovery.requireReceipts(
            observation.current,
            prepared: observation.prepared,
            resources: resources,
        )
        guards = try resources.map { try NativeAdmission.reserve($0, timeout: timeout) }
        created = index.createdDirectories + guards.flatMap(\.createdDirectories)
        if !allowRecovery {
            for resource in resources {
                let (_, paths) = try NativeAdmission.validateState(resource)
                if let path = paths.first {
                    throw nativeFailure(.recoveryNeeded, "preview requires recovery and re-observation", path)
                }
            }
        }
        for (resource, lock) in zip(resources, guards) {
            try NativeAdmission.admit(resource, lock: lock, allowRecovery: allowRecovery)
        }
        try validate()
    }

    package func validate() throws {
        try index.validate()
        for lock in guards {
            try lock.validate()
        }
    }

    package func guardFor(_ resource: ResourceRef) throws -> NativeDirectoryLock {
        guard let index = resources.firstIndex(of: resource) else {
            throw nativeFailure(.concurrentChange, "resource locks disagree", resource.stateRoot)
        }
        return guards[index]
    }
}

package struct NativeIndexRecoveryFailure: Error {
    package let resources: [ResourceResult]
    package let error: TargetError
}

package enum NativeIndexRecovery {
    package static func completion(_ pending: Index) throws -> Index {
        try Index(bundleID: pending.bundleID, contextDigest: pending.contextDigest, current: pending.pending)
    }

    package static func completes(_ current: Index?, prepared: Index?) throws -> Bool {
        guard let current,
              current.pending != nil || current.previous != nil || !current.residual.isEmpty
        else { return false }
        return try prepared == completion(current)
    }

    package static func promotes(_ current: Index?, prepared: Index) throws -> Bool {
        if try prepared == current || completes(current, prepared: prepared) {
            return false
        }
        let previous = current?.current
        let retained = Set(current?.resources ?? [])
        let desired = Set(prepared.pending?.membership.map(\.resource) ?? [])
        guard prepared.current == previous, prepared.previous == previous,
              Set(prepared.residual) == retained.union(desired)
        else { throw FlyrailValueError("unrecognized installation index preparation retained") }
        return true
    }

    package static func expectsReceipt(
        _ current: Index?, prepared: Index?, resource: ResourceRef,
    ) throws -> Bool {
        let committed = [current?.current, current?.previous].compactMap(\.self)
        let completed = try completes(current, prepared: prepared) ? [prepared?.current].compactMap(\.self) : []
        return (committed + completed).contains { $0.membership.contains { $0.resource == resource } }
    }

    package static func requireReceipts(_ current: Index?, prepared: Index?, resources: [ResourceRef]) throws {
        for resource in resources {
            let path = try appendPath(resource.stateRoot, "receipt.json")
            let receipt = try NativeObservation.read(path, as: Receipt.self, tag: "Receipt")
            if let receipt, receipt.resource != resource {
                throw nativeFailure(.invalidState, "receipt resource disagrees", path)
            }
            if try receipt == nil && expectsReceipt(current, prepared: prepared, resource: resource) {
                throw nativeFailure(
                    .invalidState, "recorded ownership receipt is missing; restore metadata before retrying", path,
                )
            }
        }
    }

    package static func recover(
        _ locks: NativeTransactionLocks, seam: NativeTransactionSeam = .none, retainingResults: Bool = false,
    ) throws -> [ResourceResult] {
        try locks.validate()
        let observed = try NativeAdmission.observeIndex(locks.target, bundleID: locks.bundleID)
        guard observed.current == locks.observation.current, observed.prepared == locks.observation.prepared else {
            throw nativeFailure(.concurrentChange, "index changed while acquiring locks", locks.target.indexRoot)
        }
        let promote: Bool
        do {
            promote = try observed.prepared.map { try promotes(observed.current, prepared: $0) } ?? false
        } catch let error as FlyrailValueError {
            throw try nativeFailure(
                .recoveryNeeded,
                error.message,
                appendPath(locks.target.indexRoot, "index.json.next"),
            )
        }
        try requireReceipts(observed.current, prepared: observed.prepared, resources: locks.resources)
        var results: [ResourceResult] = []
        for (resource, lock) in zip(locks.resources, locks.guards) {
            do {
                let (_, paths) = try NativeAdmission.validateState(resource)
                try NativeTransaction.recover(resource, lock: lock, seam: seam)
                try results.append(ResourceResult(resource: resource, status: paths.isEmpty ? .unchanged : .applied))
            } catch {
                guard retainingResults else { throw error }
                let detail = NativeTransactionExecution.detail(error)
                let paths = (try? NativeAdmission.validateState(resource).1) ?? [resource.stateRoot]
                try results.append(ResourceResult(resource: resource, status: .incomplete,
                                                  error: detail, recoveryPaths: paths))
                throw NativeIndexRecoveryFailure(resources: results, error: detail)
            }
        }
        if observed.prepared != nil {
            try locks.validate()
            try NativeObservation.requireRevision(locks.target.indexRoot, observed.snapshot.revision)
            let next = try appendPath(locks.target.indexRoot, "index.json.next")
            let revision = try NativeObservation.snapshot(next, privateStorage: true).revision
            guard try NativeAdmission.observeIndex(locks.target, bundleID: locks.bundleID) == observed else {
                throw nativeFailure(.concurrentChange, "installation index preparation changed", next)
            }
            if promote {
                try NativeTransactionIO.rename(
                    next,
                    to: appendPath(locks.target.indexRoot, "index.json"),
                    exclusive: false,
                )
            } else {
                try NativeTransactionIO.unlink(next, revision: revision)
            }
            try seam("recover-index")
        }
        return results
    }
}
