@testable import Flyrail
import Foundation
import Testing

private enum EditingNesting: CaseIterable {
    case object
    case array
    case alternating

    var snapshotBoundary: Int {
        switch self {
        case .object: 198
        case .array: 248
        case .alternating: 220
        }
    }

    var identityBoundary: Int {
        switch self {
        case .object: 197
        case .array: 247
        case .alternating: 219
        }
    }

    func value(_ depth: Int) throws -> SemanticValue {
        var value = SemanticValue.scalar(.integer(1))
        for level in 0 ..< depth {
            value = objectLevel(level) ? try SemanticValue(object: [("a", value)]) : .array([value])
        }
        return value
    }

    func syntax(_ depth: Int) -> String {
        var syntax = "1"
        for level in 0 ..< depth {
            syntax = objectLevel(level) ? "{\"a\":\(syntax)}" : "[\(syntax)]"
        }
        return syntax
    }

    private func objectLevel(_ level: Int) -> Bool {
        self == .object || (self == .alternating && level.isMultiple(of: 2))
    }
}

@Test
func editorSourceContainerBoundariesMatchReference() throws {
    let request = try keyRequest(.scalar(.integer(2)), acquisition: .takeover)
    for shape in [EditingNesting.object, .array] {
        let accepted = shape.syntax(984)
        let source = Data("{\"x\":1,\"unselected\":\(accepted)}".utf8)
        let edit = try editDocument(source: source, requests: [request])
        #expect(edit.after == Data("{\"x\":2,\"unselected\":\(accepted)}".utf8))
        #expect(edit.observations.map(\.status) == [.takenOver])
        #expect(throws: FlyrailValueError("document nesting is too deep")) {
            try editDocument(
                source: Data("{\"x\":1,\"unselected\":\(shape.syntax(985))}".utf8),
                requests: [request],
            )
        }
    }
    for comments in [false, true] {
        let accepted = Data((String(repeating: "[", count: 985) + String(repeating: "]", count: 985)).utf8)
        #expect(try JSONEditingDocument(data: accepted, comments: comments).syntax([]) == accepted)
        #expect(throws: FlyrailValueError("document nesting is too deep")) {
            try JSONEditingDocument(
                data: Data((String(repeating: "[", count: 986) + String(repeating: "]", count: 986)).utf8),
                comments: comments,
            )
        }
    }
}

@Test
func selectedAndGeneratedValuesEnforceSnapshotFrameBoundaries() throws {
    for format in [DocumentFormat.json, .jsonc] {
        for shape in EditingNesting.allCases {
            let depth = shape.snapshotBoundary - 1
            let selected = shape.syntax(depth)
            let source = Data("{\"x\":\(selected)}".utf8)
            let request = try keyRequest(.scalar(.integer(2)), format: format, acquisition: .takeover)
            let edit = try editDocument(source: source, requests: [request])
            #expect(edit.after == Data("{\"x\":2}".utf8))
            #expect(edit.observations.map(\.status) == [.takenOver])
            #expect(edit.ownership.first?.baseline?.syntax == Data(selected.utf8))

            let generatedRequest = try keyRequest(shape.value(depth), format: format)
            let generated = try editDocument(source: nil, requests: [generatedRequest])
            #expect(generated.after == Data("{\"x\":\(selected)}".utf8))
            #expect(generated.observations.map(\.status) == [.created])
            #expect(generated.provenance.emptyDocument == Data("{}".utf8))

            #expect(throws: FlyrailValueError("document nesting is too deep")) {
                try editDocument(
                    source: Data("{\"x\":\(shape.syntax(depth + 1))}".utf8),
                    requests: [request],
                )
            }
            let tooDeep = try shape.value(depth + 1)
            for source in [nil, Data("{\"x\":1}".utf8)] {
                #expect(throws: FlyrailValueError("document nesting is too deep")) {
                    try editDocument(
                        source: source,
                        requests: [keyRequest(tooDeep, format: format, acquisition: .takeover)],
                    )
                }
            }
            let conflict = try editDocument(
                source: Data("{\"x\":1}".utf8),
                requests: [keyRequest(tooDeep, format: format)],
            )
            #expect(conflict.observations.map(\.status) == [.conflict])
            #expect(conflict.observations.map(\.message) == [""])
            #expect(conflict.after == Data("{\"x\":1}".utf8))
            #expect(conflict.ownership == [])
            #expect(conflict.provenance == DocumentProvenance())
        }
    }
}

