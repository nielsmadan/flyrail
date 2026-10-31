import Darwin
import Foundation

package enum ResourceRecord: String, Sendable, CaseIterable {
    case authority
    case receipt
    case preparation
    case journal
}

package enum StateKey {
    package static func index(bundleID: String, contextDigest: String) -> String {
        "index/\(bundleID)/\(contextDigest)"
    }

    package static func destination(_ destination: ResourcePath) throws -> String {
        try stateSHA256(Data(portablePathKey(statePathText(destination)).utf8))
    }

    package static func resource(_ destination: ResourcePath, _ record: ResourceRecord) throws -> String {
        try "resource/\(Self.destination(destination))/\(record.rawValue)"
    }

    package static func resourceLock(_ destination: ResourcePath) throws -> String {
        try "resource/\(Self.destination(destination))"
    }
}

package enum StateRecords {
    package static func read<T: StateCodable>(
        _ store: any StateStore, key: String, as type: T.Type, tag: String,
    ) throws -> T? {
        guard let data = try raw(store, key: key) else { return nil }
        let value: T
        do {
            value = try StateCodec.decodeMetadata(data, as: type, tag: tag)
        } catch let error as FlyrailValueError {
            let message = error.message == "metadata record has the wrong type" ? "record has the wrong type" :
                error.message
            throw failure(store, .invalidState, "\(key): \(message)")
        }
        guard try StateCodec.encode(value) == data else {
            throw failure(store, .invalidState, "\(key): record is not canonical")
        }
        return value
    }

    package static func write<T: StateCodable>(_ store: any StateStore, key: String, value: T, expected: T?) throws {
        let encoded = try StateCodec.encode(value)
        let previous = try expected.map { try StateCodec.encode($0) }
        guard try converted({ try store.write(key, value: encoded, expected: previous) }) else {
            throw failure(store, .concurrentChange, "\(key): state changed")
        }
    }

    package static func delete(_ store: any StateStore, key: String, expected: some StateCodable) throws {
        let previous = try StateCodec.encode(expected)
        guard try converted({ try store.delete(key, expected: previous) }) else {
            throw failure(store, .concurrentChange, "\(key): state changed")
        }
    }

    package static func requireIndex(_ store: any StateStore, key: String, expected: Index?) throws {
        let previous = try expected.map { try StateCodec.encode($0) }
        guard try raw(store, key: key) == previous else {
            throw failure(store, .concurrentChange, "installation index changed")
        }
    }

    package static func lock(_ store: any StateStore, name: String, timeout: Duration) throws -> StoreLock {
        do {
            return try store.lock(name, timeout: timeout)
        } catch let error as StoreError {
            guard case .busy = error else { throw error.targetError }
            throw failure(store, .busy, error.description)
        }
    }

    package static func raw(_ store: any StateStore, key: String) throws -> Data? {
        try converted { try store.read(key) }
    }

    package static func keys(_ store: any StateStore, prefix: String) throws -> [String] {
        try converted { try store.keys(prefix: prefix) }
    }

    package static func failure(_ store: any StateStore, _ code: ErrorCode, _ message: String) -> TargetError {
        TargetError(code: code, message: message, path: store.location)
    }

    private static func converted<T>(_ body: () throws -> T) throws -> T {
        do {
            return try body()
        } catch let error as StoreError {
            throw error.targetError
        }
    }
}

package extension StoreError {
    var targetError: TargetError {
        switch self {
        case .busy:
            TargetError(code: .busy, message: description)
        case let .io(message, path, errno):
            TargetError(
                code: errno.map { [ENOTSUP, ENOSYS, EXDEV, EINVAL].contains($0) } == true ? .unsupported : .ioError,
                message: message, path: path, errno: errno,
            )
        case let .invalidKey(message):
            TargetError(code: .invalidState, message: message)
        }
    }
}
