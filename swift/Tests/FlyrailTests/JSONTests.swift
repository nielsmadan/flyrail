@testable import Flyrail
import Foundation
import Testing

@Test
func strictJSONRejectsDuplicatesAndMalformedInput() throws {
    let rejected = [
        #"{"a":1,"a":2}"#,
        #"{"a":{"b":1,"b":2}}"#,
        "[1,]",
        "01",
        "1.",
        "1e",
        "NaN",
        "Infinity",
        #""\ud800""#,
        #""\udc00""#,
        "\"\u{1}\"",
    ]
    for text in rejected {
        #expect(throws: Error.self, "must reject \(text)") {
            try JSONParser.parse(Data(text.utf8))
        }
    }
    #expect(throws: FlyrailValueError.self) {
        try JSONParser.parse(Data([0xFF]))
    }
}

@Test
func strictJSONPreservesArbitraryUnsignedDecimalTokens() throws {
    let token = "18446744073709551615"
    let parsed = try JSONParser.parse(Data(token.utf8))
    #expect(parsed.unsignedDecimalToken == token)
    #expect(parsed.signedInteger == nil)
    #expect(try String(decoding: JSONWriter.canonicalData(parsed), as: UTF8.self) == token)
}

@Test
func strictJSONAcceptsTopLevelWhitespace() throws {
    let parsed = try JSONParser.parse(Data(" \n[\"null\"]\t".utf8))
    #expect(parsed == .array([.string("null")]))
}

@Test
func canonicalJSONSortsExactUTF8KeysAndWritesLiteralUnicode() throws {
    let value = JSONValue.object([
        ("é", .string("\u{2028}")),
        ("b", .number("2")),
        ("a", .number("1")),
    ])
    let text = try String(decoding: JSONWriter.canonicalData(value), as: UTF8.self)
    #expect(text == "{\"a\":1,\"b\":2,\"é\":\"\u{2028}\"}")
}

@Test
func strictJSONEnforcesEveryLimit() {
    #expect(throws: FlyrailValueError.self) {
        try JSONParser.parse(
            Data("null".utf8),
            limits: JSONLimits(maximumInputBytes: 3, maximumDepth: 2, maximumTokens: 2, maximumStringBytes: 2),
        )
    }
    #expect(throws: FlyrailValueError.self) {
        try JSONParser.parse(
            Data("[[null]]".utf8),
            limits: JSONLimits(maximumInputBytes: 20, maximumDepth: 1, maximumTokens: 10, maximumStringBytes: 10),
        )
    }
    #expect(throws: FlyrailValueError.self) {
        try JSONParser.parse(
            Data("[null,null]".utf8),
            limits: JSONLimits(maximumInputBytes: 20, maximumDepth: 2, maximumTokens: 2, maximumStringBytes: 10),
        )
    }
    #expect(throws: FlyrailValueError.self) {
        try JSONParser.parse(
            Data(#""abc""#.utf8),
            limits: JSONLimits(maximumInputBytes: 20, maximumDepth: 2, maximumTokens: 2, maximumStringBytes: 2),
        )
    }
}
