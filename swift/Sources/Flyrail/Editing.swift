import CryptoKit
import Foundation

public func editDocument(
    source: Data?,
    requests: [DocumentRequest],
    ownership: [OwnedSelection] = [],
    provenance: DocumentProvenance = DocumentProvenance(),
) throws -> DocumentEdit {
    let format = try validateEditingInputs(requests: requests, ownership: ownership)
    let decoded = try decodeDocument(source)
    guard !requests.isEmpty else {
        return DocumentEdit(
            before: source,
            after: source,
            ownership: ownership,
            provenance: provenance,
            observations: [],
        )
    }
    if format == nil {
        return try editSections(
            source: source,
            payload: decoded.payload,
            bom: decoded.bom,
            requests: requests,
            ownership: ownership,
            provenance: provenance,
        )
    }
    return try editStructured(
        source: source,
        payload: decoded.payload,
        bom: decoded.bom,
        format: format ?? .json,
        requests: requests,
        ownership: ownership,
        provenance: provenance,
    )
}

package func observeSelections(source: Data?, ownership: [OwnedSelection]) throws -> [EditObservation] {
    guard !ownership.isEmpty else { return [] }
    let format = try validateEditingInputs(requests: [], ownership: ownership)
    let decoded = try decodeDocument(source)
    let backend = try source != nil ? format.map { try editingBackend(data: decoded.payload, format: $0) } : nil
    let pairs = ownership.compactMap { item -> SectionBoundaries? in
        guard case let .section(pair) = item.claim.selector else { return nil }
        return pair
    }
    let spans = try format == nil ? sectionSpans(in: decoded.payload, boundaries: pairs) : [:]
    return try ownership.map { item in
        let current: SelectionSnapshot?
        if source == nil {
            current = nil
        } else if case let .section(pair) = item.claim.selector {
            current = spans[pair].map { sectionSnapshot(decoded.payload.subdata(in: $0.start ..< $0.end)) }
        } else if case let .structured(selector) = item.claim.selector, let backend {
            current = try structuredSnapshot(backend, selector: selector)
        } else {
            throw FlyrailValueError("document selection requires its recorded format")
        }
        return EditObservation(
            claim: item.claim,
            status: selectionStatus(current: current, desired: item.installed.value, owned: item,
                                    acquisition: .conflict, format: format),
            current: current,
        )
    }
}

private struct DecodedDocument {
    let payload: Data
    let bom: Data
}

private struct SectionSpan {
    let start: Int
    let end: Int
}

private struct SectionEdit {
    let start: Int
    let end: Int
    let replacement: Data
}

private func decodeDocument(_ source: Data?) throws -> DecodedDocument {
    guard let source else {
        return DecodedDocument(payload: Data(), bom: Data())
    }
    let bom = source.starts(with: [0xEF, 0xBB, 0xBF])
        ? Data([0xEF, 0xBB, 0xBF])
        : Data()
    let payload = source.dropFirst(bom.count)
    _ = try decodeEditingUTF8(Data(payload))
    guard !payload.contains(0) else {
        throw FlyrailValueError("documents cannot contain NUL")
    }
    return DecodedDocument(payload: Data(payload), bom: bom)
}

private func validateEditingInputs(
    requests: [DocumentRequest],
    ownership: [OwnedSelection],
) throws -> DocumentFormat? {
    guard Set(requests.map(\.claim)).count == requests.count,
          Set(ownership.map(\.claim)).count == ownership.count
    else {
        throw FlyrailValueError("duplicate document claim")
    }
    var claims: [Claim] = []
    for claim in requests.map(\.claim) + ownership.map(\.claim)
        where !claims.contains(claim)
    {
        claims.append(claim)
    }
    for index in claims.indices {
        guard !claims[(index + 1)...].contains(where: { claims[index].overlaps($0) }) else {
            throw FlyrailValueError("document claims overlap")
        }
    }
    var formats = Set(ownership.map(\.format))
    for request in requests {
        if case let .structured(content)? = request.content {
            formats.insert(content.format)
        } else if request.content != nil {
            formats.insert(nil)
        }
    }
    guard formats.count <= 1 else {
        throw FlyrailValueError("one document must use one editor format")
    }
    return formats.first ?? nil
}

