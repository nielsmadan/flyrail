@testable import Flyrail
import Foundation
import Testing

@Test
func modelRecordsMaterializeTypeAndFields() throws {
    let record = try ModelEncoding.record(
        type: "Example",
        fields: [
            ("zeta", .scalar(.null)),
            ("alpha", .scalar(.boolean(false))),
        ]
    )
    let text = try String(decoding: record.semanticBytes(), as: UTF8.self)
    #expect(
        text ==
            #"["object",[["fields",["object",[["alpha",["boolean",false]],["zeta",["null"]]]]],["type",["string","Example"]]]]"#
    )
}

@Test
func digestUsesDomainNulAndSemanticFraming() throws {
    let digest = try ModelEncoding.digest(
        domain: "flyrail-test-v1",
        values: [.scalar(.integer(1)), .scalar(.text("é"))]
    )
    #expect(digest == "0f2c98e21d9da1ed61cd0493e6e508c5d97a6aa6dbfc4a6344a416fbdc9252ee")
}
