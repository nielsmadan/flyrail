@testable import Flyrail
import Foundation
import Testing

@Test
func TOMLBoundsCoverParsedAndGeneratedDocuments() throws {
    let request = try fixtureRequest(format: "toml", record: [
        "action": "set", "selector": ["y"], "value": 2,
    ])
    for depth in [100, 101] {
        let sources = [
            "[\(Array(repeating: "a", count: depth).joined(separator: "."))]\nx=1\n",
            "x=" + String(repeating: "[", count: depth) + "1" + String(repeating: "]", count: depth) + "\n",
            "x=" + String(repeating: "{a=", count: depth) + "1" + String(repeating: "}", count: depth) + "\n",
        ]
        for source in sources {
            if depth == 100 {
                #expect(try editDocument(source: Data(source.utf8), requests: [request]).applicable)
            } else {
                #expect(throws: FlyrailValueError.self) {
                    try editDocument(source: Data(source.utf8), requests: [request])
                }
            }
        }
        var value = SemanticValue.scalar(.integer(1))
        for _ in 0 ..< depth {
            value = .array([value])
        }
        let content = try StructuredContent(
            format: .toml, selector: Flyrail.Selector(parts: [.key(Key("x"))]), value: value,
        )
        let edit = try editDocument(source: nil, requests: [.set(.structured(content))])
        #expect(edit.applicable == (depth == 100))
        if depth == 101 {
            #expect(edit.observations.first?.message == "TOML value nested more than 100 levels deep at line 1 col 108")
        }
    }
    #expect(throws: FlyrailValueError("TOML input exceeds the byte limit")) {
        try editDocument(
            source: Data(repeating: 32, count: JSONLimits.default.maximumInputBytes + 1),
            requests: [request],
        )
    }
}

@Test
func TOMLLayoutRejectsInjectedHeadersInvalidPathsAndMismatchedDestinations() throws {
    let value = try SemanticValue(object: [
        ("env", SemanticValue(object: [])),
        ("checks", .array([SemanticValue(object: [])])),
    ])
    let malformed = [
        "not json", "[]", #"{"version":1}"#, #"{"version":true,"headers":[]}"#,
        #"{"version":1.0,"headers":[]}"#, #"{"version":1,"version":1,"headers":[]}"#,
        #"{"version":1,"headers":[5]}"#, #"{"version":1,"headers":[[[],"x"]]}"#,
        #"{"version":1,"headers":[[["missing"],"x"]]}"#,
        #"{"version":1,"headers":[[["env",0],"x"]]}"#,
        #"{"version":1,"headers":[[["checks",1],"x"]]}"#,
        #"{"version":1,"headers":[[["env"],"x"],[["env"],"x"]]}"#,
        #"{"version":1,"headers":[[["env"],"x] # injected"]]}"#,
        #"{"version":1,"headers":[[["env"],"x\n"]]}"#,
        #"{"version":1,"headers":[[["env"],"[x]"]]}"#,
        #"{"version":1,"headers":[[["env"],"x..env"]]}"#,
        #"{"version":1,"headers":[[["env"],"x.\"env\\q\""]]}"#,
    ]
    for layout in malformed {
        #expect(throws: FlyrailValueError.self) {
            try SelectionSnapshot(value: .typed(value), syntax: Data(), ownedSyntax: Data(), layout: Data(layout.utf8))
        }
    }
    let snapshot = try SelectionSnapshot(
        value: .typed(value), syntax: Data(), ownedSyntax: Data(),
        layout: Data(#"{"version":1,"headers":[[["env"],"foreign.env"]]}"#.utf8),
    )
    let claim = try Claim(kind: .structured, selector: .structured(Flyrail.Selector(parts: [.key(Key("x"))])))
    #expect(throws: FlyrailValueError("TOML restoration header does not match its destination")) {
        try OwnedSelection(claim: claim, format: .toml, installed: snapshot, disposition: .created)
    }
    #expect(throws: FlyrailValueError("restoration layout requires a TOML claim")) {
        try OwnedSelection(claim: claim, format: .jsonc, installed: snapshot, disposition: .created)
    }
}

@Test
func TOMLExactStringAndKeyIdentityRetainsForeignSpellings() throws {
    let composed = "\u{E9}"
    let decomposed = "e\u{301}"
    let source = Data("\"\(composed)\"=1\n\"\(decomposed)\"=2\nhooks=[\"\(composed)\",\"\(decomposed)\"]\n".utf8)
    let requests = try [
        fixtureRequest(format: "toml", record: [
            "action": "set", "selector": [composed], "value": 3, "acquisition": "takeover",
        ]),
        fixtureRequest(format: "toml", record: [
            "action": "set", "selector": ["hooks", ["identity": decomposed]],
            "value": decomposed, "acquisition": "adopt",
        ]),
    ]
    let installed = try editDocument(source: source, requests: requests)
    #expect(installed.applicable)
    #expect(installed
        .after == Data("\"\(composed)\"=3\n\"\(decomposed)\"=2\nhooks=[\"\(composed)\",\"\(decomposed)\"]\n".utf8))
    let removed = try editDocument(
        source: installed.after, requests: installed.ownership.map { try .retire($0.claim) },
        ownership: installed.ownership, provenance: installed.provenance,
    )
    #expect(removed.after == Data("\"\(composed)\"=1\n\"\(decomposed)\"=2\nhooks=[\"\(composed)\"]\n".utf8))
}

@Test
func schemaMatchingUsesTypedValuesAndFormatSpecificSyntax() throws {
    let integer = try DocumentSchema(format: .json, key: "version", value: .integer(1))
    #expect(try !matchesSchema(source: nil, requirement: integer))
    #expect(try !matchesSchema(source: Data(" \r\n".utf8), requirement: integer))
    #expect(try matchesSchema(source: Data("\u{FEFF}{\"version\":1}".utf8), requirement: integer))
    #expect(try !matchesSchema(source: Data("{\"version\":1.0}".utf8), requirement: integer))
    #expect(try !matchesSchema(source: Data("{\"version\":true}".utf8), requirement: integer))
    let negativeZero = try DocumentSchema(format: .json, key: "version", value: .float(Binary64(-0.0)))
    #expect(try matchesSchema(source: Data("{\"version\":-0.0}".utf8), requirement: negativeZero))
    #expect(try !matchesSchema(source: Data("{\"version\":0.0}".utf8), requirement: negativeZero))
    let jsonc = try DocumentSchema(format: .jsonc, key: "version", value: .integer(1))
    #expect(try matchesSchema(source: Data("{/* comment */\"version\":1,}".utf8), requirement: jsonc))
    #expect(throws: FlyrailValueError.self) {
        try matchesSchema(source: Data("{/* comment */\"version\":1}".utf8), requirement: integer)
    }
}