// swiftlint:disable:next function_body_length function_parameter_count
private func editSections(
    source: Data?,
    payload: Data,
    bom: Data,
    requests: [DocumentRequest],
    ownership previous: [OwnedSelection],
    provenance: DocumentProvenance,
) throws -> DocumentEdit {
    let pairs = try (requests.map(\.claim) + previous.map(\.claim)).map { claim in
        guard case let .section(pair) = claim.selector else {
            throw FlyrailValueError("retirement requires existing ownership and its document format")
        }
        return pair
    }
    let spans = try sectionSpans(in: payload, boundaries: pairs)
    var owned = previous
    var observations: [EditObservation] = []
    var edits: [SectionEdit] = []
    var additions: [(DocumentRequest, Data, EditStatus)] = []
    let documentNewline = firstNewline(in: payload)

    for request in requests {
        guard case let .section(pair) = request.claim.selector else {
            throw FlyrailValueError("text editor requires section boundaries")
        }
        let span = spans[pair]
        let current = span.map { sectionSnapshot(payload.subdata(in: $0.start ..< $0.end)) }
        let selectedNewline = span.map {
            firstNewline(in: payload.subdata(in: $0.start ..< $0.end))
        } ?? documentNewline
        let desired: Data? = if case let .section(content)? = request.content {
            try generatedSection(content, newline: selectedNewline)
        } else {
            nil
        }
        let oldIndex = owned.firstIndex { $0.claim == request.claim }
        let old = oldIndex.map { owned[$0] }
        let status = selectionStatus(
            current: current,
            desired: desired.map(SnapshotValue.bytes),
            owned: old,
            acquisition: request.acquisition,
            format: nil,
        )
        observations.append(.init(claim: request.claim, status: status, current: current))
        if status == .conflict || status == .modified {
            continue
        }
        if status == .removed {
            guard let old, let span else {
                throw FlyrailValueError("retirement requires existing ownership")
            }
            var start = span.start
            let replacement = old.baseline?.syntax ?? Data()
            if replacement.isEmpty, !old.separator.isEmpty, start >= old.separator.count {
                let separatorStart = start - old.separator.count
                let before = payload.subdata(in: separatorStart ..< start)
                let intersects = spans.values.contains {
                    separatorStart < $0.end && $0.start < start
                }
                if before == old.separator, !intersects {
                    start = separatorStart
                }
            }
            edits.append(.init(start: start, end: span.end, replacement: replacement))
            owned.removeAll { $0.claim == request.claim }
            continue
        }
        guard let desired else {
            continue
        }
        if let span {
            if status == .updated || status == .takenOver {
                edits.append(.init(start: span.start, end: span.end, replacement: desired))
            }
            let installed = if status == .updated || status == .takenOver {
                sectionSnapshot(desired)
            } else {
                current
            }
            guard let installed else {
                throw FlyrailValueError("selection is missing")
            }
            let selection = try updatedOwnership(
                request: request,
                format: nil,
                installed: installed,
                current: current,
                previous: old,
                status: status,
            )
            replaceOwned(&owned, selection)
        } else {
            additions.append((request, desired, status))
        }
    }

    let ordered = edits.sorted { $0.start < $1.start }
    for pair in zip(ordered, ordered.dropFirst()) where pair.0.end > pair.1.start {
        throw FlyrailValueError("section edits overlap")
    }
    var result = payload
    for edit in ordered.reversed() {
        result.replaceSubrange(edit.start ..< edit.end, with: edit.replacement)
    }
    for (request, desired, status) in additions {
        let newline = firstNewline(in: result)
        let separator: Data = if result.isEmpty || result.suffix(newline.count * 2) == newline + newline {
            Data()
        } else if result.suffix(newline.count) == newline {
            newline
        } else {
            newline + newline
        }
        result.append(separator)
        result.append(desired)
        let selection = try updatedOwnership(
            request: request,
            format: nil,
            installed: sectionSnapshot(desired),
            current: nil,
            previous: nil,
            status: status,
            separator: separator,
        )
        replaceOwned(&owned, selection)
    }
    _ = try sectionSpans(in: result, boundaries: pairs)
    let creation = try DocumentProvenance(
        emptyDocument: source == nil && !owned.isEmpty ? Data() : provenance.emptyDocument,
        containers: provenance.containers,
        trailingSeparators: provenance.trailingSeparators,
        schemaFields: provenance.schemaFields,
        schemaEmptyDocument: provenance.schemaEmptyDocument,
    )
    var withBOM = bom
    withBOM.append(result)
    return try editingResult(
        before: source,
        after: withBOM,
        previous: previous,
        owned: owned,
        oldProvenance: provenance,
        provenance: creation,
        observations: observations,
    )
}

