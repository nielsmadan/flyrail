@testable import Flyrail
import Foundation
import Testing

@Test
func stateCodecRetainsUnsignedCountsBeyondNativeTokens() throws {
    for token in [
        "0",
        "9223372036854775808",
        "18446744073709551615",
        "18446744073709551616",
        "1" + String(repeating: "0", count: 100),
    ] {
        let count = try Count(decimal: token)
        let node = try Node(path: "", data: Data(), mode: 0o600, uid: count, gid: count, device: count, inode: count)
        let revision = try Revision(nodes: [node])
        let raw = try StateCodec.encode(revision)
        #expect(try StateCodec.decode(raw, as: Revision.self) == revision)
        #expect(String(decoding: raw, as: UTF8.self).contains("\"inode\":\(token)"))
    }
    #expect(Count(UInt64.max).uint64 == UInt64.max)
    #expect(try Count(decimal: "18446744073709551616").uint64 == nil)
    #expect(throws: FlyrailValueError.self) { try Count(decimal: String(repeating: "1", count: 4301)) }
    for invalid in ["", "01", "-1", "1.0", "1e2", "true", "+1"] {
        #expect(throws: FlyrailValueError.self) { try Count(decimal: invalid) }
    }
}

@Test
func stateRevisionContentDigestAndStructureMatchReference() throws {
    let foreign = try Revision(nodes: [stateTestNode("", Data("Foreign\n".utf8))])
    #expect(try foreign.contentDigest == "cc391149a8acd4875b9350fa3a00bd7ab74c6f22a51bedf0ee3cecab2b598af2")
    let empty = try Revision(nodes: [stateTestNode("", Data())])
    #expect(try empty.contentDigest == "0bd98d2117cf8a3e6724a4619042c40342c3403f88b6eb856b060fc034b8a349")
    for nodes in try [
        [stateTestNode("", nil), stateTestNode("", nil)],
        [stateTestNode("not-root", nil)],
        [stateTestNode("", Data()), stateTestNode("child", nil)],
        [stateTestNode("", nil), stateTestNode("missing/child", nil)],
        [stateTestNode("", nil), stateTestNode("z", nil), stateTestNode("a", nil)],
    ] {
        #expect(throws: FlyrailValueError.self) { try Revision(nodes: nodes) }
    }
    let valid = try Revision(nodes: [
        stateTestNode("", nil), stateTestNode("a", nil), stateTestNode("a/child", Data()),
    ])
    #expect(valid.nodes.map(\.path) == ["", "a", "a/child"])
}

@Test
func stateCodecRejectsMalformedNoncanonicalAndTrailingData() throws {
    for input in stateMalformedCorpus() {
        #expect(throws: FlyrailValueError.self) { try StateCodec.transcode(input) }
    }
    let claim = try Claim(kind: .file, selector: .whole)
    let unsorted = Data(#" { "type":"Claim", "fields":{"selector":null,"kind":{"value":"file","enum":"Ownership"}} } "#
        .utf8)
    #expect(try StateCodec.decode(unsorted, as: Claim.self) == claim)
    #expect(try StateCodec.transcode(unsorted) == StateCodec.encode(claim))
    let bom = Data([0xEF, 0xBB, 0xBF]) + unsorted
    #expect(try StateCodec.decode(bom, as: Claim.self) == claim)
    #expect(throws: FlyrailValueError.self) {
        try StateCodec.decodeMetadata(StateCodec.encode(claim), as: Receipt.self, tag: "Receipt")
    }
}

