import Foundation

package enum NativeTransaction {
    package static func path(_ resource: ResourceRef, _ name: String) throws -> ResourcePath {
        try appendPath(resource.stateRoot, name)
    }

    package static func ancestors(_ resource: ResourceRef) throws -> [Ancestor] {
        let stage = try NativeObservation.ancestors(path(resource, "staging/resource"))
        let backup = try NativeObservation.ancestors(path(resource, "backup/resource"))
        return stage + backup.filter { !stage.contains($0) }
    }

    private static func validate(_ journal: Journal, lock: NativeDirectoryLock) throws {
        guard lock.stateRoot == journal.resource.stateRoot else {
            throw nativeFailure(.concurrentChange, "resource lock disagrees", journal.resource.stateRoot)
        }
        try lock.validate()
        guard try ancestors(journal.resource) == journal.ancestors else {
            throw nativeFailure(.concurrentChange, "resource ancestor changed", journal.resource.destination)
        }
        let conflicts = try NativeAuthority.hierarchyConflicts(
            journal.resource.recordedAuthority, wholeTree: journal.resource.kind != .document,
        )
        if let first = conflicts.first {
            throw nativeFailure(.conflict, "resource hierarchy reservation changed", first)
        }
        _ = try NativeAdmission.validateState(journal.resource)
    }

    private static func requireJournal(_ journal: Journal) throws {
        let selected = try path(journal.resource, "transaction.json")
        guard try NativeObservation.read(selected, as: Journal.self, tag: "Journal") == journal else {
            throw nativeFailure(.recoveryNeeded, "transaction record changed", selected)
        }
    }

    private static func layout(_ resource: ResourceRef) throws {
        let (snapshot, _) = try NativeAdmission.validateState(resource)
        for name in ["staging", "backup"] {
            guard !snapshot.revision.nodes.contains(where: {
                $0.path.hasPrefix(name + "/") && $0.path != name + "/resource" &&
                    !$0.path.hasPrefix(name + "/resource/")
            }) else {
                throw try nativeFailure(.recoveryNeeded, "unknown transaction data", path(resource, name))
            }
        }
    }

    package static func prepare(
        _ plan: ResourcePlan, lock: NativeDirectoryLock, seam: NativeTransactionSeam = .none,
    ) throws -> Journal {
        let resource = plan.resource
        guard lock.stateRoot == resource.stateRoot else {
            throw nativeFailure(.concurrentChange, "resource lock disagrees", resource.stateRoot)
        }
        try lock.validate()
        guard plan.error == nil, plan.recoveryPaths.isEmpty else {
            throw plan.error ?? nativeFailure(.recoveryNeeded, "preview requires recovery and re-observation",
                                              resource.stateRoot)
        }
        guard try NativeObservation.ancestors(path(resource, "transaction.json")) == plan.ancestors else {
            throw nativeFailure(.concurrentChange, "resource ancestor changed", resource.destination)
        }
        try NativeObservation.requireRevision(resource.stateRoot, plan.stateRevision)
        try NativeObservation.requireRevision(resource.destination, plan.before)
        if plan.requireExisting, plan.before.nodes.isEmpty {
            throw nativeFailure(.concurrentChange, "required existing resource disappeared", resource.destination)
        }
        guard try NativeObservation.read(
            path(resource, "receipt.json"), as: Receipt.self, tag: "Receipt",
        ) == plan.previous else {
            throw nativeFailure(.concurrentChange, "receipt changed during preparation", resource.stateRoot)
        }
        try NativeTransactionBudget.validate(plan)
        var storage: [(ResourcePath, Revision)] = []
        for name in ["staging", "backup"] {
            let selected = try path(resource, name)
            let expected: Revision
            if let node = plan.stateRevision.nodes.first(where: { $0.path == name }) {
                expected = try Revision(nodes: [NativeResourcePlanner.rebase(node, path: "")])
            } else {
                let created = try NativeAdmission.createPrivateDirectories(selected)
                guard created.count == 1, created.first?.path == selected else {
                    throw nativeFailure(.concurrentChange, "transaction storage changed", selected)
                }
                expected = try NativeObservation.snapshot(selected, privateStorage: true).revision
            }
            try NativeObservation.requireRevision(selected, expected)
            storage.append((selected, expected))
        }
        let captured = try ancestors(resource)
        guard captured.filter({ item in !storage.contains { $0.0 == item.path } }) == plan.ancestors else {
            throw nativeFailure(.concurrentChange, "resource ancestor changed", resource.destination)
        }
        for (selected, expected) in storage {
            try NativeObservation.requireRevision(selected, expected)
        }
        let staged = try plan.after == plan.before ? plan.before :
            NativeTransactionIO.writeRevision(path(resource, "staging/resource"), revision: plan.after, seam: seam)
        let receipt = try Receipt(
            resource: resource, transactionID: plan.receipt.transactionID, claims: plan.receipt.claims,
            provenance: plan.receipt.provenance,
            createdDirectories: plan.receipt.createdDirectories.compactMap { marker in
                staged.nodes.first { exactTextEqual($0.path, marker.path) }
            },
        )
        let journal = try Journal(
            resource: resource, previous: plan.previous, receipt: receipt, before: plan.before,
            staged: staged, published: staged, backup: plan.before, ancestors: captured,
        )
        try validate(journal, lock: lock)
        try NativeObservation.requireRevision(resource.destination, plan.before)
        guard try NativeObservation.read(
            path(resource, "receipt.json"), as: Receipt.self, tag: "Receipt",
        ) == plan.previous else {
            throw nativeFailure(.concurrentChange, "receipt changed during preparation", resource.stateRoot)
        }
        try NativeObservation.requireRevision(path(resource, "staging/resource"),
                                              plan.after == plan.before ? Revision() : staged)
        try NativeObservation.requireRevision(path(resource, "backup/resource"), Revision())
        try NativeTransactionIO.atomic(
            path(resource, "transaction.json"), value: journal, exclusive: true, phase: "journal", seam: seam,
            beforePublication: {
                try validate(journal, lock: lock)
                try NativeObservation.requireRevision(resource.destination, plan.before)
                try NativeObservation.requireRevision(path(resource, "staging/resource"),
                                                      plan.after == plan.before ? Revision() : staged)
                try NativeObservation.requireRevision(path(resource, "backup/resource"), Revision())
                guard try NativeObservation.read(
                    path(resource, "receipt.json"), as: Receipt.self, tag: "Receipt",
                ) == plan.previous else {
                    throw nativeFailure(.concurrentChange, "receipt changed during preparation", resource.stateRoot)
                }
            },
        )
        try seam("prepare")
        return journal
    }

    package static func publish(
        _ journal: Journal, lock: NativeDirectoryLock, seam: NativeTransactionSeam = .none,
        committed: () throws -> Void = {},
    ) throws {
        try requireJournal(journal)
        try validate(journal, lock: lock)
        let resource = journal.resource
        let stage = try path(resource, "staging/resource")
        let backup = try path(resource, "backup/resource")
        try NativeObservation.requireRevision(resource.destination, journal.before)
        if journal.before != journal.staged {
            try NativeObservation.requireRevision(stage, journal.staged)
            if !journal.before.nodes.isEmpty {
                try seam("rename")
                try NativeTransactionIO.move(resource.destination, to: backup, revision: journal.backup)
                try seam("backup")
                try NativeObservation.requireRevision(backup, journal.backup)
                try validate(journal, lock: lock)
            }
            if !journal.staged.nodes.isEmpty {
                try seam("staging")
                try seam("rename")
                try NativeTransactionIO.move(stage, to: resource.destination, revision: journal.staged)
            }
            try NativeObservation.requireRevision(resource.destination, journal.published)
        }
        try seam("publish")
        try publishReceipt(journal, lock: lock, seam: seam, committed: committed)
    }

    package static func publishReceipt(
        _ journal: Journal, lock: NativeDirectoryLock, seam: NativeTransactionSeam = .none,
        committed: () throws -> Void = {},
    ) throws {
        try requireJournal(journal)
        try layout(journal.resource)
        try validate(journal, lock: lock)
        let resource = journal.resource
        try NativeObservation.requireRevision(path(resource, "staging/resource"), Revision())
        try NativeObservation.requireRevision(resource.destination, journal.published)
        try NativeObservation.requireRevision(
            path(resource, "backup/resource"), journal.before == journal.staged ? Revision() : journal.backup,
        )
        for name in ["authority.json.next", "transaction.json.next", "receipt.json.next"] {
            let selected = try path(resource, name)
            if try NativeFilesystem.metadata(selected) != nil {
                throw nativeFailure(.recoveryNeeded, "unexpected metadata preparation before commit", selected)
            }
        }
        let receiptPath = try path(resource, "receipt.json")
        let before = try NativeObservation.snapshot(receiptPath, privateStorage: true)
        guard try NativeObservation.read(receiptPath, as: Receipt.self, tag: "Receipt") == journal.previous else {
            throw nativeFailure(.concurrentChange, "receipt changed before commit", resource.stateRoot)
        }
        try NativeObservation.require(receiptPath, before)
        try seam("receipt")
        try NativeTransactionIO.atomic(
            receiptPath, value: journal.receipt, expected: before.revision, phase: "receipt", seam: seam,
            beforePublication: {
                try requireJournal(journal)
                try layout(resource)
                try validate(journal, lock: lock)
                try NativeObservation.require(receiptPath, before)
                try NativeObservation.requireRevision(resource.destination, journal.published)
                try NativeObservation.requireRevision(path(resource, "staging/resource"), Revision())
                try NativeObservation.requireRevision(
                    path(resource, "backup/resource"), journal.before == journal.staged ? Revision() : journal.backup,
                )
            },
            committed: committed,
        )
    }

    package static func cleanup(
        _ journal: Journal, lock: NativeDirectoryLock, seam: NativeTransactionSeam = .none,
    ) throws {
        try requireJournal(journal)
        try layout(journal.resource)
        try validate(journal, lock: lock)
        let receiptPath = try path(journal.resource, "receipt.json")
        let receiptSnapshot = try NativeObservation.snapshot(receiptPath, privateStorage: true)
        let current = try NativeObservation.read(receiptPath, as: Receipt.self, tag: "Receipt")
        if current != journal.receipt {
            guard current == journal.previous,
                  try NativeObservation.snapshot(journal.resource.destination).revision == journal.before,
                  try NativeObservation.snapshot(path(journal.resource, "backup/resource")).revision.nodes.isEmpty
            else {
                throw nativeFailure(.recoveryNeeded, "cleanup requires committed or restored ownership",
                                    journal.resource.stateRoot)
            }
        }
        let metadata = try preparedMetadata(journal)
        func beforeRemoval() throws {
            try validate(journal, lock: lock)
            try requireJournal(journal)
            try NativeObservation.require(receiptPath, receiptSnapshot)
        }
        for (name, revisions) in [
            ("staging/resource", [journal.staged, journal.published]),
            ("backup/resource", [journal.backup, journal.before]),
        ] {
            let selected = try path(journal.resource, name)
            let actual = try NativeObservation.snapshot(selected).revision
            if !actual.nodes.isEmpty {
                guard revisions.contains(where: { actual.nodes == Array($0.nodes.prefix(actual.nodes.count)) }) else {
                    throw nativeFailure(.recoveryNeeded, "unexpected cleanup revision", selected)
                }
                try NativeTransactionIO.destroy(selected, revision: actual, seam: seam, beforeRemoval: beforeRemoval)
            }
        }
        for (selected, snapshot) in metadata {
            try beforeRemoval()
            try NativeTransactionIO.unlink(selected, revision: snapshot.revision)
            try seam("cleanup-metadata")
        }
        try seam("cleanup")
        try beforeRemoval()
        let selected = try path(journal.resource, "transaction.json")
        let snapshot = try NativeObservation.snapshot(selected, privateStorage: true)
        guard try NativeObservation.read(selected, as: Journal.self, tag: "Journal") == journal else {
            throw nativeFailure(.recoveryNeeded, "transaction record changed", selected)
        }
        try NativeTransactionIO.unlink(selected, revision: snapshot.revision)
    }

    private static func preparedMetadata(_ journal: Journal) throws -> [(ResourcePath, NativeResourceSnapshot)] {
        var recognizedMetadata: [(ResourcePath, NativeResourceSnapshot)] = []
        for name in ["receipt.json.next", "transaction.json.next", "authority.json.next"] {
            let selected = try path(journal.resource, name)
            let snapshot = try NativeObservation.snapshot(selected, privateStorage: true)
            guard !snapshot.revision.nodes.isEmpty else { continue }
            let recognized: Bool = switch name {
            case "receipt.json.next":
                try NativeObservation.read(selected, as: Receipt.self, tag: "Receipt") == journal.receipt
            case "transaction.json.next":
                try NativeObservation.read(selected, as: Journal.self, tag: "Journal") == journal
            default:
                try NativeObservation.read(selected, as: ResourceRef.self, tag: "ResourceRef") == journal.resource
            }
            guard recognized else {
                throw nativeFailure(.recoveryNeeded, "staged metadata changed", selected)
            }
            recognizedMetadata.append((selected, snapshot))
        }
        return recognizedMetadata
    }

    package static func rollback(
        _ journal: Journal, lock: NativeDirectoryLock, seam: NativeTransactionSeam = .none,
    ) throws {
        try requireJournal(journal)
        try layout(journal.resource)
        try validate(journal, lock: lock)
        _ = try preparedMetadata(journal)
        let resource = journal.resource
        let stagePath = try path(resource, "staging/resource")
        let backupPath = try path(resource, "backup/resource")
        let actual = try NativeObservation.snapshot(resource.destination)
        let stage = try NativeObservation.snapshot(stagePath).revision
        let backup = try NativeObservation.snapshot(backupPath).revision
        let record = try NativeObservation.snapshot(path(resource, "transaction.json"), privateStorage: true)
        if !actual.revision.nodes.isEmpty,
           [journal.before, journal.staged, journal.published].contains(actual.revision),
           let stamp = record.stamps.first, actual.hasReplacementEvidence(after: stamp)
        {
            throw nativeFailure(.recoveryNeeded, "destination revision changed after preparation", resource.destination)
        }
        if actual.revision == journal.before, backup.nodes.isEmpty {
            try seam("rollback")
            try cleanup(journal, lock: lock, seam: seam)
            return
        }
        guard stage.nodes.isEmpty || [journal.staged, journal.published].contains(stage) else {
            throw nativeFailure(.recoveryNeeded, "unexpected staged revision", stagePath)
        }
        guard backup.nodes.isEmpty || [journal.backup, journal.before].contains(backup) else {
            throw nativeFailure(.recoveryNeeded, "unexpected backup revision", backupPath)
        }
        if journal.before == journal.staged {
            try NativeObservation.requireRevision(resource.destination, journal.before)
        } else if !backup.nodes.isEmpty {
            if !actual.revision.nodes.isEmpty {
                guard [journal.published, journal.staged].contains(actual.revision), stage.nodes.isEmpty else {
                    throw nativeFailure(.recoveryNeeded, "unrecognized published revision", resource.destination)
                }
                try NativeTransactionIO.move(resource.destination, to: stagePath, revision: actual.revision)
                try seam("rollback-stage")
            }
            try validate(journal, lock: lock)
            try NativeTransactionIO.move(backupPath, to: resource.destination, revision: backup)
            try seam("rollback-backup")
            try NativeObservation.requireRevision(resource.destination, journal.before)
        } else if actual.revision == journal.before {
        } else if journal.before.nodes.isEmpty, actual.revision == journal.published, stage.nodes.isEmpty {
            try NativeTransactionIO.move(resource.destination, to: stagePath, revision: actual.revision)
            try seam("rollback-stage")
        } else {
            throw nativeFailure(.recoveryNeeded, "cannot restore complete previous revision", resource.destination)
        }
        try seam("rollback")
        try cleanup(journal, lock: lock, seam: seam)
    }

    package static func recover(
        _ resource: ResourceRef, lock: NativeDirectoryLock, seam: NativeTransactionSeam = .none,
    ) throws {
        guard lock.stateRoot == resource.stateRoot else {
            throw nativeFailure(.concurrentChange, "resource lock disagrees", resource.stateRoot)
        }
        try lock.validate()
        let (_, pending) = try NativeAdmission.validateState(resource)
        guard let first = pending.first else { return }
        let selected = try path(resource, "transaction.json")
        var journal = try NativeObservation.read(selected, as: Journal.self, tag: "Journal")
        if journal == nil {
            let next = try path(resource, "transaction.json.next")
            let snapshot = try NativeObservation.snapshot(next, privateStorage: true)
            if let candidate = try NativeObservation.read(next, as: Journal.self, tag: "Journal"),
               candidate.resource == resource
            {
                try validate(candidate, lock: lock)
                try NativeObservation.require(next, snapshot)
                try NativeTransactionIO.move(next, to: selected, revision: snapshot.revision)
                try seam("recover-journal")
                journal = candidate
            }
        }
        guard let journal, journal.resource == resource else {
            throw nativeFailure(.recoveryNeeded, "unrecognized preparation data retained", first)
        }
        let receipt = try NativeObservation.read(path(resource, "receipt.json"), as: Receipt.self, tag: "Receipt")
        if receipt == journal.receipt {
            try cleanup(journal, lock: lock, seam: seam)
        } else if receipt == journal.previous {
            try rollback(journal, lock: lock, seam: seam)
        } else {
            throw nativeFailure(.recoveryNeeded, "receipt disagrees with transaction", resource.stateRoot)
        }
    }
}