private func sectionSpans(
    in data: Data,
    boundaries: [SectionBoundaries],
) throws -> [SectionBoundaries: SectionSpan] {
    var pairs = Set(boundaries)
    for line in documentLines(data) {
        let bare = data.subdata(in: line.bare)
        if let standard = try standardBoundary(bare) {
            try pairs.insert(SectionBoundaries(marker: standard.marker))
        }
    }
    var markers: [Data: (SectionBoundaries, Bool)] = [:]
    for pair in pairs {
        for (text, opening) in [(pair.start, true), (pair.end, false)] {
            let bytes = Data(text.utf8)
            guard markers[bytes] == nil else {
                throw FlyrailValueError("section boundaries overlap")
            }
            markers[bytes] = (pair, opening)
        }
    }
    var spans: [SectionBoundaries: SectionSpan] = [:]
    var opened: (SectionBoundaries, Int)?
    for line in documentLines(data) {
        let bare = data.subdata(in: line.bare)
        if let marker = markers[bare] {
            if marker.1 {
                guard opened == nil, spans[marker.0] == nil else {
                    throw FlyrailValueError("nested or repeated section boundaries")
                }
                opened = (marker.0, line.full.lowerBound)
            } else {
                guard opened?.0 == marker.0 else {
                    throw FlyrailValueError("malformed or intersecting section boundaries")
                }
                spans[marker.0] = .init(start: opened?.1 ?? 0, end: line.full.upperBound)
                opened = nil
            }
        } else {
            let hasFlyrail = bare.range(of: Data("<!-- flyrail:".utf8)) != nil
            let hasKnown = markers.keys.contains { bare.range(of: $0) != nil }
            guard !hasFlyrail, !hasKnown else {
                throw FlyrailValueError("section boundaries must occupy complete lines")
            }
        }
    }
    guard opened == nil else {
        throw FlyrailValueError("section has no closing boundary")
    }
    return spans
}

private struct DocumentLine {
    let bare: Range<Int>
    let full: Range<Int>
}

private func documentLines(_ data: Data) -> [DocumentLine] {
    let bytes = Array(data)
    var lines: [DocumentLine] = []
    var start = 0
    var index = 0
    while index < bytes.count {
        if bytes[index] == 10 || bytes[index] == 13 {
            let newlineEnd = bytes[index] == 13 && index + 1 < bytes.count && bytes[index + 1] == 10
                ? index + 2
                : index + 1
            lines.append(.init(bare: start ..< index, full: start ..< newlineEnd))
            start = newlineEnd
            index = newlineEnd
        } else {
            index += 1
        }
    }
    if start < bytes.count {
        lines.append(.init(bare: start ..< bytes.count, full: start ..< bytes.count))
    }
    return lines
}

private func standardBoundary(_ data: Data) throws -> (marker: String, opening: Bool)? {
    guard let line = String(data: data, encoding: .utf8),
          line.hasPrefix("<!-- flyrail:"),
          line.hasSuffix(" -->")
    else {
        return nil
    }
    let body = line.dropFirst("<!-- flyrail:".count).dropLast(" -->".count)
    if body.hasSuffix(":start") {
        return (String(body.dropLast(":start".count)), true)
    }
    if body.hasSuffix(":end") {
        return (String(body.dropLast(":end".count)), false)
    }
    return nil
}

private func firstNewline(in data: Data) -> Data {
    let bytes = Array(data)
    for index in bytes.indices {
        if bytes[index] == 13 {
            return index + 1 < bytes.count && bytes[index + 1] == 10
                ? Data([13, 10])
                : Data([13])
        }
        if bytes[index] == 10 {
            return Data([10])
        }
    }
    return Data([10])
}

private func generatedSection(_ content: SectionContent, newline: Data) throws -> Data {
    let selector = try content.selector
    var output = Data(selector.start.utf8)
    output.append(newline)
    let body = normalizeNewlines(Data(content.text.utf8), to: newline)
    output.append(body)
    if !body.isEmpty, !body.hasSuffix(newline) {
        output.append(newline)
    }
    output.append(Data(selector.end.utf8))
    output.append(newline)
    return output
}

private func normalizeNewlines(_ data: Data, to newline: Data) -> Data {
    let bytes = Array(data)
    var output = Data()
    var index = 0
    while index < bytes.count {
        if bytes[index] == 13 {
            output.append(newline)
            index += index + 1 < bytes.count && bytes[index + 1] == 10 ? 2 : 1
        } else if bytes[index] == 10 {
            output.append(newline)
            index += 1
        } else {
            output.append(bytes[index])
            index += 1
        }
    }
    return output
}