@Test
func stateEncoderAndDecoderEnforceEveryBudget() throws {
    let key = try Key(String(repeating: "x", count: 100))
    let raw = try StateCodec.encode(key)
    for limits in [
        JSONLimits(maximumInputBytes: raw.count - 1, maximumDepth: 512, maximumTokens: 1000, maximumStringBytes: 1000),
        JSONLimits(maximumInputBytes: 1000, maximumDepth: 1, maximumTokens: 1000, maximumStringBytes: 1000),
        JSONLimits(maximumInputBytes: 1000, maximumDepth: 512, maximumTokens: 5, maximumStringBytes: 1000),
        JSONLimits(maximumInputBytes: 1000, maximumDepth: 512, maximumTokens: 1000, maximumStringBytes: 99),
    ] {
        #expect(throws: FlyrailValueError.self) { try StateCodec.encode(key, limits: limits) }
        #expect(throws: FlyrailValueError.self) { try StateCodec.decode(raw, as: Key.self, limits: limits) }
    }
    let depth = Data((String(repeating: "[", count: 514) + "0" + String(repeating: "]", count: 514)).utf8)
    #expect(throws: FlyrailValueError.self) { try StateCodec.transcode(depth) }
    let boundary = Data((String(repeating: "[", count: 512) + "0" + String(repeating: "]", count: 512)).utf8)
    #expect(try StateCodec.transcode(boundary) == boundary + Data([10]))
    let oversized = Data(repeating: 32, count: JSONLimits.default.maximumInputBytes + 1)
    #expect(throws: FlyrailValueError.self) { try StateCodec.transcode(oversized) }
    let string = Data(("\"" + String(repeating: "x", count: JSONLimits.default.maximumStringBytes + 1) + "\"").utf8)
    #expect(throws: FlyrailValueError.self) { try StateCodec.transcode(string) }
    #expect(throws: FlyrailValueError.self) {
        try StateCodec.transcode(Data(String(repeating: "1", count: 4301).utf8))
    }
    let payload = Data(repeating: 0, count: JSONLimits.default.maximumStringBytes / 2 + 1)
    let node = try stateTestNode("", payload)
    #expect(node == node)
    #expect(throws: FlyrailValueError.self) { try StateCodec.encode(node) }
    let counts = Data(("[" + String(repeating: "0,", count: JSONLimits.default.maximumTokens) + "0]").utf8)
    #expect(throws: FlyrailValueError.self) { try StateCodec.transcode(counts) }
}

@Test
func statePathsAndTextIdentitiesRetainExactUTF8() throws {
    let composed = try ResourcePath("/fixture/é")
    let decomposed = try ResourcePath("/fixture/e\u{301}")
    #expect(composed != decomposed)
    #expect(try statePath(for: composed) == statePath(for: decomposed))
    let key1 = try Key("é")
    let key2 = try Key("e\u{301}")
    #expect(try StateCodec.encode(key1) != StateCodec.encode(key2))
    #expect(try StateCodec.decode(StateCodec.encode(key2), as: Key.self) == key2)
    let nonUTF8 = try ResourcePath(platformBytes: [47, 102, 255])
    #expect(throws: FlyrailValueError.self) { try StateCodec.path(nonUTF8) }
    let resource = try stateTestResource("AGENTS.md")
    let first = try stateTestOwned(resource, version: "é")
    let second = try stateTestOwned(resource, version: "e\u{301}")
    #expect(first != second)
    #expect(Set([first, second]).count == 2)
    #expect(try StateCodec.decode(StateCodec.encode(first), as: OwnedClaim.self) == first)
    let zero = try Binary64(bitPattern: 0)
    let negative = try Binary64(bitPattern: 1 << 63)
    let member1 = try Member(identity: .scalar(.float(zero)))
    let member2 = try Member(identity: .scalar(.float(negative)))
    #expect(try StateCodec.encode(member1) != StateCodec.encode(member2))
}

@Test
func stateIndexesUnionAndValidateAllGenerations() throws {
    let a = try stateTestResource("a.md")
    let b = try stateTestResource("b.md")
    let c = try stateTestResource("c.md")
    let generation = try stateTestGeneration([a, b])
    let index = try Index(
        bundleID: "team", contextDigest: String(repeating: "d", count: 64),
        current: generation, pending: stateTestGeneration([b, c]), previous: generation, residual: [c],
    )
    let expected = [a, b, c].sorted {
        $0.stateRoot.platformBytes.lexicographicallyPrecedes($1.stateRoot.platformBytes)
    }
    #expect(index.resources == expected)
    #expect(try StateCodec.decode(StateCodec.encode(index), as: Index.self) == index)
    #expect(throws: FlyrailValueError.self) { try stateTestGeneration([a, a]) }
    #expect(throws: FlyrailValueError.self) {
        try GenerationMember(
            resource: a,
            claimIDs: [String(repeating: "a", count: 64), String(repeating: "a", count: 64)],
        )
    }
    let tree = try ResourceRef(destination: a.destination, kind: .tree, stateRoot: a.stateRoot)
    #expect(throws: FlyrailValueError.self) {
        try Index(
            bundleID: "team",
            contextDigest: String(repeating: "d", count: 64),
            current: generation,
            residual: [tree],
        )
    }
}

