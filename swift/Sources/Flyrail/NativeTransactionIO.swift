import Darwin
import Foundation

package struct NativeTransactionSeam {
    private let action: (String) throws -> Void

    package init(_ action: @escaping (String) throws -> Void = { _ in }) {
        self.action = action
    }

    package func callAsFunction(_ boundary: String) throws {
        try action(boundary)
    }

    package static var none: NativeTransactionSeam {
        NativeTransactionSeam()
    }
}

package enum NativeTransactionIO {
    package static func createFile(_ path: ResourcePath, data: Data, created: () throws -> Void = {}) throws {
        guard let parent = try NativeFilesystem.parent(path) else { throw nativeIO(ENOENT, path: path) }
        let raw = try NativeFilesystem.withName(parent.name) {
            openat(parent.descriptor.raw, $0, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
        }
        guard raw >= 0 else { throw nativeIO(errno, path: path) }
        let descriptor = NativeDescriptor(raw)
        try NativeSecurity.ensurePrivate(descriptor, path: path)
        try created()
        try data.withUnsafeBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                let size = Darwin.write(raw, bytes.baseAddress?.advanced(by: offset), bytes.count - offset)
                if size < 0 {
                    if errno == EINTR {
                        continue
                    }
                    throw nativeIO(errno, path: path)
                }
                guard size > 0 else { throw nativeIO(EIO, path: path) }
                offset += size
            }
        }
        guard try NativeFilesystem.identity(
            parent.descriptor.raw, name: parent.name, descriptor: descriptor, path: path,
        ) else { throw nativeFailure(.concurrentChange, "metadata staging changed", path) }
        try NativeSecurity.ensurePrivate(descriptor, path: path)
        try parent.validate()
    }

    package static func rename(_ source: ResourcePath, to destination: ResourcePath, exclusive: Bool) throws {
        guard let from = try NativeFilesystem.parent(source),
              let to = try NativeFilesystem.parent(destination)
        else { throw nativeIO(ENOENT, path: destination) }
        try from.validate()
        try to.validate()
        let status = try NativeFilesystem.withName(from.name) { name in
            try NativeFilesystem.withName(to.name) { other in
                renameatx_np(from.descriptor.raw, name, to.descriptor.raw, other, exclusive ? UInt32(RENAME_EXCL) : 0)
            }
        }
        guard status == 0 else { throw nativeIO(errno, path: destination) }
        try from.validate()
        try to.validate()
    }

    package static func move(_ source: ResourcePath, to destination: ResourcePath, revision: Revision) throws {
        try NativeObservation.requireRevision(source, revision)
        try rename(source, to: destination, exclusive: true)
        try NativeObservation.requireRevision(destination, revision)
    }

    package static func unlink(_ path: ResourcePath, revision: Revision) throws {
        try NativeObservation.requireRevision(path, revision)
        guard let parent = try NativeFilesystem.parent(path) else { throw nativeIO(ENOENT, path: path) }
        let status = try NativeFilesystem.withName(parent.name) {
            unlinkat(parent.descriptor.raw, $0, revision.nodes.first?.data == nil ? AT_REMOVEDIR : 0)
        }
        guard status == 0 else { throw nativeIO(errno, path: path) }
        try parent.validate()
    }

    package static func destroy(
        _ path: ResourcePath, revision: Revision, seam: NativeTransactionSeam = .none,
        beforeRemoval: () throws -> Void = {},
    ) throws {
        for count in stride(from: revision.nodes.count, through: 1, by: -1) {
            try beforeRemoval()
            let prefix = try Revision(nodes: Array(revision.nodes.prefix(count)))
            try NativeObservation.requireRevision(path, prefix)
            let node = revision.nodes[count - 1]
            let selected = try node.path.isEmpty ? path : appendPath(path, node.path)
            guard let parent = try NativeFilesystem.parent(selected) else { throw nativeIO(ENOENT, path: selected) }
            let status = try NativeFilesystem.withName(parent.name) {
                unlinkat(parent.descriptor.raw, $0, node.data == nil ? AT_REMOVEDIR : 0)
            }
            guard status == 0 else { throw nativeIO(errno, path: selected) }
            try parent.validate()
            try seam("cleanup-node")
        }
    }

    package static func writeRevision(
        _ path: ResourcePath, revision: Revision, seam: NativeTransactionSeam = .none,
    ) throws -> Revision {
        try NativeTransactionBudget.validateRevision(revision, destination: path)
        for node in revision.nodes {
            let selected = try node.path.isEmpty ? path : appendPath(path, node.path)
            if let data = node.data {
                try createFile(selected, data: data)
            } else {
                guard let parent = try NativeFilesystem.parent(selected) else {
                    throw nativeIO(ENOENT, path: selected)
                }
                let status = try NativeFilesystem.withName(parent.name) { mkdirat(parent.descriptor.raw, $0, 0o700) }
                guard status == 0 else { throw nativeIO(errno, path: selected) }
                try parent.validate()
            }
            guard let parent = try NativeFilesystem.parent(selected) else { throw nativeIO(ENOENT, path: selected) }
            let descriptor = try NativeFilesystem.open(
                parent.descriptor.raw, name: parent.name, directory: node.data == nil, path: selected,
            )
            let metadata = try NativeFilesystem.stat(descriptor, path: selected)
            guard let uid = node.uid.uint32, let gid = node.gid.uint32, let mode = node.mode.uint32 else {
                throw nativeIO(ENOTSUP, message: "resource metadata cannot be preserved", path: selected)
            }
            if metadata.st_uid != uid || metadata.st_gid != gid {
                guard fchown(descriptor.raw, uid, gid) == 0 else { throw nativeIO(errno, path: selected) }
            }
            guard fchmod(descriptor.raw, mode_t(mode)) == 0 else { throw nativeIO(errno, path: selected) }
            guard try NativeFilesystem.identity(
                parent.descriptor.raw, name: parent.name, descriptor: descriptor, path: selected,
            ) else { throw nativeFailure(.concurrentChange, "staged resource changed", selected) }
            try parent.validate()
            try seam("stage-node")
        }
        let actual = try NativeObservation.snapshot(path).revision
        guard actual.nodes.count == revision.nodes.count,
              zip(actual.nodes, revision.nodes).allSatisfy({
                  exactTextEqual($0.path, $1.path) && $0.data == $1.data && $0.mode == $1.mode &&
                      $0.uid == $1.uid && $0.gid == $1.gid
              })
        else { throw nativeFailure(.concurrentChange, "staged resource differs from intended revision", path) }
        guard zip(actual.nodes, revision.nodes).allSatisfy({ $1.inode == 0 || $0.security == $1.security }) else {
            throw nativeIO(ENOTSUP, message: "cannot preserve resource security metadata", path: path)
        }
        return actual
    }
}
