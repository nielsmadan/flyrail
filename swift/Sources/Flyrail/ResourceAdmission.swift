import Darwin
import Foundation

package struct ResourceState: Sendable, Equatable {
    package let revision: Revision
    package let receipt: Receipt?
    package let recoveryPaths: [ResourcePath]
}

package enum ResourceAdmission {
    package static func admit(_ resource: ResourceRef, store: any StateStore) throws {
        let destination = resource.destination
        let authority = try checkedAuthority(resource)
        if let current = try StateRecords.authority(store, destination) {
            guard current == resource else {
                throw nativeFailure(.invalidState, "authority identity disagrees", destination)
            }
        } else {
            try StateRecords.write(
                store, key: StateKey.resource(destination, .authority), value: resource, expected: nil,
            )
        }
        try validateKind(resource)
        let conflicts = try hierarchyConflicts(authority, store: store, wholeTree: resource.kind != .document)
        if let first = conflicts.first {
            throw nativeFailure(.conflict, "ancestor or descendant resource conflicts", first)
        }
    }

    package static func checkedAuthority(_ resource: ResourceRef) throws -> ResourceAuthority {
        do {
            guard try NativeAuthority.canonicalDestination(resource.destination) == resource.destination else {
                throw FlyrailValueError("resource canonical destination changed")
            }
            return try resource.recordedAuthority
        } catch let error as FlyrailValueError {
            throw TargetError(code: .invalidState, message: error.message)
        }
    }

    package static func validateKind(_ resource: ResourceRef) throws {
        guard let metadata = try NativeFilesystem.metadata(resource.destination) else { return }
        try NativeFilesystem.ordinary(metadata, path: resource.destination)
        guard (metadata.st_mode & S_IFMT == S_IFREG) == (resource.kind == .document) else {
            throw nativeFailure(.unsafePath, "resource kind disagrees with existing destination", resource.destination)
        }
    }

    package static func hierarchyConflicts(
        _ authority: ResourceAuthority, store: any StateStore, wholeTree: Bool,
    ) throws -> [ResourcePath] {
        let destination = authority.destination
        let parts = try NativeFilesystem.components(destination)
        let ancestors = try (1 ..< max(parts.count, 1)).map {
            try ResourcePath(platformBytes: [47] + parts.prefix($0).joined(separator: [47]))
        }
        guard wholeTree else {
            return try ancestors.filter { try StateRecords.raw(store, key: StateKey.resource($0, .authority)) != nil }
        }
        let listed = try StateRecords.keys(store, prefix: "resource/").filter { $0.hasSuffix("/authority") }
        let keys = Set(listed)
        var conflicts = try ancestors.filter { try keys.contains(StateKey.resource($0, .authority)) }
        let recorded = try listed.compactMap {
            try StateRecords.read(store, key: $0, as: ResourceRef.self, tag: "ResourceRef")
        }
        conflicts += try recorded.map(\.destination).filter {
            let inner = try NativeFilesystem.components($0)
            return inner.count > parts.count && Array(inner.prefix(parts.count)) == parts
        }
        var metadata = stat()
        if try NativeFilesystem.withName(destination.platformBytes, { fstatat(AT_FDCWD, $0, &metadata, 0) }) == 0 {
            conflicts += try NativeTransactionDirectory.entries(inside: destination)
        }
        return Array(Set(conflicts)).sorted { $0.platformBytes.lexicographicallyPrecedes($1.platformBytes) }
    }

    package static func storedReceipt(_ resource: ResourceRef, store: any StateStore) throws -> Receipt? {
        let receipt = try StateRecords.receipt(store, resource.destination)
        if let receipt, receipt.resource != resource {
            throw nativeFailure(.invalidState, "receipt resource disagrees", resource.destination)
        }
        return receipt
    }

    package static func observe(_ resource: ResourceRef, store: any StateStore) throws -> ResourceState {
        let recoveryPaths = try ResourceTransaction.pendingRecovery(resource, store: store)
        let receipt = try storedReceipt(resource, store: store)
        return try ResourceState(
            revision: NativeObservation.snapshot(resource.destination).revision,
            receipt: receipt,
            recoveryPaths: recoveryPaths,
        )
    }
}
