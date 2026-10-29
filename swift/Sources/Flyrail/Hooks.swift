import Foundation

public let hookInputLimit = 1_048_576
public let hookOutputLimit = 65536
public let hookErrorLimit = 16384
public let hookTextLimit = 8192
public let hookJSONDepthLimit = 64
public let hookHandlerLimit = 64
public let hookEventTimeoutLimitMilliseconds = 300_000

public struct HookText: Sendable, Hashable {
    public let utf16CodeUnits: [UInt16]

    public init(utf16CodeUnits: [UInt16]) {
        self.utf16CodeUnits = utf16CodeUnits
    }

    public init(_ value: String) {
        utf16CodeUnits = Array(value.utf16)
    }

    public var string: String? {
        String(decodingExactUTF16: utf16CodeUnits)
    }

    fileprivate var isEmpty: Bool {
        utf16CodeUnits.isEmpty
    }

    fileprivate var isBlank: Bool {
        utf16CodeUnits.allSatisfy {
            switch $0 {
            case 0x0009 ... 0x000D, 0x0020, 0x00A0, 0x1680, 0x2000 ... 0x200A,
                 0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0xFEFF:
                true
            default:
                false
            }
        }
    }

    fileprivate var utf8Length: Int {
        utf16CodeUnits.withUnsafeBufferPointer {
            String(decoding: $0, as: UTF16.self).utf8.count
        }
    }

    fileprivate func equals(_ value: String) -> Bool {
        utf16CodeUnits.elementsEqual(value.utf16)
    }
}

public struct HookFailure: Error, Sendable, Equatable, CustomStringConvertible {
    public let code: String

    public init(_ code: String) {
        self.code = code
    }

    public var description: String {
        "flyrail-hook:\(code)"
    }
}

public struct HookJSON: Sendable, Equatable {
    fileprivate let value: HookValue

    public init(data: Data) throws {
        var parser = try HookParser(data: data, limit: hookInputLimit)
        value = try parser.parse()
    }

    public init(string: HookText) throws {
        value = .string(string)
        try validateHookJSON(value, limit: hookInputLimit)
    }

    public init(string: String) throws {
        try self.init(string: HookText(string))
    }

    public init(boolean: Bool) throws {
        value = .boolean(boolean)
    }

    public init(integer: Int64) throws {
        value = .number(Double(integer))
    }

    public init(array: [HookJSON]) throws {
        value = .array(array.map(\.value))
        try validateHookJSON(value, limit: hookInputLimit)
    }

    public init(object: [(String, HookJSON)]) throws {
        value = .object(object.map { (HookText($0.0), $0.1.value) })
        try validateHookJSON(value, limit: hookInputLimit)
    }

    public init() {
        value = .null
    }

    public var data: Data {
        encodeHookJSON(value)
    }

    public var isObject: Bool {
        if case .object = value {
            true
        } else {
            false
        }
    }

    public var text: HookText? {
        if case let .string(text) = value {
            text
        } else {
            nil
        }
    }
}

public struct HookRequest: Sendable, Equatable {
    public let event: HookEvent
    public let context: HookJSON
    public let tool: HookText?
    public let input: HookJSON?
    public let result: HookJSON?

    public init(
        event: HookEvent,
        context: HookJSON,
        tool: HookText? = nil,
        input: HookJSON? = nil,
        result: HookJSON? = nil,
    ) throws {
        guard context.isObject else { throw HookFailure("host-input") }
        let toolEvent = event == .beforeTool || event == .afterTool
        if toolEvent, tool?.isEmpty != false {
            throw HookFailure("host-tool")
        }
        if toolEvent, input == nil {
            throw HookFailure("host-arguments")
        }
        if event == .afterTool, result == nil {
            throw HookFailure("host-result")
        }
        if event == .prompt, input?.text == nil {
            throw HookFailure("host-prompt")
        }
        if !toolEvent, tool != nil {
            throw HookFailure("input-fields")
        }
        if event != .beforeTool, event != .afterTool, event != .prompt, input != nil {
            throw HookFailure("input-fields")
        }
        if event != .afterTool, result != nil {
            throw HookFailure("input-fields")
        }
        self.event = event
        self.context = context
        self.tool = tool
        self.input = input
        self.result = result
        guard data.count <= hookInputLimit else { throw HookFailure("input-limit") }
    }