@Test
func memberIdentitiesAndBothNeighbourAnchorsEnforceFrameBoundaries() throws {
    let whole = try memberRequest(key: [])
    let named = try memberRequest(key: ["name"])
    for shape in EditingNesting.allCases {
        for depth in [shape.identityBoundary - 1, shape.identityBoundary] {
            let nested = shape.syntax(depth)
            for side in [false, true] {
                for (request, selected) in [(whole, "\"b\""), (named, "{\"name\":\"b\"}")] {
                    let elements = side ? "\(selected),\(nested)" : "\(nested),\(selected)"
                    let source = Data("{\"hooks\":[\(elements)]}".utf8)
                    if depth < shape.identityBoundary {
                        let edit = try editDocument(source: source, requests: [request])
                        #expect(edit.after == source)
                        #expect(edit.observations.map(\.status) == [.conflict])
                        #expect(edit.observations
                            .map(\.message) == ["generated array member does not match its selector"])
                        let position = try #require(edit.observations.first?.current?.position)
                        let anchor = side ? position.following : position.previous
                        #expect(try anchor?.identity == (shape.value(depth)))
                    } else {
                        #expect(throws: FlyrailValueError("document nesting is too deep")) {
                            try editDocument(source: source, requests: [request])
                        }
                    }
                }
            }
        }
    }
}

@Test
func nonScalarNativeIdentityAndFallbackAnchorHaveIndependentBudgets() throws {
    let request = try memberRequest(key: ["name"])
    for depth in [195, 196, 197] {
        let nested = EditingNesting.object.syntax(depth)
        let source = Data("{\"hooks\":[{\"name\":\(nested)},{\"name\":\"b\"}]}".utf8)
        if depth == 195 {
            let edit = try editDocument(source: source, requests: [request])
            #expect(edit.observations.map(\.message) == ["generated array member does not match its selector"])
            let anchor = try #require(edit.observations.first?.current?.position?.previous)
            #expect(anchor.key == [])
            #expect(try anchor.identity == SemanticValue(object: [("name", EditingNesting.object.value(depth))]))
        } else {
            #expect(throws: FlyrailValueError("document nesting is too deep")) {
                try editDocument(source: source, requests: [request])
            }
        }
    }
}

@Test
func generatedEncoderAndPublishedSourceDepthErrorsAreNotConflicts() throws {
    for depth in [985, 986] {
        let value = try EditingNesting.array.value(depth)
        #expect(throws: FlyrailValueError("document nesting is too deep")) {
            try editDocument(source: nil, requests: [keyRequest(value)])
        }
    }
}

private func keyRequest(
    _ value: SemanticValue,
    format: DocumentFormat = .json,
    acquisition: Acquisition = .conflict,
) throws -> DocumentRequest {
    try .set(.structured(StructuredContent(
        format: format,
        selector: Flyrail.Selector(parts: [.key(Key("x"))]),
        value: value,
    )), acquisition: acquisition)
}

private func memberRequest(key: [String]) throws -> DocumentRequest {
    let value: SemanticValue = key
        .isEmpty ? .scalar(.text("c")) : try SemanticValue(object: [("name", .scalar(.text("c")))])
    return try .set(.structured(StructuredContent(
        format: .json,
        selector: Flyrail.Selector(parts: [
            .key(Key("hooks")),
            .member(Member(identity: .scalar(.text("b")), key: key)),
        ]),
        value: value,
    )), acquisition: .takeover)
}
