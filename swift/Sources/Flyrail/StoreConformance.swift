import Foundation

public struct StoreConformanceFailure: Error, Sendable, Hashable, CustomStringConvertible {
    public let message: String

    public init(_ message: String) {
        self.message = message
    }

    public var description: String {
        message
    }
}

public func checkStore(_ open: () throws -> any StateStore) throws {
    let first = try open()
    let second = try open()
    try checkRecords(first, second)
    try checkKeyValidation(first)
    try checkLocks(first, second)
}

private func expect(_ condition: Bool, _ message: String) throws {
    guard condition else { throw StoreConformanceFailure(message) }
}

private func excludes(_ store: any StateStore, _ name: String, timeout: Duration) throws -> Bool {
    do {
        let lock = try store.lock(name, timeout: timeout)
        lock.release()
        return false
    } catch StoreError.busy {
        return true
    }
}

private func checkRecords(_ first: any StateStore, _ second: any StateStore) throws {
    try expect(first.read("conformance/absent") == nil, "absent keys must read as None")
    try expect(
        first.write("conformance/a", value: Data("one".utf8), expected: nil),
        "creating an absent key must succeed",
    )
    try expect(second.read("conformance/a") == Data("one".utf8), "a write must be visible through another handle")
    try expect(
        !second.write("conformance/a", value: Data("two".utf8), expected: nil), "creating an existing key must fail",
    )
    try expect(
        !second.write("conformance/a", value: Data("two".utf8), expected: Data("other".utf8)),
        "a stale expected value must fail",
    )
    try expect(first.read("conformance/a") == Data("one".utf8), "a failed write must leave the value unchanged")
    try expect(
        second.write("conformance/a", value: Data("two".utf8), expected: Data("one".utf8)),
        "a matching expected value must succeed",
    )
    try expect(first.write("conformance/empty", value: Data(), expected: nil), "creating an empty value must succeed")
    try expect(second.read("conformance/empty") == Data(), "empty values must round-trip")
    let binary = Data(0 ... UInt8.max)
    try expect(first.write("conformance/binary", value: binary, expected: nil), "creating a binary value must succeed")
    try expect(second.read("conformance/binary") == binary, "binary values must round-trip exactly")
    try expect(
        first.write("conformance/ab", value: Data("x".utf8), expected: nil),
        "creating a sibling key must succeed",
    )
    try expect(
        second.keys(prefix: "conformance/a") == ["conformance/a", "conformance/ab"],
        "keys must list prefix matches in sorted order",
    )
    try expect(!first.delete("conformance/a", expected: Data("one".utf8)), "a stale delete must fail")
    try expect(first.delete("conformance/a", expected: Data("two".utf8)), "a matching delete must succeed")
    try expect(second.read("conformance/a") == nil, "a deleted key must read as None")
    try expect(!first.delete("conformance/a", expected: Data("two".utf8)), "deleting an absent key must fail")
    try expect(
        second.keys(prefix: "conformance/") == ["conformance/ab", "conformance/binary", "conformance/empty"],
        "deleted keys must not be listed",
    )
}

private func checkKeyValidation(_ first: any StateStore) throws {
    for key in ["", "../escape", "Upper/case", "trailing/", "a//b"] {
        do {
            _ = try first.read(key)
        } catch StoreError.invalidKey {
            continue
        }
        throw StoreConformanceFailure("invalid key \(pythonStringRepr(key)) must raise InvalidKey")
    }
}

private func checkLocks(_ first: any StateStore, _ second: any StateStore) throws {
    let held = try first.lock("conformance/lock", timeout: .zero)
    do {
        defer { held.release() }
        try expect(excludes(second, "conformance/lock", timeout: .zero), "a held lock must exclude another handle")
        let started = ContinuousClock.now
        try expect(
            excludes(second, "conformance/lock", timeout: .milliseconds(200)),
            "a held lock must exclude a bounded wait",
        )
        try expect(
            started.duration(to: .now) >= .milliseconds(150), "a bounded lock wait must wait before failing",
        )
        try expect(!excludes(second, "conformance/other", timeout: .zero), "different lock names must not exclude")
    }
    try expect(!excludes(second, "conformance/lock", timeout: .zero), "a released lock must be acquirable")
}