private func sectionSnapshot(_ data: Data) -> SelectionSnapshot {
    .init(value: .bytes(data), syntax: data, ownedSyntax: data)
}

// swiftlint:disable:next cyclomatic_complexity function_body_length function_parameter_count
private func editStructured(
    source: Data?,
    payload: Data,
    bom: Data,
    format: DocumentFormat,
    requests: [DocumentRequest],
    ownership previous: [OwnedSelection],
    provenance: DocumentProvenance,
) throws -> DocumentEdit {
    let selectors = try (requests.map(\.claim) + previous.map(\.claim)).map { claim in
        guard case let .structured(selector) = claim.selector else {
            throw FlyrailValueError("structured editor requires a Selector")
        }
        return selector
    }
    let rootArray = selectors.contains {
        if case .member? = $0.parts.first {
            true
        } else {
            false
        }
    }
    let empty = format == .toml ? Data() : Data(rootArray ? "[]".utf8 : "{}".utf8)
    let initial = source == nil ? empty : payload
    var backend = try editingBackend(data: initial, format: format)
    var owned = previous
    var containers = provenance.containers
    var observations: [EditObservation] = []
    var preimages: [Claim: SelectionSnapshot?] = [:]
    for request in requests {
        guard case let .structured(selector) = request.claim.selector else {
            throw FlyrailValueError("structured editor requires a Selector")
        }
        preimages[request.claim] = try structuredSnapshot(backend, selector: selector)
    }

    var batchGroups: [[OwnedSelection]] = []
    let retiringTakeovers = requests.compactMap { request -> OwnedSelection? in
        guard request.content == nil,
              let old = previous.first(where: { $0.claim == request.claim }),
              old.baseline != nil,
              case let .structured(selector) = request.claim.selector,
              case .member? = selector.parts.last,
              selectionStatus(
                  current: preimages[request.claim] ?? nil,
                  desired: nil,
                  owned: old,
                  acquisition: request.acquisition,
                  format: format,
              ) == .removed
        else {
            return nil
        }
        return old
    }
    for selection in retiringTakeovers {
        guard case let .structured(selector) = selection.claim.selector else {
            continue
        }
        let prefix = Array(selector.parts.dropLast())
        if let index = batchGroups.firstIndex(where: {
            guard case let .structured(first)? = $0.first?.claim.selector else { return false }
            return Array(first.parts.dropLast()) == prefix
        }) {
            batchGroups[index].append(selection)
        } else {
            batchGroups.append([selection])
        }
    }
    let batched = batchGroups.filter { $0.count > 1 }
    var retired = Set<Claim>()

    for request in requests {
        guard case let .structured(selector) = request.claim.selector else {
            throw FlyrailValueError("structured editor requires a Selector")
        }
        let current = preimages[request.claim] ?? nil
        let content: StructuredContent? = if case let .structured(content)? = request.content {
            content
        } else {
            nil
        }
        let desired = content?.value
        let old = previous.first { $0.claim == request.claim }
        var status = selectionStatus(
            current: current,
            desired: desired.map(SnapshotValue.typed),
            owned: old,
            acquisition: request.acquisition,
            format: format,
        )
        if status == .conflict || status == .modified {
            observations.append(.init(claim: request.claim, status: status, current: current))
            continue
        }
        do {
            let candidate = if status == .current || status == .adopted, content?.placement == nil {
                backend
            } else {
                try editingBackend(data: backend.data, format: format)
            }
            if status == .removed {
                guard let old else {
                    throw FlyrailValueError("retirement requires existing ownership")
                }
                if let group = batched.first(where: { $0.contains(where: { $0.claim == request.claim }) }),
                   !retired.contains(request.claim)
                {
                    try restoreBatch(candidate, selections: group)
                    retired.formUnion(group.map(\.claim))
                } else if !retired.contains(request.claim) {
                    try restoreStructured(candidate, selector: selector, baseline: old.baseline)
                }
                owned.removeAll { $0.claim == request.claim }
            } else if let content {
                if status != .adopted && status != .current || content.placement != nil {
                    let before = candidate.data
                    try putStructured(candidate, content: content, containers: &containers)
                    if status == .current, candidate.data != before {
                        status = .updated
                    }
                }
                guard let installed = try structuredSnapshot(candidate, selector: selector),
                      installed.value == .typed(content.value)
                else {
                    throw FlyrailValueError("published selection differs from the requested value")
                }
                try replaceOwned(
                    &owned,
                    updatedOwnership(
                        request: request,
                        format: format,
                        installed: installed,
                        current: current,
                        previous: old,
                        status: status,
                    ),
                )
            }
            backend = candidate
            observations.append(.init(claim: request.claim, status: status, current: current))
        } catch let error as FlyrailValueError {
            if error.message == "document nesting is too deep" {
                throw error
            }
            observations.append(.init(
                claim: request.claim,
                status: .conflict,
                current: current,
                message: error.message,
            ))
        }
    }

    if observations.contains(where: { $0.status == .conflict || $0.status == .modified }) {
        return DocumentEdit(
            before: source,
            after: source,
            ownership: previous,
            provenance: provenance,
            observations: observations,
        )
    }
    containers = try pruneContainers(backend, containers: containers, owned: owned)
    var separators = provenance.trailingSeparators
    if format == .toml, source != nil, !payload.isEmpty,
       payload.last != 10, payload.last != 13, backend.data.last == 10
    {
        let digest = SHA256.hash(data: bom + payload).map { String(format: "%02x", $0) }.joined()
        if !separators.contains(digest) {
            separators.append(digest)
        }
    }
    let creation = try DocumentProvenance(
        emptyDocument: source == nil && !owned.isEmpty ? empty : provenance.emptyDocument,
        containers: containers,
        trailingSeparators: separators,
        schemaFields: provenance.schemaFields,
        schemaEmptyDocument: provenance.schemaEmptyDocument,
    )
    var result = bom
    result.append(backend.data)
    return try editingResult(
        before: source,
        after: result,
        previous: previous,
        owned: owned,
        oldProvenance: provenance,
        provenance: creation,
        observations: observations,
    )
}

