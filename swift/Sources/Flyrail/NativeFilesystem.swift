import Darwin
import Foundation

package struct NativeLimits: Sendable {
    package var maximumFileBytes = 64 * 1024 * 1024
    package var maximumTotalBytes = 256 * 1024 * 1024
    package var maximumNodes = 10000
    package var maximumDepth = 128
    package static let `default` = NativeLimits()
}

package struct NativeStamp: Sendable, Equatable {
    package let device: UInt64
    package let inode: UInt64
    package let mode: UInt16
    package let uid: UInt32
    package let gid: UInt32
    package let links: UInt16
    package let flags: UInt32
    package let size: Int64
    package let modifiedSeconds: Int
    package let modifiedNanos: Int
    package let changedSeconds: Int
    package let changedNanos: Int
    package let createdSeconds: Int
    package let createdNanos: Int

    package init(_ metadata: stat) {
        device = UInt64(UInt32(bitPattern: metadata.st_dev))
        inode = metadata.st_ino
        mode = metadata.st_mode
        uid = metadata.st_uid
        gid = metadata.st_gid
        links = metadata.st_nlink
        flags = metadata.st_flags
        size = metadata.st_size
        modifiedSeconds = metadata.st_mtimespec.tv_sec
        modifiedNanos = metadata.st_mtimespec.tv_nsec
        changedSeconds = metadata.st_ctimespec.tv_sec
        changedNanos = metadata.st_ctimespec.tv_nsec
        createdSeconds = metadata.st_birthtimespec.tv_sec
        createdNanos = metadata.st_birthtimespec.tv_nsec
    }
}

package final class NativeDescriptor {
    package let raw: Int32
    package init(_ raw: Int32) {
        self.raw = raw
    }

    deinit { Darwin.close(raw) }
}

package struct NativeParent {
    package let descriptor: NativeDescriptor
    package let name: [UInt8]
    package let path: ResourcePath
    package let ancestors: [Ancestor]

    package func validate() throws {
        let current = try NativeFilesystem.parent(path)
        guard current?.ancestors == ancestors else {
            throw nativeFailure(.concurrentChange, "resource ancestor changed", path)
        }
    }
}

