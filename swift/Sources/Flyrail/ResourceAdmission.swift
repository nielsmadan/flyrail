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
        do {
            guard try NativeAuthority.canonicalDestination(destination) == destination else {
                throw FlyrailValueError("resource canonical destination changed")
            }
        } catch let error as FlyrailValueError {
            throw TargetError(code: .invalidState, message: error.message)
        }
        let key = try StateKey.resource(destination, .authority)
        if let current = try StateRecords.read(store, key: key, as: ResourceRef.self, tag: "ResourceRef") {
            guard current == resource else {
                throw nativeFailure(.invalidState, "authority identity disagrees", destination)
            }
        } else {
            try StateRecords.write(store, key: key, value: resource, expected: nil)
        }
        try validateKind(resource)
        let conflicts = try hierarchyConflicts(
            resource.recordedAuthority, store: store, wholeTree: resource.kind != .document,
        )
        if let first = conflicts.first {
            throw nativeFailure(.conflict, "ancestor or descendant resource conflicts", first)
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
        var conflicts = try ancestors.filter {
            try StateRecords.raw(store, key: StateKey.resource($0, .authority)) != nil
        }
        if wholeTree {
            let recorded = try StateRecords.keys(store, prefix: "resource/").filter { $0.hasSuffix("/authority") }
                .compactMap { try StateRecords.read(store, key: $0, as: ResourceRef.self, tag: "ResourceRef") }
            conflicts += try recorded.map(\.destination).filter {
                let inner = try NativeFilesystem.components($0)
                return inner.count > parts.count && Array(inner.prefix(parts.count)) == parts
            }
            var metadata = stat()
            if try NativeFilesystem.withName(destination.platformBytes, { fstatat(AT_FDCWD, $0, &metadata, 0) }) == 0 {
                conflicts += try NativeTransactionDirectory.entries(inside: destination)
            }
        }
        let sorted = conflicts.sorted { $0.platformBytes.lexicographicallyPrecedes($1.platformBytes) }
        return sorted.enumerated().filter { $0.offset == 0 || sorted[$0.offset - 1] != $0.element }.map(\.element)
    }

    package static func storedReceipt(_ resource: ResourceRef, store: any StateStore) throws -> Receipt? {
        let key = try StateKey.resource(resource.destination, .receipt)
        let receipt = try StateRecords.read(store, key: key, as: Receipt.self, tag: "Receipt")
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
