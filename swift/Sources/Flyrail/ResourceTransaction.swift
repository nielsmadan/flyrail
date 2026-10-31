import Darwin
import Foundation

package struct NativeSimulatedCrash: Error, Sendable, Equatable {
    package let boundary: String

    package init(_ boundary: String) {
        self.boundary = boundary
    }
}

package enum ResourceTransaction {
    package static let boundaries = [
        "resource-precondition", "index-require", "guard-lookup", "preparation", "directory", "stage-node", "staged",
        "journal", "prepare", "backup-move", "backup", "rename", "staging", "publish", "publish-receipt", "receipt",
        "receipt-reread", "receipt-committed", "cleanup-node", "cleanup", "rollback-stage", "rollback-backup",
        "rollback", "index-completion",
    ]
    package static let recoveryBoundaries = ["rollback-stage", "rollback-backup", "rollback"]

    package static func planAncestors(destination: ResourcePath, directory: ResourcePath) throws -> [Ancestor] {
        try union([destination, TransactionLayout.stagingDirectory(directory)])
    }

    package static func journalAncestors(destination: ResourcePath, directory: ResourcePath) throws -> [Ancestor] {
        let resources = try TransactionLayout.resources(directory)
        return try union([destination, resources.staged, resources.backup])
    }

    package static func prepare(
        _ plan: ResourcePlan, store: any StateStore, seam: NativeTransactionSeam = .none,
    ) throws -> Journal {
        let resource = plan.resource
        let destination = resource.destination
        let directory = plan.transactionDirectory
        guard NativeTransactionDirectory.recognized(
            store: store, destination: destination, transactionID: plan.receipt.transactionID, directory: directory,
        ) else {
            throw TargetError(code: .invalidState, message: "planned transaction directory is not recognized")
        }
        guard try planAncestors(destination: destination, directory: directory) == plan.ancestors else {
            throw nativeFailure(.concurrentChange, "resource ancestor changed", destination)
        }
        try NativeObservation.requireRevision(destination, plan.before)
        if plan.requireExisting, plan.before.nodes.isEmpty {
            throw nativeFailure(.concurrentChange, "required existing resource disappeared", destination)
        }
        guard try StateRecords.receipt(store, destination) == plan.previous else {
            throw nativeFailure(.concurrentChange, "receipt changed during preparation", destination)
        }
        if let retained = try records(resource, store: store).preparation {
            throw nativeFailure(.recoveryNeeded, "transaction record retained", retained.directory)
        }
        let preparation: Preparation
        do {
            preparation = try Preparation(
                resource: resource, transactionID: plan.receipt.transactionID, directory: directory,
            )
        } catch let error as FlyrailValueError {
            throw nativeFailure(.invalidState, error.message, destination)
        }
        try StateRecords.write(
            store, key: StateKey.resource(destination, .preparation), value: preparation, expected: nil,
        )
        try seam("preparation")
        if try NativeFilesystem.withName(directory.platformBytes, { Darwin.mkdir($0, 0o700) }) != 0 {
            let code = errno
            release(preparation, store: store)
            if code == EEXIST {
                throw nativeFailure(.recoveryNeeded, "unrecognized transaction data retained", directory)
            }
            throw nativeIO(code, path: directory)
        }
        var staged: [Revision] = []
        let journal: Journal
        do {
            let ancestors = try storage(plan)
            try seam("directory")
            if plan.after != plan.before {
                staged = try [NativeTransactionIO.writeRevision(
                    TransactionLayout.resources(directory).staged, revision: plan.after, seam: seam,
                )]
                try seam("staged")
            }
            let prepared = try journalFor(plan, store: store, ancestors: ancestors, staged: staged.first ?? plan.before)
            staged = [prepared.staged, prepared.published]
            try record(prepared, directory: directory, store: store, seam: seam)
            journal = prepared
        } catch let crash as NativeSimulatedCrash {
            throw crash
        } catch {
            discard(directory, revisions: staged)
            if absent(directory) {
                release(preparation, store: store)
            }
            throw error
        }
        try seam("prepare")
        return journal
    }

    package static func publish(
        _ journal: Journal, store: any StateStore, seam: NativeTransactionSeam = .none,
        committed: () -> Void = {},
    ) throws {
        let destination = journal.resource.destination
        let directory = try transactionDirectory(journal, store: store)
        let (staged, backup) = try TransactionLayout.resources(directory)
        try requireJournal(journal, store: store, directory: directory)
        try validateAncestors(journal, store: store, directory: directory)
        try NativeObservation.requireRevision(destination, journal.before)
        if journal.before != journal.staged {
            try NativeObservation.requireRevision(staged, journal.staged)
            if !journal.before.nodes.isEmpty {
                try seam("backup-move")
                try NativeTransactionIO.move(destination, to: backup, revision: journal.backup)
                try seam("backup")
                try seam("rename")
                try NativeObservation.requireRevision(backup, journal.backup)
            }
            if !journal.staged.nodes.isEmpty {
                try seam("staging")
                try seam("rename")
                try NativeTransactionIO.move(staged, to: destination, revision: journal.staged)
            }
            try NativeObservation.requireRevision(destination, journal.published)
        }
        try seam("publish")
        try validateAncestors(journal, store: store, directory: directory)
        try NativeObservation.requireRevision(destination, journal.published)
        try publishReceipt(journal, store: store, seam: seam, committed: committed)
    }

    package static func publishReceipt(
        _ journal: Journal, store: any StateStore, seam: NativeTransactionSeam = .none,
        committed: () -> Void = {},
    ) throws {
        try seam("publish-receipt")
        let directory = try transactionDirectory(journal, store: store)
        try requireJournal(journal, store: store, directory: directory)
        try layout(directory)
        try validateAncestors(journal, store: store, directory: directory)
        let (staged, backup) = try TransactionLayout.resources(directory)
        let empty = try Revision()
        try NativeObservation.requireRevision(staged, empty)
        try NativeObservation.requireRevision(journal.resource.destination, journal.published)
        try NativeObservation.requireRevision(backup, journal.before == journal.staged ? empty : journal.backup)
        guard try StateRecords.receipt(store, journal.resource.destination) == journal.previous else {
            throw nativeFailure(.concurrentChange, "receipt changed before commit", journal.resource.destination)
        }
        try seam("receipt")
        try StateRecords.write(
            store, key: StateKey.resource(journal.resource.destination, .receipt), value: journal.receipt,
            expected: journal.previous,
        )
        committed()
        try seam("receipt-reread")
        try seam("receipt-committed")
    }

    package static func cleanup(_ journal: Journal, store: any StateStore, seam: NativeTransactionSeam = .none) throws {
        let preparation = try preparation(for: journal, store: store)
        let directory = preparation.directory
        try requireJournal(journal, store: store, directory: directory)
        try layout(directory)
        try validateAncestors(journal, store: store, directory: directory)
        let (staged, backup) = try TransactionLayout.resources(directory)
        for (path, expected) in [
            (staged, [journal.staged, journal.published]),
            (backup, [journal.backup, journal.before]),
        ] {
            let current = try NativeObservation.snapshot(path).revision
            guard !current.nodes.isEmpty else { continue }
            guard expected.contains(where: { current.nodes == Array($0.nodes.prefix(current.nodes.count)) }) else {
                throw nativeFailure(.recoveryNeeded, "unexpected cleanup revision", path)
            }
            try NativeTransactionIO.destroy(path, revision: current, seam: seam)
        }
        try removeDirectories(directory, ignoringFailures: false)
        try seam("cleanup")
        try StateRecords.delete(
            store,
            key: StateKey.resource(journal.resource.destination, .journal),
            expected: journal,
        )
        try StateRecords.delete(
            store, key: StateKey.resource(journal.resource.destination, .preparation), expected: preparation,
        )
    }

    package static func transactionDirectory(_ journal: Journal, store: any StateStore) throws -> ResourcePath {
        try preparation(for: journal, store: store).directory
    }

    private static func union(_ paths: [ResourcePath]) throws -> [Ancestor] {
        var result: [Ancestor] = []
        for path in paths {
            for item in try NativeObservation.ancestors(path) where !result.contains(item) {
                result.append(item)
            }
        }
        return result
    }

    private static func record(
        _ journal: Journal, directory: ResourcePath, store: any StateStore, seam: NativeTransactionSeam,
    ) throws {
        var times = [timespec(tv_sec: 0, tv_nsec: Int(UTIME_NOW)), timespec(tv_sec: 0, tv_nsec: Int(UTIME_NOW))]
        guard try NativeFilesystem.withName(directory.platformBytes, { utimensat(AT_FDCWD, $0, &times, 0) }) == 0 else {
            throw nativeIO(errno, path: directory)
        }
        try seam("journal")
        try StateRecords.write(
            store, key: StateKey.resource(journal.resource.destination, .journal), value: journal, expected: nil,
        )
    }

    private static func storage(_ plan: ResourcePlan) throws -> [Ancestor] {
        let destination = plan.resource.destination
        let directory = plan.transactionDirectory
        try ensurePrivate(directory)
        let created = try TransactionLayout.directories(directory)
        var storage: [(ResourcePath, Revision)] = []
        for path in created.dropFirst() {
            guard try NativeFilesystem.withName(path.platformBytes, { Darwin.mkdir($0, 0o700) }) == 0 else {
                throw nativeIO(errno, path: path)
            }
            let expected = try NativeObservation.snapshot(path).revision
            try ensurePrivate(path)
            storage.append((path, expected))
        }
        let ancestors = try journalAncestors(destination: destination, directory: directory)
        for (path, expected) in storage {
            try NativeObservation.requireRevision(path, expected)
        }
        guard ancestors.filter({ !created.contains($0.path) }) == plan.ancestors else {
            throw nativeFailure(.concurrentChange, "resource ancestor changed", destination)
        }
        return ancestors
    }

    private static func journalFor(
        _ plan: ResourcePlan, store: any StateStore, ancestors: [Ancestor], staged: Revision,
    ) throws -> Journal {
        let resource = plan.resource
        let journal: Journal
        do {
            let receipt = try Receipt(
                resource: resource, transactionID: plan.receipt.transactionID, claims: plan.receipt.claims,
                provenance: plan.receipt.provenance,
                createdDirectories: plan.receipt.createdDirectories.compactMap { marker in
                    staged.nodes.first { exactTextEqual($0.path, marker.path) }
                },
            )
            journal = try Journal(
                resource: resource, previous: plan.previous, receipt: receipt, before: plan.before,
                staged: staged, published: staged, backup: plan.before, ancestors: ancestors,
            )
        } catch let error as FlyrailValueError {
            throw nativeFailure(.invalidState, error.message, resource.destination)
        }
        try validateAncestors(journal, store: store, directory: plan.transactionDirectory)
        try NativeObservation.requireRevision(resource.destination, plan.before)
        return journal
    }

    private static func discard(_ directory: ResourcePath, revisions: [Revision]) {
        do {
            let (staged, _) = try TransactionLayout.resources(directory)
            let current = try NativeObservation.snapshot(staged).revision
            if !current.nodes.isEmpty, revisions.contains(current) {
                try NativeTransactionIO.destroy(staged, revision: current)
            }
            try removeDirectories(directory, ignoringFailures: true)
        } catch {
            return
        }
    }

    private static func release(_ preparation: Preparation, store: any StateStore) {
        let destination = preparation.resource.destination
        do {
            guard try StateRecords.raw(store, key: StateKey.resource(destination, .journal)) == nil else { return }
            try StateRecords.delete(store, key: StateKey.resource(destination, .preparation), expected: preparation)
        } catch {
            return
        }
    }

    private static func requireJournal(_ journal: Journal, store: any StateStore, directory: ResourcePath) throws {
        guard try StateRecords.journal(store, journal.resource.destination) == journal else {
            throw nativeFailure(.recoveryNeeded, "transaction record changed", directory)
        }
    }
}

