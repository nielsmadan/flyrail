import Foundation

package final class NativeTransactionLocks {
    package let target: InstallationTarget
    package let bundleID: String
    package let resources: [ResourceRef]
    package let created: [Ancestor]
    private let indexLock: StoreLock
    private var resourceLocks: [StoreLock]

    package init(
        target: InstallationTarget, bundleID: String, resources requested: [ResourceRef], timeout: Duration,
        validateIndex: () throws -> Void,
    ) throws {
        let store = target.store
        var created = try NativeTransactionDirectory.createStoreParents(store)
        let indexLock = try StateRecords.lock(
            store, name: InstallationIndex.key(bundleID: bundleID, target: target), timeout: timeout,
        )
        var locks: [StoreLock] = []
        do {
            try validateIndex()
            let resources = destinationOrder(Array(Set(requested)))
            for resource in resources {
                try ResourceAdmission.validateKind(resource)
                let lock = try StateRecords.lock(
                    store, name: StateKey.resourceLock(resource.destination), timeout: timeout,
                )
                locks.append(lock)
                created += try NativeTransactionDirectory.createParents(of: resource.destination)
                try ResourceAdmission.admit(resource, store: store)
            }
            self.target = target
            self.bundleID = bundleID
            self.resources = resources
            self.created = created
            self.indexLock = indexLock
            resourceLocks = locks
        } catch {
            locks.reversed().forEach { $0.release() }
            indexLock.release()
            throw error
        }
    }

    deinit { release() }

    package func release() {
        resourceLocks.reversed().forEach { $0.release() }
        resourceLocks = []
        indexLock.release()
    }
}
