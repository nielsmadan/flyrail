import Darwin
import Foundation

public extension ResourceAuthority {
    init(destination: ResourcePath) throws {
        try self.init(recordedDestination: NativeAuthority.canonicalDestination(destination))
    }
}

package enum NativeAuthority {
    package static func reference(_ destination: ResourcePath, kind: ResourceKind) throws -> ResourceRef {
        try ResourceRef(destination: canonicalDestination(destination), kind: kind)
    }

    package static func canonicalDestination(_ path: ResourcePath) throws -> ResourcePath {
        guard !path.platformBytes.isEmpty else {
            throw FlyrailValueError("resource destination must be a nonempty string path without NUL")
        }
        let selected = try absolute(path)
        let parts = try NativeFilesystem.components(selected)
        guard !parts.isEmpty else {
            throw FlyrailValueError("resource destination cannot be a root or contain traversal")
        }
        try validateDestination(selected)
        let ancestors = try NativeObservation.ancestors(selected)
        let root = try ResourcePath("/")
        let raw = Darwin.open("/", O_SEARCH | O_NOFOLLOW | O_CLOEXEC)
        guard raw >= 0 else { throw nativeIO(errno, path: root) }
        var descriptor = NativeDescriptor(raw)
        var current = root
        var missing: [[UInt8]] = []
        for part in parts.dropLast() {
            if !missing.isEmpty {
                missing.append(part); continue
            }
            let candidate = try NativeFilesystem.child(current, part)
            guard let metadata = try NativeFilesystem.metadata(descriptor.raw, name: part, path: candidate) else {
                missing.append(part)
                continue
            }
            try safe(metadata, path: candidate)
            guard metadata.st_mode & S_IFMT == S_IFDIR else {
                throw nativeFailure(.unsafePath, "resource ancestor must be a directory", candidate)
            }
            let opened = try NativeFilesystem.open(
                descriptor.raw,
                name: part,
                directory: true,
                path: candidate,
                flags: O_SEARCH,
            )
            guard try NativeFilesystem.identity(descriptor.raw, name: part, descriptor: opened, path: candidate) else {
                throw nativeFailure(.concurrentChange, "resource parent changed during canonicalization", candidate)
            }
            descriptor = opened
            current = candidate
        }
        let physical = try NativeFilesystem.physicalPath(descriptor, path: current)
        let opened = try NativeFilesystem.stat(descriptor, path: current)
        guard let verified = try NativeFilesystem.metadata(physical),
              opened.st_dev == verified.st_dev, opened.st_ino == verified.st_ino
        else { throw nativeFailure(.concurrentChange, "resource parent changed during canonicalization", current) }
        let selectedName = missing.first ?? parts[parts.count - 1]
        let name = try canonicalChild(descriptor, parent: physical, name: selectedName)
        var result = try NativeFilesystem.child(physical, name)
        if !missing.isEmpty {
            for part in missing.dropFirst() + [parts[parts.count - 1]] {
                result = try NativeFilesystem.child(result, part)
            }
        }
        try validateDestination(result)
        guard try NativeObservation.ancestors(selected) == ancestors else {
            throw nativeFailure(.concurrentChange, "resource parent changed during canonicalization", selected)
        }
        return result
    }

    package static func absolute(_ path: ResourcePath) throws -> ResourcePath {
        if path.platformBytes.first == 47 {
            return path
        }
        var bytes = [CChar](repeating: 0, count: Int(MAXPATHLEN))
        guard getcwd(&bytes, bytes.count) != nil else { throw nativeIO(errno, path: path) }
        let cwd = bytes.prefix(while: { $0 != 0 }).map { UInt8(bitPattern: $0) }
        return try ResourcePath(platformBytes: cwd + [47] + path.platformBytes)
    }

    private static func validateDestination(_ path: ResourcePath) throws {
        for bytes in try NativeFilesystem.components(path) {
            guard let name = String(bytes: bytes, encoding: .utf8) else {
                throw nativeFailure(.unsafePath, "nonportable resource path", path)
            }
            try validateRelativePath(name, label: "resource path component")
            if NativeTransactionDirectory.isTransactionName(name) {
                throw FlyrailValueError("resource destination enters reserved management state")
            }
        }
    }

    private static func canonicalChild(
        _ descriptor: NativeDescriptor, parent: ResourcePath, name: [UInt8],
    ) throws -> [UInt8] {
        let candidate = try NativeFilesystem.child(parent, name)
        guard let text = String(bytes: name, encoding: .utf8) else {
            throw nativeFailure(.unsafePath, "nonportable resource path", candidate)
        }
        let matches = try matchingNames(descriptor, parent: parent, name: text)
        guard matches.count <= 1
        else { throw nativeFailure(.unsafePath, "portable resource path collision", candidate) }
        guard let physical = matches.first else { return name }
        let physicalPath = try NativeFilesystem.child(parent, physical)
        guard let metadata = try NativeFilesystem.metadata(descriptor.raw, name: physical, path: physicalPath) else {
            throw nativeFailure(.concurrentChange, "listed path disappeared during inspection", physicalPath)
        }
        try safe(metadata, path: physicalPath)
        if physical != name {
            guard let alias = try NativeFilesystem.metadata(descriptor.raw, name: name, path: candidate) else {
                throw nativeFailure(.unsafePath, "resource path has a conflicting portable spelling", candidate)
            }
            guard alias.st_dev == metadata.st_dev, alias.st_ino == metadata.st_ino else {
                throw nativeFailure(.unsafePath, "resource path alias is ambiguous", candidate)
            }
        }
        guard try matchingNames(descriptor, parent: parent, name: text) == matches else {
            throw nativeFailure(.concurrentChange, "selected directory entries changed", candidate)
        }
        return physical
    }

    package static func matchingNames(
        _ descriptor: NativeDescriptor, parent: ResourcePath, name: String,
    ) throws -> [[UInt8]] {
        let key = portablePathKey(name)
        return try NativeFilesystem.names(descriptor, path: parent).filter { bytes in
            String(bytes: bytes, encoding: .utf8).map { portablePathKey($0) == key } ?? false
        }
    }

    package static func managedSpelling(_ path: ResourcePath) throws {
        guard let parent = try NativeFilesystem.parent(path) else { return }
        let parentPath = try NativeFilesystem.physicalPath(parent.descriptor, path: path)
        let matches = try matchingNames(
            parent.descriptor, parent: parentPath, name: String(decoding: parent.name, as: UTF8.self),
        )
        guard matches.count <= 1 else { throw nativeFailure(.unsafePath, "portable filename collision", path) }
        if let first = matches.first, first != parent.name {
            throw nativeFailure(.unsafePath, "managed directory spelling mismatch", path)
        }
        if let metadata = try NativeFilesystem.metadata(parent.descriptor.raw, name: parent.name, path: path) {
            try NativeFilesystem.ordinary(metadata, path: path)
            guard metadata.st_mode & S_IFMT == S_IFDIR else {
                throw nativeFailure(.unsafePath, "expected a directory", path)
            }
        }
        try parent.validate()
    }

    package static func safe(_ metadata: stat, path: ResourcePath) throws {
        let type = metadata.st_mode & S_IFMT
        guard type == S_IFREG || type == S_IFDIR else {
            throw nativeFailure(
                .unsafePath,
                "resource links and special files are unsupported: \(path.displayString)",
                path,
            )
        }
        guard type != S_IFREG || metadata.st_nlink == 1 else {
            throw nativeFailure(.unsafePath, "hard-linked resources are ambiguous: \(path.displayString)", path)
        }
    }
}