extension ResourceTransaction {
    static func records(
        _ resource: ResourceRef, store: any StateStore,
    ) throws -> (preparation: Preparation?, journal: Journal?) {
        let destination = resource.destination
        let preparation = try StateRecords.preparation(store, destination)
        if let preparation, !known(preparation, store: store), !absent(preparation.directory) {
            throw unrecognizedDirectory(store)
        }
        let journal = try StateRecords.journal(store, destination)
        if let journal {
            _ = try matching(recognized(preparation, store: store), journal: journal, store: store)
        }
        return (preparation, journal)
    }

    static func preparation(for journal: Journal, store: any StateStore) throws -> Preparation {
        let preparation = try StateRecords.preparation(store, journal.resource.destination)
        return try matching(recognized(preparation, store: store), journal: journal, store: store)
    }

    static func known(_ preparation: Preparation, store: any StateStore) -> Bool {
        NativeTransactionDirectory.recognized(
            store: store, destination: preparation.resource.destination, transactionID: preparation.transactionID,
            directory: preparation.directory,
        )
    }

    static func unrecognizedDirectory(_ store: any StateStore) -> TargetError {
        StateRecords.failure(store, .recoveryNeeded, "transaction record names an unrecognized directory")
    }

    static func layout(_ directory: ResourcePath) throws {
        try NativeAuthority.managedSpelling(directory)
        for child in try children(directory) {
            guard [TransactionLayout.staging, TransactionLayout.backup]
                .contains(String(decoding: child.name, as: UTF8.self))
            else {
                throw nativeFailure(.recoveryNeeded, "unknown transaction data", child.path)
            }
            guard let metadata = try metadata(child.path) else { throw nativeIO(ENOENT, path: child.path) }
            try NativeFilesystem.ordinary(metadata, path: child.path)
            guard metadata.st_mode & S_IFMT == S_IFDIR else {
                throw nativeFailure(.unsafePath, "expected a directory", child.path)
            }
            guard try children(child.path).allSatisfy({ $0.name == Array(TransactionLayout.resource.utf8) }) else {
                throw nativeFailure(.recoveryNeeded, "unknown transaction data", child.path)
            }
        }
    }

