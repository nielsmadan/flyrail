import CoreFoundation
@testable import Flyrail
import Foundation
import Testing

@Test
func portableSectionJSONAndJSONCEditsMatchFixtures() throws {
    let fixture = try editFixture()
    for item in try dictionaries(fixture["cases"]) {
        guard let formatName = item["format"] as? String,

              let steps = item["steps"] as? [[String: Any]]
        else {
            continue
        }
        var source = dataValue(item["before"])
        var ownership: [OwnedSelection] = []
        var provenance = DocumentProvenance()
        var baselines: [Claim: SelectionSnapshot] = [:]
        for step in steps {
            let observed = step.keys.contains("observed") ? dataValue(step["observed"]) : source
            let records = step["requests"] as? [[String: Any]] ?? [step]
            let requests = try records.map { try fixtureRequest(format: formatName, record: $0) }
            let edit = try editDocument(
                source: observed,
                requests: requests,
                ownership: ownership,
                provenance: provenance,
            )
            #expect(edit.after == dataValue(step["expected"]))
            let expected = (step["statuses"] as? [String] ?? [step["status"] as? String ?? ""])
                .compactMap(EditStatus.init(rawValue:))
            #expect(edit.observations.map(\.status) == expected)
            for selected in edit.ownership {
                if selected.disposition == .takenOver {
                    if let baseline = baselines[selected.claim] {
                        #expect(selected.baseline == baseline)
                    } else if let baseline = selected.baseline {
                        baselines[selected.claim] = baseline
                    }
                } else {
                    #expect(selected.baseline == nil)
                }
            }
            if !edit.applicable {
                #expect(edit.ownership == ownership)
                #expect(edit.provenance == provenance)
            }
            source = edit.after
            ownership = edit.ownership
            provenance = edit.provenance
        }
    }
}

@Test
func ambiguousSectionJSONAndJSONCDocumentsAreRejected() throws {
    let fixture = try editFixture()
    for item in try dictionaries(fixture["invalid"]) {
        guard let format = item["format"] as? String,
              let document = item["document"] as? String,
              let selectors = item["selectors"] as? [Any]
        else {
            continue
        }
        let requests = try selectors.map { selector in
            var record: [String: Any] = [
                "action": "set",
                "selector": selector,
                "text": "body",
                "value": 1,
            ]
            if format == "text" {
                record.removeValue(forKey: "value")
            }
            return try fixtureRequest(format: format, record: record)
        }
        #expect(throws: FlyrailValueError.self) {
            try editDocument(source: Data(document.utf8), requests: requests)
        }
    }
}

@Test
func JSONEditingRejectsMalformedOversizedAndDeepDocuments() throws {
    let content = try StructuredContent(
        format: .json,
        selector: Flyrail.Selector(parts: [.key(Key("x"))]),
        value: .scalar(.integer(1)),
    )
    let request = try DocumentRequest.set(.structured(content))
    for malformed in [
        #"{"x":1,"x":2}"#,
        #"{"x":NaN}"#,
        #"{"x":"\ud800"}"#,
        #"{"x":[1,]}"#,
    ] {
        #expect(throws: FlyrailValueError.self) {
            try editDocument(source: Data(malformed.utf8), requests: [request])
        }
    }
    let oversized = Data(repeating: 32, count: JSONLimits.default.maximumInputBytes + 1)
    #expect(throws: FlyrailValueError.self) {
        try editDocument(source: oversized, requests: [request])
    }
    let deep = String(repeating: "[", count: 986) +
        "0" +
        String(repeating: "]", count: 986)
    let deepContent = try StructuredContent(
        format: .json,
        selector: Flyrail.Selector(parts: [
            .member(Member(identity: .scalar(.integer(0)))),
        ]),
        value: .scalar(.integer(0)),
    )
    #expect(throws: FlyrailValueError.self) {
        try editDocument(
            source: Data(deep.utf8),
            requests: [DocumentRequest.set(.structured(deepContent), acquisition: .adopt)],
        )
    }
}

@Test
func contentClaimsAcceptOnlySharedContent() throws {
    let error = FlyrailValueError("document content must be a section or structured selection")
    #expect(throws: error) {
        try contentClaim(.file(FileContent(data: Data("x".utf8))))
    }
    #expect(throws: error) {
        try contentClaim(.tree(TreeContent(entries: [])))
    }
    let section = try contentClaim(.section(SectionContent(marker: "one", text: "body")))
    let structured = try contentClaim(.structured(StructuredContent(
        format: .json,
        selector: Flyrail.Selector(parts: [.key(Key("x"))]),
        value: .scalar(.boolean(true)),
    )))
    #expect(section.kind == .section)
    #expect(structured.kind == .structured)
}

