import CryptoKit
import Darwin
import Foundation

package struct LoadoutRead: Sendable, Hashable {
    let path: ResourcePath
    let physical: ResourcePath
    let identity: [UInt64]?
    let digest: String?

    func require() throws {
        guard let (current, _) = try? loadoutRead(path, expected: physical), current == self else {
            throw nativeFailure(.concurrentChange, "Loadout routing input changed", path)
        }
    }

    func requireBinding() throws {
        guard (try? NativeTransactionDirectory.physicalLocation(path)) == physical else {
            throw nativeFailure(.concurrentChange, "Loadout routing input changed", path)
        }
    }
}

package func loadoutContains(_ root: ResourcePath, _ path: ResourcePath) -> Bool {
    path == root || path.platformBytes.starts(with: root.platformBytes + [47])
}

private func loadoutRead(
    _ path: ResourcePath, within root: ResourcePath? = nil, expected: ResourcePath? = nil,
) throws -> (LoadoutRead, Data?) {
    let physical = try NativeTransactionDirectory.physicalLocation(path)
    if let expected, physical != expected {
        throw nativeFailure(.concurrentChange, "Loadout input path changed", path)
    }
    if let root, !loadoutContains(root, physical) {
        throw LoadoutDiagnostic("escaped-path", "Loadout input escapes its declared source", path)
    }
    guard let metadata = try NativeFilesystem.metadata(physical) else {
        var logical = stat()
        if try NativeFilesystem.withName(path.platformBytes, { lstat($0, &logical) }) == 0 {
            throw nativeFailure(.unsafePath, "Dangling Loadout input", path)
        }
        return (LoadoutRead(path: path, physical: physical, identity: nil, digest: nil), nil)
    }
    let stamp = NativeStamp(metadata)
    let identity = [stamp.device, stamp.inode, UInt64(stamp.mode), UInt64(stamp.uid), UInt64(stamp.gid)]
    if metadata.st_mode & S_IFMT == S_IFDIR {
        return (LoadoutRead(path: path, physical: physical, identity: identity, digest: nil), nil)
    }
    let limit = 8 * 1024 * 1024
    guard metadata.st_mode & S_IFMT == S_IFREG, metadata.st_size <= limit else {
        throw nativeFailure(.unsafePath, "Expected an ordinary Loadout input within the size limit", path)
    }
    var limits = NativeLimits.default
    limits.maximumFileBytes = limit
    let snapshot = try NativeObservation.snapshot(physical, limits: limits)
    guard let data = snapshot.revision.data, snapshot.stamps.first == stamp,
          try NativeTransactionDirectory.physicalLocation(path) == physical
    else {
        throw nativeFailure(.concurrentChange, "Loadout input changed while reading", path)
    }
    return (LoadoutRead(path: path, physical: physical, identity: identity,
                        digest: SHA256.hash(data: data).hexadecimal), data)
}

package struct LoadoutInputs {
    private var observations: [ResourcePath: LoadoutRead] = [:]

    var conditions: [LoadoutRead] {
        observations.values.sorted { $0.path.platformBytes.lexicographicallyPrecedes($1.path.platformBytes) }
    }

    func observedFile(_ path: ResourcePath) -> Bool {
        observations[path]?.digest != nil
    }

    func physical(_ path: ResourcePath) throws -> ResourcePath {
        guard let result = observations[path] else {
            throw LoadoutDiagnostic("invalid-profile", "Profile provenance was not captured", path)
        }
        return result.physical
    }

    mutating func read(
        _ path: ResourcePath, code: String, within root: ResourcePath? = nil,
    ) throws -> (LoadoutRead, Data?) {
        let (stamp, data): (LoadoutRead, Data?)
        do {
            (stamp, data) = try loadoutRead(path, within: root)
        } catch let error as LoadoutDiagnostic {
            throw error
        } catch {
            throw LoadoutDiagnostic(code, "Cannot read Loadout input: \(error)", path)
        }
        if let previous = observations.updateValue(stamp, forKey: path), previous != stamp {
            throw LoadoutDiagnostic("concurrent-change", "Loadout input changed during preview", path)
        }
        return (stamp, data)
    }

    mutating func file(
        _ path: ResourcePath, code: String, within root: ResourcePath? = nil,
    ) throws -> Data? {
        let (stamp, data) = try read(path, code: code, within: root)
        guard stamp.identity == nil || data != nil else {
            throw LoadoutDiagnostic(code, "Expected a regular Loadout file", path)
        }
        return data
    }

    mutating func table(
        _ path: ResourcePath, code: String, within root: ResourcePath? = nil,
    ) throws -> LoadoutTable? {
        guard let data = try file(path, code: code, within: root) else { return nil }
        do {
            var scanner = try TOMLScanner(data: data)
            return try loadoutTable(scanner.parse().root.selectedValue(), path, "TOML")
        } catch {
            throw LoadoutDiagnostic(code, "Invalid Loadout TOML: \(error)", path)
        }
    }

    mutating func directory(_ path: ResourcePath) throws -> ResourcePath {
        let (read, data) = try read(path, code: "invalid-source")
        guard read.identity != nil, data == nil else {
            throw LoadoutDiagnostic("invalid-source", "Expected an existing Loadout source directory", path)
        }
        return read.physical
    }
}

package func loadoutAbsolute(_ path: ResourcePath) throws -> ResourcePath {
    guard path.filePath.isAbsolute, String(bytes: path.platformBytes, encoding: .utf8) != nil else {
        throw FlyrailValueError("Loadout routes require absolute UTF-8 paths")
    }
    return path
}

package func loadoutSourcePath(
    _ raw: String, home: ResourcePath, relativeTo root: ResourcePath? = nil,
) throws -> ResourcePath {
    if raw == "~" {
        return home
    }
    if raw.hasPrefix("~/") {
        return try ResourcePath(home.displayString + "/" + raw.dropFirst(2))
    }
    guard !raw.isEmpty, !raw.hasPrefix("~") else { throw FlyrailValueError("Use ~/ for the explicit home directory") }
    if raw.hasPrefix("/") {
        return try loadoutAbsolute(ResourcePath(raw))
    }
    if let root {
        return try ResourcePath(root.displayString + "/" + raw)
    }
    throw FlyrailValueError("Registration source must be absolute or begin with ~/")
}
