@testable import Flyrail
import Foundation
import Testing

@Test
func TOMLCompoundedDottedInlineDepthIsBoundedBeforeMaterialization() throws {
    let prefix = "{" + Array(repeating: "a", count: 100).joined(separator: ".") + "="
    let source = "x=" + String(repeating: prefix, count: 100) + "1" + String(repeating: "}", count: 100) + "\n"
    let request = try fixtureRequest(format: "toml", record: ["action": "set", "selector": ["y"], "value": 1])
    #expect(throws: FlyrailValueError.self) {
        try editDocument(source: Data(source.utf8), requests: [request])
    }
}

@Test
func TOMLDottedForeignAncestorsConflictAtomicallyOnRetirement() throws {
    for selector in [["x", "a"], ["x", "a", "b"]] {
        let source = Data((selector.joined(separator: ".") + "=1\nforeign=2\n").utf8)
        let request = try fixtureRequest(format: "toml", record: [
            "action": "set", "selector": selector, "value": 1, "acquisition": "adopt",
        ])
        let adopted = try editDocument(source: source, requests: [request])
        let retired = try retireTOMLRegression(adopted)
        #expect(!retired.applicable)
        #expect(retired.after == source)
        #expect(retired.ownership == adopted.ownership)
        #expect(retired.observations.first?.message == "TOML layout cannot preserve the selected and foreign values")
    }
}

@Test
func TOMLDottedTakeoverRestoresBaselineBytesAndRejectsMixedSpelling() throws {
    let original = Data("x.a=1\n# owned between\n\nx.b=2\nforeign=3\n".utf8)
    let request = try fixtureRequest(format: "toml", record: [
        "action": "set", "selector": ["x"], "value": ["a": 5, "b": 6], "acquisition": "takeover",
    ])
    let installed = try editDocument(source: original, requests: [request])
    #expect(installed.applicable)
    #expect(try retireTOMLRegression(installed).after == original)
    let mixed = Data("x.a=1\n\"x\".b=2\nforeign=3\n".utf8)
    let conflict = try editDocument(source: mixed, requests: [request])
    #expect(!conflict.applicable)
    #expect(conflict.after == mixed)
    #expect(conflict.observations.first?.message == "TOML layout cannot preserve the selected and foreign values")
}

@Test
func TOMLInlineInsertionRoundTripsClosingTrivia() throws {
    let request = try fixtureRequest(format: "toml", record: [
        "action": "set", "selector": ["x", "b"], "value": 3,
    ])
    for gap in ["", " ", "   ", "\t \t"] {
        let original = Data("x={a=1\(gap)}\nforeign=2\n".utf8)
        let installed = try editDocument(source: original, requests: [request])
        #expect(installed.applicable)
        #expect(installed.after == Data("x={a=1,\(gap)b = 3}\nforeign=2\n".utf8))
        #expect(try retireTOMLRegression(installed).after == original)
    }
}

@Test
func TOMLDetachedCommentsKeepDeclarationIndentation() throws {
    for (source, value) in [("  x=1 # foreign\n", 1 as Any), ("  [x] # foreign\na=1\n", ["a": 1] as Any)] {
        let request = try fixtureRequest(format: "toml", record: [
            "action": "set", "selector": ["x"], "value": value, "acquisition": "adopt",
        ])
        let adopted = try editDocument(source: Data(source.utf8), requests: [request])
        let retired = try retireTOMLRegression(adopted)
        #expect(retired.applicable)
        #expect(retired.after == Data("   # foreign\n".utf8))
    }
}

@Test
func TOMLDottedTableAndAOTUpdatesUseDeclarationRelativePaths() throws {
    let cases: [(String, [Any], String)] = [
        ("[mcp]\nfoo.command=1\nfoo.other=2\n", ["mcp", "foo"], "[mcp]\nfoo.command = 3\n"),
        (
            "[[hooks]]\nname=\"a\"\nfoo.command=1\nfoo.other=2\n",
            ["hooks", ["identity": "a", "key": ["name"]], "foo"],
            "[[hooks]]\nname=\"a\"\nfoo.command = 3\n",
        ),
    ]
    for (source, selector, expected) in cases {
        let request = try fixtureRequest(format: "toml", record: [
            "action": "set", "selector": selector, "value": ["command": 3], "acquisition": "takeover",
        ])
        let installed = try editDocument(source: Data(source.utf8), requests: [request])
        #expect(installed.applicable)
        #expect(installed.after == Data(expected.utf8))
        #expect(try retireTOMLRegression(installed).after == Data(source.utf8))
    }
}

@Test
func TOMLMalformedForeignTokensMatchReferenceDiagnostics() throws {
    let request = try fixtureRequest(format: "toml", record: ["action": "set", "selector": ["y"], "value": 1])
    let cases = [
        ("x={}\nx.y=2\n", "Key \"x\" already exists. at line 2 col 0"),
        ("x=1_e2\n", "Invalid number at line 1 col 6"),
        ("x=1e_2\n", "Invalid number at line 1 col 6"),
        ("x=\"\\u+123\"\n", "Invalid unicode value at line 1 col 4"),
        ("x=2000-01-01T12:34:56+01\n", "Invalid number at line 1 col 24"),
    ]
    for (source, diagnostic) in cases {
        #expect(throws: FlyrailValueError(diagnostic)) {
            try editDocument(source: Data(source.utf8), requests: [request])
        }
    }
}

