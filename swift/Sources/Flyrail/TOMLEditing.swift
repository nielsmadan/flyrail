import Foundation

package final class TOMLEditingDocument: StructuredEditingDocument {
    package private(set) var data: Data
    private var parsed: TOMLScanResult
    private let newline: String

    package init(data: Data) throws {
        self.data = data
        var scanner = try TOMLScanner(data: data)
        parsed = try scanner.parse()
        newline = data.range(of: Data([13, 10])) == nil ? "\n" : "\r\n"
    }

    package func native(_ path: [DocumentPathPart]) throws -> SemanticValue {
        try node(path).value()
    }

    package func syntax(_ path: [DocumentPathPart]) throws -> Data {
        let selected = try node(path)
        _ = try selected.selectedValue()
        if selected.kind != .table, selected.kind != .aot {
            return Data("value = \(text(selected.start, selected.end))\(newline)".utf8)
        }
        return try fragment(path, synthetic: true)
    }

    package func ownedSyntax(_ path: [DocumentPathPart]) throws -> Data {
        let selected = try node(path)
        if selected.kind != .table, selected.kind != .aot {
            _ = try selected.selectedValue()
            return data.subdata(in: selected.start ..< selected.end)
        }
        return try fragment(path, synthetic: false)
    }

    package func layout(_ path: [DocumentPathPart]) throws -> Data {
        let selected = try node(path)
        guard selected.kind == .table || selected.kind == .aot else { return Data() }
        let headers = try selectedStatements(path).filter { $0.header && $0.path != path }
        guard !headers.isEmpty else { return Data() }
        let entries: [JSONValue] = headers.map { statement in
            .array([
                .array(statement.path.dropFirst(path.count).map {
                    switch $0 {
                    case let .key(name): .string(name)
                    case let .index(index): .number(String(index))
                    }
                }),
                .string(statement.spelling),
            ])
        }
        return try Data("{\"version\":1,\"headers\":".utf8) + JSONWriter.canonicalData(.array(entries)) + Data("}".utf8)
    }

    package func emptySyntax(_ path: [DocumentPathPart]) throws -> Data {
        let selected = try node(path)
        if selected.kind != .table, selected.kind != .aot {
            return data.subdata(in: selected.start ..< selected.end)
        }
        let statements = try selectedStatements(path)
        let own = statements.first { $0.header && $0.path == path }
        let start = own?.end ?? statements.first?.start ?? data.count
        let end = tableEnd(path, statements: statements)
        var raw = data.subdata(in: min(start, end) ..< end)
        if let own {
            let suffix = text(own.valueEnd, own.end)
            if let comment = suffix.firstIndex(of: "#") {
                raw.append(contentsOf: suffix[..<comment].utf8)
                raw.append(contentsOf: suffix[comment...].trimmingCharacters(in: .newlines).utf8)
            }
        }
        return raw
    }

    package func positionPrefix(_ path: [DocumentPathPart]) throws -> Data {
        guard case let .index(index)? = path.last else { throw FlyrailValueError("array position requires a member") }
        let parent = try node(Array(path.dropLast()))
        guard parent.kind == .array else { return Data() }
        let start = index == 0 ? parent.start + 1 : parent.commas[index - 1] + 1
        return data.subdata(in: start ..< parent.items[index].start)
    }

    package func put(
        _ path: [DocumentPathPart], value: SemanticValue, syntax: Data?, layout: Data,
    ) throws {
        guard let part = path.last else { throw FlyrailValueError("document selector path is missing") }
        let parentPath = Array(path.dropLast())
        let parent = try node(parentPath)
        let current = try? node(path)
        if case .index = part, parent.kind == .array {
            guard let current else { throw FlyrailValueError("document selector path is missing") }
            try publish(
                [(current.start, current.end, inlineFragment(value, syntax: syntax))],
                path: path,
                value: value,
            )
            return
        }
        if let current, current.kind != .table, current.kind != .aot {
            try publish(
                [(current.start, current.end, inlineFragment(value, syntax: syntax))],
                path: path,
                value: value,
            )
            return
        }
        if let current {
            if current.kind == .table {
                guard case .object = value else { throw representationError() }
            } else {
                guard case .array = value, tomlTableValue(value) else { throw representationError() }
            }
            let statements = try selectedStatements(path)
            let start = statements.first?.start ?? data.count
            let end = statements.last?.end ?? start
            let raw: Data = if current.dotted {
                try dottedReplacement(path, value: value, syntax: syntax)
            } else {
                try tableReplacement(path, value: value, syntax: syntax, layout: layout)
            }
            try publish([(start, end, raw)], path: path, value: value)
            return
        }
        guard case let .key(key) = part else { throw FlyrailValueError("TOML selector crosses a scalar") }
        if parent.kind == .inline {
            guard !tomlTableValue(value) else { throw FlyrailValueError("Inline tables cannot contain a table") }
            let raw = try inlineFragment(value, syntax: syntax)
            let gapStart = parent.fields.last?.1.end ?? parent.start + 1
            let comma = parent.fields.isEmpty ? "" : ","
            let rawField = try Data("\(tomlKey(key)) = ".utf8) + raw
            var patches = [(parent.end - 1, parent.end - 1, rawField)]
            if !comma.isEmpty {
                patches.append((gapStart, gapStart, Data(comma.utf8)))
            }
            try publish(
                patches,
                path: path,
                value: value,
            )
            return
        }
        guard parent.kind == .table else { throw FlyrailValueError("TOML selector crosses a scalar") }
        let raw: String
        let insertion: Int
        if parent.dotted {
            let prefix = try physicalName(parentPath)
            raw =
                try "\(prefix).\(tomlKey(key)) = \(String(decoding: inlineFragment(value, syntax: syntax), as: UTF8.self))\(newline)"
            insertion = try selectedStatements(parentPath).last?.end ?? data.count
        } else if tomlTableValue(value) {
            raw = try reboundFragment(path, value: value, syntax: syntax, layout: layout)
            insertion = try tableEnd(parentPath, statements: selectedStatements(parentPath))
        } else {
            raw =
                try "\(tomlKey(key)) = \(String(decoding: inlineFragment(value, syntax: syntax), as: UTF8.self))\(newline)"
            let statements = try selectedStatements(parentPath)
            insertion = statements.first(where: { $0.header && $0.path != parentPath })?.start ??
                tableEnd(parentPath, statements: statements)
        }
        var prefix = ""
        if insertion > 0, data[insertion - 1] != 10 {
            prefix = newline
        }
        try publish([(insertion, insertion, Data((prefix + raw).utf8))], path: path, value: value)
    }

    // swiftlint:disable:next function_parameter_count
    package func insert(
        _ path: [DocumentPathPart], index: Int, value: SemanticValue,
        syntax: Data?, prefix: Data?, layout: Data,
    ) throws {
        let parent = try node(path)
        guard index >= 0,
              index <= parent.items.count else { throw FlyrailValueError("array insertion index is out of range") }
        if parent.kind == .array {
            let raw = try inlineFragment(value, syntax: syntax)
            var patches: [(Int, Int, Data)]
            if index < parent.items.count {
                let boundary = index == 0 ? parent.start + 1 : parent.commas[index - 1] + 1
                let start = try prefixed(boundary, parent.items[index].start, prefix)
                patches = [(start, start, raw + Data([44]))]
            } else {
                var start = parent.end - 1
                if let prefix {
                    let boundary = parent.commas.count == parent.items.count && !parent.commas.isEmpty
                        ? (parent.commas.last ?? parent.start) + 1
                        : parent.items.last?.end ?? parent.start + 1
                    start = try prefixed(boundary, start, prefix)
                }
                patches = [(start, start, raw)]
                if !parent.items.isEmpty, parent.commas.count < parent.items.count {
                    let end = parent.items.last?.end ?? start
                    patches.append((end, end, Data([44])))
                } else if !parent.commas.isEmpty {
                    patches[0] = (start, start, raw + Data([44]))
                }
            }
            try publishInsertion(patches, path: path, index: index, value: value)
            return
        }
        guard parent.kind == .aot, case .object = value else {
            throw FlyrailValueError("TOML member selector requires an array")
        }
        let members = try selectedStatements(path).filter { $0.header && $0.path.count == path.count + 1 }
        let insertion = try index < members.count ? members[index].start : tableEnd(
            path,
            statements: selectedStatements(path),
        )
        let raw = try reboundFragment(path + [.index(index)], value: value, syntax: syntax, layout: layout)
        try publishInsertion([(insertion, insertion, Data(raw.utf8))], path: path, index: index, value: value)
    }

    package func delete(_ path: [DocumentPathPart]) throws {
        guard let part = path.last else { throw FlyrailValueError("document selector path is missing") }
        let parent = try node(Array(path.dropLast()))
        let current = try node(path)
        if case let .index(index) = part, parent.kind == .array {
            var patches = [(current.start, current.end, Data())]
            if !parent.commas.isEmpty {
                let comma = parent.commas[min(index, parent.commas.count - 1)]
                patches.append((comma, comma + 1, Data()))
            }
            try publishDeletion(patches, path: path)
            return
        }
        if parent.kind == .inline {
            guard case let .key(key) = part,
                  let index = parent.fields.firstIndex(where: { exactTextEqual($0.0, key) }),
                  let keyStart = parent.inlineKeys.first(where: { exactTextEqual($0.0, key) })?.1
            else { throw FlyrailValueError("TOML layout cannot preserve valid syntax") }
            var patches = [(keyStart, current.end, Data())]
            if !parent.commas.isEmpty {
                let comma = parent.commas[min(index, parent.commas.count - 1)]
                patches.append((comma, comma + 1, Data()))
            }
            try publishDeletion(patches, path: path)
            return
        }
        let statements = try selectedStatements(path)
        guard let first = statements.first, let last = statements.last else {
            throw FlyrailValueError("document selector path is missing")
        }
        let own = statements.first { $0.header && $0.path == path }
        let live = own ?? (current.kind == .scalar || current.kind == .array || current.kind == .inline ? first : nil)
        let comment = live.map(detachedComment) ?? Data()
        var replacement = comment
        if parent.kind == .table, !parent.explicit, !parent.dotted,
           !Array(path.dropLast()).isEmpty, parent.fields.count == 1
        {
            replacement = try Data("[\(physicalName(Array(path.dropLast())))]\(newline)".utf8) + replacement
        }
        try publishDeletion([(first.start, last.end, replacement)], path: path)
    }

    // swiftlint:disable:next function_parameter_count
    package func relocate(
        _ path: [DocumentPathPart], index: Int, value: SemanticValue,
        syntax: Data?, prefix: Data?, layout: Data,
    ) throws {
        let parentPath = Array(path.dropLast())
        let parent = try node(parentPath)
        if parent.kind == .aot {
            let statements = try selectedStatements(path)
            guard let first = statements.first else {
                throw FlyrailValueError("document selector path is missing")
            }
            let end = tableEnd(path, statements: statements)
            let moved = data.subdata(in: first.start ..< end)
            try patch([(first.start, end, Data())])
            let remaining = try selectedStatements(parentPath)
                .filter { $0.header && $0.path.count == parentPath.count + 1 }
            let insertion = index < remaining.count ? remaining[index].start :
                remaining.last.map { member in tableEnd(
                    member.path,
                    statements: parsed.statements.filter { $0.path.starts(with: member.path) },
                ) } ??
                first.start
            try patch([(insertion, insertion, moved)])
            try put(parentPath + [.index(index)], value: value, syntax: syntax, layout: layout)
        } else {
            try delete(path)
            try insert(parentPath, index: index, value: value, syntax: syntax, prefix: prefix, layout: layout)
        }
    }

    private func node(_ path: [DocumentPathPart]) throws -> TOMLNode {
        try tomlNode(parsed.root, path: path)
    }

    private func selectedStatements(_ path: [DocumentPathPart]) throws -> [TOMLStatement] {
        let indices = parsed.statements.indices.filter { parsed.statements[$0].path.starts(with: path) }
        if let first = indices.first, let last = indices.last,
           indices != Array(first ... last)
        {
            throw FlyrailValueError("a noncontiguous TOML table cannot be an owned unit")
        }
        return indices.map { parsed.statements[$0] }
    }

    private func tableEnd(_ path: [DocumentPathPart], statements: [TOMLStatement]) -> Int {
        if path.isEmpty {
            return data.count
        }
        guard let last = statements.last else { return data.count }
        return parsed.statements.first { $0.start >= last.end && !$0.path.starts(with: path) }?.start ?? data.count
    }

    private func fragment(_ path: [DocumentPathPart], synthetic: Bool) throws -> Data {
        let selected = try node(path)
        _ = try selected.selectedValue()
        let statements = try selectedStatements(path)
        guard let first = statements.first, let last = statements.last else {
            return synthetic ? Data("[value]\(newline)".utf8) : Data()
        }
        let own = statements.first { $0.header && $0.path == path }
        let fragmented = own != nil && own?.start != first.start
        let dottedPrefixes = selected.dotted ? statements.filter { !$0.header }.map { statement in
            statement.keyParts.prefix(max(0, path.count - (statement.path.count - statement.keyParts.count)))
                .map(\.1).joined(separator: ".")
        } : []
        let mixedDotted = dottedPrefixes.contains { !exactTextEqual($0, dottedPrefixes.first ?? "") }
        var previousPrefix: String?
        var patches: [(Int, Int, Data)] = []
        for statement in statements {
            if statement.header {
                if statement.path == path {
                    if synthetic || fragmented {
                        let header = statement.aot ? "[[value]]" : "[value]"
                        patches.append((statement.start, statement.end, Data((header + newline).utf8)))
                    } else {
                        patches.append((statement.start, statement.end, Data()))
                    }
                } else if synthetic || statement.aot {
                    let keyCount = statement.path.dropFirst(path.count).filter {
                        if case .key = $0 {
                            true
                        } else {
                            false
                        }
                    }.count
                    let suffix = statement.keyParts.suffix(keyCount).map(\.1).joined(separator: ".")
                    let name = !synthetic && selected.kind == .table ? suffix :
                        "value" + (keyCount == 0 ? "" : "." + suffix)
                    patches.append((statement.keyStart, statement.keyEnd, Data(name.utf8)))
                }
            } else if selected.dotted {
                let relative = Array(statement.path.dropFirst(path.count))
                let count = relative.count
                let key = statement.keyParts.suffix(count).map(\.1).joined(separator: ".")
                let prefix = statement.keyParts.dropLast(count).map(\.1).joined(separator: ".")
                let header = mixedDotted && previousPrefix.map { !exactTextEqual($0, prefix) } != false
                    ? "[value]\(newline)" : ""
                patches.append((statement.keyStart, statement.keyEnd, Data((header + key).utf8)))
                previousPrefix = prefix
            }
        }
        var raw = data
        for patch in patches.sorted(by: { $0.0 > $1.0 }) {
            raw.replaceSubrange(patch.0 ..< patch.1, with: patch.2)
        }
        let delta = patches.reduce(0) { $0 + $1.2.count - ($1.1 - $1.0) }
        var result = raw.subdata(in: first.start ..< last.end + delta)
        if synthetic, own == nil, selected.dotted, !mixedDotted {
            result = Data("[value]\(newline)".utf8) + result
        }
        return result
    }

    private func physicalName(_ path: [DocumentPathPart]) throws -> String {
        if let header = parsed.statements.first(where: { $0.header && $0.path == path }) {
            return header.spelling
        }
        if (try? node(path).dotted) == true,
           let statement = parsed.statements.first(where: { !$0.header && $0.path.starts(with: path) })
        {
            let contextCount = statement.path.count - statement.keyParts.count
            return statement.keyParts.prefix(max(0, path.count - contextCount)).map(\.1).joined(separator: ".")
        }
        return try path.compactMap { part -> String? in
            if case let .key(key) = part {
                try tomlKey(key)
            } else {
                nil
            }
        }.joined(separator: ".")
    }

    private func inlineFragment(_ value: SemanticValue, syntax: Data?) throws -> Data {
        guard let syntax else {
            let raw = try Data(tomlInline(value).utf8)
            var scanner = try TOMLScanner(data: Data("value = ".utf8) + raw)
            _ = try scanner.parse()
            return raw
        }
        var scanner = try TOMLScanner(data: syntax)
        let result = try scanner.parse()
        let node = try tomlNode(result.root, path: [.key("value")])
        if node.kind != .table, node.kind != .aot {
            return syntax.subdata(in: node.start ..< node.end)
        }
        return try Data(tomlInline(node.value()).utf8)
    }

    private func dottedReplacement(_ path: [DocumentPathPart], value: SemanticValue, syntax: Data?) throws -> Data {
        let prefix = try physicalName(path)
        let spellings = try selectedStatements(path).filter { !$0.header }.map { statement in
            statement.keyParts.prefix(max(0, path.count - (statement.path.count - statement.keyParts.count)))
                .map(\.1).joined(separator: ".")
        }
        guard spellings.allSatisfy({ exactTextEqual($0, prefix) }) else {
            throw FlyrailValueError("TOML layout cannot preserve the selected and foreign values")
        }
        if let syntax {
            var scanner = try TOMLScanner(data: syntax)
            let result = try scanner.parse()
            var patches: [(Int, Int, Data)] = []
            for statement in result.statements {
                if statement.header {
                    guard statement.path == [.key("value")] else {
                        throw FlyrailValueError("TOML layout cannot preserve the selected and foreign values")
                    }
                    patches.append((statement.start, statement.end, Data()))
                    continue
                }
                let keys = statement.path.dropFirst().compactMap { part -> String? in
                    if case let .key(key) = part {
                        key
                    } else {
                        nil
                    }
                }
                let rendered = statement.keyParts.map(\.1).joined(separator: ".")
                let key: String = keys.count == statement.keyParts.count
                    ? rendered : try keys.dropLast(statement.keyParts.count).map(tomlKey)
                    .joined(separator: ".") + "." + rendered
                patches.append((statement.keyStart, statement.keyEnd, Data("\(prefix).\(key)".utf8)))
            }
            var raw = syntax
            for patch in patches.sorted(by: { $0.0 > $1.0 }) {
                raw.replaceSubrange(patch.0 ..< patch.1, with: patch.2)
            }
            return raw
        }
        guard case let .object(fields) = value else { throw representationError() }
        if fields.contains(where: {
            if case .array = $0.1 {
                tomlTableValue($0.1)
            } else {
                false
            }
        }) {
            throw FlyrailValueError("TOML layout cannot preserve the selected and foreign values")
        }
        return try Data(fields.map {
            try "\(prefix).\(tomlKey($0.0)) = \(tomlInline($0.1))\(newline)"
        }.joined().utf8)
    }

    private func tableReplacement(
        _ path: [DocumentPathPart], value: SemanticValue, syntax: Data?, layout: Data,
    ) throws -> Data {
        let statements = try selectedStatements(path)
        let own = statements.first { $0.header && $0.path == path }
        let raw = try reboundFragment(path, value: value, syntax: syntax, layout: layout, forceHeader: own != nil)
        var scanner = try TOMLScanner(data: Data(raw.utf8))
        let replacement = try scanner.parse()
        guard let own else { return Data(raw.utf8) }
        let destination = path.compactMap { part -> String? in
            if case let .key(key) = part {
                key
            } else {
                nil
            }
        }.map { Data($0.utf8) }
        guard let header = replacement.statements.first(where: {
            $0.header && $0.path.compactMap { part -> Data? in
                if case let .key(key) = part {
                    Data(key.utf8)
                } else {
                    nil
                }
            } == destination
        }) else { throw FlyrailValueError("TOML layout cannot preserve valid syntax") }
        var result = Data(raw.utf8)
        result.replaceSubrange(header.start ..< header.end, with: data[own.start ..< own.end])
        return result
    }

    private func reboundFragment(
        _ path: [DocumentPathPart], value: SemanticValue, syntax: Data?, layout: Data, forceHeader: Bool = false,
    ) throws -> String {
        let name = try physicalName(path)
        let element = if case .index? = path.last {
            true
        } else {
            false
        }
        guard let syntax else {
            let synthetic = try tomlGenerated(
                value,
                name: "value",
                newline: newline,
                element: element,
                forceHeader: forceHeader,
            )
            var scanner = try TOMLScanner(data: Data(synthetic.utf8))
            _ = try scanner.parse()
            return try tomlGenerated(value, name: name, newline: newline, element: element, forceHeader: forceHeader)
        }
        var scanner = try TOMLScanner(data: syntax)
        let result = try scanner.parse()
        let headers = try tomlLayoutHeaders(layout)
        var raw = syntax
        var patches: [(Int, Int, Data)] = []
        for statement in result.statements where statement.header {
            var relative = Array(statement.path.dropFirst())
            if element, case .index? = relative.first {
                relative.removeFirst()
            }
            let name: String
            if let stored = headers.first(where: { $0.0 == relative }) {
                let expected = path + relative
                let keys = expected.compactMap { part -> Data? in
                    if case let .key(key) = part {
                        Data(key.utf8)
                    } else {
                        nil
                    }
                }
                guard try tomlHeaderPath(stored.1).map({ Data($0.utf8) }) == keys else {
                    throw FlyrailValueError("TOML restoration header does not match its destination")
                }
                name = stored.1
            } else {
                name = try physicalName(path) + relative.compactMap { part -> String? in
                    if case let .key(key) = part {
                        try "." + tomlKey(key)
                    } else {
                        nil
                    }
                }.joined()
            }
            patches.append((statement.keyStart, statement.keyEnd, Data(name.utf8)))
        }
        if result.statements.first?.header == false {
            guard let statement = result.statements.first else { return "" }
            patches.append((statement.keyStart, statement.keyEnd, Data(name.utf8)))
        }
        for patch in patches.sorted(by: { $0.0 > $1.0 }) {
            raw.replaceSubrange(patch.0 ..< patch.1, with: patch.2)
        }
        return String(decoding: raw, as: UTF8.self)
    }

    private func detachedComment(_ statement: TOMLStatement) -> Data {
        let suffix = text(statement.valueEnd, statement.end)
        guard suffix.contains("#") else { return Data() }
        let indentationEnd = statement.header ? statement.keyStart - (statement.aot ? 2 : 1) : statement.keyStart
        return data.subdata(in: statement.start ..< indentationEnd) + Data(suffix.utf8)
    }

    private func text(_ start: Int, _ end: Int) -> String {
        String(decoding: data[start ..< end], as: UTF8.self)
    }

    private func prefixed(_ start: Int, _ end: Int, _ prefix: Data?) throws -> Int {
        let prefix = prefix ?? Data()
        guard data[start ..< end].starts(with: prefix) else {
            throw FlyrailValueError("original array position trivia changed")
        }
        return start + prefix.count
    }

    private func representationError() -> FlyrailValueError {
        FlyrailValueError("TOML table representation changes cannot preserve foreign layout")
    }

    private func patch(_ patches: [(Int, Int, Data)]) throws {
        var candidate = data
        for patch in patches.sorted(by: { $0.0 == $1.0 ? $0.1 > $1.1 : $0.0 > $1.0 }) {
            candidate.replaceSubrange(patch.0 ..< patch.1, with: patch.2)
        }
        var scanner = try TOMLScanner(data: candidate)
        let result = try scanner.parse()
        data = candidate
        parsed = result
    }

    private func publish(_ patches: [(Int, Int, Data)], path: [DocumentPathPart], value: SemanticValue) throws {
        let expected = try replacingTOMLValue(parsed.root.value(), path: path, value: value)
        let previous = data
        do {
            try patch(patches)
            guard try parsed.root.value() == expected else {
                throw FlyrailValueError("TOML layout cannot preserve the selected and foreign values")
            }
        } catch {
            var scanner = try TOMLScanner(data: previous)
            parsed = try scanner.parse()
            data = previous
            if let error = error as? FlyrailValueError,
               error.message == "TOML layout cannot preserve the selected and foreign values"
            {
                throw error
            }
            throw FlyrailValueError("TOML layout cannot preserve valid syntax")
        }
    }

    private func publishInsertion(
        _ patches: [(Int, Int, Data)], path: [DocumentPathPart], index: Int, value: SemanticValue,
    ) throws {
        guard case var .array(items) = try native(path)
        else { throw FlyrailValueError("member selector requires an array") }
        items.insert(value, at: index)
        try publish(patches, path: path, value: .array(items))
    }

    private func publishDeletion(_ patches: [(Int, Int, Data)], path: [DocumentPathPart]) throws {
        let parentPath = Array(path.dropLast())
        if case .index? = path.last, try node(parentPath).kind == .aot,
           try node(parentPath).items.count == 1
        {
            try publishDeletion(patches, path: parentPath)
            return
        }
        let value: SemanticValue
        switch try (path.last, native(parentPath)) {
        case let (.key(key), .object(fields)):
            value = try SemanticValue(object: fields.filter { !exactTextEqual($0.0, key) })
        case let (.index(index), .array(items)):
            value = .array(items.enumerated().filter { $0.offset != index }.map(\.element))
        default: throw FlyrailValueError("document selector path is missing")
        }
        try publish(patches, path: parentPath, value: value)
    }
}

private func replacingTOMLValue(
    _ current: SemanticValue, path: [DocumentPathPart], value: SemanticValue,
) throws -> SemanticValue {
    guard let first = path.first else { return value }
    let remaining = Array(path.dropFirst())
    switch (first, current) {
    case let (.key(key), .object(fields)):
        if fields.contains(where: { exactTextEqual($0.0, key) }) {
            return try SemanticValue(object: fields.map { name, child in
                try (name, exactTextEqual(name, key) ? replacingTOMLValue(child, path: remaining, value: value) : child)
            })
        }
        guard remaining.isEmpty else { throw FlyrailValueError("document selector path is missing") }
        return try SemanticValue(object: fields + [(key, value)])
    case let (.index(index), .array(items)) where items.indices.contains(index):
        return try .array(items.enumerated().map { offset, child in
            try offset == index ? replacingTOMLValue(child, path: remaining, value: value) : child
        })
    default: throw FlyrailValueError("document selector path is missing")
    }
}
