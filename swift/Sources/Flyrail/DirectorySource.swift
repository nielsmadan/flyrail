import Darwin
import Foundation

private let sourceFileTypeMask = mode_t(S_IFMT)
private let sourceRegularFile = mode_t(S_IFREG)
private let sourceDirectory = mode_t(S_IFDIR)

private struct SourceSignature: Sendable, Equatable {
    let device: UInt64
    let inode: UInt64
    let mode: UInt16
    let size: Int64
    let modifiedSeconds: Int
    let modifiedNanoseconds: Int
    let changedSeconds: Int
    let changedNanoseconds: Int

    init(_ metadata: stat) {
        device = UInt64(metadata.st_dev)
        inode = UInt64(metadata.st_ino)
        mode = metadata.st_mode
        size = metadata.st_size
        modifiedSeconds = metadata.st_mtimespec.tv_sec
        modifiedNanoseconds = metadata.st_mtimespec.tv_nsec
        changedSeconds = metadata.st_ctimespec.tv_sec
        changedNanoseconds = metadata.st_ctimespec.tv_nsec
    }

    var isDirectory: Bool {
        mode_t(mode) & sourceFileTypeMask == sourceDirectory
    }

    var isRegular: Bool {
        mode_t(mode) & sourceFileTypeMask == sourceRegularFile
    }
}

private struct DirectoryWalkState {
    let executables: [String]
    var budget: BundleEntryBudget
    var entries: [BundleEntry] = []
}