    public init(data: Data) throws {
        var parser = try HookParser(data: data, limit: hookInputLimit)
        let raw = try parser.parse()
        guard case let .object(fields) = raw else { throw HookFailure("host-input") }
        let allowed = ["version", "event", "context", "tool", "input", "result"]
        guard fields.allSatisfy({ item in allowed.contains(where: item.0.equals) }) else {
            throw HookFailure("input-fields")
        }
        guard field(fields, "version").map(isOne) == true else { throw HookFailure("input-version") }
        guard case let .string(eventText)? = field(fields, "event"),
              let eventString = eventText.string,
              let event = HookEvent(rawValue: eventString)
        else {
            throw HookFailure("input-event")
        }
        guard let context = field(fields, "context") else { throw HookFailure("host-input") }
        let tool: HookText?
        if case let .string(value)? = field(fields, "tool") {
            tool = value
        } else if field(fields, "tool") != nil {
            throw HookFailure("host-tool")
        } else {
            tool = nil
        }
        try self.init(
            event: event,
            context: HookJSON(value: context),
            tool: tool,
            input: field(fields, "input").map(HookJSON.init(value:)),
            result: field(fields, "result").map(HookJSON.init(value:)),
        )
    }

    public var data: Data {
        var fields: [(HookText, HookValue)] = [
            (HookText("version"), .integer(1)),
            (HookText("event"), .string(HookText(event.rawValue))),
            (HookText("context"), context.value),
        ]
        if let tool {
            fields.append((HookText("tool"), .string(tool)))
        }
        if let input {
            fields.append((HookText("input"), input.value))
        }
        if let result {
            fields.append((HookText("result"), result.value))
        }
        return encodeHookJSON(.object(fields))
    }
}

public struct HookResponse: Sendable, Equatable {
    public let outcome: HookOutcome
    public let value: HookJSON?

    public static let continuing = HookResponse(outcome: .continue, value: nil)

    public static func block(_ reason: HookText) throws -> Self {
        try text(.block, reason, code: "reason")
    }

    public static func ask(_ reason: HookText) throws -> Self {
        try text(.ask, reason, code: "reason")
    }

    public static func context(_ text: HookText) throws -> Self {
        try self.text(.context, text, code: "context")
    }

    public static func modifyInput(event: HookEvent, input: HookJSON) throws -> Self {
        guard event == .prompt ? input.text != nil : input.isObject else {
            throw HookFailure("replacement")
        }
        return Self(outcome: .modifyInput, value: input)
    }

    public init(data: Data, event: HookEvent, declared: [HookOutcome]) throws {
        guard data.count <= hookOutputLimit else { throw HookFailure("stdout-limit") }
        let raw: HookValue
        do {
            var parser = try HookParser(data: data, limit: hookOutputLimit)
            raw = try parser.parse()
        } catch {
            throw HookFailure("invalid-output")
        }
        guard case let .object(fields) = raw,
              field(fields, "version").map(isOne) == true,
              case let .string(outcomeText)? = field(fields, "outcome"),
              let outcomeName = outcomeText.string,
              let outcome = HookOutcome(rawValue: outcomeName),
              declared.contains(outcome)
        else {
            throw HookFailure("undeclared-outcome")
        }
        let payload: String? = switch outcome {
        case .block, .ask: "reason"
        case .context: "text"
        case .modifyInput: "input"
        case .continue: nil
        }
        let allowed = ["version", "outcome"] + (payload.map { [$0] } ?? [])
        guard fields.count == allowed.count,
              fields.allSatisfy({ item in allowed.contains(where: item.0.equals) })
        else {
            throw HookFailure("output-fields")
        }
        switch outcome {
        case .continue:
            self = .continuing
        case .block, .ask, .context:
            guard case let .string(text)? = payload.flatMap({ field(fields, $0) }) else {
                throw HookFailure(outcome == .context ? "context" : "reason")
            }
            self = try Self.text(outcome, text, code: outcome == .context ? "context" : "reason")
        case .modifyInput:
            guard let input = payload.flatMap({ field(fields, $0) }) else {
                throw HookFailure("replacement")
            }
            self = try Self.modifyInput(event: event, input: HookJSON(value: input))
        }
    }