package func structuredSnapshot(
    _ backend: any StructuredEditingDocument,
    selector: Selector,
) throws -> SelectionSnapshot? {
    guard let path = try resolve(backend, selector: selector) else {
        return nil
    }
    let value = try backend.native(path)
    try checkEditingFrames(value)
    let syntax = try backend.syntax(path)
    let position: ArrayPosition? = if case let .member(member)? = selector.parts.last {
        try arrayPosition(backend, path: path, member: member)
    } else {
        nil
    }
    return try .init(
        value: .typed(value),
        syntax: syntax,
        ownedSyntax: backend.ownedSyntax(path),
        position: position,
        layout: backend.layout(path),
    )
}

private func resolve(
    _ backend: any StructuredEditingDocument,
    selector: Selector,
) throws -> [DocumentPathPart]? {
    var path: [DocumentPathPart] = []
    for part in selector.parts {
        let value = try backend.native(path)
        switch part {
        case let .key(key):
            guard case let .object(items) = value else {
                throw FlyrailValueError("object key selector crosses a non-object")
            }
            guard items.contains(where: { exactTextEqual($0.0, key.name) }) else {
                return nil
            }
            path.append(.key(key.name))
        case let .member(member):
            guard case let .array(items) = value else {
                throw FlyrailValueError("member selector requires an array")
            }
            guard let index = try memberIndex(items, member: member) else {
                return nil
            }
            path.append(.index(index))
        }
    }
    return path
}

private func selectedIdentity(_ value: SemanticValue, member: Member) throws -> SemanticValue? {
    var current = value
    for name in member.key {
        guard case let .object(items) = current,
              let next = items.first(where: { exactTextEqual($0.0, name) })?.1
        else {
            return nil
        }
        current = next
    }
    try checkEditingFrames(current, identity: true)
    if member.key.isEmpty {
        return current
    }
    if case .scalar = current {
        return current
    }
    return nil
}

private func memberIndex(_ array: [SemanticValue], member: Member) throws -> Int? {
    var seen = Set<SemanticValue>()
    var found: Int?
    for (index, item) in array.enumerated() {
        guard let identity = try selectedIdentity(item, member: member) else {
            continue
        }
        guard seen.insert(identity).inserted else {
            throw FlyrailValueError("duplicate or ambiguous array member identities")
        }
        if identity == member.identity {
            found = index
        }
    }
    return found
}

private func anchor(
    _ array: [SemanticValue],
    index: Int,
    key: [String],
) throws -> Member {
    let probe = try Member(identity: .scalar(.null), key: key)
    let result: Member
    if let identity = try selectedIdentity(array[index], member: probe) {
        result = try Member(identity: identity, key: key)
    } else {
        try checkEditingFrames(array[index], identity: true)
        result = try Member(identity: array[index])
    }
    guard try memberIndex(array, member: result) == index else {
        throw FlyrailValueError("array position has an ambiguous anchor")
    }
    return result
}

