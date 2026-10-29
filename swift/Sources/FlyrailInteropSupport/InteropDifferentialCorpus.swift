import Flyrail
import Foundation

package extension Data {
    init(interopHex text: some StringProtocol) throws {
        let digits = Array(text.utf8)
        guard digits.count.isMultiple(of: 2) else { throw FlyrailValueError("invalid hexadecimal data") }
        func nibble(_ digit: UInt8) throws -> UInt8 {
            switch digit {
            case UInt8(ascii: "0") ... UInt8(ascii: "9"): digit - UInt8(ascii: "0")
            case UInt8(ascii: "a") ... UInt8(ascii: "f"): digit - UInt8(ascii: "a") + 10
            case UInt8(ascii: "A") ... UInt8(ascii: "F"): digit - UInt8(ascii: "A") + 10
            default: throw FlyrailValueError("invalid hexadecimal data")
            }
        }
        self = try Data(stride(from: 0, to: digits.count, by: 2).map {
            try nibble(digits[$0]) << 4 | nibble(digits[$0 + 1])
        })
    }
}

package extension InteropDifferential {
    struct ValueRow {
        package let name: String
        package let bytes: Data
    }

    struct BundleRow {
        package let name: String
        package let manifest: Data
        package let entries: [BundleEntry]
    }

    struct RenderRow {
        package let name: String
        package let filename: String
        package let data: Data
        package let value: String
        package let fileFirst: Bool
        package let alphaFirst: Bool
    }

    enum EditFormat {
        case section
        case structured(DocumentFormat)
    }

    enum EditOperation {
        case set(text: String, acquisition: Acquisition)
        case retire
    }

    struct EditRow {
        package let name: String
        package let format: EditFormat
        package let source: Data?
        package let operations: [EditOperation]
    }

    enum LifecycleKind: String {
        case file
        case alias
        case casefold
        case toml
        case json
        case section
        case shared
    }

    struct LifecycleRow {
        package let name: String
        package let kind: LifecycleKind
        package let initial: Data?
        package let first: String
        package let second: String
        package let acquisition: Acquisition
    }

    struct Corpus {
        package let values: [ValueRow]
        package let bundles: [BundleRow]
        package let renders: [RenderRow]
        package let edits: [EditRow]
        package let lifecycles: [LifecycleRow]

        package init(text: String) throws {
            let rows = text.split(separator: "\n").filter { !$0.hasPrefix("#") }
                .map { $0.split(separator: "\t", omittingEmptySubsequences: false).map(String.init) }
            let categories = ["value", "bundle", "render", "edit", "lifecycle"]
            guard rows.allSatisfy({ categories.contains($0[0]) }) else {
                throw FlyrailValueError("unknown differential category")
            }
            for row in rows {
                guard row.count >= 2 else { throw FlyrailValueError("invalid differential row") }
                try validateIdentifier(row[1], label: "differential case")
            }
            func category(_ name: String) -> [[String]] { rows.filter { $0[0] == name } }
            values = try category("value").map(Self.value)
            bundles = try category("bundle").map(Self.bundle)
            renders = try category("render").map(Self.render)
            edits = try category("edit").map(Self.edit)
            lifecycles = try category("lifecycle").map(Self.lifecycle)
        }

        private static func value(_ row: [String]) throws -> ValueRow {
            guard row.count == 3 else { throw FlyrailValueError("invalid differential value row") }
            return try ValueRow(name: row[1], bytes: Data(interopHex: row[2]))
        }

        private static func bundle(_ row: [String]) throws -> BundleRow {
            guard row.count >= 5, let count = Int(row[4]), (0 ... 10000).contains(count),
                  row.count == 5 + count * 3
            else { throw FlyrailValueError("invalid differential bundle row") }
            let entries = try stride(from: 5, to: row.count, by: 3).map { index in
                try BundleEntry(path: row[index], data: row[index + 1] == "-" ? nil : Data(interopHex: row[index + 1]),
                                executable: row[index + 2] == "1")
            }
            return try BundleRow(name: row[1], manifest: Data(interopHex: row[3]), entries: entries)
        }

        private static func render(_ row: [String]) throws -> RenderRow {
            guard row.count == 7, ["file-first", "section-first"].contains(row[5]),
                  ["alpha-first", "zeta-first"].contains(row[6])
            else { throw FlyrailValueError("invalid differential render row") }
            return try RenderRow(name: row[1], filename: row[2], data: Data(interopHex: row[3]), value: row[4],
                                 fileFirst: row[5] == "file-first", alphaFirst: row[6] == "alpha-first")
        }

        private static func edit(_ row: [String]) throws -> EditRow {
            guard row.count >= 5, let count = Int(row[4]), (0 ... 1000).contains(count),
                  row.count == 5 + count * 3 else { throw FlyrailValueError("invalid differential edit row") }
            let format: EditFormat
            if row[2] == "section" {
                format = .section
            } else if let structured = DocumentFormat(rawValue: row[2]) {
                format = .structured(structured)
            } else { throw FlyrailValueError("invalid differential document format") }
            let operations = try stride(from: 5, to: row.count, by: 3).map { index -> EditOperation in
                if row[index] == "set", let acquisition = Acquisition(rawValue: row[index + 2]) {
                    return .set(text: row[index + 1], acquisition: acquisition)
                }
                guard row[index] == "retire" else { throw FlyrailValueError("invalid differential edit operation") }
                return .retire
            }
            return try EditRow(name: row[1], format: format, source: row[3] == "-" ? nil : Data(interopHex: row[3]),
                               operations: operations)
        }

        private static func lifecycle(_ row: [String]) throws -> LifecycleRow {
            guard row.count == 7, let kind = LifecycleKind(rawValue: row[2]),
                  let acquisition = Acquisition(rawValue: row[6])
            else { throw FlyrailValueError("invalid differential lifecycle row") }
            let initial: Data? = if row[3] == "-" {
                nil
            } else if acquisition == .adopt {
                Data(row[4].utf8)
            } else {
                try Data(interopHex: row[3])
            }
            return LifecycleRow(name: row[1], kind: kind, initial: initial, first: row[4], second: row[5],
                                acquisition: acquisition)
        }
    }
}