    public var data: Data {
        var fields: [(HookText, HookValue)] = [
            (HookText("version"), .integer(1)),
            (HookText("outcome"), .string(HookText(outcome.rawValue))),
        ]
        if let value {
            let name = switch outcome {
            case .block, .ask: "reason"
            case .context: "text"
            case .modifyInput: "input"
            case .continue: ""
            }
            if !name.isEmpty {
                fields.append((HookText(name), value.value))
            }
        }
        return encodeHookJSON(.object(fields))
    }

    private init(outcome: HookOutcome, value: HookJSON?) {
        self.outcome = outcome
        self.value = value
    }

    private static func text(_ outcome: HookOutcome, _ text: HookText, code: String) throws -> Self {
        guard !text.isBlank, text.utf8Length <= hookTextLimit else { throw HookFailure(code) }
        return try Self(outcome: outcome, value: HookJSON(string: text))
    }
}

public enum HookShell: String, Sendable, Hashable {
    case posix
    case cmd
}

public struct HookRuntime: Sendable, Hashable {
    public let node: ResourcePath?
    public let shell: HookShell?
    public let resources: [BundleEntry]

    public init(node: ResourcePath? = nil, shell: HookShell? = nil) throws {
        if let node {
            let value = node.displayString
            let components = node.platformBytes.split(separator: 47, omittingEmptySubsequences: false)
            guard value.hasPrefix("/"), value != "/", !value.contains("/../"),
                  !components.contains(where: { $0.elementsEqual([46, 46]) }),
                  !value.unicodeScalars.contains(where: { $0.value < 32 })
            else {
                throw FlyrailValueError("hook runtime requires an absolute Node 24+ executable path")
            }
        }
        self.node = node
        self.shell = shell
        resources = try ["runner.mts", "bridge.mts", "process.mts"].map { name in
            guard let url = Bundle.module.url(
                forResource: name,
                withExtension: nil,
                subdirectory: "Resources/runtime",
            ) else {
                throw FlyrailValueError("packaged hook runtime is missing \(name)")
            }
            return try BundleEntry(path: name, data: Data(contentsOf: url))
        }
    }
}

public enum HookEnvironment: Sendable, Hashable {
    case text(String)
    case reference(String)
}

public struct HookEnvironmentEntry: Sendable, Hashable {
    public let name: String
    public let value: HookEnvironment

    public init(name: String, value: HookEnvironment) {
        self.name = name
        self.value = value
    }
}

public struct HookHandler: Sendable, Hashable {
    public let id: String
    public let event: HookEvent
    public let argv: [String]
    public let environment: [HookEnvironmentEntry]
    public let cwd: ResourcePath
    public let timeoutMilliseconds: Int
    public let outcomes: [HookOutcome]
    public let tools: [String]

