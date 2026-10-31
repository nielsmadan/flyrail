import Darwin
import Foundation

public final class DirectoryStore: StateStore {
    private static let mutationTimeout = Duration.seconds(10)
    private static let lockPoll = Duration.milliseconds(20)

    private let root: ResourcePath
    private let records: ResourcePath
    private let locks: ResourcePath
    private let staging: ResourcePath

    public init(_ path: ResourcePath) throws {
        guard !path.platformBytes.isEmpty else {
            throw FlyrailValueError("directory store path must be a nonempty string path without NUL")
        }
        do {
            root = try Self.normalized(NativeAuthority.absolute(path), original: path)
        } catch let error as TargetError {
            throw StoreError.io(message: error.message, path: error.path, errno: error.errno)
        }
        records = try NativeFilesystem.child(root, Array("records".utf8))
        locks = try NativeFilesystem.child(root, Array("locks".utf8))
        staging = try NativeFilesystem.child(root, Array("staging".utf8))
    }

    public var location: ResourcePath? {
        root
    }

    public var stagingDirectory: ResourcePath? {
        staging
    }

    public func read(_ key: String) throws -> Data? {
        let path = try record(key)
        try requirePrivate()
        return try Self.contents(path)
    }

    public func write(_ key: String, value: Data, expected: Data?) throws -> Bool {
        let path = try record(key)
        let mutation = try holdFileLock(root, name: "mutation", timeout: Self.mutationTimeout)
        defer { mutation.release() }
        guard try read(key) == expected else { return false }
        try replace(path, value: value)
        return true
    }

    public func delete(_ key: String, expected: Data) throws -> Bool {
        let path = try record(key)
        let mutation = try holdFileLock(root, name: "mutation", timeout: Self.mutationTimeout)
        defer { mutation.release() }
        guard try read(key) == expected else { return false }
        guard Self.call(path, { unlink($0) }) == 0 else { throw Self.failure(errno, path) }
        try Self.syncDirectory(records)
        return true
    }

    public func keys(prefix: String) throws -> [String] {
        try requirePrivate()
        guard let names = try Self.names(records) else { return [] }
        return names
            .filter { $0.first != 46 }
            .map { String(decoding: $0, as: UTF8.self).replacingOccurrences(of: "~", with: "/") }
            .filter { $0.utf8.starts(with: prefix.utf8) }
            .sorted(by: utf8Less)
    }

    public func lock(_ name: String, timeout: Duration) throws -> StoreLock {
        try validateLockRequest(name, timeout: timeout)
        return try holdFileLock(locks, name: name, timeout: timeout)
    }

    private var trusted: [ResourcePath] {
        [root, records, locks, staging]
    }

    private func record(_ key: String) throws -> ResourcePath {
        try validateStoreKey(key)
        return try NativeFilesystem.child(records, Array(key.replacingOccurrences(of: "/", with: "~").utf8))
    }

    private func requirePrivate() throws {
        try trusted.forEach(Self.requirePrivate)
    }

    private static func requirePrivate(_ path: ResourcePath) throws {
        var metadata = stat()
        guard call(path, { lstat($0, &metadata) }) == 0 else {
            let code = errno
            guard code == ENOENT else { throw failure(code, path) }
            return
        }
        guard metadata.st_mode & 0o077 == 0, metadata.st_uid == geteuid() else { throw refusal(path) }
        let descriptor = call(path) { Darwin.open($0, O_RDONLY | O_NONBLOCK | O_CLOEXEC | O_SYMLINK) }
        guard descriptor >= 0 else {
            let code = errno
            guard code == ENOENT else { throw failure(code, path) }
            return
        }
        defer { close(descriptor) }
        do {
            _ = try NativeSecurity.capture(descriptor, path: path)
        } catch let error as TargetError {
            guard error.errno != ENOTSUP else { throw refusal(path) }
            guard error.errno != ENOENT else { return }
            throw StoreError.io(message: error.message, path: error.path, errno: error.errno)
        }
    }

    private static func refusal(_ path: ResourcePath) -> StoreError {
        .io(
            message: "[Errno \(EACCES)] state store must be private to its owner: \(pythonStringRepr(path.displayString))",
            path: path, errno: EACCES,
        )
    }

    private func holdFileLock(_ directory: ResourcePath, name: String, timeout: Duration) throws -> StoreLock {
        try requirePrivate()
        try Self.privateDirectory(directory)
        let path = try NativeFilesystem.child(directory, Array(name.replacingOccurrences(of: "/", with: "~").utf8))
        let descriptor = Self.call(path) { Darwin.open($0, O_RDWR | O_CREAT | O_CLOEXEC | O_NOFOLLOW, 0o600) }
        guard descriptor >= 0 else { throw Self.failure(errno, path) }
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while flock(descriptor, LOCK_EX | LOCK_NB) != 0 {
            let code = errno
            guard code != EINTR else { continue }
            guard [EWOULDBLOCK, EAGAIN, EACCES, EDEADLK].contains(code) else {
                close(descriptor)
                throw Self.failure(code, path)
            }
            let remaining = ContinuousClock.now.duration(to: deadline)
            guard remaining > .zero else {
                close(descriptor)
                throw StoreError.busy(name: name)
            }
            Thread.sleep(forTimeInterval: storeSeconds(min(remaining, Self.lockPoll)))
        }
        return StoreLock {
            flock(descriptor, LOCK_UN)
            close(descriptor)
        }
    }

