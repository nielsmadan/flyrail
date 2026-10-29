import Darwin
import Foundation

package final class NativeDirectoryLock {
    package let stateRoot: ResourcePath
    package let createdDirectories: [Ancestor]
    package let directory: NativeDescriptor
    private let parent: NativeParent
    private let stateEvidence: Ancestor
    private let descriptor: NativeDescriptor
    private let lockStamp: NativeStamp
    private let lockSecurity: Data

    package init(stateRoot: ResourcePath, timeout: Duration) throws {
        guard timeout >= .zero else { throw FlyrailValueError("lock timeout must be finite and nonnegative") }
        self.stateRoot = stateRoot
        createdDirectories = try NativeAdmission.createPrivateDirectories(stateRoot)
        guard let parent = try NativeFilesystem.parent(stateRoot) else { throw nativeIO(ENOENT, path: stateRoot) }
        self.parent = parent
        directory = try NativeFilesystem.open(
            parent.descriptor.raw,
            name: parent.name,
            directory: true,
            path: stateRoot,
        )
        try NativeSecurity.ensurePrivate(directory, path: stateRoot)
        stateEvidence = try NativeFilesystem.ancestor(directory, path: stateRoot)
        let path = try appendPath(stateRoot, "lock")
        let raw = openat(directory.raw, "lock", O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
        if raw >= 0 {
            descriptor = NativeDescriptor(raw)
        } else if errno == EEXIST {
            descriptor = try NativeFilesystem.open(
                directory.raw,
                name: Array("lock".utf8),
                directory: false,
                path: path,
                flags: O_RDWR,
            )
        } else {
            throw nativeIO(errno, path: path)
        }
        try NativeSecurity.ensureCoordinationLock(descriptor, path: path)
        let metadata = try NativeFilesystem.stat(descriptor, path: path)
        guard metadata.st_mode & S_IFMT == S_IFREG else {
            throw nativeFailure(.invalidState, "metadata must be a regular file", path)
        }
        lockStamp = NativeStamp(metadata)
        lockSecurity = try NativeSecurity.capture(descriptor.raw, path: path)
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while flock(descriptor.raw, LOCK_EX | LOCK_NB) != 0 {
            let code = errno
            guard [EACCES, EAGAIN, EDEADLK, EINTR].contains(code) else { throw nativeIO(code, path: path) }
            guard clock.now < deadline else { throw nativeFailure(.busy, "target lock is busy", path) }
            let remaining = clock.now.duration(to: deadline)
            let seconds = min(
                0.02,
                Double(remaining.components.seconds) + Double(remaining.components.attoseconds) / 1e18,
            )
            Thread.sleep(forTimeInterval: max(0, seconds))
        }
        try validate()
    }

    deinit { flock(descriptor.raw, LOCK_UN) }

    package func validate() throws {
        try parent.validate()
        guard try NativeFilesystem.identity(
            parent.descriptor.raw, name: parent.name, descriptor: directory, path: stateRoot,
        ), try NativeFilesystem.ancestor(directory, path: stateRoot) == stateEvidence else {
            throw nativeFailure(.concurrentChange, "resource ancestor changed", stateRoot)
        }
        try NativeSecurity.ensurePrivate(directory, path: stateRoot)
        let path = try appendPath(stateRoot, "lock")
        guard try NativeFilesystem.identity(
            directory.raw, name: Array("lock".utf8), descriptor: descriptor, path: path,
        ), try NativeStamp(NativeFilesystem.stat(descriptor, path: path)) == lockStamp,
        try NativeSecurity.capture(descriptor.raw, path: path) == lockSecurity else {
            throw nativeFailure(.concurrentChange, "lock file changed", path)
        }
        try NativeSecurity.ensureCoordinationLock(descriptor, path: path)
    }
}

package struct NativeResourceObservation: Sendable, Equatable {
    package let revision: Revision
    package let snapshot: NativeResourceSnapshot
    package let state: NativeResourceSnapshot
    package let receipt: Receipt?
    package let recovery: [ResourcePath]
    package let ancestors: [Ancestor]
}

package struct NativeIndexObservation: Sendable, Equatable {
    package let current: Index?
    package let prepared: Index?
    package let snapshot: NativeResourceSnapshot
}

package enum NativeAdmission {
    private static let stateEntries: Set<String> = [
        "lock", "authority.json", "receipt.json", "transaction.json", "staging", "backup",
        "authority.json.next", "receipt.json.next", "transaction.json.next",
    ]

    package static func reserve(_ resource: ResourceRef, timeout: Duration) throws -> NativeDirectoryLock {
        try NativeDirectoryLock(stateRoot: resource.stateRoot, timeout: timeout)
    }

    package static func reserveIndex(_ target: InstallationTarget, timeout: Duration) throws -> NativeDirectoryLock {
        try NativeDirectoryLock(stateRoot: target.indexRoot, timeout: timeout)
    }

    package static func createPrivateDirectories(_ path: ResourcePath) throws -> [Ancestor] {
        try NativeAuthority.managedSpelling(path)
        var created: [Ancestor] = []
        if try NativeFilesystem.parent(path) == nil {
            let parts = try NativeFilesystem.components(path)
            let parentPath = try ResourcePath(platformBytes: [47] + parts.dropLast().joined(separator: [47]))
            created += try createPrivateDirectories(parentPath)
        }
        guard let parent = try NativeFilesystem.parent(path) else { throw nativeIO(ENOENT, path: path) }
        if try NativeFilesystem.metadata(parent.descriptor.raw, name: parent.name, path: path) == nil {
            let result = try NativeFilesystem.withName(parent.name) { mkdirat(parent.descriptor.raw, $0, 0o700) }
            if result != 0, errno != EEXIST {
                throw nativeIO(errno, path: path)
            }
            if result == 0 {
                let directory = try NativeFilesystem.open(
                    parent.descriptor.raw,
                    name: parent.name,
                    directory: true,
                    path: path,
                )
                try NativeSecurity.ensurePrivate(directory, path: path)
                try created.append(NativeFilesystem.ancestor(directory, path: path))
            }
        }
        try NativeAuthority.managedSpelling(path)
        let directory = try NativeFilesystem.open(parent.descriptor.raw, name: parent.name, directory: true, path: path)
        try NativeSecurity.ensurePrivate(directory, path: path)
        try parent.validate()
        return created
    }

    package static func validateKind(_ resource: ResourceRef) throws {
        guard let metadata = try NativeFilesystem.metadata(resource.destination) else { return }
        try NativeFilesystem.ordinary(metadata, path: resource.destination)
        guard (metadata.st_mode & S_IFMT == S_IFREG) == (resource.kind == .document) else {
            throw nativeFailure(.unsafePath, "resource kind disagrees with existing destination", resource.destination)
        }
    }

    package static func validateState(_ resource: ResourceRef) throws -> (NativeResourceSnapshot, [ResourcePath]) {
        try NativeAuthority.managedSpelling(resource.stateRoot)
        let snapshot = try NativeObservation.snapshot(
            resource.stateRoot, privateStorage: true, resourcePayloads: true,
        )
        var recovery: [ResourcePath] = []
        for node in snapshot.revision.nodes.dropFirst() where !node.path.contains("/") {
            let child = try appendPath(resource.stateRoot, node.path)
            guard stateEntries.contains(node.path) else {
                throw nativeFailure(.invalidState, "unknown resource state entry", child)
            }
            if node.path == "staging" || node.path == "backup" {
                guard node.data == nil else { throw nativeFailure(.unsafePath, "expected a directory", child) }
                if snapshot.revision.nodes.contains(where: { $0.path.hasPrefix(node.path + "/") }) {
                    recovery.append(child)
                }
            } else {
                guard node.data != nil else {
                    throw nativeFailure(.invalidState, "metadata must be a regular file", child)
                }
            }
            if node.path == "transaction.json" || node.path.hasSuffix(".next") {
                recovery.append(child)
            }
        }
        let header = try appendPath(resource.stateRoot, "authority.json")
        if let recorded = try NativeObservation.read(header, as: ResourceRef.self, tag: "ResourceRef"),
           recorded != resource
        {
            throw nativeFailure(.invalidState, "authority identity disagrees", resource.stateRoot)
        }
        try NativeObservation.require(resource.stateRoot, snapshot)
        return (snapshot, recovery)
    }

    package static func admit(_ resource: ResourceRef, lock: NativeDirectoryLock, allowRecovery: Bool = true) throws {
        guard lock.stateRoot == resource.stateRoot else {
            throw nativeFailure(.invalidState, "resource lock disagrees", resource.stateRoot)
        }
        try lock.validate()
        guard try NativeAuthority.canonicalDestination(resource.destination) == resource.destination else {
            throw nativeFailure(.invalidState, "resource canonical destination changed", resource.destination)
        }
        _ = try validateState(resource)
        try validateKind(resource)
        try initializeHeader(resource, lock: lock, allowRecovery: allowRecovery)
        let authority = try resource.recordedAuthority
        let conflicts = try NativeAuthority.hierarchyConflicts(authority, wholeTree: resource.kind != .document)
        if let first = conflicts.first {
            throw nativeFailure(.conflict, "ancestor or descendant resource fence conflicts", first)
        }
        _ = try NativeObservation.snapshot(resource.destination)
        try lock.validate()
    }

    private static func initializeHeader(
        _ resource: ResourceRef,
        lock: NativeDirectoryLock,
        allowRecovery: Bool,
    ) throws {
        let header = try appendPath(resource.stateRoot, "authority.json")
        let prepared = try appendPath(resource.stateRoot, "authority.json.next")
        if !allowRecovery, try NativeFilesystem.metadata(prepared) != nil {
            throw nativeFailure(.recoveryNeeded, "preview requires recovery and re-observation", prepared)
        }
        if try NativeObservation.read(header, as: ResourceRef.self, tag: "ResourceRef") != nil {
            return
        }
        if let recorded = try NativeObservation.read(prepared, as: ResourceRef.self, tag: "ResourceRef") {
            guard allowRecovery else {
                throw nativeFailure(.recoveryNeeded, "preview requires recovery and re-observation", prepared)
            }
            guard recorded == resource else {
                throw nativeFailure(.recoveryNeeded, "unrecognized metadata preparation", prepared)
            }
        } else {
            let payload = try StateCodec.encode(resource)
            let raw = openat(
                lock.directory.raw,
                "authority.json.next",
                O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
                0o600,
            )
            guard raw >= 0 else {
                if errno ==
                    EEXIST
                {
                    throw nativeFailure(.recoveryNeeded, "unrecognized metadata preparation", prepared)
                }
                throw nativeIO(errno, path: prepared)
            }
            let descriptor = NativeDescriptor(raw)
            try NativeSecurity.ensurePrivate(descriptor, path: prepared)
            try payload.withUnsafeBytes { bytes in
                var offset = 0
                while offset < bytes.count {
                    let written = Darwin.write(raw, bytes.baseAddress?.advanced(by: offset), bytes.count - offset)
                    if written < 0 {
                        if errno == EINTR {
                            continue
                        }
                        throw nativeIO(errno, path: prepared)
                    }
                    guard written > 0 else { throw nativeIO(EIO, path: prepared) }
                    offset += written
                }
            }
            guard try NativeFilesystem.identity(
                lock.directory.raw, name: Array("authority.json.next".utf8), descriptor: descriptor, path: prepared,
            ) else { throw nativeFailure(.concurrentChange, "metadata staging changed", prepared) }
            try NativeSecurity.ensurePrivate(descriptor, path: prepared)
        }
        let before = try NativeObservation.snapshot(prepared, privateStorage: true)
        guard try NativeObservation.read(prepared, as: ResourceRef.self, tag: "ResourceRef") == resource else {
            throw nativeFailure(.concurrentChange, "metadata staging changed", prepared)
        }
        try lock.validate()
        try NativeObservation.require(prepared, before)
        guard renameatx_np(
            lock.directory.raw,
            "authority.json.next",
            lock.directory.raw,
            "authority.json",
            UInt32(RENAME_EXCL),
        ) == 0 else {
            throw nativeIO(errno, path: header)
        }
        try NativeObservation.requireRevision(header, before.revision)
        try lock.validate()
    }

    package static func observe(
        _ resource: ResourceRef, lock: NativeDirectoryLock? = nil,
    ) throws -> NativeResourceObservation {
        if let lock {
            guard lock.stateRoot == resource.stateRoot else {
                throw nativeFailure(.invalidState, "resource lock disagrees", resource.stateRoot)
            }
            try lock.validate()
        }
        guard try NativeAuthority.canonicalDestination(resource.destination) == resource.destination else {
            throw nativeFailure(.invalidState, "resource canonical destination changed", resource.destination)
        }
        try validateKind(resource)
        let ancestors = try NativeObservation.ancestors(appendPath(resource.stateRoot, "transaction.json"))
        let (state, recovery) = try validateState(resource)
        let snapshot = try NativeObservation.snapshot(resource.destination)
        let receipt = try NativeObservation.read(
            appendPath(resource.stateRoot, "receipt.json"), as: Receipt.self, tag: "Receipt",
        )
        if let receipt, receipt.resource != resource {
            throw nativeFailure(.invalidState, "receipt resource disagrees", resource.stateRoot)
        }
        try NativeObservation.require(resource.stateRoot, state)
        try NativeObservation.require(resource.destination, snapshot)
        guard try NativeObservation.ancestors(appendPath(resource.stateRoot, "transaction.json")) == ancestors else {
            throw nativeFailure(.concurrentChange, "resource ancestor changed", resource.stateRoot)
        }
        try lock?.validate()
        return NativeResourceObservation(
            revision: snapshot.revision, snapshot: snapshot, state: state, receipt: receipt,
            recovery: recovery, ancestors: ancestors,
        )
    }

    package static func observeIndex(_ target: InstallationTarget, bundleID: String) throws -> NativeIndexObservation {
        try validateIdentifier(bundleID, label: "bundle id")
        try NativeAuthority.managedSpelling(target.indexRoot)
        let snapshot = try NativeObservation.snapshot(target.indexRoot, privateStorage: true)
        for node in snapshot.revision.nodes.dropFirst() {
            let path = try appendPath(target.indexRoot, node.path)
            guard ["lock", "index.json", "index.json.next"].contains(node.path) else {
                throw nativeFailure(.invalidState, "unknown installation index entry", path)
            }
            guard node.data != nil else { throw nativeFailure(.invalidState, "expected a regular file", path) }
        }
        let current = try NativeObservation.read(
            appendPath(target.indexRoot, "index.json"),
            as: Index.self,
            tag: "Index",
        )
        let prepared = try NativeObservation.read(
            appendPath(target.indexRoot, "index.json.next"),
            as: Index.self,
            tag: "Index",
        )
        for index in [current, prepared].compactMap(\.self) {
            guard index.bundleID == bundleID, try index.contextDigest == target.contextDigest else {
                throw nativeFailure(.invalidState, "installation index context disagrees", target.indexRoot)
            }
        }
        try NativeObservation.require(target.indexRoot, snapshot)
        return NativeIndexObservation(current: current, prepared: prepared, snapshot: snapshot)
    }

    package static func validateIndexSeparation(_ target: InstallationTarget, resources: [ResourceRef]) throws {
        for resource in resources {
            for path in [resource.destination, resource.stateRoot] {
                let left = try NativeFilesystem.components(path)
                let right = try NativeFilesystem.components(target.indexRoot)
                let count = min(left.count, right.count)
                if left.prefix(count).elementsEqual(right.prefix(count)) {
                    throw FlyrailValueError("installation index overlaps a resource or authority")
                }
            }
        }
    }
}