func stateTestNode(_ path: String, _ data: Data?, inode: Count = 1) throws -> Node {
    try Node(path: path, data: data, mode: 0o600, uid: 0, gid: 0, device: 1, inode: inode)
}

func stateTestResource(_ name: String, kind: ResourceKind = .document) throws -> ResourceRef {
    let destination = try ResourcePath("/fixture/project/\(name)")
    return try ResourceRef(destination: destination, kind: kind, stateRoot: statePath(for: destination))
}

func stateTestOwned(_ resource: ResourceRef, version: String = "1", inode: Count = 1) throws -> OwnedClaim {
    let claim = try Claim(kind: resource.kind == .tree ? .tree : .file, selector: .whole)
    return try OwnedClaim(
        claim: claim, claimID: claim.identity(destination: resource.destination), bundleID: "team", artifactID: "guide",
        version: version, bundleDigest: String(repeating: "a", count: 64),
        renderDigest: String(repeating: "b", count: 64),
        installed: Revision(nodes: [stateTestNode("", Data("é \0 日本語\n".utf8), inode: inode)]),
    )
}

func stateTestGeneration(_ refs: [ResourceRef]) throws -> Generation {
    try Generation(
        version: "opaque é \0", bundleDigest: String(repeating: "a", count: 64),
        renderDigest: String(repeating: "b", count: 64),
        membership: refs.map { try GenerationMember(resource: $0, claimIDs: [String(repeating: "c", count: 64)]) },
    )
}

func stateMalformedCorpus() -> [Data] {
    [
        #"{"type":"Unknown","fields":{}}"#,
        #"{"type":"Key","fields":{"name":"a","extra":0}}"#,
        #"{"type":"Key","fields":{}}"#,
        #"{"type":"Key","fields":{"name":"a"},"extra":0}"#,
        #"{"type":"Key","type":"Key","fields":{"name":"a"}}"#,
        #"{"type":"Key","fields":{"name":"a","name":"b"}}"#,
        #"{"type":"Key","fields":{"name":true}}"#,
        #"{"type":"Key","fields":{"name":"\ud800"}}"#,
        #"{"type":[],"fields":{}}"#,
        #"{"enum":"Ownership","value":"bogus"}"#,
        #"{"enum":"Ownership","value":true}"#,
        #"{"enum":"Ownership","value":"file","extra":0}"#,
        #"{"enum":"Key","value":"a"}"#,
        #"{"type":"Ownership","fields":{"value":"file"}}"#,
        #"{"bytes":"AB"}"#,
        #"{"bytes":"ab cd"}"#,
        #"{"bytes":"f"}"#,
        #"{"bytes":true}"#,
        #"{"path":"relative"}"#,
        #"{"path":"/a/../b"}"#,
        #"{"path":"/a/./b"}"#,
        #"{"path":"/a//b"}"#,
        #"{"path":"/a\u0000b"}"#,
        #"{"path":"/a/"}"#,
        #"{"value":["integer","9223372036854775808"]}"#,
        #"{"value":["integer","01"]}"#,
        #"{"value":["float","nan"]}"#,
        #"{"value":["float","0x1p+0"]}"#,
        #"{"value":["object",[["z",["null"]],["a",["null"]]]]}"#,
        #"{"value":["object",[["a",["null"]],["a",["null"]]]]}"#,
        #"{"type":"Claim","fields":{"kind":{"enum":"Ownership","value":"file"},"selector":0}}"#,
        #"{"type":"Claim","fields":{"kind":{"enum":"Ownership","value":"tree"},"selector":"x"}}"#,
        #"{"type":"Revision","fields":{"nodes":true}}"#,
        "1.0", "1e0", "NaN", "Infinity", "-Infinity", "null null", "[] trailing",
    ].map { Data($0.utf8) } + [Data([0xFF]), Data([34, 0xC0, 0x80, 34])]
}