private func arrayPosition(
    _ backend: any StructuredEditingDocument,
    path: [DocumentPathPart],
    member: Member,
) throws -> ArrayPosition {
    guard case let .array(array) = try backend.native(Array(path.dropLast())),
          case let .index(index)? = path.last
    else {
        throw FlyrailValueError("array position requires an array member")
    }
    return try .init(
        previous: index > 0 ? anchor(array, index: index - 1, key: member.key) : nil,
        following: index + 1 < array.count ? anchor(array, index: index + 1, key: member.key) : nil,
        prefix: backend.positionPrefix(path),
    )
}

private func putStructured(
    _ backend: any StructuredEditingDocument,
    content: StructuredContent,
    containers: inout [CreatedContainer],
) throws {
    let selector = content.selector
    guard let part = selector.parts.last else {
        throw FlyrailValueError("a selector must select a key or member")
    }
    let parent = try createParents(backend, selector: selector, containers: &containers)
    let current = try backend.native(parent)
    switch part {
    case let .key(key):
        guard case .object = current else {
            throw FlyrailValueError("object key selector crosses a non-object")
        }
        try backend.put(parent + [.key(key.name)], value: content.value)
    case let .member(member):
        guard case let .array(array) = current else {
            throw FlyrailValueError("member selector requires an array")
        }
        guard try selectedIdentity(content.value, member: member) == member.identity else {
            throw FlyrailValueError("generated array member does not match its selector")
        }
        if let index = try memberIndex(array, member: member) {
            var remaining = array
            remaining.remove(at: index)
            let insertion = try insertionIndex(remaining, placement: content.placement)
            if content.placement == nil || insertion == index {
                try backend.put(parent + [.index(index)], value: content.value)
            } else {
                try backend.relocate(
                    parent + [.index(index)],
                    index: insertion,
                    value: content.value,
                )
            }
        } else {
            let insertion = try insertionIndex(array, placement: content.placement)
            try backend.insert(parent, index: insertion, value: content.value)
        }
    }
}

private func createParents(
    _ backend: any StructuredEditingDocument,
    selector: Selector,
    containers: inout [CreatedContainer],
) throws -> [DocumentPathPart] {
    var path: [DocumentPathPart] = []
    for index in 0 ..< max(0, selector.parts.count - 1) {
        let part = selector.parts[index]
        let node = try backend.native(path)
        switch part {
        case let .key(key):
            guard case let .object(items) = node else {
                throw FlyrailValueError("object key selector crosses a non-object")
            }
            path.append(.key(key.name))
            if !items.contains(where: { exactTextEqual($0.0, key.name) }) {
                let value: SemanticValue = if case .key = selector.parts[index + 1] {
                    try SemanticValue(object: [])
                } else {
                    .array([])
                }
                try backend.put(path, value: value)
                let created = try Selector(parts: Array(selector.parts.prefix(index + 1)))
                try containers.append(.init(selector: created, emptySyntax: backend.emptySyntax(path)))
            }
        case let .member(member):
            guard case let .array(items) = node else {
                throw FlyrailValueError("member selector requires an array")
            }
            guard let selected = try memberIndex(items, member: member) else {
                throw FlyrailValueError("intermediate array member is missing")
            }
            path.append(.index(selected))
        }
    }
    return path
}

private func insertionIndex(
    _ array: [SemanticValue],
    placement: Placement?,
) throws -> Int {
    let position = placement?.position ?? .last
    switch position {
    case .first:
        return 0
    case .last:
        return array.count
    case .before, .after:
        guard let anchor = placement?.anchor,
              let index = try memberIndex(array, member: anchor)
        else {
            throw FlyrailValueError("array placement anchor is missing")
        }
        return index + (position == .after ? 1 : 0)
    }
}

