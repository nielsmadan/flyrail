import Darwin
import Foundation

package enum NativeTransactionBudget {
    package static func validate(_ plan: ResourcePlan) throws {
        guard plan.before != plan.after || plan.previous != plan.receipt else { return }
        guard plan.previous?.transactionID != plan.receipt.transactionID else {
            throw nativeFailure(.invalidState, "transaction requires a fresh receipt identity", plan.resource.stateRoot)
        }
        try validateRevision(plan.before, destination: plan.resource.destination)
        try validateRevision(plan.after, destination: plan.resource.destination)
        let changed = plan.before != plan.after
        let staged = try changed ? boundedRevision(plan.after) : plan.before
        let receipt = try Receipt(
            resource: plan.resource, transactionID: plan.receipt.transactionID, claims: plan.receipt.claims,
            provenance: plan.receipt.provenance,
            createdDirectories: plan.receipt.createdDirectories.compactMap { marker in
                staged.nodes.first { exactTextEqual($0.path, marker.path) }
            },
        )
        let captured = try boundedAncestors(plan)
        let journal = try Journal(
            resource: plan.resource, previous: plan.previous, receipt: receipt,
            before: plan.before, staged: staged, published: staged, backup: plan.before, ancestors: captured,
        )
        let receiptBytes = try StateCodec.encode(receipt).count
        let journalBytes = try StateCodec.encode(journal).count
        let storage = changed ? staged.nodes + plan.before.nodes : []
        let missingDirectories = ["staging", "backup"].filter { name in
            !plan.stateRevision.nodes.contains { $0.path == name }
        }.count
        let nodes = plan.stateRevision.nodes.count + missingDirectories + storage.count + 2
        let total = plan.stateRevision.nodes.reduce(0) { $0 + ($1.data?.count ?? 0) } +
            storage.reduce(0) { $0 + ($1.data?.count ?? 0) } + receiptBytes + journalBytes
        guard nodes <= NativeLimits.default.maximumNodes,
              storage.allSatisfy({ depth($0.path) + 2 <= NativeLimits.default.maximumDepth })
        else {
            throw nativeIO(
                ENOTSUP,
                message: "transaction storage exceeds the node limit",
                path: plan.resource.stateRoot,
            )
        }
        guard total <= NativeLimits.default.maximumTotalBytes else {
            throw nativeIO(
                ENOTSUP,
                message: "transaction storage exceeds the byte limit",
                path: plan.resource.stateRoot,
            )
        }
    }

    package static func validateRevision(_ revision: Revision, destination: ResourcePath) throws {
        let limits = NativeLimits.default
        guard revision.nodes.count <= limits.maximumNodes,
              revision.nodes.allSatisfy({ depth($0.path) <= limits.maximumDepth })
        else { throw nativeIO(ENOTSUP, message: "resource exceeds the node limit", path: destination) }
        guard revision.nodes.allSatisfy({ ($0.data?.count ?? 0) <= limits.maximumFileBytes }),
              revision.nodes.reduce(0, { $0 + ($1.data?.count ?? 0) }) <= limits.maximumTotalBytes
        else { throw nativeIO(ENOTSUP, message: "resource exceeds the byte limit", path: destination) }
        for node in revision.nodes {
            let selected = try node.path.isEmpty ? destination : appendPath(destination, node.path)
            guard node.uid.uint32 != nil, node.gid.uint32 != nil, let mode = node.mode.uint32 else {
                throw nativeIO(ENOTSUP, message: "resource metadata cannot be preserved", path: selected)
            }
            if node.data == nil, mode & 0o700 != 0o700 {
                throw nativeIO(ENOTSUP, message: "directory mode cannot support transaction storage and cleanup",
                               path: selected)
            }
        }
    }

    private static func depth(_ path: String) -> Int {
        path.isEmpty ? 0 : path.split(separator: "/").count
    }

    private static func boundedRevision(_ revision: Revision) throws -> Revision {
        try Revision(nodes: revision.nodes.map {
            try Node(path: $0.path, data: $0.data, mode: $0.mode, uid: $0.uid, gid: $0.gid,
                     device: Count(UInt64(UInt32.max)), inode: Count(UInt64.max), security: $0.security)
        })
    }

    private static func boundedAncestors(_ plan: ResourcePlan) throws -> [Ancestor] {
        var captured = try NativeTransaction.ancestors(plan.resource)
        for name in ["staging", "backup"] {
            let selected = try NativeTransaction.path(plan.resource, name)
            if !captured.contains(where: { $0.path == selected }) {
                try captured.append(Ancestor(
                    path: selected, device: Count(UInt64(UInt32.max)), inode: Count(UInt64.max), mode: 0o700,
                    uid: Count(UInt64(geteuid())), gid: Count(UInt64(getegid())),
                    security: JSONWriter.canonicalData(.object([
                        ("acl", .string("")), ("flags", .number("0")),
                        ("schema_version", .number("1")), ("xattrs", .array([])),
                    ])),
                ))
            }
        }
        return captured
    }
}
