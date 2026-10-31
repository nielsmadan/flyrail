import Darwin
import Foundation

package enum NativeSecurity {
    private static let maximumMetadataBytes = 4 * 1024 * 1024
    private static let provenance = Array("com.apple.provenance".utf8)

    package static func capture(_ descriptor: Int32, path: ResourcePath, ancestor: Bool = false) throws -> Data {
        var metadata = stat()
        guard fstat(descriptor, &metadata) == 0 else { throw nativeIO(errno, path: path) }
        if !ancestor, metadata.st_flags != 0 {
            throw nativeIO(ENOTSUP, message: "file flags cannot be preserved safely", path: path)
        }
        let values = try attributes(descriptor, path: path)
        if !ancestor, values.contains(where: { $0.0 != provenance }) {
            throw nativeIO(ENOTSUP, message: "extended attributes are unsupported", path: path)
        }
        let acl = try aclText(descriptor, path: path, ancestor: ancestor)
        if ancestor {
            return try JSONWriter.canonicalData(.object([
                ("acl", .string(acl.nativeHex)),
                ("flags", .number(String(metadata.st_flags))),
                ("schema_version", .number("1")),
                ("xattrs", .array(values.map { .array([.string(Data($0.0).nativeHex), .string($0.1.nativeHex)]) })),
            ]))
        }
        guard let value = values.first else { return Data() }
        return Data(provenance + [0]) + value.1
    }

    private static func attributes(_ descriptor: Int32, path: ResourcePath) throws -> [([UInt8], Data)] {
        let size = flistxattr(descriptor, nil, 0, 0)
        guard size >= 0 else { throw nativeIO(errno, message: "cannot inspect extended attributes", path: path) }
        guard size <= maximumMetadataBytes else {
            throw nativeIO(ENOTSUP, message: "extended metadata exceeds the byte limit", path: path)
        }
        var names = [UInt8](repeating: 0, count: size)
        let filled = names.withUnsafeMutableBytes {
            flistxattr(descriptor, $0.baseAddress?.assumingMemoryBound(to: CChar.self), $0.count, 0)
        }
        guard filled == size else { throw nativeIO(EAGAIN, message: "extended attributes changed", path: path) }
        let attributes = names.split(separator: 0).map(Array.init).sorted { $0.lexicographicallyPrecedes($1) }
        var totalBytes = size
        return try attributes.map { name in
            let payload = try NativeFilesystem.withName(name) { name in
                let size = fgetxattr(descriptor, name, nil, 0, 0, 0)
                guard size >= 0 else { throw nativeIO(errno, message: "cannot read extended metadata", path: path) }
                guard size <= maximumMetadataBytes - totalBytes else {
                    throw nativeIO(ENOTSUP, message: "extended metadata exceeds the byte limit", path: path)
                }
                var payload = Data(count: size)
                let filled = payload.withUnsafeMutableBytes { fgetxattr(descriptor, name, $0.baseAddress, size, 0, 0) }
                guard filled == size else { throw nativeIO(EAGAIN, message: "extended metadata changed", path: path) }
                totalBytes += size
                return payload
            }
            return (name, payload)
        }
    }

    private static func aclText(_ descriptor: Int32, path: ResourcePath, ancestor: Bool) throws -> Data {
        guard let acl = acl_get_fd_np(descriptor, ACL_TYPE_EXTENDED) else {
            let code = errno
            if code == ENOENT {
                return Data()
            }
            throw nativeIO(code, message: "cannot inspect file ACL", path: path)
        }
        defer { acl_free(UnsafeMutableRawPointer(acl)) }
        var entry: acl_entry_t?
        if acl_get_entry(acl, Int32(ACL_FIRST_ENTRY.rawValue), &entry) == -1 {
            guard errno == EINVAL else { throw nativeIO(errno, message: "cannot inspect file ACL", path: path) }
            return Data()
        }
        guard ancestor else { throw nativeIO(ENOTSUP, message: "extended ACLs are unsupported", path: path) }
        var length = 0
        guard let text = acl_to_text(acl, &length) else {
            throw nativeIO(errno, message: "cannot capture ancestor ACL", path: path)
        }
        defer { acl_free(text) }
        guard length >= 0, length <= maximumMetadataBytes else {
            throw nativeIO(ENOTSUP, message: "extended metadata exceeds the byte limit", path: path)
        }
        return Data(bytes: text, count: length)
    }

    package static func ensurePrivate(_ descriptor: NativeDescriptor, path: ResourcePath) throws {
        let metadata = try NativeFilesystem.stat(descriptor, path: path)
        try NativeFilesystem.ordinary(metadata, path: path)
        guard metadata.st_mode & 0o077 == 0, metadata.st_uid == geteuid() else {
            throw nativeIO(ENOTSUP, message: "management storage must be private to its owner", path: path)
        }
        _ = try capture(descriptor.raw, path: path)
    }
}

package extension Data {
    var nativeHex: String {
        map { String(format: "%02x", $0) }.joined()
    }
}