    private func replace(_ path: ResourcePath, value: Data) throws {
        try Self.privateDirectory(records)
        let name = path.platformBytes.split(separator: 47).last.map(Array.init) ?? []
        let suffix = String(format: "%016llx", UInt64.random(in: .min ... .max))
        let temporary = try NativeFilesystem.child(records, [46] + name + Array(".\(suffix).tmp".utf8))
        let descriptor = Self.call(temporary) {
            Darwin.open($0, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
        }
        guard descriptor >= 0 else { throw Self.failure(errno, temporary) }
        do {
            defer { close(descriptor) }
            try Self.writeAll(descriptor, value: value, path: temporary)
            try Self.fullSync(descriptor, path: temporary)
            guard Self.call(temporary, { source in Self.call(path) { rename(source, $0) } }) == 0 else {
                throw Self.failure(errno, path)
            }
        } catch {
            _ = Self.call(temporary) { unlink($0) }
            throw error
        }
        try Self.syncDirectory(records)
    }

    private static func normalized(_ path: ResourcePath, original: ResourcePath) throws -> ResourcePath {
        let bytes = original.platformBytes
        let prefix: [UInt8] = bytes.starts(with: [47, 47]) && !bytes.starts(with: [47, 47, 47]) ? [47, 47] : [47]
        let parts = path.platformBytes.split(separator: 47).filter { !$0.elementsEqual([46]) }
        return try ResourcePath(platformBytes: prefix + parts.joined(separator: [47]))
    }

    private static func privateDirectory(_ path: ResourcePath) throws {
        var missing: [ResourcePath] = []
        var probe: ResourcePath? = path
        while let current = probe, !exists(current) {
            missing.append(current)
            probe = parentPath(current)
        }
        for directory in missing.reversed() {
            guard call(directory, { mkdir($0, 0o700) }) == 0 else {
                let code = errno
                guard code == EEXIST else { throw failure(code, directory) }
                continue
            }
            if let parent = parentPath(directory) {
                try syncDirectory(parent)
            }
        }
    }

    private static func exists(_ path: ResourcePath) -> Bool {
        var metadata = stat()
        return call(path) { stat($0, &metadata) } == 0
    }

    private static func contents(_ path: ResourcePath) throws -> Data? {
        let descriptor = call(path) { Darwin.open($0, O_RDONLY | O_CLOEXEC) }
        guard descriptor >= 0 else {
            let code = errno
            guard code == ENOENT else { throw failure(code, path) }
            return nil
        }
        defer { close(descriptor) }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 65536)
        while true {
            let count = buffer.withUnsafeMutableBytes { Darwin.read(descriptor, $0.baseAddress, $0.count) }
            if count > 0 {
                data.append(contentsOf: buffer[..<count])
            } else if count == 0 {
                return data
            } else if errno != EINTR {
                throw failure(errno, path)
            }
        }
    }

    private static func writeAll(_ descriptor: Int32, value: Data, path: ResourcePath) throws {
        var offset = 0
        while offset < value.count {
            let written = value.withUnsafeBytes { bytes in
                Darwin.write(descriptor, bytes.baseAddress?.advanced(by: offset), bytes.count - offset)
            }
            if written >= 0 {
                offset += written
            } else if errno != EINTR {
                throw failure(errno, path)
            }
        }
    }

    private static func fullSync(_ descriptor: Int32, path: ResourcePath) throws {
        while fcntl(descriptor, F_FULLFSYNC) != 0 {
            let code = errno
            guard code == EINTR else { throw failure(code, path) }
        }
    }

    private static func syncDirectory(_ path: ResourcePath) throws {
        let descriptor = call(path) { Darwin.open($0, O_RDONLY | O_CLOEXEC) }
        guard descriptor >= 0 else { throw failure(errno, path) }
        defer { close(descriptor) }
        try fullSync(descriptor, path: path)
    }

    private static func names(_ path: ResourcePath) throws -> [[UInt8]]? {
        guard let directory = call(path, { opendir($0) }) else {
            let code = errno
            guard code == ENOENT else { throw failure(code, path) }
            return nil
        }
        defer { closedir(directory) }
        var names: [[UInt8]] = []
        errno = 0
        while let entry = readdir(directory) {
            names.append(withUnsafeBytes(of: entry.pointee.d_name) { Array($0.prefix(while: { $0 != 0 })) })
            errno = 0
        }
        guard errno == 0 else { throw failure(errno, path) }
        return names
    }

    private static func call<T>(_ path: ResourcePath, _ body: (UnsafePointer<CChar>) -> T) -> T {
        let terminated = path.platformBytes.map { CChar(bitPattern: $0) } + [0]
        return terminated.withUnsafeBufferPointer { body($0.baseAddress!) }
    }

    private static func failure(_ code: Int32, _ path: ResourcePath) -> StoreError {
        .io(message: nativeIO(code, path: path).message, path: path, errno: code)
    }
}
