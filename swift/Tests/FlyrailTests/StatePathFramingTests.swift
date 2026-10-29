@testable import Flyrail
import Foundation
import Testing

@Test
func statePathsPreserveTheCanonicalDoubleSlashRoot() throws {
    let path = try ResourcePath("//fixture/é.md")
    #expect(path.platformBytes == Array("//fixture/é.md".utf8))
    #expect(try path != ResourcePath("/fixture/é.md"))
    let encoded = try StateCodec.path(path)
    #expect(encoded == .object([("path", .string("//fixture/é.md"))]))
    #expect(try StateCodec.decodePath(encoded) == path)
    #expect(try StateCodec.transcode(Data(#"{"path":"//"}"#.utf8)) == Data("{\"path\":\"//\"}\n".utf8))
    let authority = try ResourceAuthority(recordedDestination: path)
    #expect(try statePathText(authority.stateRoot).hasPrefix("//fixture/"))
    #expect(try statePathText(authority.lockPath).hasPrefix("//fixture/"))
    let bytes = Array("//fixture/é.md".utf8)
    #expect(try ResourcePath(platformBytes: bytes) == path)
}

@Test
func stateSemanticWrappersRoundTripNearTheirDepthLimit() throws {
    var record = #"["null"]"#
    for _ in 0 ..< 250 {
        record = #"["array",["# + record + "]]"
    }
    let data = Data(("{\"value\":" + record + "}").utf8)
    #expect(try StateCodec.transcode(data) == data + Data([10]))
    var value = SemanticValue.scalar(.null)
    for _ in 0 ..< 1000 {
        value = .array([value])
    }
    let oversized = value
    #expect(throws: FlyrailValueError.self) { try StateCodec.semantic(oversized) }
}
