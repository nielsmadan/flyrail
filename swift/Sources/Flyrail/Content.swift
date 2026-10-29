import Foundation

public enum DocumentFormat: String, Sendable, Hashable {
    case json
    case jsonc
    case toml
}

public enum FileMode: String, Sendable, Hashable {
    case `private`
    case readable
    case executable
}

public struct Key: Sendable, Hashable {
    public let name: String

    public init(_ name: String) throws {
        guard !name.isEmpty else {
            throw FlyrailValueError("selector keys must be nonempty")
        }
        self.name = name
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(type: "Key", fields: [("name", .scalar(.text(name)))])
    }
}

public struct Member: Sendable, Hashable {
    public let identity: SemanticValue
    public let key: [String]

    public init(identity: SemanticValue, key: [String] = []) throws {
        for name in key {
            _ = try Key(name)
        }
        if !key.isEmpty, case .scalar = identity {} else if !key.isEmpty {
            throw FlyrailValueError("native member identity must be a scalar")
        }
        self.identity = identity
        self.key = key
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "Member",
            fields: [
                ("identity", identity.modelValue()),
                ("key", .array(key.map { .scalar(.text($0)) })),
            ],
        )
    }
}

public enum SelectorPart: Sendable, Hashable {
    case key(Key)
    case member(Member)

    package func modelValue() throws -> SemanticValue {
        switch self {
        case let .key(value): try value.modelValue()
        case let .member(value): try value.modelValue()
        }
    }
}

public struct Selector: Sendable, Hashable {
    public let parts: [SelectorPart]

    public init(parts: [SelectorPart]) throws {
        guard !parts.isEmpty else {
            throw FlyrailValueError("a selector must select a key or member")
        }
        self.parts = parts
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "Selector",
            fields: [("parts", .array(parts.map { try $0.modelValue() }))],
        )
    }
}

public enum Position: String, Sendable, Hashable {
    case first
    case last
    case before
    case after
}

public struct Placement: Sendable, Hashable {
    public let position: Position
    public let anchor: Member?

    public init(position: Position = .last, anchor: Member? = nil) throws {
        let anchored = position == .before || position == .after
        guard anchored == (anchor != nil) else {
            throw FlyrailValueError("before/after placement requires exactly one member anchor")
        }
        self.position = position
        self.anchor = anchor
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "Placement",
            fields: [
                ("anchor", anchor.map { try $0.modelValue() } ?? .scalar(.null)),
                ("position", .scalar(.text(position.rawValue))),
            ],
        )
    }
}

public struct FileContent: Sendable, Hashable {
    public let data: Data
    public let mode: FileMode

    public init(data: Data, mode: FileMode = .private) {
        self.data = data
        self.mode = mode
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "FileContent",
            fields: [
                ("data", ModelEncoding.bytes(data)),
                ("mode", .scalar(.text(mode.rawValue))),
            ],
        )
    }
}

public struct SectionBoundaries: Sendable, Hashable {
    public let start: String
    public let end: String

    public init(start: String, end: String) throws {
        for line in [start, end] {
            guard !isPythonBlank(line), !line.contains("\r"), !line.contains("\n"), !line.contains("\0") else {
                throw FlyrailValueError("section boundaries must be nonblank single lines without NUL")
            }
        }
        guard !exactTextEqual(start, end) else {
            throw FlyrailValueError("section boundaries must be distinct")
        }
        self.start = start
        self.end = end
    }

    public init(marker: String) throws {
        try validateIdentifier(marker, label: "section marker")
        try self.init(
            start: "<!-- flyrail:\(marker):start -->",
            end: "<!-- flyrail:\(marker):end -->",
        )
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "SectionBoundaries",
            fields: [
                ("end", .scalar(.text(end))),
                ("start", .scalar(.text(start))),
            ],
        )
    }
}

public struct SectionContent: Sendable, Hashable {
    public let marker: String
    public let text: String
    public let boundaries: SectionBoundaries?