    static func validateAncestors(_ journal: Journal, store: any StateStore, directory: ResourcePath) throws {
        let resource = journal.resource
        let current = try journalAncestors(destination: resource.destination, directory: directory)
        let retained = journal.ancestors.filter { current.contains($0) || !contains(directory, $0.path) }
        guard current == retained else {
            throw nativeFailure(.concurrentChange, "resource ancestor changed", resource.destination)
        }
        try ensurePrivateStorage(directory)
        let conflicts = try ResourceAdmission.hierarchyConflicts(
            ResourceAdmission.checkedAuthority(resource), store: store, wholeTree: resource.kind != .document,
        )
        if let first = conflicts.first {
            throw nativeFailure(.conflict, "resource hierarchy reservation changed", first)
        }
    }

    static func removeDirectories(_ directory: ResourcePath, ignoringFailures: Bool) throws {
        for path in try TransactionLayout.directories(directory).dropFirst() + [directory] {
            guard try NativeFilesystem.withName(path.platformBytes, { Darwin.rmdir($0) }) != 0 else { continue }
            let code = errno
            if code != ENOENT, !ignoringFailures {
                throw nativeIO(code, path: path)
            }
        }
    }

    static func ensurePrivateStorage(_ directory: ResourcePath) throws {
        for path in try TransactionLayout.directories(directory).filter(pathExists) {
            try ensurePrivate(path)
        }
    }

