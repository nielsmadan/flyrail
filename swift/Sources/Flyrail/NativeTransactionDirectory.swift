import Darwin
import Foundation

package enum NativeTransactionDirectory {
    private static let maximumLinks = 40

    package static func physicalLocation(_ path: ResourcePath) throws -> ResourcePath {
        var existing = try resolve(NativeAuthority.absolute(path).platformBytes)
        var missing: [[UInt8]] = []
        while !isDirectory(existing), let slash = existing.lastIndex(of: 47), existing != [47] {
            missing.insert(Array(existing[(slash + 1)...]), at: 0)
            existing = slash == 0 ? [47] : Array(existing[..<slash])
        }
        let directory = try ResourcePath(platformBytes: existing)
        let raw = try NativeFilesystem.withName(existing) {
            Darwin.open($0, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        }
        guard raw >= 0 else { throw nativeIO(errno, path: directory) }
        let descriptor = NativeDescriptor(raw)
        let opened = try NativeFilesystem.stat(descriptor, path: directory)
        var physical = try NativeFilesystem.physicalPath(descriptor, path: directory)
        var current = stat()
        guard try NativeFilesystem.withName(physical.platformBytes, { lstat($0, &current) }) == 0 else {
            throw nativeIO(errno, path: physical)
        }
        guard opened.st_dev == current.st_dev, opened.st_ino == current.st_ino else {
            throw nativeFailure(.unsafePath, "resource parent changed during canonicalization", directory)
        }
        for name in missing {
            physical = try NativeFilesystem.child(physical, name)
        }
        return physical
    }

    package static func storePaths(_ store: any StateStore) throws -> [ResourcePath] {
        try [store.location, store.stagingDirectory].compactMap(\.self).map(physicalLocation)
    }

    package static func createParents(of path: ResourcePath) throws -> [Ancestor] {
        let parts = try NativeFilesystem.components(path)
        var created: [Ancestor] = []
        for count in stride(from: 1, to: parts.count, by: 1) {
            let ancestor = try ResourcePath(platformBytes: [47] + parts.prefix(count).joined(separator: [47]))
            guard let parent = try NativeFilesystem.parent(ancestor) else { throw nativeIO(ENOENT, path: ancestor) }
            if try NativeFilesystem.metadata(parent.descriptor.raw, name: parent.name, path: ancestor) != nil {
                continue
            }
            let result = try NativeFilesystem.withName(parent.name) { mkdirat(parent.descriptor.raw, $0, 0o700) }
            if result != 0 {
                let code = errno
                guard code == EEXIST else { throw nativeIO(code, path: ancestor) }
                continue
            }
            let directory = try NativeFilesystem.open(
                parent.descriptor.raw, name: parent.name, directory: true, path: ancestor,
            )
            try created.append(NativeFilesystem.ancestor(directory, path: ancestor))
        }
        return created
    }

    package static func createStoreParents(_ store: any StateStore) throws -> [Ancestor] {
        var created: [Ancestor] = []
        if let location = store.location {
            created += try createParents(of: physicalLocation(location))
        }
        if let staging = store.stagingDirectory {
            created += try createParents(of: appendPath(physicalLocation(staging), "transaction"))
        }
        return created
    }

    package static func isTransactionName(_ name: String) -> Bool {
        let folded = portablePathKey(name)
        return folded.hasPrefix(".flyrail-") && folded.hasSuffix(".txn")
    }

    package static func sameMount(_ first: ResourcePath, _ second: ResourcePath) -> Bool {
        guard let left = device(first.platformBytes), let right = device(second.platformBytes) else { return false }
        return left == right
    }

    package static func staged(store: any StateStore, destination: ResourcePath,
                               transactionID: String) -> ResourcePath?
    {
        guard let staging = store.stagingDirectory,
              let root = try? physicalLocation(staging),
              let key = try? StateKey.destination(destination)
        else { return nil }
        return try? NativeFilesystem.child(root, Array("\(key)-\(transactionID)".utf8))
    }

    package static func choose(
        store: any StateStore, destination: ResourcePath, transactionID: String,
    ) throws -> ResourcePath {
        let fallback: ResourcePath
        do {
            fallback = try transactionPath(destination: destination, transactionID: transactionID)
        } catch let error as FlyrailValueError {
            throw TargetError(code: .invalidState, message: error.message)
        }
        guard let selected = staged(store: store, destination: destination, transactionID: transactionID),
              let left = parent(selected), let right = parent(destination), sameMount(left, right)
        else { return fallback }
        return selected
    }

    package static func recognized(
        store: any StateStore, destination: ResourcePath, transactionID: String, directory: ResourcePath,
    ) -> Bool {
        if (try? transactionPath(destination: destination, transactionID: transactionID)) == directory {
            return true
        }
        return staged(store: store, destination: destination, transactionID: transactionID) == directory
    }

    package static func entries(inside destination: ResourcePath) throws -> [ResourcePath] {
        guard let parent = try NativeFilesystem.parent(destination), let metadata = try NativeFilesystem.metadata(
            parent.descriptor.raw,
            name: parent.name,
            path: destination,
        )
        else { throw nativeIO(ENOENT, path: destination) }
        try NativeAuthority.safe(metadata, path: destination)
        guard metadata.st_mode & S_IFMT == S_IFDIR else {
            throw nativeFailure(.unsafePath, "whole-tree resource must be a directory", destination)
        }
        var pending = [destination]
        var entries: [ResourcePath] = []
        while let directory = pending.popLast() {
            guard let location = try NativeFilesystem.parent(directory) else { throw nativeIO(ENOENT, path: directory) }
            let descriptor = try NativeFilesystem.open(
                location.descriptor.raw, name: location.name, directory: true, path: directory,
            )
            for name in try NativeFilesystem.names(descriptor, path: directory) {
                let child = try NativeFilesystem.child(directory, name)
                if isTransactionName(String(decoding: name, as: UTF8.self)) {
                    entries.append(child)
                    continue
                }
                guard let metadata = try NativeFilesystem.metadata(descriptor.raw, name: name, path: child) else {
                    throw nativeIO(ENOENT, path: child)
                }
                try NativeAuthority.safe(metadata, path: child)
                if metadata.st_mode & S_IFMT == S_IFDIR {
                    pending.append(child)
                }
            }
        }
        return entries
    }

    private static func parent(_ path: ResourcePath) -> ResourcePath? {
        let bytes = path.platformBytes
        guard let slash = bytes.lastIndex(of: 47), bytes != [47] else { return nil }
        return try? ResourcePath(platformBytes: slash == 0 ? [47] : Array(bytes[..<slash]))
    }

    private static func device(_ bytes: [UInt8]) -> dev_t? {
        var candidate = bytes
        while true {
            var metadata = stat()
            guard let status = try? NativeFilesystem.withName(candidate, { fstatat(AT_FDCWD, $0, &metadata, 0) }) else {
                return nil
            }
            if status == 0 {
                return metadata.st_dev
            }
            guard errno == ENOENT, let slash = candidate.lastIndex(of: 47), candidate != [47] else { return nil }
            candidate = slash == 0 ? [47] : Array(candidate[..<slash])
        }
    }

    private static func components(_ bytes: [UInt8]) -> [[UInt8]?] {
        let names: [[UInt8]?] = bytes.split(separator: 47).map(Array.init).filter { $0 != [46] }
        return bytes.first == 47 ? [nil] + names : names
    }

    private static func resolve(_ bytes: [UInt8]) -> [UInt8] {
        var pending = components(bytes)
        var links = 0
        var resolved: [UInt8] = [47]
        while !pending.isEmpty {
            guard let name = pending.removeFirst() else {
                resolved = [47]
                continue
            }
            if name == [46, 46] {
                if let slash = resolved.lastIndex(of: 47) {
                    resolved = slash == 0 ? [47] : Array(resolved[..<slash])
                }
                continue
            }
            let candidate = resolved == [47] ? resolved + name : resolved + [47] + name
            if links < maximumLinks, let target = link(candidate) {
                links += 1
                pending.insert(contentsOf: components(target), at: 0)
            } else {
                resolved = candidate
            }
        }
        return resolved
    }

    private static func link(_ path: [UInt8]) -> [UInt8]? {
        var metadata = stat()
        guard (try? NativeFilesystem.withName(path, { lstat($0, &metadata) })) == 0,
              metadata.st_mode & S_IFMT == S_IFLNK,
              let resolved = try? ResourcePath(platformBytes: path)
        else { return nil }
        return try? NativeFilesystem.readlink(AT_FDCWD, name: path, path: resolved)
    }

    private static func isDirectory(_ path: [UInt8]) -> Bool {
        var metadata = stat()
        return (try? NativeFilesystem.withName(path) { fstatat(AT_FDCWD, $0, &metadata, 0) }) == 0 &&
            metadata.st_mode & S_IFMT == S_IFDIR
    }
}
