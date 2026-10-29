import CryptoKit
import Foundation

package func statePathText(_ path: ResourcePath) throws -> String {
    try statePathText(path.platformBytes)
}

package func statePathText(_ bytes: [UInt8]) throws -> String {
    let parts = bytes.split(separator: 47)
    guard let text = String(bytes: bytes, encoding: .utf8),
          bytes.first == 47,
          !parts.contains(where: { $0.elementsEqual([46, 46]) })
    else {
        throw FlyrailValueError("state paths must be absolute without traversal")
    }
    let prefix: [UInt8] = bytes.starts(with: [47, 47]) && !bytes.starts(with: [47, 47, 47]) ? [47, 47] : [47]
    let canonical = prefix + parts.filter { !$0.elementsEqual([46]) }.joined(separator: [47])
    guard bytes.elementsEqual(canonical) else {
        throw FlyrailValueError("state record is not canonical")
    }
    return text
}

package func requireResourcePath(_ path: ResourcePath) throws {
    let text = try statePathText(path)
    guard text != "/", text != "//" else {
        throw FlyrailValueError("filesystem roots are not resources")
    }
}

public func statePath(for destination: ResourcePath) throws -> ResourcePath {
    try requireResourcePath(destination)
    let bytes = try Array(statePathText(destination).utf8)
    guard let slash = bytes.lastIndex(of: 47) else {
        throw FlyrailValueError("expected an absolute concrete path")
    }
    let basename = String(decoding: bytes[(slash + 1)...], as: UTF8.self)
    let digest = stateSHA256(Data(portablePathKey(basename).utf8))
    return try ResourcePath(String(decoding: bytes[...slash], as: UTF8.self) + ".flyrail-\(digest).state")
}

package func stateSHA256(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}

package func hexadecimalIdentity(_ text: String, length: Int = 64) throws {
    guard text.utf8.count == length,
          text.utf8.allSatisfy({ (48 ... 57).contains($0) || (97 ... 102).contains($0) })
    else {
        throw FlyrailValueError("invalid hexadecimal identity")
    }
}

public struct ResourceAuthority: Sendable, Hashable {
    public let destination: ResourcePath
    public let stateRoot: ResourcePath

    package init(recordedDestination: ResourcePath) throws {
        destination = recordedDestination
        stateRoot = try statePath(for: recordedDestination)
    }

    public var lockPath: ResourcePath {
        get throws { try appendPath(stateRoot, "lock") }
    }
}

public extension Claim {
    func identity(destination: ResourcePath) throws -> String {
        let selected: JSONValue = switch selector {
        case .whole:
            .null
        case let .subtree(path):
            .string(path)
        case let .section(boundaries):
            .array([.string(boundaries.start), .string(boundaries.end)])
        case let .structured(selector):
            try .array(selector.parts.map { part in
                switch part {
                case let .key(key):
                    .array([.string("key"), .string(key.name)])
                case let .member(member):
                    try .array([
                        .string("member"),
                        .array(member.key.map(JSONValue.string)),
                        stateSemanticRecord(member.identity),
                    ])
                }
            })
        }
        let record = try stateSemanticFraming(.array([
            .string(statePathText(destination)), .string(kind.rawValue), selected,
        ]))
        var framed = Data("flyrail-claim-v1\0".utf8)
        try framed.append(JSONWriter.canonicalData(record))
        return stateSHA256(framed)
    }

    func identity(authority: ResourceAuthority) throws -> String {
        try identity(destination: authority.destination)
    }
}
