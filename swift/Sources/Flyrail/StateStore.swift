import Foundation

public protocol StateStore: AnyObject, Sendable {
    var location: ResourcePath? { get }
    var stagingDirectory: ResourcePath? { get }
    func read(_ key: String) throws -> Data?
    func write(_ key: String, value: Data, expected: Data?) throws -> Bool
    func delete(_ key: String, expected: Data) throws -> Bool
    func keys(prefix: String) throws -> [String]
    func lock(_ name: String, timeout: Duration) throws -> StoreLock
}

public extension StateStore {
    var location: ResourcePath? {
        nil
    }

    var stagingDirectory: ResourcePath? {
        nil
    }
}

public enum StoreError: Error, Sendable, Hashable, CustomStringConvertible {
    case busy(name: String)
    case io(message: String, path: ResourcePath?, errno: Int32?)
    case invalidKey(message: String)

    public var description: String {
        switch self {
        case let .busy(name):
            "state store lock is busy: \(name)"
        case let .io(message, _, _), let .invalidKey(message):
            message
        }
    }
}

public final class StoreLock {
    private var pending: (() -> Void)?

    public init(release: @escaping () -> Void) {
        pending = release
    }

    public func release() {
        guard let pending else { return }
        self.pending = nil
        pending()
    }

    deinit { release() }
}

private let maximumKeyBytes = 200

private func validKeySegment(_ segment: ArraySlice<UInt8>) -> Bool {
    guard let first = segment.first, isLowercaseOrDigit(first) else { return false }
    return segment.dropFirst().allSatisfy { isLowercaseOrDigit($0) || [46, 95, 45].contains($0) }
}

private func isLowercaseOrDigit(_ byte: UInt8) -> Bool {
    (97 ... 122).contains(byte) || (48 ... 57).contains(byte)
}

public func validateStoreKey(_ key: String, label: String = "state key") throws {
    let bytes = Array(key.utf8)
    guard bytes.count <= maximumKeyBytes,
          bytes.split(separator: 47, omittingEmptySubsequences: false).allSatisfy(validKeySegment)
    else {
        throw StoreError.invalidKey(message: "\(label) must be slash-separated lowercase segments")
    }
}

package func validateLockTimeout(_ timeout: Duration) throws {
    guard timeout >= .zero else { throw FlyrailValueError("lock timeout must be finite and nonnegative") }
}

package func validateLockRequest(_ name: String, timeout: Duration) throws {
    try validateStoreKey(name, label: "lock name")
    try validateLockTimeout(timeout)
}

package func storeSeconds(_ duration: Duration) -> Double {
    Double(duration.components.seconds) + Double(duration.components.attoseconds) / 1e18
}

public final class MemoryStore: StateStore, @unchecked Sendable {
    private let condition = NSCondition()
    private var values: [String: Data] = [:]
    private var held: Set<String> = []

    public init() {}

    public func read(_ key: String) throws -> Data? {
        try validateStoreKey(key)
        condition.lock()
        defer { condition.unlock() }
        return values[key]
    }

    public func write(_ key: String, value: Data, expected: Data?) throws -> Bool {
        try validateStoreKey(key)
        condition.lock()
        defer { condition.unlock() }
        guard values[key] == expected else { return false }
        values[key] = value
        return true
    }

    public func delete(_ key: String, expected: Data) throws -> Bool {
        try validateStoreKey(key)
        condition.lock()
        defer { condition.unlock() }
        guard values[key] == expected else { return false }
        values[key] = nil
        return true
    }

    public func keys(prefix: String) throws -> [String] {
        condition.lock()
        defer { condition.unlock() }
        return values.keys.filter { $0.utf8.starts(with: prefix.utf8) }.sorted(by: utf8Less)
    }

    public func lock(_ name: String, timeout: Duration) throws -> StoreLock {
        try validateLockRequest(name, timeout: timeout)
        let deadline = ContinuousClock.now.advanced(by: timeout)
        condition.lock()
        defer { condition.unlock() }
        while held.contains(name) {
            let remaining = ContinuousClock.now.duration(to: deadline)
            guard remaining > .zero else { throw StoreError.busy(name: name) }
            condition.wait(until: Date(timeIntervalSinceNow: storeSeconds(remaining)))
        }
        held.insert(name)
        return StoreLock { [self] in unhold(name) }
    }

    private func unhold(_ name: String) {
        condition.lock()
        held.remove(name)
        condition.broadcast()
        condition.unlock()
    }
}