    public init(
        id: String,
        event: HookEvent,
        argv: [String],
        environment: [HookEnvironmentEntry],
        cwd: ResourcePath,
        timeoutMilliseconds: Int,
        outcomes: [HookOutcome],
        tools: [String],
    ) throws {
        try validateIdentifier(id, label: "artifact id")
        guard cwd.displayString.hasPrefix("/"), !cwd.displayString.split(separator: "/").contains("..") else {
            throw FlyrailValueError("hook cwd must be an absolute path")
        }
        guard !argv.isEmpty else { throw FlyrailValueError("argv requires a command") }
        guard !argv.contains(where: { $0.contains("\0") }) else {
            throw FlyrailValueError("command arguments cannot contain NUL")
        }
        guard argv.first?.isEmpty == false else { throw FlyrailValueError("command cannot be empty") }
        guard (1 ... hookEventTimeoutLimitMilliseconds).contains(timeoutMilliseconds) else {
            throw FlyrailValueError("hook timeout must be 1-300000 integer milliseconds")
        }
        guard !outcomes.isEmpty else {
            throw FlyrailValueError("hook requires declared HookOutcome values")
        }
        guard Set(outcomes).count == outcomes.count else {
            throw FlyrailValueError("hook outcomes must be unique")
        }
        guard tools.allSatisfy({ !$0.isEmpty && !$0.contains("\0") }) else {
            throw FlyrailValueError("tool names must be nonempty and cannot contain NUL")
        }
        guard Set(tools.map { Array($0.utf8) }).count == tools.count else {
            throw FlyrailValueError("tool names must be unique")
        }
        guard tools.isEmpty || event == .beforeTool || event == .afterTool else {
            throw FlyrailValueError("tool selection requires a tool event")
        }
        var names: Set<String> = []
        for item in environment {
            _ = try EnvRef(item.name)
            guard names.insert(item.name).inserted else {
                throw FlyrailValueError("duplicate environment name")
            }
            switch item.value {
            case let .text(value) where value.contains("\0"):
                throw FlyrailValueError("environment values cannot contain NUL")
            case let .reference(value):
                _ = try EnvRef(value)
            default:
                break
            }
        }
        self.id = id
        self.event = event
        self.argv = argv
        self.environment = environment.sorted { utf8Less($0.name, $1.name) }
        self.cwd = cwd
        self.timeoutMilliseconds = timeoutMilliseconds
        self.outcomes = outcomes.sorted { utf8Less($0.rawValue, $1.rawValue) }
        self.tools = tools.sorted(by: utf8Less)
    }
}

public struct HookConfig: Sendable, Hashable {
    public let host: String
    public let handlers: [HookHandler]

    public init(host: String, handlers: [HookHandler]) throws {
        guard !host.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw FlyrailValueError("hook host must be a nonblank string")
        }
        for event in HookEvent.allCases {
            let selected = handlers.filter { $0.event == event }
            guard selected.count <= hookHandlerLimit else {
                throw FlyrailValueError("hook event cannot have more than 64 handlers")
            }
            guard selected.map(\.timeoutMilliseconds).reduce(0, +) <= hookEventTimeoutLimitMilliseconds else {
                throw FlyrailValueError("hook event timeout sum cannot exceed 300000 milliseconds")
            }
        }
        self.host = host
        self.handlers = handlers.sorted { utf8Less($0.id, $1.id) }
    }

    public var data: Data {
        let values = handlers.map { handler -> HookValue in
            let environment = handler.environment.map { item -> (HookText, HookValue) in
                let value = switch item.value {
                case let .text(text): HookValue.string(HookText(text))
                case let .reference(name):
                    HookValue.object([(HookText("ref"), .string(HookText(name)))])
                }
                return (HookText(item.name), value)
            }
            return .object([
                (HookText("id"), .string(HookText(handler.id))),
                (HookText("event"), .string(HookText(handler.event.rawValue))),
                (HookText("argv"), .array(handler.argv.map { .string(HookText($0)) })),
                (HookText("cwd"), .string(HookText(handler.cwd.displayString))),
                (HookText("env"), .object(environment)),
                (HookText("timeout_ms"), .integer(Int64(handler.timeoutMilliseconds))),
                (HookText("outcomes"), .array(handler.outcomes.map { .string(HookText($0.rawValue)) })),
                (HookText("tools"), .array(handler.tools.map { .string(HookText($0)) })),
            ])
        }
        return encodeHookJSON(.object([
            (HookText("host"), .string(HookText(host))),
            (HookText("handlers"), .array(values)),
        ]))
    }
}

public struct HookEffects: Sendable, Equatable {
    public let input: HookJSON?
    public let contexts: [HookText]
    public let decision: HookOutcome?
    public let reason: HookText?

