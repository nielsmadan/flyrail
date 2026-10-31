import Darwin
import Foundation

package struct NativeResourceSnapshot: Sendable, Equatable {
    package let revision: Revision
    package let ancestors: [Ancestor]
    package let stamps: [NativeStamp]

    package func hasReplacementEvidence(after journal: NativeStamp) -> Bool {
        stamps.contains { stamp in
            (stamp.modifiedSeconds, stamp.modifiedNanos) > (journal.modifiedSeconds, journal.modifiedNanos) ||
                (stamp.createdSeconds, stamp.createdNanos) > (journal.modifiedSeconds, journal.modifiedNanos)
        }
    }
}

package enum NativeObservation {
    package static func snapshot(
        _ path: ResourcePath, limits: NativeLimits = .default,
    ) throws -> NativeResourceSnapshot {
        if String(bytes: path.platformBytes, encoding: .utf8) == nil {
            throw nativeFailure(.unsafePath, "nonportable resource path", path)
        }
        guard let parent = try NativeFilesystem.parent(path) else {
            let before = try ancestors(path)
            guard try NativeFilesystem.parent(path) == nil, try ancestors(path) == before else {
                throw nativeFailure(.concurrentChange, "resource ancestor changed", path)
            }
            return try NativeResourceSnapshot(revision: Revision(), ancestors: before, stamps: [])
        }
        let walker = NativeRevisionWalk(limits: limits)
        _ = try walker.walk(parent.descriptor, name: parent.name, path: path, relative: "", depth: 0)
        try walker.finish(parent.descriptor)
        try parent.validate()
        if walker.nodes.isEmpty,
           try NativeFilesystem.metadata(parent.descriptor.raw, name: parent.name, path: path) != nil
        {
            throw nativeFailure(.concurrentChange, "path changed during inspection", path)
        }
        let ordered = walker.nodes.indices.sorted {
            walker.nodes[$0].path.utf8.lexicographicallyPrecedes(walker.nodes[$1].path.utf8)
        }
        return try NativeResourceSnapshot(
            revision: Revision(nodes: ordered.map { walker.nodes[$0] }),
            ancestors: parent.ancestors, stamps: ordered.map { walker.bindings[$0].stamp },
        )
    }

    package static func ancestors(_ path: ResourcePath) throws -> [Ancestor] {
        var selected = path
        while true {
            if let parent = try NativeFilesystem.parent(selected) {
                try parent.validate()
                return parent.ancestors
            }
            let parts = try NativeFilesystem.components(selected)
            selected = try ResourcePath(platformBytes: [47] + parts.dropLast().joined(separator: [47]))
        }
    }

    package static func require(_ path: ResourcePath, _ expected: NativeResourceSnapshot) throws {
        guard try snapshot(path) == expected else {
            throw nativeFailure(.concurrentChange, "complete resource revision changed", path)
        }
    }

    package static func requireRevision(_ path: ResourcePath, _ revision: Revision) throws {
        guard try snapshot(path).revision == revision else {
            throw nativeFailure(.concurrentChange, "complete resource revision changed", path)
        }
    }
}

private struct NativeNodeBinding {
    let name: [UInt8]
    let path: ResourcePath
    let stamp: NativeStamp
    let security: Data
    let names: [[UInt8]]?
    var children: [Int] = []
}

private final class NativeRevisionWalk {
    let limits: NativeLimits
    var totalBytes = 0
    var nodes: [Node] = []
    var bindings: [NativeNodeBinding] = []

    init(limits: NativeLimits) {
        self.limits = limits
    }

    func walk(
        _ parent: NativeDescriptor, name: [UInt8], path: ResourcePath, relative: String, depth: Int,
    ) throws -> Int? {
        guard nodes.count < limits.maximumNodes, depth <= limits.maximumDepth else {
            throw nativeIO(ENOTSUP, message: "resource exceeds the node limit", path: path)
        }
        guard let before = try NativeFilesystem.metadata(parent.raw, name: name, path: path) else {
            if depth > 0 {
                throw nativeFailure(.concurrentChange, "listed path disappeared during inspection", path)
            }
            return nil
        }
        try NativeFilesystem.ordinary(before, path: path)
        try NativeFilesystem.resourceMode(before.st_mode, path: path)
        let isDirectory = before.st_mode & S_IFMT == S_IFDIR
        let descriptor = try NativeFilesystem.open(parent.raw, name: name, directory: isDirectory, path: path)
        let opened = try NativeFilesystem.stat(descriptor, path: path)
        guard NativeStamp(before) == NativeStamp(opened) else {
            throw nativeFailure(.concurrentChange, "file changed while opening", path)
        }
        let security = try NativeSecurity.capture(descriptor.raw, path: path)
        let data = try isDirectory ? nil : NativeFilesystem.read(
            descriptor, path: path, maximumBytes: min(limits.maximumFileBytes, limits.maximumTotalBytes - totalBytes),
        )
        totalBytes += data?.count ?? 0
        let names = try isDirectory ? NativeFilesystem.portableNames(descriptor, path: path) : nil
        do {
            try nodes.append(Node(
                path: relative, data: data, mode: Count(UInt64(before.st_mode & 0o777)),
                uid: Count(UInt64(before.st_uid)), gid: Count(UInt64(before.st_gid)),
                device: Count(UInt64(UInt32(bitPattern: before.st_dev))), inode: Count(before.st_ino),
                security: security,
            ))
        } catch let error as FlyrailValueError {
            throw nativeFailure(.unsafePath, error.message, path)
        }
        let index = bindings.count
        bindings.append(NativeNodeBinding(
            name: name, path: path, stamp: NativeStamp(opened),
            security: security, names: names,
        ))
        var children: [Int] = []
        for child in names ?? [] {
            guard let text = String(bytes: child, encoding: .utf8) else {
                throw try nativeFailure(.unsafePath, "nonportable resource path", NativeFilesystem.child(path, child))
            }
            if let childIndex = try walk(
                descriptor, name: child, path: NativeFilesystem.child(path, child),
                relative: relative.isEmpty ? text : "\(relative)/\(text)", depth: depth + 1,
            ) {
                children.append(childIndex)
            }
        }
        bindings[index].children = children
        try validate(bindings[index], parent: parent, descriptor: descriptor)
        return index
    }

    func finish(_ parent: NativeDescriptor, index: Int = 0) throws {
        guard !bindings.isEmpty else { return }
        let binding = bindings[index]
        let descriptor = try NativeFilesystem.open(
            parent.raw, name: binding.name, directory: binding.names != nil, path: binding.path,
        )
        try validate(binding, parent: parent, descriptor: descriptor)
        for child in binding.children {
            try finish(descriptor, index: child)
        }
        try validate(binding, parent: parent, descriptor: descriptor)
    }

    private func validate(
        _ binding: NativeNodeBinding, parent: NativeDescriptor, descriptor: NativeDescriptor,
    ) throws {
        let path = binding.path
        guard try NativeFilesystem.identity(
            parent.raw, name: binding.name, descriptor: descriptor, path: path,
        ), try NativeStamp(NativeFilesystem.stat(descriptor, path: path)) == binding.stamp else {
            throw nativeFailure(.concurrentChange, "path changed during inspection", path)
        }
        guard try NativeSecurity.capture(descriptor.raw, path: path) == binding.security else {
            throw nativeFailure(.concurrentChange, "security metadata changed", path)
        }
        if let names = binding.names {
            guard try NativeFilesystem.portableNames(descriptor, path: path) == names else {
                throw nativeFailure(.concurrentChange, "directory entries changed", path)
            }
        }
    }
}