package enum NativeFilesystem {
    package static func withName<T>(
        _ bytes: [UInt8], _ body: (UnsafePointer<CChar>) throws -> T,
    ) throws -> T {
        guard !bytes.contains(0) else { throw FlyrailValueError("embedded null byte") }
        let terminated = bytes.map { CChar(bitPattern: $0) } + [0]
        return try terminated.withUnsafeBufferPointer { buffer in
            guard let address = buffer.baseAddress else { throw FlyrailValueError("empty path") }
            return try body(address)
        }
    }

    package static func components(_ path: ResourcePath) throws -> [[UInt8]] {
        let bytes = path.platformBytes
        guard bytes.first == 47 else { throw FlyrailValueError("expected an absolute concrete path") }
        let parts = bytes.split(separator: 47).map(Array.init)
        guard !parts.contains([46, 46]) else {
            throw FlyrailValueError("resource destination cannot be a root or contain traversal")
        }
        return parts.filter { $0 != [46] }
    }

    package static func parent(_ path: ResourcePath) throws -> NativeParent? {
        let parts = try components(path)
        let rootPath = try ResourcePath("/")
        let raw = Darwin.open("/", O_SEARCH | O_NOFOLLOW | O_CLOEXEC)
        guard raw >= 0 else { throw nativeIO(errno, path: rootPath) }
        var current = NativeDescriptor(raw)
        var selected = rootPath
        var evidence = try [ancestor(current, path: selected)]
        for part in parts.dropLast() {
            selected = try child(selected, part)
            guard let before = try metadata(current.raw, name: part, path: selected) else { return nil }
            try ordinary(before, path: selected, ancestor: true)
            let opened = try open(current.raw, name: part, directory: true, path: selected, flags: O_SEARCH)
            let after = try stat(opened, path: selected)
            guard before.st_dev == after.st_dev, before.st_ino == after.st_ino,
                  before.st_mode == after.st_mode, before.st_uid == after.st_uid, before.st_gid == after.st_gid,
                  before.st_flags == after.st_flags
            else {
                throw nativeFailure(.concurrentChange, "ancestor changed during inspection", selected)
            }
            let captured = try ancestor(opened, path: selected)
            guard try identity(current.raw, name: part, descriptor: opened, path: selected) else {
                throw nativeFailure(.concurrentChange, "ancestor changed during inspection", selected)
            }
            evidence.append(captured)
            current = opened
        }
        return NativeParent(descriptor: current, name: parts.last ?? [46], path: path, ancestors: evidence)
    }

    package static func metadata(_ path: ResourcePath) throws -> stat? {
        guard let parent = try parent(path) else { return nil }
        let result = try metadata(parent.descriptor.raw, name: parent.name, path: path)
        try parent.validate()
        return result
    }

    package static func metadata(_ descriptor: Int32, name: [UInt8], path: ResourcePath) throws -> stat? {
        var result = Darwin.stat()
        let status = try withName(name) { fstatat(descriptor, $0, &result, AT_SYMLINK_NOFOLLOW) }
        if status != 0 {
            let code = errno
            if code == ENOENT {
                return nil
            }
            throw nativeIO(code, path: path)
        }
        return result
    }

    package static func stat(_ descriptor: NativeDescriptor, path: ResourcePath) throws -> stat {
        var result = Darwin.stat()
        guard fstat(descriptor.raw, &result) == 0 else { throw nativeIO(errno, path: path) }
        return result
    }

    package static func ordinary(_ metadata: stat, path: ResourcePath, ancestor: Bool = false) throws {
        let type = metadata.st_mode & S_IFMT
        if ancestor {
            guard type == S_IFDIR else {
                throw nativeFailure(.unsafePath, "resource ancestor is not an ordinary directory", path)
            }
        } else if (type != S_IFREG && type != S_IFDIR) || (type == S_IFREG && metadata.st_nlink != 1) {
            throw nativeFailure(.unsafePath, "managed symlinks, reparse points and special files are unsupported", path)
        }
    }

    package static func open(
        _ descriptor: Int32, name: [UInt8], directory: Bool, path: ResourcePath,
        flags: Int32 = O_RDONLY,
    ) throws -> NativeDescriptor {
        let options = flags | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK | (directory ? O_DIRECTORY : 0)
        let result = try withName(name) { openat(descriptor, $0, options) }
        guard result >= 0 else {
            let code = errno
            if code == ELOOP || code == ENOTDIR {
                throw nativeFailure(
                    .unsafePath,
                    "managed symlinks, reparse points and special files are unsupported",
                    path,
                )
            }
            if code == ENOENT {
                throw nativeFailure(.concurrentChange, "file disappeared while opening", path)
            }
            throw nativeIO(code, path: path)
        }
        let opened = NativeDescriptor(result)
        try ordinary(stat(opened, path: path), path: path, ancestor: directory)
        return opened
    }

    package static func identity(
        _ parent: Int32, name: [UInt8], descriptor: NativeDescriptor, path: ResourcePath,
    ) throws -> Bool {
        guard let named = try metadata(parent, name: name, path: path) else { return false }
        let opened = try stat(descriptor, path: path)
        return named.st_dev == opened.st_dev && named.st_ino == opened.st_ino &&
            named.st_mode & UInt16(S_IFMT) == opened.st_mode & UInt16(S_IFMT)
    }

    package static func physicalPath(_ descriptor: NativeDescriptor, path: ResourcePath) throws -> ResourcePath {
        var buffer = [CChar](repeating: 0, count: Int(MAXPATHLEN))
        guard fcntl(descriptor.raw, F_GETPATH, &buffer) == 0 else { throw nativeIO(errno, path: path) }
        let bytes = buffer.prefix(while: { $0 != 0 }).map { UInt8(bitPattern: $0) }
        guard String(bytes: bytes, encoding: .utf8) != nil else {
            throw try nativeFailure(.unsafePath, "nonportable resource path", ResourcePath(platformBytes: bytes))
        }
        return try ResourcePath(platformBytes: bytes)
    }

    package static func names(_ descriptor: NativeDescriptor, path: ResourcePath) throws -> [[UInt8]] {
        let duplicate = openat(descriptor.raw, ".", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard duplicate >= 0 else { throw nativeIO(errno, path: path) }
        guard let directory = fdopendir(duplicate) else {
            let code = errno
            Darwin.close(duplicate)
            throw nativeIO(code, path: path)
        }
        defer { closedir(directory) }
        var names: [[UInt8]] = []
        errno = 0
        while let entry = readdir(directory) {
            let bytes = withUnsafeBytes(of: entry.pointee.d_name) { Array($0.prefix(while: { $0 != 0 })) }
            if bytes != [46], bytes != [46, 46] {
                names.append(bytes)
            }
            guard names.count <= NativeLimits.default.maximumNodes else {
                throw nativeIO(ENOTSUP, message: "resource exceeds the node limit", path: path)
            }
            errno = 0
        }
        guard errno == 0 else { throw nativeIO(errno, path: path) }
        return names.sorted { $0.lexicographicallyPrecedes($1) }
    }

    package static func portableNames(_ descriptor: NativeDescriptor, path: ResourcePath) throws -> [[UInt8]] {
        let names = try names(descriptor, path: path)
        try validatePortableNames(names, path: path)
        return names
    }

    package static func validatePortableNames(_ names: [[UInt8]], path: ResourcePath) throws {
        var keys: Set<String> = []
        for name in names {
            let selected = try child(path, name)
            guard let text = String(bytes: name, encoding: .utf8) else {
                throw nativeFailure(.unsafePath, "nonportable resource path", selected)
            }
            guard keys.insert(portablePathKey(text)).inserted else {
                throw nativeFailure(.unsafePath, "portable filename collision", selected)
            }
        }
    }

    package static func resourceMode(_ mode: mode_t, path: ResourcePath) throws {
        guard mode & 0o7000 == 0 else {
            throw nativeFailure(.unsafePath, "special permission bits are unsupported", path)
        }
    }

    package static func read(
        _ descriptor: NativeDescriptor, path: ResourcePath, maximumBytes: Int,
    ) throws -> Data {
        let before = try stat(descriptor, path: path)
        guard before.st_size >= 0, before.st_size <= maximumBytes else {
            throw nativeIO(ENOTSUP, message: "resource exceeds the byte limit", path: path)
        }
        var result = Data()
        var buffer = [UInt8](repeating: 0, count: 65536)
        while true {
            let received = buffer.withUnsafeMutableBytes { Darwin.read(descriptor.raw, $0.baseAddress, $0.count) }
            if received < 0 {
                if errno == EINTR {
                    continue
                }
                throw nativeIO(errno, path: path)
            }
            if received == 0 {
                break
            }
            guard received <= maximumBytes - result.count else {
                throw nativeIO(ENOTSUP, message: "resource exceeds the byte limit", path: path)
            }
            result.append(contentsOf: buffer.prefix(received))
        }
        guard try NativeStamp(stat(descriptor, path: path)) == NativeStamp(before) else {
            throw nativeFailure(.concurrentChange, "file changed while reading", path)
        }
        return result
    }

    package static func readlink(_ parent: Int32, name: [UInt8], path: ResourcePath) throws -> [UInt8] {
        var bytes = [UInt8](repeating: 0, count: Int(MAXPATHLEN) + 1)
        let size = try withName(name) { name in
            bytes.withUnsafeMutableBytes { readlinkat(parent, name, $0.baseAddress, $0.count) }
        }
        guard size >= 0 else { throw nativeIO(errno, path: path) }
        guard size < bytes.count else { throw nativeIO(ENAMETOOLONG, path: path) }
        return Array(bytes.prefix(size))
    }

    package static func child(_ path: ResourcePath, _ name: [UInt8]) throws -> ResourcePath {
        try ResourcePath(platformBytes: path.platformBytes + (path.platformBytes == [47] ? [] : [47]) + name)
    }

    package static func ancestor(_ descriptor: NativeDescriptor, path: ResourcePath) throws -> Ancestor {
        let before = try stat(descriptor, path: path)
        try ordinary(before, path: path, ancestor: true)
        let security = try NativeSecurity.capture(descriptor.raw, path: path, ancestor: true)
        let after = try stat(descriptor, path: path)
        guard before.st_dev == after.st_dev, before.st_ino == after.st_ino,
              before.st_mode == after.st_mode, before.st_uid == after.st_uid, before.st_gid == after.st_gid,
              try security == NativeSecurity.capture(descriptor.raw, path: path, ancestor: true)
        else { throw nativeFailure(.concurrentChange, "ancestor changed during inspection", path) }
        return try Ancestor(
            path: path, device: Count(UInt64(UInt32(bitPattern: before.st_dev))),
            inode: Count(before.st_ino), mode: Count(UInt64(before.st_mode & 0o7777)),
            uid: Count(UInt64(before.st_uid)), gid: Count(UInt64(before.st_gid)), security: security,
        )
    }
}

package func nativeFailure(_ code: ErrorCode, _ message: String, _ path: ResourcePath) -> TargetError {
    TargetError(code: code, message: message, path: path)
}

package func nativeIO(_ code: Int32, message: String? = nil, path: ResourcePath) -> TargetError {
    let unsupported = [ENOTSUP, ENOSYS, EXDEV, EINVAL].contains(code)
    let detail = message ?? String(cString: strerror(code))
    let filename = pythonStringRepr(path.displayString)
    let framed = message == nil ? filename : "PosixPath(\(filename))"
    return TargetError(
        code: unsupported ? .unsupported : .ioError,
        message: "[Errno \(code)] \(detail): \(framed)", path: path, errno: code,
    )
}