    public init(
        input: HookJSON? = nil,
        contexts: [HookText] = [],
        decision: HookOutcome? = nil,
        reason: HookText? = nil,
    ) throws {
        guard !contexts.contains(where: \.isBlank),
              contexts.map(\.utf8Length).reduce(max(0, contexts.count - 1), +) <= hookTextLimit
        else {
            throw HookFailure("context-limit")
        }
        guard (decision != nil) == (reason != nil),
              decision == nil || decision == .block || decision == .ask
        else {
            throw HookFailure("output-fields")
        }
        if let reason, reason.isBlank || reason.utf8Length > hookTextLimit {
            throw HookFailure("reason")
        }
        self.input = input
        self.contexts = contexts
        self.decision = decision
        self.reason = reason
    }

    public init(data: Data) throws {
        let raw: HookValue
        do {
            var parser = try HookParser(data: data, limit: hookOutputLimit)
            raw = try parser.parse()
        } catch {
            throw HookFailure("invalid-output")
        }
        guard case let .object(fields) = raw,
              fields.allSatisfy({
                  ["input", "contexts", "decision", "reason"].contains(where: $0.0.equals)
              }),
              case let .array(rawContexts)? = field(fields, "contexts")
        else {
            throw HookFailure("output-fields")
        }
        let contexts = try rawContexts.map {
            guard case let .string(text) = $0 else { throw HookFailure("context") }
            return text
        }
        let decision: HookOutcome?
        if case let .string(text)? = field(fields, "decision"),
           let value = text.string,
           value == "block" || value == "ask"
        {
            decision = HookOutcome(rawValue: value)
        } else if field(fields, "decision") != nil {
            throw HookFailure("undeclared-outcome")
        } else {
            decision = nil
        }
        let reason: HookText?
        if case let .string(text)? = field(fields, "reason") {
            reason = text
        } else if field(fields, "reason") != nil {
            throw HookFailure("reason")
        } else {
            reason = nil
        }
        try self.init(
            input: field(fields, "input").map(HookJSON.init(value:)),
            contexts: contexts,
            decision: decision,
            reason: reason,
        )
    }

    public var data: Data {
        var fields: [(HookText, HookValue)] = [
            (HookText("contexts"), .array(contexts.map { .string($0) })),
        ]
        if let input {
            fields.append((HookText("input"), input.value))
        }
        if let decision {
            fields.append((HookText("decision"), .string(HookText(decision.rawValue))))
        }
        if let reason {
            fields.append((HookText("reason"), .string(reason)))
        }
        return encodeHookJSON(.object(fields))
    }
}

private indirect enum HookValue: Sendable, Equatable {
    case null
    case boolean(Bool)
    case integer(Int64)
    case number(Double)
    case string(HookText)
    case array([HookValue])
    case object([(HookText, HookValue)])

    static func == (left: Self, right: Self) -> Bool {
        switch (left, right) {
        case (.null, .null): true
        case let (.boolean(left), .boolean(right)): left == right
        case let (.integer(left), .integer(right)): left == right
        case let (.number(left), .number(right)): left.bitPattern == right.bitPattern
        case let (.string(left), .string(right)): left == right
        case let (.array(left), .array(right)): left == right
        case let (.object(left), .object(right)):
            left.count == right.count && zip(left, right).allSatisfy {
                $0.0.0 == $0.1.0 && $0.0.1 == $0.1.1
            }
        default: false
        }
    }
}

private extension HookJSON {
    init(value: HookValue) {
        self.value = value
    }
}

private struct HookParser {
    let bytes: [UInt8]
    let limit: Int
    var position = 0

    init(data: Data, limit: Int) throws {
        guard data.count <= limit else { throw HookFailure("input-limit") }
        guard String(data: data, encoding: .utf8) != nil else { throw HookFailure("invalid-json") }
        bytes = Array(data)
        self.limit = limit
    }

    mutating func parse() throws -> HookValue {
        skipWhitespace()
        let result = try parseValue(depth: 0)
        skipWhitespace()
        guard position == bytes.count else { throw HookFailure("invalid-json") }
        try validateHookJSON(result, limit: limit)
        return result
    }