private func restoreStructured(
    _ backend: any StructuredEditingDocument,
    selector: Selector,
    baseline: SelectionSnapshot?,
) throws {
    guard let path = try resolve(backend, selector: selector) else {
        throw FlyrailValueError("owned selection is missing")
    }
    guard let baseline else {
        try backend.delete(path)
        return
    }
    guard case let .typed(value) = baseline.value else {
        throw FlyrailValueError("structured baseline requires a typed value")
    }
    if case .key? = selector.parts.last {
        try backend.put(path, value: value, syntax: baseline.syntax, layout: baseline.layout)
        return
    }
    guard let position = baseline.position,
          case let .array(array) = try backend.native(Array(path.dropLast())),
          case let .index(index)? = path.last
    else {
        throw FlyrailValueError("array baseline requires its original position and typed value")
    }
    var remaining = array
    remaining.remove(at: index)
    let insertion = try restorationIndex(remaining, position: position)
    if insertion == index {
        try backend.put(path, value: value, syntax: baseline.syntax, layout: baseline.layout)
    } else {
        try backend.relocate(
            path,
            index: insertion,
            value: value,
            syntax: baseline.syntax,
            prefix: position.prefix,
            layout: baseline.layout,
        )
    }
}

private func restorationIndex(
    _ array: [SemanticValue],
    position: ArrayPosition,
) throws -> Int {
    let previous = try position.previous.flatMap { try memberIndex(array, member: $0) }
    let following = try position.following.flatMap { try memberIndex(array, member: $0) }
    if let previous, let following, previous >= following {
        throw FlyrailValueError("foreign array order prevents safe restoration")
    }
    if let previous {
        return previous + 1
    }
    if let following {
        return following
    }
    guard array.isEmpty else {
        throw FlyrailValueError("original array position has no surviving anchors")
    }
    return 0
}

private func restoreBatch(
    _ backend: any StructuredEditingDocument,
    selections: [OwnedSelection],
) throws {
    var snapshots: [Int: SelectionSnapshot] = [:]
    var arrayPath: [DocumentPathPart] = []
    var array: [SemanticValue] = []
    for selection in selections {
        guard case let .structured(selector) = selection.claim.selector,
              let baseline = selection.baseline,
              let path = try resolve(backend, selector: selector),
              case let .index(index)? = path.last,
              case let .array(native) = try backend.native(Array(path.dropLast()))
        else {
            throw FlyrailValueError("array restoration requires a baseline and selector")
        }
        arrayPath = Array(path.dropLast())
        array = native
        snapshots[index] = baseline
    }
    var edges = Dictionary(uniqueKeysWithValues: array.indices.map { ($0, Set<Int>()) })
    let foreign = array.indices.filter { snapshots[$0] == nil }
    for pair in zip(foreign, foreign.dropFirst()) {
        edges[pair.1, default: []].insert(pair.0)
    }
    for (index, baseline) in snapshots {
        guard let position = baseline.position else {
            throw FlyrailValueError("array baseline requires its original position")
        }
        let previous = try position.previous.flatMap { try memberIndex(array, member: $0) }
        let following = try position.following.flatMap { try memberIndex(array, member: $0) }
        if previous == nil, following == nil, array.count > 1 {
            throw FlyrailValueError("original array position has no surviving anchors")
        }
        if let previous {
            edges[index, default: []].insert(previous)
        }
        if let following {
            edges[following, default: []].insert(index)
        }
    }
    var order: [Int] = []
    while !edges.isEmpty {
        let ready = edges.keys.sorted {
            let left = (snapshots[$0] == nil ? 1 : 0, $0)
            let right = (snapshots[$1] == nil ? 1 : 0, $1)
            return left < right
        }.first { edges[$0]?.isEmpty == true }
        guard let ready else {
            throw FlyrailValueError("foreign array order prevents safe restoration")
        }
        order.append(ready)
        edges.removeValue(forKey: ready)
        for key in edges.keys {
            edges[key]?.remove(ready)
        }
    }
    var current = Array(array.indices)
    for (target, identity) in order.enumerated() {
        while current[target] != identity, snapshots[identity] == nil {
            let path = arrayPath + [.index(target)]
            let value = try backend.native(path)
            let syntax = try backend.syntax(path)
            try backend.relocate(
                path,
                index: current.count - 1,
                value: value,
                syntax: syntax,
                layout: backend.layout(path),
            )
            current.append(current.remove(at: target))
        }
        guard let baseline = snapshots[identity],
              case let .typed(value) = baseline.value,
              let position = baseline.position
        else {
            continue
        }
        guard let index = current.firstIndex(of: identity) else {
            throw FlyrailValueError("owned array member is missing")
        }
        if index == target {
            try backend.put(arrayPath + [.index(index)], value: value, syntax: baseline.syntax, layout: baseline.layout)
        } else {
            try backend.relocate(
                arrayPath + [.index(index)],
                index: target,
                value: value,
                syntax: baseline.syntax,
                prefix: position.prefix,
                layout: baseline.layout,
            )
            current.insert(current.remove(at: index), at: target)
        }
    }
}