@Test
func TOMLNativeTemporalPrecisionTruncatesSemanticsAndPreservesTokens() throws {
    let values: [(String, SemanticValue)] = try [
        ("00:00:00.123456789", .scalar(.time(LocalTime("00:00:00.123456")))),
        ("2000-01-01T00:00:00.123456789", .scalar(.dateTime(LocalDateTime("2000-01-01T00:00:00.123456")))),
        ("2000-01-01T00:00:00.123456789Z", .scalar(.offsetDateTime(OffsetDateTime("2000-01-01T00:00:00.123456Z")))),
        (
            "2000-01-01 01:00:00.123456789+01:00",
            .scalar(.offsetDateTime(OffsetDateTime("2000-01-01T00:00:00.123456Z"))),
        ),
    ]
    for (token, value) in values {
        let original = Data("x=\(token)\n".utf8)
        let document = try TOMLEditingDocument(data: original)
        #expect(try document.native([.key("x")]) == value)
        let request = try fixtureRequest(format: "toml", record: ["action": "set", "selector": ["y"], "value": 1])
        let installed = try editDocument(source: original, requests: [request])
        #expect(installed.after == original + Data("y = 1\n".utf8))
        #expect(try retireTOMLRegression(installed).after == original)
    }
}

private func retireTOMLRegression(_ edit: DocumentEdit) throws -> DocumentEdit {
    try editDocument(
        source: edit.after, requests: edit.ownership.map { try .retire($0.claim) },
        ownership: edit.ownership, provenance: edit.provenance,
    )
}

func tomlReviewRegressionCases() -> [[String: Any]] {
    func row(_ name: String, _ source: String, _ selector: [Any], _ value: Any, acquisition: String) -> [String: Any] {
        [
            "name": name, "format": "toml", "before": source,
            "steps": [
                ["action": "set", "selector": selector, "value": value, "acquisition": acquisition],
                ["action": "retire", "selector": selector],
            ],
        ]
    }
    var cases = [
        row("toml-review-dotted-retire", "x.a=1\nforeign=2\n", ["x", "a"], 1, acquisition: "adopt"),
        row("toml-review-deep-dotted-retire", "x.a.b=1\nforeign=2\n", ["x", "a", "b"], 1, acquisition: "adopt"),
        row(
            "toml-review-dotted-restore-gap",
            "x.a=1\n# owned between\n\nx.b=2\nforeign=3\n",
            ["x"],
            ["a": 5, "b": 6],
            acquisition: "takeover",
        ),
        row(
            "toml-review-mixed-prefix",
            "x.a=1\n\"x\".b=2\nforeign=3\n",
            ["x"],
            ["a": 5, "b": 6],
            acquisition: "takeover",
        ),
        row("toml-review-detach-indented-scalar", "  x=1 # foreign\n", ["x"], 1, acquisition: "adopt"),
        row("toml-review-detach-indented-table", "  [x] # foreign\na=1\n", ["x"], ["a": 1], acquisition: "adopt"),
        row(
            "toml-review-table-relative",
            "[mcp]\nfoo.command=1\nfoo.other=2\n",
            ["mcp", "foo"],
            ["command": 3],
            acquisition: "takeover",
        ),
        row("toml-review-aot-relative", "[[hooks]]\nname=\"a\"\nfoo.command=1\nfoo.other=2\n",
            ["hooks", ["identity": "a", "key": ["name"]], "foo"], ["command": 3], acquisition: "takeover"),
    ]
    for (index, gap) in ["", " ", "   ", "\t \t"].enumerated() {
        cases.append(row(
            "toml-review-inline-gap-\(index)",
            "x={a=1\(gap)}\nforeign=2\n",
            ["x", "b"],
            3,
            acquisition: "conflict",
        ))
    }
    let malformed = ["x={}\nx.y=2\n", "x=1_e2\n", "x=1e_2\n", "x=\"\\u+123\"\n", "x=2000-01-01T12:34:56+01\n"]
    for (index, source) in malformed.enumerated() {
        cases.append(row("toml-review-invalid-foreign-\(index)", source, ["y"], 1, acquisition: "conflict"))
    }
    for (index, temporal) in [
        ("00:00:00.123456789", "time", "00:00:00.123456"),
        ("2000-01-01T00:00:00.123456789", "datetime", "2000-01-01T00:00:00.123456"),
        ("2000-01-01T00:00:00.123456789Z", "datetime", "2000-01-01T00:00:00.123456+00:00"),
        ("2000-01-01 01:00:00.123456789+01:00", "datetime", "2000-01-01T00:00:00.123456+00:00"),
    ].enumerated() {
        cases.append(row(
            "toml-review-native-precision-\(index)",
            "x=\(temporal.0)\n",
            ["y"],
            1,
            acquisition: "conflict",
        ))
        cases.append([
            "name": "toml-review-selected-precision-\(index)", "format": "toml", "before": "x=\(temporal.0)\n",
            "steps": [
                [
                    "action": "set", "selector": ["x"], "semantic_value": [temporal.1, temporal.2],
                    "acquisition": "adopt",
                ],
                ["action": "retire", "selector": ["x"]],
            ],
        ])
    }
    let prefix = "{" + Array(repeating: "a", count: 10).joined(separator: ".") + "="
    let compounded = "x=" + String(repeating: prefix, count: 4) + "1" + String(repeating: "}", count: 4) + "\n"
    cases.append(row("toml-review-compounded-depth", compounded, ["y"], 1, acquisition: "conflict"))
    return cases
}