@Test(.enabled(if: repositoryCheckout))
func packagedEditFixtureMatchesRoot() throws {
    let packaged = try Data(contentsOf: #require(Bundle.module.url(
        forResource: "edits",
        withExtension: "json",
        subdirectory: "Fixtures",
    )))
    #expect(try packaged == Data(contentsOf: repositoryFile("spec/fixtures/edits.json")))
}

func editFixture() throws -> [String: Any] {
    let url = try #require(Bundle.module.url(
        forResource: "edits",
        withExtension: "json",
        subdirectory: "Fixtures",
    ))
    let data = try Data(contentsOf: url)
    guard let result = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw FlyrailValueError("invalid edit fixture")
    }
    return result
}

private func dictionaries(_ value: Any?) throws -> [[String: Any]] {
    guard let result = value as? [[String: Any]] else {
        throw FlyrailValueError("invalid edit fixture records")
    }
    return result
}

private func dataValue(_ value: Any?) -> Data? {
    (value as? String).map { Data($0.utf8) }
}

func fixtureRequest(
    format: String,
    record: [String: Any],
) throws -> DocumentRequest {
    let claim: Claim
    let selector: Flyrail.Selector?
    if format == "text" {
        guard let raw = record["selector"] as? [String: String],
              let start = raw["start"],
              let end = raw["end"]
        else {
            throw FlyrailValueError("invalid section fixture selector")
        }
        claim = try Claim(
            kind: .section,
            selector: .section(SectionBoundaries(start: start, end: end)),
        )
        selector = nil
    } else {
        guard let raw = record["selector"] as? [Any] else {
            throw FlyrailValueError("invalid structured fixture selector")
        }
        let parsed = try Flyrail.Selector(parts: raw.map(fixtureSelectorPart))
        claim = try Claim(kind: .structured, selector: .structured(parsed))
        selector = parsed
    }
    if record["action"] as? String == "retire" {
        return try .retire(claim)
    }
    let acquisition = Acquisition(rawValue: record["acquisition"] as? String ?? "conflict") ?? .conflict
    if format == "text" {
        guard case let .section(boundaries) = claim.selector,
              let text = record["text"] as? String
        else {
            throw FlyrailValueError("invalid section fixture")
        }
        let content = try SectionContent(
            marker: record["marker"] as? String ?? "fixture",
            text: text,
            boundaries: boundaries,
        )
        return try .set(.section(content), acquisition: acquisition)
    }
    guard let selector,
          let documentFormat = DocumentFormat(rawValue: format),
          let rawValue = record["semantic_value"] ?? record["value"]
    else {
        throw FlyrailValueError("invalid structured fixture")
    }
    let placement: Placement? = if let raw = record["placement"] as? [String: Any],
                                   let positionName = raw["position"] as? String,
                                   let position = Position(rawValue: positionName)
    {
        try Placement(
            position: position,
            anchor: (raw["anchor"] as? [String: Any]).map(fixtureMember),
        )
    } else {
        nil
    }
    let value = record["semantic_value"] != nil
        ? try SemanticValue(semanticBytes: JSONSerialization.data(withJSONObject: rawValue))
        : try fixtureSemanticValue(rawValue)
    let content = try StructuredContent(
        format: documentFormat,
        selector: selector,
        value: value,
        placement: placement,
    )
    return try .set(.structured(content), acquisition: acquisition)
}

private func fixtureSelectorPart(_ value: Any) throws -> SelectorPart {
    if let key = value as? String {
        return try .key(Key(key))
    }
    guard let record = value as? [String: Any] else {
        throw FlyrailValueError("invalid fixture selector part")
    }
    return try .member(fixtureMember(record))
}

private func fixtureMember(_ record: [String: Any]) throws -> Member {
    let identity = if let semantic = record["semantic_identity"] {
        try SemanticValue(
            semanticBytes: JSONSerialization.data(withJSONObject: semantic),
        )
    } else if let identity = record["identity"] {
        try fixtureSemanticValue(identity)
    } else {
        throw FlyrailValueError("fixture member identity is missing")
    }
    return try Member(identity: identity, key: record["key"] as? [String] ?? [])
}

private func fixtureSemanticValue(_ value: Any) throws -> SemanticValue {
    if value is NSNull {
        return .scalar(.null)
    }
    if let value = value as? String {
        return .scalar(.text(value))
    }
    if let value = value as? NSNumber {
        if CFGetTypeID(value) == CFBooleanGetTypeID() {
            return .scalar(.boolean(value.boolValue))
        }
        let token = value.stringValue
        if token.contains(".") || String(cString: value.objCType) == "d" {
            return try .scalar(.float(Binary64(value.doubleValue)))
        }
        return .scalar(.integer(value.int64Value))
    }
    if let values = value as? [Any] {
        return try .array(values.map(fixtureSemanticValue))
    }
    if let values = value as? [String: Any] {
        return try SemanticValue(object: values.map { try ($0.key, fixtureSemanticValue($0.value)) })
    }
    throw FlyrailValueError("unsupported fixture value")
}