    mutating func parseValue(depth: Int) throws -> HookValue {
        guard let byte = peek else { throw HookFailure("invalid-json") }
        switch byte {
        case 123: return try parseObject(depth: depth)
        case 91: return try parseArray(depth: depth)
        case 34: return try .string(parseString())
        case 116: try literal("true"); return .boolean(true)
        case 102: try literal("false"); return .boolean(false)
        case 110: try literal("null"); return .null
        case 45, 48 ... 57: return try .number(parseNumber())
        default: throw HookFailure("invalid-json")
        }
    }

    mutating func parseObject(depth: Int) throws -> HookValue {
        guard depth < hookJSONDepthLimit else { throw HookFailure("json-depth") }
        position += 1
        skipWhitespace()
        var values: [(HookText, HookValue)] = []
        var keys: Set<HookText> = []
        if peek == 125 {
            position += 1; return .object(values)
        }
        while true {
            guard peek == 34 else { throw HookFailure("invalid-json") }
            let key = try parseString()
            guard keys.insert(key).inserted,
                  !["__proto__", "constructor", "prototype"].contains(where: key.equals)
            else {
                throw HookFailure("json-key")
            }
            skipWhitespace()
            guard peek == 58 else { throw HookFailure("invalid-json") }
            position += 1
            skipWhitespace()
            try values.append((key, parseValue(depth: depth + 1)))
            skipWhitespace()
            if peek == 125 {
                position += 1; return .object(values)
            }
            guard peek == 44 else { throw HookFailure("invalid-json") }
            position += 1
            skipWhitespace()
        }
    }

    mutating func parseArray(depth: Int) throws -> HookValue {
        guard depth < hookJSONDepthLimit else { throw HookFailure("json-depth") }
        position += 1
        skipWhitespace()
        var values: [HookValue] = []
        if peek == 93 {
            position += 1; return .array(values)
        }
        while true {
            try values.append(parseValue(depth: depth + 1))
            skipWhitespace()
            if peek == 93 {
                position += 1; return .array(values)
            }
            guard peek == 44 else { throw HookFailure("invalid-json") }
            position += 1
            skipWhitespace()
        }
    }

    mutating func parseString() throws -> HookText {
        guard peek == 34 else { throw HookFailure("invalid-json") }
        position += 1
        var units: [UInt16] = []
        while let byte = peek {
            switch byte {
            case 34:
                position += 1
                return HookText(utf16CodeUnits: units)
            case 92:
                position += 1
                guard let escape = peek else { throw HookFailure("invalid-json") }
                position += 1
                switch escape {
                case 34, 47, 92: units.append(UInt16(escape))
                case 98: units.append(8)
                case 102: units.append(12)
                case 110: units.append(10)
                case 114: units.append(13)
                case 116: units.append(9)
                case 117: try units.append(hex4())
                default: throw HookFailure("invalid-json")
                }
            case 0 ... 31:
                throw HookFailure("invalid-json")
            case 32 ... 127:
                units.append(UInt16(byte))
                position += 1
            default:
                guard let tail = String(bytes: bytes[position...], encoding: .utf8),
                      let character = tail.first
                else {
                    throw HookFailure("invalid-json")
                }
                units.append(contentsOf: character.utf16)
                position += character.utf8.count
            }
        }
        throw HookFailure("invalid-json")
    }

    mutating func parseNumber() throws -> Double {
        let start = position
        if peek == 45 {
            position += 1
        }
        if peek == 48 {
            position += 1
            if peek.map({ (48 ... 57).contains($0) }) == true {
                throw HookFailure("invalid-json")
            }
        } else {
            guard peek.map({ (49 ... 57).contains($0) }) == true else {
                throw HookFailure("invalid-json")
            }
            digits()
        }
        if peek == 46 {
            position += 1
            guard peek.map({ (48 ... 57).contains($0) }) == true else {
                throw HookFailure("invalid-json")
            }
            digits()
        }
        if peek == 101 || peek == 69 {
            position += 1
            if peek == 43 || peek == 45 {
                position += 1
            }
            guard peek.map({ (48 ... 57).contains($0) }) == true else {
                throw HookFailure("invalid-json")
            }
            digits()
        }
        guard let number = Double(String(decoding: bytes[start ..< position], as: UTF8.self)),
              number.isFinite
        else {
            throw HookFailure("json-number")
        }
        return number
    }