    public init(marker: String, text: String, boundaries: SectionBoundaries? = nil) throws {
        try validateIdentifier(marker, label: "section marker")
        guard !text.contains("\0"), !text.contains("<!-- flyrail:") else {
            throw FlyrailValueError("section text cannot contain NUL or Flyrail markers")
        }
        if let boundaries {
            let lines = pythonLines(text)
            guard !lines.contains(where: {
                exactTextEqual($0, boundaries.start) || exactTextEqual($0, boundaries.end)
            }) else {
                throw FlyrailValueError("section text cannot contain its boundary lines")
            }
        }
        self.marker = marker
        self.text = text
        self.boundaries = boundaries
    }

    public var selector: SectionBoundaries {
        get throws {
            try boundaries ?? SectionBoundaries(marker: marker)
        }
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "SectionContent",
            fields: [
                ("boundaries", boundaries.map { try $0.modelValue() } ?? .scalar(.null)),
                ("marker", .scalar(.text(marker))),
                ("text", .scalar(.text(text))),
            ],
        )
    }
}

public struct StructuredContent: Sendable, Hashable {
    public let format: DocumentFormat
    public let selector: Selector
    public let value: SemanticValue
    public let placement: Placement?

    public init(
        format: DocumentFormat,
        selector: Selector,
        value: SemanticValue,
        placement: Placement? = nil,
    ) throws {
        if let placement {
            guard case let .member(member) = selector.parts.last else {
                throw FlyrailValueError("placement is only valid for an array member")
            }
            guard placement.anchor != member else {
                throw FlyrailValueError("a member cannot be its own insertion anchor")
            }
        }
        self.format = format
        self.selector = selector
        self.value = value
        self.placement = placement
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "StructuredContent",
            fields: [
                ("format", .scalar(.text(format.rawValue))),
                ("placement", placement.map { try $0.modelValue() } ?? .scalar(.null)),
                ("selector", selector.modelValue()),
                ("value", value.modelValue()),
            ],
        )
    }
}

public struct DocumentSchema: Sendable, Hashable {
    public let format: DocumentFormat
    public let key: String
    public let value: ScalarValue

    public init(format: DocumentFormat, key: String, value: ScalarValue) throws {
        guard format == .json || format == .jsonc else {
            throw FlyrailValueError("document schema scaffolds require JSON or JSONC")
        }
        _ = try Key(key)
        self.format = format
        self.key = key
        self.value = value
    }

    public var content: StructuredContent {
        get throws {
            try StructuredContent(
                format: format,
                selector: Selector(parts: [.key(Key(key))]),
                value: .scalar(value),
            )
        }
    }

    package func modelValue() throws -> SemanticValue {
        try ModelEncoding.record(
            type: "DocumentSchema",
            fields: [
                ("format", .scalar(.text(format.rawValue))),
                ("key", .scalar(.text(key))),
                ("value", SemanticValue.scalar(value).modelValue()),
            ],
        )
    }
}

public enum Content: Sendable, Hashable {
    case file(FileContent)
    case tree(TreeContent)
    case section(SectionContent)
    case structured(StructuredContent)

    package func modelValue() throws -> SemanticValue {
        switch self {
        case let .file(value): try value.modelValue()
        case let .tree(value): try value.modelValue()
        case let .section(value): try value.modelValue()
        case let .structured(value): try value.modelValue()
        }
    }
}

private func isPythonBlank(_ value: String) -> Bool {
    value.unicodeScalars.allSatisfy {
        switch $0.value {
        case 9 ... 13, 28 ... 32, 0x85, 0xA0, 0x1680, 0x2000 ... 0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000:
            true
        default:
            false
        }
    }
}

private func pythonLines(_ value: String) -> [String] {
    var lines: [String] = []
    let scalars = Array(value.unicodeScalars)
    var start = 0
    var index = 0
    let breaks: Set<UInt32> = [10, 13, 11, 12, 28, 29, 30, 0x85, 0x2028, 0x2029]
    while index < scalars.count {
        if breaks.contains(scalars[index].value) {
            lines.append(String(String.UnicodeScalarView(scalars[start ..< index])))
            index += scalars[index].value == 13 && index + 1 < scalars.count &&
                scalars[index + 1].value == 10 ? 2 : 1
            start = index
        } else {
            index += 1
        }
    }
    if start < scalars.count {
        lines.append(String(String.UnicodeScalarView(scalars[start...])))
    }
    return lines
}
