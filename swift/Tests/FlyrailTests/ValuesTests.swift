@testable import Flyrail
import Foundation
import Testing

@Test
func semanticScalarsUseCanonicalRecords() throws {
    let cases: [(SemanticValue, String)] = try [
        (.scalar(.null), #"["null"]"#),
        (.scalar(.boolean(true)), #"["boolean",true]"#),
        (.scalar(.integer(Int64.min)), #"["integer","-9223372036854775808"]"#),
        (.scalar(.float(Binary64(-0.0))), #"["float","-0x0.0p+0"]"#),
        (.scalar(.text("é")), #"["string","é"]"#),
        (.scalar(.date(CalendarDate(year: 2026, month: 1, day: 2))), #"["date","2026-01-02"]"#),
        (.scalar(.time(LocalTime(hour: 1, minute: 2, second: 3))), #"["time","01:02:03.000000"]"#),
    ]
    for (value, expected) in cases {
        #expect(try String(decoding: value.semanticBytes(), as: UTF8.self) == expected)
        #expect(try SemanticValue(semanticBytes: Data(expected.utf8)) == value)
    }
}

@Test
func binary64UsesFiniteBitExactIdentity() throws {
    let positive = try Binary64(0.0)
    let negative = try Binary64(-0.0)
    #expect(positive != negative)
    #expect(positive.pythonHex == "0x0.0p+0")
    #expect(negative.pythonHex == "-0x0.0p+0")
    #expect(try Binary64(bitPattern: 1).pythonHex == "0x0.0000000000001p-1022")
    #expect(try Binary64(Double.greatestFiniteMagnitude).pythonHex == "0x1.fffffffffffffp+1023")
    #expect(throws: FlyrailValueError.self) {
        try Binary64(Double.infinity)
    }
    #expect(throws: FlyrailValueError.self) {
        try Binary64(Double.nan)
    }
}

@Test
func exactTextDoesNotUseCanonicalEquivalence() throws {
    let composed = SemanticValue.scalar(.text("é"))
    let decomposed = SemanticValue.scalar(.text("e\u{301}"))
    #expect(composed != decomposed)

    let object = try SemanticValue(object: [
        ("é", composed),
        ("e\u{301}", decomposed),
        ("a", .scalar(.integer(1))),
    ])
    let encoded = try String(decoding: object.semanticBytes(), as: UTF8.self)
    #expect(encoded == #"["object",[["a",["integer","1"]],["é",["string","é"]],["é",["string","é"]]]]"#)
}

@Test
func semanticObjectIdentityIgnoresMemberOrder() throws {
    let first = SemanticValue.object([
        ("b", .scalar(.integer(2))),
        ("a", .scalar(.integer(1))),
    ])
    let second = try SemanticValue(object: [
        ("a", .scalar(.integer(1))),
        ("b", .scalar(.integer(2))),
    ])
    #expect(first == second)
    #expect(first.hashValue == second.hashValue)
}

@Test
func semanticRecordsRejectNoncanonicalAndInvalidValues() {
    let rejected = [
        (#"["integer","01"]"#, "semantic scalar encoding is not canonical"),
        (#"["integer","-0"]"#, "semantic scalar encoding is not canonical"),
        (#"["integer","+1"]"#, "semantic scalar encoding is not canonical"),
        (#"["integer","9223372036854775808"]"#, "integers must fit signed 64 bits"),
        (#"["float","0x1.8p+0"]"#, "semantic scalar encoding is not canonical"),
        (#"["float","nan"]"#, "float hexadecimal literal is malformed"),
        (#"["date","2026-1-02"]"#, "date must be written as YYYY-MM-DD"),
        (#"["time","01:02:03"]"#, "semantic scalar encoding is not canonical"),
        (
            #"["datetime","2026-01-02T05:02:03.000000+05:00"]"#,
            "time fraction must have 1-6 digits"
        ),
        (#"["object",[["a",["null"]],["a",["null"]]]]"#, "duplicate object key"),
        (#"["array",["integer","1"]]"#, "semantic values must be tagged arrays"),
        (#"["null",null]"#, "semantic scalar payload has the wrong type"),
        ("{}", "semantic values must be tagged arrays"),
    ]
    for (text, message) in rejected {
        #expect(semanticError(text) == message)
    }
}

private func semanticError(_ text: String) -> String? {
    do {
        _ = try SemanticValue(semanticBytes: Data(text.utf8))
        return nil
    } catch let error as FlyrailValueError {
        return error.message
    } catch {
        return String(describing: error)
    }
}

@Test
func resourcePathPreservesPlatformBytes() throws {
    let bytes: [UInt8] = [47, 116, 109, 112, 47, 255]
    let path = try ResourcePath(platformBytes: bytes)
    #expect(path.platformBytes == bytes)
    #expect(throws: FlyrailValueError.self) {
        try ResourcePath(platformBytes: [97, 0, 98])
    }
}