    mutating func hex4() throws -> UInt16 {
        guard position + 4 <= bytes.count else { throw HookFailure("invalid-json") }
        var value: UInt16 = 0
        for byte in bytes[position ..< position + 4] {
            guard let digit = byte.hexDigit else { throw HookFailure("invalid-json") }
            value = value * 16 + UInt16(digit)
        }
        position += 4
        return value
    }

    mutating func literal(_ value: String) throws {
        let literal = Array(value.utf8)
        guard position + literal.count <= bytes.count,
              bytes[position ..< position + literal.count].elementsEqual(literal)
        else {
            throw HookFailure("invalid-json")
        }
        position += literal.count
    }

    mutating func digits() {
        while peek.map({ (48 ... 57).contains($0) }) == true {
            position += 1
        }
    }

    mutating func skipWhitespace() {
        while peek.map({ [9, 10, 13, 32].contains($0) }) == true {
            position += 1
        }
    }

    var peek: UInt8? {
        bytes.indices.contains(position) ? bytes[position] : nil
    }
}

private func validateHookJSON(_ value: HookValue, depth: Int = 0, limit: Int) throws {
    switch value {
    case let .number(number) where !number.isFinite:
        throw HookFailure("json-number")
    case let .array(values):
        guard depth < hookJSONDepthLimit else { throw HookFailure("json-depth") }
        for value in values {
            try validateHookJSON(value, depth: depth + 1, limit: limit)
        }
    case let .object(values):
        guard depth < hookJSONDepthLimit else { throw HookFailure("json-depth") }
        var keys: Set<HookText> = []
        for (key, value) in values {
            guard keys.insert(key).inserted,
                  !["__proto__", "constructor", "prototype"].contains(where: key.equals)
            else {
                throw HookFailure("json-key")
            }
            try validateHookJSON(value, depth: depth + 1, limit: limit)
        }
    default:
        break
    }
    if depth == 0, encodeHookJSON(value).count > limit {
        throw HookFailure("input-limit")
    }
}

private func encodeHookJSON(_ value: HookValue) -> Data {
    var output: [UInt8] = []
    writeHookValue(value, to: &output)
    return Data(output)
}

private func writeHookValue(_ value: HookValue, to output: inout [UInt8]) {
    switch value {
    case .null: output.append(contentsOf: "null".utf8)
    case let .boolean(value): output.append(contentsOf: (value ? "true" : "false").utf8)
    case let .integer(value): output.append(contentsOf: String(value).utf8)
    case let .number(value): output.append(contentsOf: javascriptNumber(value).utf8)
    case let .string(value): writeHookString(value, to: &output)
    case let .array(values):
        output.append(91)
        for (index, value) in values.enumerated() {
            if index > 0 {
                output.append(44)
            }
            writeHookValue(value, to: &output)
        }
        output.append(93)
    case let .object(values):
        output.append(123)
        for (index, value) in values.enumerated() {
            if index > 0 {
                output.append(44)
            }
            writeHookString(value.0, to: &output)
            output.append(58)
            writeHookValue(value.1, to: &output)
        }
        output.append(125)
    }
}

private func writeHookString(_ value: HookText, to output: inout [UInt8]) {
    output.append(34)
    var index = 0
    while index < value.utf16CodeUnits.count {
        let unit = value.utf16CodeUnits[index]
        switch unit {
        case 0x22: output.append(contentsOf: "\\\"".utf8)
        case 0x5C: output.append(contentsOf: "\\\\".utf8)
        case 0x08: output.append(contentsOf: "\\b".utf8)
        case 0x0C: output.append(contentsOf: "\\f".utf8)
        case 0x0A: output.append(contentsOf: "\\n".utf8)
        case 0x0D: output.append(contentsOf: "\\r".utf8)
        case 0x09: output.append(contentsOf: "\\t".utf8)
        case 0x00 ... 0x1F:
            output.append(contentsOf: String(format: "\\u%04x", unit).utf8)
        case 0xD800 ... 0xDBFF
            where index + 1 < value.utf16CodeUnits.count &&
            (0xDC00 ... 0xDFFF).contains(value.utf16CodeUnits[index + 1]):
            let high = UInt32(unit - 0xD800)
            let low = UInt32(value.utf16CodeUnits[index + 1] - 0xDC00)
            if let scalar = Unicode.Scalar(0x10000 + (high << 10) + low) {
                output.append(contentsOf: String(scalar).utf8)
                index += 1
            }
        case 0xD800 ... 0xDFFF:
            output.append(contentsOf: String(format: "\\u%04x", unit).utf8)
        default:
            if let scalar = Unicode.Scalar(UInt32(unit)) {
                output.append(contentsOf: String(scalar).utf8)
            }
        }
        index += 1
    }
    output.append(34)
}

