import Foundation

public struct BundleIdentity: Sendable, Hashable {
    public let id: String
    public let version: String

    public init(id: String, version: String) throws {
        try validateIdentifier(id, label: "bundle id")
        try validateVersion(version)
        self.id = id
        self.version = version
    }

    public static func == (left: Self, right: Self) -> Bool {
        exactTextEqual(left.id, right.id) && exactTextEqual(left.version, right.version)
    }

    public func hash(into hasher: inout Hasher) {
        id.utf8.forEach { hasher.combine($0) }
        version.utf8.forEach { hasher.combine($0) }
    }
}

package let unicodeCaseFoldingVersion = UnicodeCaseFolding.version

package func validateIdentifier(_ value: String, label: String) throws {
    let bytes = Array(value.utf8)
    let segments = bytes.split(separator: 45, omittingEmptySubsequences: false)
    let shaped = (1 ... 64).contains(bytes.count) &&
        segments.allSatisfy {
            !$0.isEmpty && $0.allSatisfy { (97 ... 122).contains($0) || (48 ... 57).contains($0) }
        }
    guard shaped else {
        throw FlyrailValueError("\(label) must be 1-64 lowercase ASCII letters/digits with internal hyphens")
    }
    guard !reservedWindowsNames.contains(value) else {
        throw FlyrailValueError("\(label) is a reserved Windows name")
    }
}

package func validateVersion(_ value: String) throws {
    guard !value.allSatisfy(isPythonSpace) else {
        throw FlyrailValueError("version must be nonblank")
    }
}

package func validateRelativePath(_ value: String, label: String) throws {
    let parts = value.unicodeScalars.split(separator: "/", omittingEmptySubsequences: false)
    guard !value.isEmpty,
          parts.allSatisfy({ !$0.isEmpty && !$0.elementsEqual(".".unicodeScalars) &&
                  !$0.elementsEqual("..".unicodeScalars)
          })
    else {
        throw FlyrailValueError("\(label) must be an unambiguous relative path")
    }
    guard !value.unicodeScalars.contains(where: {
        invalidPathScalars.contains($0.value) || isControl($0.value)
    }) else {
        throw FlyrailValueError("\(label) contains a nonportable character")
    }
    guard exactTextEqual(value, nfc(value)) else {
        throw FlyrailValueError("\(label) must contain NFC-normalized Unicode scalar values")
    }
    for part in parts {
        guard part.last != ".", part.last != " " else {
            throw FlyrailValueError("\(label) contains a component ending in a dot or space")
        }
        let stem = String(String.UnicodeScalarView(
            part.split(separator: ".", maxSplits: 1, omittingEmptySubsequences: false)[0],
        ))
        if reservedWindowsNames.contains(fullCaseFold(stem).trimmingSuffix(while: { $0 == " " })) {
            throw FlyrailValueError("\(label) contains a reserved Windows name")
        }
    }
}

package func portablePathKey(_ value: String) -> String {
    nfc(fullCaseFold(value))
}

package func ancestors(_ value: String) -> [String] {
    value.split(separator: "/", omittingEmptySubsequences: false).reduce(into: []) { paths, part in
        paths.append(paths.last.map { "\($0)/\(part)" } ?? String(part))
    }
}

package func fullCaseFold(_ value: String) -> String {
    let folded = value.unicodeScalars.flatMap { scalar -> [Unicode.Scalar] in
        guard let mapping = caseFoldMapping(for: scalar.value) else {
            return [scalar]
        }
        return mapping.compactMap(Unicode.Scalar.init)
    }
    return String(String.UnicodeScalarView(folded))
}

private let reservedWindowsNames: Set<String> = [
    "con", "prn", "aux", "nul", "conin$", "conout$",
    "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "com¹", "com²", "com³",
    "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    "lpt¹", "lpt²", "lpt³",
]

private let invalidPathScalars: Set<UInt32> = [60, 62, 58, 34, 92, 124, 63, 42]

package func nfc(_ value: String) -> String {
    (value as NSString).precomposedStringWithCanonicalMapping
}

private func caseFoldMapping(for scalar: UInt32) -> [UInt32]? {
    var lower = 0
    var upper = UnicodeCaseFolding.mappings.count
    while lower < upper {
        let middle = lower + (upper - lower) / 2
        let candidate = UnicodeCaseFolding.mappings[middle]
        if candidate.0 < scalar {
            lower = middle + 1
        } else {
            upper = middle
        }
    }
    guard lower < UnicodeCaseFolding.mappings.count,
          UnicodeCaseFolding.mappings[lower].0 == scalar
    else {
        return nil
    }
    return UnicodeCaseFolding.mappings[lower].1
}

private func isControl(_ scalar: UInt32) -> Bool {
    (0 ... 31).contains(scalar) || (127 ... 159).contains(scalar)
}

private func isPythonSpace(_ character: Character) -> Bool {
    character.unicodeScalars.allSatisfy {
        switch $0.value {
        case 9 ... 13, 28 ... 32, 0x85, 0xA0, 0x1680, 0x2000 ... 0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000:
            true
        default:
            false
        }
    }
}

private extension String {
    func trimmingSuffix(while predicate: (Character) -> Bool) -> String {
        var result = self
        while result.last.map(predicate) == true {
            result.removeLast()
        }
        return result
    }
}
