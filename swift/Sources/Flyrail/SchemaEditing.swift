import Foundation

public func matchesSchema(source: Data?, requirement: DocumentSchema) throws -> Bool {
    guard let source, !schemaWhitespace(source) else { return false }
    let data = source.starts(with: [0xEF, 0xBB, 0xBF]) ? Data(source.dropFirst(3)) : source
    let backend = try JSONEditingDocument(data: data, comments: requirement.format == .jsonc)
    return try structuredSnapshot(backend, selector: requirement.content.selector)?
        .value == .typed(.scalar(requirement.value))
}

public func editSchemaDocument(
    source: Data?, requests: [DocumentRequest], ownership: [OwnedSelection] = [],
    provenance: DocumentProvenance = DocumentProvenance(),
    activeSchemas: [DocumentSchema], validate: Bool,
) throws -> DocumentEdit {
    let unique = Set(activeSchemas)
    guard unique.count == Set(activeSchemas.map { Data($0.key.utf8) }).count else {
        throw FlyrailValueError("incompatible document schema requirements")
    }
    var requirements: [DocumentSchema] = []
    for schema in activeSchemas where !requirements.contains(schema) {
        requirements.append(schema)
    }
    let schemaClaims = try requirements.map { try contentClaim(.structured($0.content)) }
    guard !schemaClaims.contains(where: { schema in
        (requests.map(\.claim) + ownership.map(\.claim)).contains { schema.overlaps($0) }
    }) else {
        throw FlyrailValueError("document schema overlaps an app-owned field")
    }
    if activeSchemas.isEmpty, provenance.schemaFields.isEmpty, provenance.schemaEmptyDocument == nil {
        return try editDocument(source: source, requests: requests, ownership: ownership, provenance: provenance)
    }
    let normalized = source.map { schemaWhitespace($0) ? Data("{}".utf8) : $0 }
    let emptySource = normalized == source ? provenance.schemaEmptyDocument : source
    let bytes = normalized ?? Data("{}".utf8)
    let body = bytes.starts(with: [0xEF, 0xBB, 0xBF]) ? Data(bytes.dropFirst(3)) : bytes
    let backend = try JSONEditingDocument(data: body, comments: true)
    var additions: [DocumentRequest] = []
    if validate {
        for schema in requirements {
            let current = try structuredSnapshot(backend, selector: schema.content.selector)
            if current == nil {
                try additions.append(.set(.structured(schema.content)))
            } else if current?.value != .typed(.scalar(schema.value)) {
                return .init(
                    before: source, after: source, ownership: ownership, provenance: provenance,
                    observations: requests.map {
                        .init(claim: $0.claim, status: .conflict, current: nil, message: "document schema differs")
                    },
                )
            }
        }
    }
    let created = try provenance.schemaFields.filter { item in
        guard case let .structured(selector) = item.claim.selector else { return false }
        return try structuredSnapshot(backend, selector: selector) == item.installed &&
            !requests.contains { $0.content != nil && $0.claim.overlaps(item.claim) }
    }
    let createdIDs = Set(created.map(\.claim) + additions.map(\.claim))
    let ordinary = try schemaProvenance(provenance, fields: [], empty: nil)
    let edit = try editDocument(
        source: normalized, requests: requests + additions,
        ownership: ownership + created, provenance: ordinary,
    )
    guard edit.applicable else {
        return .init(
            before: source, after: source, ownership: ownership, provenance: provenance,
            observations: edit.observations,
        )
    }
    let fields = edit.ownership.filter { createdIDs.contains($0.claim) }
    let owned = edit.ownership.filter { !createdIDs.contains($0.claim) }
    let observations = edit.observations.filter { !createdIDs.contains($0.claim) }
    if activeSchemas.isEmpty {
        let candidate = try editDocument(
            source: edit.after, requests: fields.map { try .retire($0.claim) },
            ownership: owned + fields, provenance: edit.provenance,
        )
        if candidate.applicable, owned.isEmpty, candidate.after == nil || emptySchemaObject(candidate.after) {
            return .init(
                before: source, after: emptySource ?? candidate.after,
                ownership: candidate.ownership, provenance: candidate.provenance,
                observations: observations,
            )
        }
        return try .init(
            before: source, after: edit.after, ownership: owned,
            provenance: schemaProvenance(edit.provenance, fields: [], empty: owned.isEmpty ? nil : emptySource),
            observations: observations,
        )
    }
    return try .init(
        before: source, after: edit.after, ownership: owned,
        provenance: schemaProvenance(edit.provenance, fields: fields, empty: emptySource),
        observations: observations,
    )
}

private func schemaWhitespace(_ data: Data) -> Bool {
    data.allSatisfy { [9, 10, 11, 12, 13, 32].contains($0) }
}

private func emptySchemaObject(_ data: Data?) -> Bool {
    data?.filter { ![9, 10, 11, 12, 13, 32].contains($0) } == Data("{}".utf8)
}

private func schemaProvenance(
    _ provenance: DocumentProvenance, fields: [OwnedSelection], empty: Data?,
) throws -> DocumentProvenance {
    try .init(
        emptyDocument: provenance.emptyDocument, containers: provenance.containers,
        trailingSeparators: provenance.trailingSeparators,
        schemaFields: fields, schemaEmptyDocument: empty,
    )
}
