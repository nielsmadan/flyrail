import Foundation

package enum InstallationIndex {
    package static func key(bundleID: String, target: InstallationTarget) throws -> String {
        try StateKey.index(bundleID: bundleID, contextDigest: target.contextDigest)
    }

    package static func read(bundleID: String, target: InstallationTarget) throws -> Index? {
        let index = try StateRecords.read(
            target.store, key: key(bundleID: bundleID, target: target), as: Index.self, tag: "Index",
        )
        if let index, try index.bundleID != bundleID || index.contextDigest != target.contextDigest {
            throw TargetError(
                code: .invalidState, message: "installation index context disagrees", path: target.store.location,
            )
        }
        return index
    }

    package static func expectsReceipt(_ index: Index?, resource: ResourceRef) -> Bool {
        [index?.current, index?.previous].compactMap(\.self).contains { generation in
            generation.membership.contains { $0.resource == resource }
        }
    }

    package static func isPending(_ index: Index?) -> Bool {
        guard let index else { return false }
        return index.pending != nil || index.previous != nil || !index.residual.isEmpty
    }
}