    static func ensurePrivate(_ path: ResourcePath) throws {
        guard let parent = try NativeFilesystem.parent(path) else { throw nativeIO(ENOENT, path: path) }
        let descriptor = try NativeFilesystem.open(
            parent.descriptor.raw,
            name: parent.name,
            directory: true,
            path: path,
        )
        try NativeSecurity.ensurePrivate(descriptor, path: path)
    }

    static func metadata(_ path: ResourcePath) throws -> stat? {
        var result = stat()
        guard try NativeFilesystem.withName(path.platformBytes, { lstat($0, &result) }) == 0 else {
            let code = errno
            guard code == ENOENT else { throw nativeIO(code, path: path) }
            return nil
        }
        return result
    }

    static func pathExists(_ path: ResourcePath) throws -> Bool {
        try metadata(path) != nil
    }

    private static func absent(_ path: ResourcePath) -> Bool {
        var result = stat()
        return (try? NativeFilesystem.withName(path.platformBytes) { lstat($0, &result) == 0 ? 0 : errno }) == ENOENT
    }

    private static func recognized(_ preparation: Preparation?, store: any StateStore) throws -> Preparation? {
        if let preparation, !known(preparation, store: store) {
            throw unrecognizedDirectory(store)
        }
        return preparation
    }

    private static func matching(
        _ preparation: Preparation?, journal: Journal, store: any StateStore,
    ) throws -> Preparation {
        guard let preparation, preparation.resource == journal.resource,
              preparation.transactionID == journal.receipt.transactionID
        else {
            throw StateRecords.failure(store, .invalidState, "transaction record has no matching preparation")
        }
        return preparation
    }

    private static func contains(_ directory: ResourcePath, _ path: ResourcePath) -> Bool {
        path == directory || path.platformBytes.starts(with: directory.platformBytes + [47])
    }

    private static func children(_ directory: ResourcePath) throws -> [(name: [UInt8], path: ResourcePath)] {
        let raw = try NativeFilesystem.withName(directory.platformBytes) {
            Darwin.open($0, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        }
        guard raw >= 0 else {
            let code = errno
            guard code == ENOENT else { throw nativeIO(code, path: directory) }
            return []
        }
        let descriptor = NativeDescriptor(raw)
        return try NativeFilesystem.names(descriptor, path: directory).map {
            try ($0, NativeFilesystem.child(directory, $0))
        }
    }
}

package enum TransactionLayout {
    package static let staging = "staging"
    package static let backup = "backup"
    package static let resource = "resource"

    package static func stagingDirectory(_ directory: ResourcePath) throws -> ResourcePath {
        try appendPath(directory, staging)
    }

    package static func directories(_ directory: ResourcePath) throws -> [ResourcePath] {
        try [directory, stagingDirectory(directory), appendPath(directory, backup)]
    }

    package static func resources(_ directory: ResourcePath) throws -> (staged: ResourcePath, backup: ResourcePath) {
        try (appendPath(directory, "\(staging)/\(resource)"), appendPath(directory, "\(backup)/\(resource)"))
    }
}
