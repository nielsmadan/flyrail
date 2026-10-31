import Darwin
import Foundation

extension ResourceTransaction {
    package static func rollback(_ journal: Journal, store: any StateStore,
                                 seam: NativeTransactionSeam = .none) throws
    {
        let destination = journal.resource.destination
        let directory = try transactionDirectory(journal, store: store)
        let (stagedPath, backupPath) = try TransactionLayout.resources(directory)
        try layout(directory)
        try validateAncestors(journal, store: store, directory: directory)
        let actual = try NativeObservation.snapshot(destination).revision
        let staged = try NativeObservation.snapshot(stagedPath).revision
        let backup = try NativeObservation.snapshot(backupPath).revision
        if !actual.nodes.isEmpty, [journal.before, journal.staged, journal.published].contains(actual),
           try pathExists(directory)
        {
            try requireUnchanged(destination, actual, since: modified(directory))
        }
        if actual == journal.before, backup.nodes.isEmpty {
            try seam("rollback")
            try cleanup(journal, store: store, seam: seam)
            return
        }
        guard staged.nodes.isEmpty || [journal.staged, journal.published].contains(staged) else {
            throw nativeFailure(.recoveryNeeded, "unexpected staged revision", stagedPath)
        }
        guard backup.nodes.isEmpty || [journal.before, journal.backup].contains(backup) else {
            throw nativeFailure(.recoveryNeeded, "unexpected backup revision", backupPath)
        }
        try restore(journal, actual: actual, staged: staged, backup: backup, directory: directory, seam: seam)
        try seam("rollback")
        try cleanup(journal, store: store, seam: seam)
    }

    package static func recover(
        _ resource: ResourceRef, store: any StateStore, seam: NativeTransactionSeam = .none,
    ) throws {
        let (preparation, journal) = try records(resource, store: store)
        guard let preparation else { return }
        guard preparation.resource == resource else {
            throw nativeFailure(.recoveryNeeded, "transaction record disagrees", preparation.directory)
        }
        guard let journal else {
            try abandon(preparation, store: store)
            return
        }
        guard journal.previous != journal.receipt else {
            throw nativeFailure(
                .recoveryNeeded, "transaction record repeats the previous receipt", preparation.directory,
            )
        }
        let current = try StateRecords.receipt(store, resource.destination)
        if current == journal.receipt {
            try cleanup(journal, store: store, seam: seam)
        } else if current == journal.previous {
            try rollback(journal, store: store, seam: seam)
        } else {
            throw nativeFailure(.recoveryNeeded, "receipt disagrees with transaction", resource.destination)
        }
    }

    package static func pendingRecovery(_ resource: ResourceRef, store: any StateStore) throws -> [ResourcePath] {
        try records(resource, store: store).preparation.map { [$0.directory] } ?? []
    }

    package static func changedAfter(boundary: timespec, modified: timespec, born: timespec) -> Bool {
        let later = { (value: timespec) in (value.tv_sec, value.tv_nsec) > (boundary.tv_sec, boundary.tv_nsec) }
        return later(modified) || later(born)
    }

    // swiftlint:disable:next function_parameter_count
    private static func restore(
        _ journal: Journal, actual: Revision, staged: Revision, backup: Revision, directory: ResourcePath,
        seam: NativeTransactionSeam,
    ) throws {
        let destination = journal.resource.destination
        let (stagedPath, backupPath) = try TransactionLayout.resources(directory)
        if journal.before == journal.staged {
            try NativeObservation.requireRevision(destination, journal.before)
        } else if !backup.nodes.isEmpty {
            if !actual.nodes.isEmpty {
                guard [journal.published, journal.staged].contains(actual), staged.nodes.isEmpty else {
                    throw nativeFailure(.recoveryNeeded, "unrecognized published revision", destination)
                }
                try NativeTransactionIO.move(destination, to: stagedPath, revision: actual)
                try seam("rollback-stage")
            }
            try NativeTransactionIO.move(backupPath, to: destination, revision: backup)
            try seam("rollback-backup")
            try NativeObservation.requireRevision(destination, journal.before)
        } else if actual == journal.before {
            return
        } else if journal.before.nodes.isEmpty, actual == journal.published, staged.nodes.isEmpty {
            try NativeTransactionIO.move(destination, to: stagedPath, revision: actual)
            try seam("rollback-stage")
        } else {
            throw nativeFailure(.recoveryNeeded, "cannot restore complete previous revision", destination)
        }
    }

    private static func abandon(_ preparation: Preparation, store: any StateStore) throws {
        let directory = preparation.directory
        if try pathExists(directory) {
            guard known(preparation, store: store) else { throw unrecognizedDirectory(store) }
            let unrecognized = nativeFailure(.recoveryNeeded, "unrecognized transaction data retained", directory)
            let (staged, backup) = try TransactionLayout.resources(directory)
            let retained: Revision
            let current: Revision
            do {
                try layout(directory)
                try ensurePrivateStorage(directory)
                retained = try NativeObservation.snapshot(backup).revision
                current = try NativeObservation.snapshot(staged).revision
            } catch let error as TargetError where error.errno == nil || error.errno == ENOTSUP {
                throw unrecognized
            }
            guard retained.nodes.isEmpty else { throw unrecognized }
            try NativeTransactionIO.destroy(staged, revision: current)
            try removeDirectories(directory, ignoringFailures: false)
        }
        try StateRecords.delete(
            store, key: StateKey.resource(preparation.resource.destination, .preparation), expected: preparation,
        )
    }

    private static func modified(_ directory: ResourcePath) throws -> timespec {
        guard let metadata = try metadata(directory) else { throw nativeIO(ENOENT, path: directory) }
        return metadata.st_mtimespec
    }

    private static func requireUnchanged(_ path: ResourcePath, _ revision: Revision, since boundary: timespec) throws {
        for node in revision.nodes {
            let selected = try node.path.isEmpty ? path : appendPath(path, node.path)
            var metadata = stat()
            guard try NativeFilesystem.withName(selected.platformBytes, { lstat($0, &metadata) }) == 0 else {
                let code = errno
                if code == ENOENT {
                    throw nativeFailure(.concurrentChange, "path changed during inspection", selected)
                }
                throw nativeIO(code, path: selected)
            }
            if changedAfter(boundary: boundary, modified: metadata.st_mtimespec, born: metadata.st_birthtimespec) {
                throw nativeFailure(.recoveryNeeded, "destination revision changed after preparation", selected)
            }
        }
    }
}