private func javascriptNumber(_ value: Double) -> String {
    if value == 0 {
        return "0"
    }
    let shortest = String(value)
    let negative = shortest.hasPrefix("-")
    let unsigned = negative ? String(shortest.dropFirst()) : shortest
    let exponentParts = unsigned.lowercased().split(
        separator: "e",
        maxSplits: 1,
        omittingEmptySubsequences: false,
    )
    let rawCoefficient = String(exponentParts[0])
    let coefficient = rawCoefficient.hasSuffix(".0")
        ? String(rawCoefficient.dropLast(2))
        : rawCoefficient
    let explicitExponent = exponentParts.count == 2 ? Int(exponentParts[1]) ?? 0 : 0
    let digits = coefficient.replacingOccurrences(of: ".", with: "")
    let decimalIndex = (coefficient.firstIndex(of: ".").map {
        coefficient.distance(from: coefficient.startIndex, to: $0)
    } ?? coefficient.count) + explicitExponent
    let exponent = decimalIndex - 1
    let sign = negative ? "-" : ""
    if (-6 ..< 21).contains(exponent) {
        if decimalIndex <= 0 {
            return "\(sign)0.\(String(repeating: "0", count: -decimalIndex))\(digits)"
        }
        if decimalIndex >= digits.count {
            return sign + digits + String(repeating: "0", count: decimalIndex - digits.count)
        }
        let split = digits.index(digits.startIndex, offsetBy: decimalIndex)
        return "\(sign)\(digits[..<split]).\(digits[split...])"
    }
    let tail = digits.count == 1 ? "" : ".\(digits.dropFirst())"
    return "\(sign)\(digits.first.map(String.init) ?? "0")\(tail)e\(exponent >= 0 ? "+" : "")\(exponent)"
}

private func field(_ fields: [(HookText, HookValue)], _ name: String) -> HookValue? {
    fields.first(where: { $0.0.equals(name) })?.1
}

private func isOne(_ value: HookValue) -> Bool {
    switch value {
    case .integer(1): true
    case let .number(value): value.bitPattern == 1.0.bitPattern
    default: false
    }
}

private extension UInt8 {
    var hexDigit: UInt8? {
        switch self {
        case 48 ... 57: self - 48
        case 65 ... 70: self - 65 + 10
        case 97 ... 102: self - 97 + 10
        default: nil
        }
    }
}

private extension String {
    init?(decodingExactUTF16 units: [UInt16]) {
        var index = 0
        var scalars: [Unicode.Scalar] = []
        while index < units.count {
            let unit = units[index]
            if (0xD800 ... 0xDBFF).contains(unit) {
                guard index + 1 < units.count, (0xDC00 ... 0xDFFF).contains(units[index + 1]) else {
                    return nil
                }
                let value = 0x10000 + (UInt32(unit - 0xD800) << 10) + UInt32(units[index + 1] - 0xDC00)
                guard let scalar = Unicode.Scalar(value) else { return nil }
                scalars.append(scalar)
                index += 2
            } else {
                guard !(0xDC00 ... 0xDFFF).contains(unit), let scalar = Unicode.Scalar(UInt32(unit)) else {
                    return nil
                }
                scalars.append(scalar)
                index += 1
            }
        }
        self = String(String.UnicodeScalarView(scalars))
    }
}