private func pruneContainers(
    _ backend: any StructuredEditingDocument,
    containers: [CreatedContainer],
    owned: [OwnedSelection],
) throws -> [CreatedContainer] {
    var retained: [CreatedContainer] = []
    for container in containers.sorted(by: {
        $0.selector.parts.count > $1.selector.parts.count
    }) {
        guard let path = try resolve(backend, selector: container.selector) else {
            continue
        }
        let value = try backend.native(path)
        let isEmpty = if case let .array(items) = value {
            items.isEmpty
        } else if case let .object(items) = value {
            items.isEmpty
        } else {
            false
        }
        let claim = try Claim(kind: .structured, selector: .structured(container.selector))
        if isEmpty,
           try backend.emptySyntax(path) == container.emptySyntax,
           !owned.contains(where: { claim.overlaps($0.claim) })
        {
            try backend.delete(path)
        } else {
            retained.append(container)
        }
    }
    return retained.reversed()
}

private func selectionStatus(
    current: SelectionSnapshot?,
    desired: SnapshotValue?,
    owned: OwnedSelection?,
    acquisition: Acquisition,
    format: DocumentFormat?,
) -> EditStatus {
    if let owned {
        if current == nil ||
            current?.value != owned.installed.value ||
            (format != .json && current?.ownedSyntax != owned.installed.ownedSyntax)
        {
            return .modified
        }
        if desired == nil {
            return .removed
        }
        return current?.value == desired ? .current : .updated
    }
    guard let desired else {
        return .conflict
    }
    guard let current else {
        return .created
    }
    if acquisition == .takeover {
        return .takenOver
    }
    if acquisition == .adopt, current.value == desired {
        return .adopted
    }
    return .conflict
}

// swiftlint:disable:next function_parameter_count
private func updatedOwnership(
    request: DocumentRequest,
    format: DocumentFormat?,
    installed: SelectionSnapshot,
    current: SelectionSnapshot?,
    previous: OwnedSelection?,
    status: EditStatus,
    separator: Data = Data(),
) throws -> OwnedSelection {
    if let previous {
        return try OwnedSelection(
            claim: previous.claim,
            format: previous.format,
            installed: installed,
            disposition: previous.disposition,
            baseline: previous.baseline,
            separator: previous.separator,
        )
    }
    let disposition: Disposition = switch status {
    case .created:
        .created
    case .takenOver:
        .takenOver
    case .adopted:
        .adopted
    default:
        throw FlyrailValueError("new ownership requires an acquisition status")
    }
    return try OwnedSelection(
        claim: request.claim,
        format: format,
        installed: installed,
        disposition: disposition,
        baseline: disposition == .takenOver ? current : nil,
        separator: separator,
    )
}

private func replaceOwned(_ owned: inout [OwnedSelection], _ selection: OwnedSelection) {
    if let index = owned.firstIndex(where: { $0.claim == selection.claim }) {
        owned[index] = selection
    } else {
        owned.append(selection)
    }
}

// swiftlint:disable:next function_parameter_count
private func editingResult(
    before: Data?,
    after: Data,
    previous: [OwnedSelection],
    owned: [OwnedSelection],
    oldProvenance: DocumentProvenance,
    provenance: DocumentProvenance,
    observations: [EditObservation],
) throws -> DocumentEdit {
    if observations.contains(where: { $0.status == .conflict || $0.status == .modified }) {
        return .init(
            before: before,
            after: before,
            ownership: previous,
            provenance: oldProvenance,
            observations: observations,
        )
    }
    if owned.isEmpty {
        var after = after
        for suffix in [Data([13, 10]), Data([10]), Data([13])] where after.hasSuffix(suffix) {
            let candidate = Data(after.dropLast(suffix.count))
            let digest = SHA256.hash(data: candidate).map { String(format: "%02x", $0) }.joined()
            if oldProvenance.trailingSeparators.contains(digest) {
                after = candidate; break
            }
        }
        let result = provenance.emptyDocument != nil && after == provenance.emptyDocument
            ? nil
            : after
        return .init(
            before: before,
            after: result,
            ownership: [],
            provenance: DocumentProvenance(),
            observations: observations,
        )
    }
    return .init(
        before: before,
        after: after,
        ownership: owned,
        provenance: provenance,
        observations: observations,
    )
}

private extension Data {
    func hasSuffix(_ suffix: Data) -> Bool {
        count >= suffix.count && self[(count - suffix.count)...].elementsEqual(suffix)
    }
}