package final class DirectoryBundleSource: BundleSource, @unchecked Sendable {
    package let canonicalRoot: ResourcePath
    private let rootFD: Int32
    private let suppliedRoot: ResourcePath
    private var observed: [[[UInt8]]: SourceSignature] = [:]
    private var listings: [[[UInt8]]: [[UInt8]]] = [:]
    private var closed = false

    package init(root: ResourcePath) throws {
        suppliedRoot = root
        let pathMetadata = try lstatPath(root.platformBytes)
        try validateOrdinary(pathMetadata, display: root.displayString)
        rootFD = try openPath(root.platformBytes, flags: O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        do {
            let opened = try statDescriptor(rootFD)
            guard SourceSignature(opened) == SourceSignature(pathMetadata) else {
                throw FlyrailValueError("bundle source changed while opening: \(root.displayString)")
            }
            guard SourceSignature(opened).isDirectory else {
                throw FlyrailValueError("bundle root must be a directory")
            }
            canonicalRoot = try descriptorPath(rootFD)
        } catch {
            Darwin.close(rootFD)
            throw error
        }
        observed[[]] = SourceSignature(pathMetadata)
        listings[[]] = try listDirectory(rootFD, display: root.displayString)
        observed[[]] = try SourceSignature(statDescriptor(rootFD))
    }

    deinit {
        close()
    }

    package func close() {
        guard !closed else {
            return
        }
        Darwin.close(rootFD)
        closed = true
    }

    package func manifest() throws -> Data {
        let components = try locate("flyrail.json")
        return try readFile(components)
    }

    package func tree(spec: SkillSpec, budget: inout BundleEntryBudget) throws -> [BundleEntry] {
        let root = try locate(spec.path)
        let descriptor = try openComponents(root, directory: true)
        defer { Darwin.close(descriptor) }
        let signature = try observe(root, descriptor: descriptor)
        guard signature.isDirectory else {
            throw FlyrailValueError("skill source must be a directory: \(spec.path)")
        }
        var state = DirectoryWalkState(executables: spec.executables, budget: budget)
        try walk(
            descriptor: descriptor,
            components: root,
            installedPath: spec.name,
            sourceRelativePath: "",
            state: &state
        )
        budget = state.budget
        return state.entries
    }

    package func readFile(path: String) throws -> Data {
        try readFile(locate(path))
    }

    package func readTree(path: String, executables: [String]) throws -> [BundleEntry] {
        let root = try locate(path)
        let descriptor = try openComponents(root, directory: true)
        defer { Darwin.close(descriptor) }
        guard try observe(root, descriptor: descriptor).isDirectory else {
            throw FlyrailValueError("skill source must be an existing directory: \(path)")
        }
        var state = DirectoryWalkState(executables: executables, budget: BundleEntryBudget(limits: .default))
        try walk(
            descriptor: descriptor,
            components: root,
            installedPath: "content",
            sourceRelativePath: "",
            state: &state
        )
        return try state.entries.compactMap { entry in
            if exactTextEqual(entry.path, "content") {
                return nil
            }
            guard entry.path.hasPrefix("content/") else {
                throw FlyrailValueError("invalid source tree")
            }
            return try BundleEntry(
                path: String(entry.path.dropFirst("content/".count)),
                data: entry.data,
                executable: entry.executable
            )
        }
    }

    package func finish() throws {
        let currentRoot = try lstatPath(canonicalRoot.platformBytes)
        guard SourceSignature(currentRoot) == observed[[]] else {
            throw changed(canonicalRoot.displayString)
        }
        for (components, expected) in observed {
            let metadata = try statComponents(components)
            guard SourceSignature(metadata) == expected else {
                throw changed(display(components))
            }
        }
        for (components, expected) in listings {
            let descriptor = try openComponents(components, directory: true)
            defer { Darwin.close(descriptor) }
            guard try listDirectory(descriptor, display: display(components)) == expected else {
                throw changed(display(components))
            }
        }
    }

    private func locate(_ relative: String) throws -> [[UInt8]] {
        var components: [[UInt8]] = []
        for part in relative.split(separator: "/", omittingEmptySubsequences: false) {
            let expected = String(part)
            let descriptor = try openComponents(components, directory: true)
            defer { Darwin.close(descriptor) }
            _ = try observe(components, descriptor: descriptor)
            let names = try observeListing(components, descriptor: descriptor)
            let matches = names.compactMap { bytes -> [UInt8]? in
                guard let name = String(bytes: bytes, encoding: .utf8),
                      portablePathKey(name) == portablePathKey(expected)
                else {
                    return nil
                }
                return bytes
            }
            guard matches.count == 1,
                  let selected = matches.first,
                  logicalSourceName(selected).map({ exactTextEqual($0, expected) }) == true
            else {
                throw FlyrailValueError("missing, case-mismatched or ambiguous source path: \(relative)")
            }
            components.append(selected)
            _ = try observePath(components)
        }
        return components
    }

    private func walk(
        descriptor: Int32,
        components: [[UInt8]],
        installedPath: String,
        sourceRelativePath: String,
        state: inout DirectoryWalkState
    ) throws {
        _ = try observe(components, descriptor: descriptor)
        let directory = try BundleEntry(path: installedPath)
        try state.budget.reserve(path: directory.path, dataBytes: nil)
        state.entries.append(directory)
        let names = try observeListing(components, descriptor: descriptor)
        try validatePortableNames(names, parent: display(components))
        for bytes in names {
            guard let name = logicalSourceName(bytes) else {
                throw FlyrailValueError("source name must contain NFC-normalized Unicode scalar values")
            }
            try validateRelativePath(name, label: "source name")
            let childComponents = components + [bytes]
            let installedChild = "\(installedPath)/\(name)"
            let sourceChild = sourceRelativePath.isEmpty ? name : "\(sourceRelativePath)/\(name)"
            let metadata = try observePath(childComponents)
            let signature = SourceSignature(metadata)
            if signature.isDirectory {
                let child = try openComponents(childComponents, directory: true)
                defer { Darwin.close(child) }
                try walk(
                    descriptor: child,
                    components: childComponents,
                    installedPath: installedChild,
                    sourceRelativePath: sourceChild,
                    state: &state
                )
            } else if signature.isRegular {
                guard signature.size >= 0 else {
                    throw FlyrailValueError("bundle entry exceeds the byte limit")
                }
                try state.budget.reserve(path: installedChild, dataBytes: UInt64(signature.size))
                let data = try readFile(childComponents)
                try state.entries.append(BundleEntry(
                    path: installedChild,
                    data: data,
                    executable: state.executables.contains(where: { exactTextEqual($0, sourceChild) })
                ))
            } else {
                throw unsupported(display(childComponents))
            }
        }
        _ = try observe(components, descriptor: descriptor)
        _ = try observeListing(components, descriptor: descriptor)
    }

    private func readFile(_ components: [[UInt8]]) throws -> Data {
        guard let name = components.last else {
            throw FlyrailValueError("expected a source file: \(canonicalRoot.displayString)")
        }
        let parent = try openComponents(Array(components.dropLast()), directory: true)
        defer { Darwin.close(parent) }
        let before = try observePath(components)
        guard SourceSignature(before).isRegular else {
            throw FlyrailValueError("expected a source file: \(display(components))")
        }
        guard before.st_size >= 0,
              UInt64(before.st_size) <= BundleLimits.default.maximumMemberBytes
        else {
            throw FlyrailValueError("bundle entry exceeds the byte limit")
        }
        let descriptor = try withCString(name) {
            Darwin.openat(parent, $0, O_RDONLY | O_NOFOLLOW | O_CLOEXEC)
        }
        guard descriptor >= 0 else {
            throw changed(display(components), action: "opening")
        }
        defer { Darwin.close(descriptor) }
        let opened = try statDescriptor(descriptor)
        guard SourceSignature(opened) == SourceSignature(before) else {
            throw changed(display(components), action: "opening")
        }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            let count = buffer.withUnsafeMutableBytes {
                Darwin.read(descriptor, $0.baseAddress, $0.count)
            }
            guard count != -1 else {
                throw posixError("cannot read bundle source")
            }
            if count == .zero {
                break
            }
            guard UInt64(data.count + count) <= BundleLimits.default.maximumMemberBytes else {
                throw FlyrailValueError("bundle entry exceeds the byte limit")
            }
            data.append(contentsOf: buffer[0 ..< count])
        }
        let after = try statDescriptor(descriptor)
        guard SourceSignature(after) == SourceSignature(opened),
              try SourceSignature(statComponents(components)) == SourceSignature(opened)
        else {
            throw changed(display(components), action: "reading")
        }
        observed[components] = SourceSignature(after)
        return data
    }

    private func openComponents(_ components: [[UInt8]], directory: Bool) throws -> Int32 {
        var descriptor = Darwin.openat(rootFD, ".", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard descriptor >= 0 else {
            throw posixError("cannot reopen bundle root descriptor")
        }
        if components.isEmpty {
            return descriptor
        }
        for (index, component) in components.enumerated() {
            let flags = O_RDONLY | O_NOFOLLOW | O_CLOEXEC |
                ((index < components.count - 1 || directory) ? O_DIRECTORY : 0)
            let child = try withCString(component) {
                Darwin.openat(descriptor, $0, flags)
            }
            Darwin.close(descriptor)
            guard child >= 0 else {
                throw errno == ELOOP || errno == ENOTDIR
                    ? unsupported(display(Array(components.prefix(index + 1))))
                    : changed(display(Array(components.prefix(index + 1))))
            }
            descriptor = child
        }
        return descriptor
    }

    private func statComponents(_ components: [[UInt8]]) throws -> stat {
        guard let name = components.last else {
            return try statDescriptor(rootFD)
        }
        let parent = try openComponents(Array(components.dropLast()), directory: true)
        defer { Darwin.close(parent) }
        var metadata = stat()
        let result = try withCString(name) {
            Darwin.fstatat(parent, $0, &metadata, AT_SYMLINK_NOFOLLOW)
        }
        guard result == 0 else {
            throw changed(display(components))
        }
        try validateOrdinary(metadata, display: display(components))
        return metadata
    }

    private func observePath(_ components: [[UInt8]]) throws -> stat {
        let metadata = try statComponents(components)
        let signature = SourceSignature(metadata)
        if let previous = observed[components], previous != signature {
            throw changed(display(components))
        }
        observed[components] = signature
        return metadata
    }

    private func observe(_ components: [[UInt8]], descriptor: Int32) throws -> SourceSignature {
        let signature = try SourceSignature(statDescriptor(descriptor))
        if let previous = observed[components], previous != signature {
            throw changed(display(components))
        }
        observed[components] = signature
        return signature
    }

    private func observeListing(_ components: [[UInt8]], descriptor: Int32) throws -> [[UInt8]] {
        let names = try listDirectory(descriptor, display: display(components))
        if let previous = listings[components], previous != names {
            throw changed(display(components))
        }
        listings[components] = names
        return names
    }

    private func display(_ components: [[UInt8]]) -> String {
        let suffix = components.compactMap { String(bytes: $0, encoding: .utf8) }.joined(separator: "/")
        return suffix.isEmpty ? canonicalRoot.displayString : "\(canonicalRoot.displayString)/\(suffix)"
    }
}

package func snapshotArchiveFile(
    _ path: ResourcePath,
    limit: UInt64
) throws -> (data: Data, canonicalPath: ResourcePath) {
    let before = try lstatPath(path.platformBytes)
    try validateOrdinary(before, display: path.displayString)
    guard SourceSignature(before).isRegular else {
        throw FlyrailValueError("archive source must be a regular file")
    }
    guard UInt64(before.st_size) <= limit else {
        throw FlyrailValueError("archive exceeds the byte limit")
    }
    let descriptor = try openPath(path.platformBytes, flags: O_RDONLY | O_NOFOLLOW | O_CLOEXEC)
    defer { Darwin.close(descriptor) }
    let opened = try statDescriptor(descriptor)
    guard SourceSignature(opened) == SourceSignature(before) else {
        throw changed(path.displayString, action: "opening")
    }
    var data = Data()
    data.reserveCapacity(Int(before.st_size))
    var buffer = [UInt8](repeating: 0, count: 64 * 1024)
    while true {
        let count = buffer.withUnsafeMutableBytes {
            Darwin.read(descriptor, $0.baseAddress, $0.count)
        }
        guard count != -1 else {
            throw posixError("cannot read archive source")
        }
        if count == .zero {
            break
        }
        guard UInt64(data.count + count) <= limit else {
            throw FlyrailValueError("archive exceeds the byte limit")
        }
        data.append(contentsOf: buffer[0 ..< count])
    }
    let after = try statDescriptor(descriptor)
    let pathAfter = try lstatPath(path.platformBytes)
    guard SourceSignature(after) == SourceSignature(opened),
          SourceSignature(pathAfter) == SourceSignature(opened)
    else {
        throw changed(path.displayString, action: "reading")
    }
    return try (data, descriptorPath(descriptor))
}

private func validatePortableNames(_ names: [[UInt8]], parent: String) throws {
    var keys: Set<String> = []
    for bytes in names {
        guard let name = String(bytes: bytes, encoding: .utf8) else {
            continue
        }
        guard keys.insert(portablePathKey(name)).inserted else {
            throw FlyrailValueError("portable source name collision: \(parent)/\(name)")
        }
    }
}

private func logicalSourceName(_ bytes: [UInt8]) -> String? {
    String(bytes: bytes, encoding: .utf8)
}

private func listDirectory(_ descriptor: Int32, display: String) throws -> [[UInt8]] {
    let duplicate = Darwin.openat(descriptor, ".", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
    guard duplicate >= 0, let directory = fdopendir(duplicate) else {
        if duplicate >= 0 {
            Darwin.close(duplicate)
        }
        throw posixError("cannot list bundle source directory: \(display)")
    }
    defer { closedir(directory) }
    var names: [[UInt8]] = []
    errno = 0
    while let entry = readdir(directory) {
        let bytes = withUnsafeBytes(of: entry.pointee.d_name) { raw in
            Array(raw.prefix(while: { $0 != 0 }))
        }
        if bytes == [46] || bytes == [46, 46] {
            continue
        }
        names.append(bytes)
        errno = 0
    }
    guard errno == 0 else {
        throw posixError("cannot list bundle source directory: \(display)")
    }
    return names.sorted { $0.lexicographicallyPrecedes($1) }
}

private func validateOrdinary(_ metadata: stat, display: String) throws {
    let kind = mode_t(metadata.st_mode) & sourceFileTypeMask
    guard kind == sourceRegularFile || kind == sourceDirectory else {
        throw unsupported(display)
    }
}

private func statDescriptor(_ descriptor: Int32) throws -> stat {
    var metadata = stat()
    guard Darwin.fstat(descriptor, &metadata) == 0 else {
        throw posixError("cannot inspect bundle source descriptor")
    }
    return metadata
}

private func lstatPath(_ bytes: [UInt8]) throws -> stat {
    var metadata = stat()
    let result = try withCString(bytes) {
        Darwin.lstat($0, &metadata)
    }
    guard result == 0 else {
        throw posixError("cannot inspect bundle source")
    }
    return metadata
}

private func openPath(_ bytes: [UInt8], flags: Int32) throws -> Int32 {
    let descriptor = try withCString(bytes) {
        Darwin.open($0, flags)
    }
    guard descriptor >= 0 else {
        throw errno == ELOOP
            ? unsupported(String(decoding: bytes, as: UTF8.self))
            : posixError("cannot open bundle source")
    }
    return descriptor
}

private func descriptorPath(_ descriptor: Int32) throws -> ResourcePath {
    var buffer = [CChar](repeating: 0, count: Int(MAXPATHLEN))
    guard Darwin.fcntl(descriptor, F_GETPATH, &buffer) == 0 else {
        throw posixError("cannot resolve bundle source")
    }
    let bytes = buffer.prefix(while: { $0 != 0 }).map { UInt8(bitPattern: $0) }
    return try ResourcePath(platformBytes: bytes)
}

private func withCString<T>(_ bytes: [UInt8], body: (UnsafePointer<CChar>) throws -> T) throws -> T {
    var terminated = bytes.map { CChar(bitPattern: $0) }
    terminated.append(0)
    return try terminated.withUnsafeBufferPointer { pointer in
        guard let base = pointer.baseAddress else {
            throw FlyrailValueError("source path cannot be empty")
        }
        return try body(base)
    }
}

private func changed(_ display: String, action: String = "loading") -> FlyrailValueError {
    FlyrailValueError("bundle source changed while \(action): \(display)")
}

private func unsupported(_ display: String) -> FlyrailValueError {
    FlyrailValueError("source symlinks and special files are not supported: \(display)")
}

private func posixError(_ operation: String) -> FlyrailValueError {
    FlyrailValueError("\(operation): \(String(cString: strerror(errno)))")
}
