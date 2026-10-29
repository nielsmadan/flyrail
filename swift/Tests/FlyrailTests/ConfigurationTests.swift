@testable import Flyrail
import FlyrailInteropSupport
import Foundation
import Testing

@Test
func configurationFixturesMatchDigestsAndReconstruction() throws {
    let fixture = try configurationFixture()
    for rawCase in fixture["cases"] as? [[String: Any]] ?? [] {
        let manifest = try #require(rawCase["manifest"] as? [String: Any])
        let data = try JSONSerialization.data(withJSONObject: manifest, options: [.sortedKeys])
        let entries = try (rawCase["entries"] as? [[String: Any]] ?? []).map { item in
            try BundleEntry(
                path: #require(item["path"] as? String),
                data: (item["data_hex"] as? String).map { try Data(interopHex: $0) }
            )
        }
        let bundle = try FlyrailBundle(manifest: data, entries: entries)
        #expect(bundle.schemaVersion == 2)
        #expect(bundle.contentDigest == rawCase["content_digest"] as? String)
        let rebuilt = try FlyrailBundle(
            identity: bundle.identity,
            artifacts: bundle.artifacts,
            assets: bundle.assets,
            dependencies: bundle.dependencies
        )
        #expect(rebuilt.contentDigest == bundle.contentDigest)
        #expect(try rebuilt.differentialCanonicalValue() == bundle.differentialCanonicalValue())
    }
}

@Test
func configurationManifestRejectsUnknownAndDuplicateFields() throws {
    let source =
        #"{"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":"instruction","id":"guide","text":"Hello"}]}"#
    for invalid in [
        source.replacingOccurrences(of: #""id":"team""#, with: #""id":"team","id":"team""#),
        source.replacingOccurrences(of: #""text":"Hello""#, with: #""text":"Hello","extra":true"#),
        source.replacingOccurrences(of: #""schema_version":2"#, with: #""schema_version":2.0"#),
    ] {
        #expect(throws: Error.self) {
            try FlyrailBundle(manifest: Data(invalid.utf8), entries: [])
        }
    }
}

@Test
func configurationManifestKeepsIntegerAndFloatTokensDistinct() throws {
    func manifest(_ value: String) -> Data {
        Data("""
        {"schema_version":2,"id":"team","version":"1","artifacts":[\
        {"kind":"native","id":"settings","family":"instructions",\
        "audience":{"agent":"claude","scope":"project"},"destination":"settings.json",\
        "content":{"kind":"structured","format":"json","selector":["value"],"value":\(value)}}]}
        """.utf8)
    }

    _ = try FlyrailBundle(manifest: manifest("1.5"), entries: [])
    _ = try FlyrailBundle(manifest: manifest("9223372036854775807"), entries: [])
    #expect(throws: FlyrailValueError.self) {
        try FlyrailBundle(manifest: manifest("9223372036854775808"), entries: [])
    }
    #expect(throws: FlyrailValueError.self) {
        try FlyrailBundle(manifest: manifest("-9223372036854775809"), entries: [])
    }
}

@Test
func sectionBoundariesRecognizeCRLFWithoutNormalizingText() throws {
    let boundaries = try SectionBoundaries(start: "START", end: "END")
    let accepted = try SectionContent(
        marker: "section",
        text: "prefix\r\nnot START\r\nsuffix",
        boundaries: boundaries
    )
    #expect(accepted.text == "prefix\r\nnot START\r\nsuffix")
    #expect(throws: FlyrailValueError.self) {
        try SectionContent(
            marker: "section",
            text: "prefix\r\nSTART\r\nsuffix",
            boundaries: boundaries
        )
    }
    #expect(throws: FlyrailValueError.self) {
        try SectionContent(
            marker: "section",
            text: "prefix\r\nEND\r\nsuffix",
            boundaries: boundaries
        )
    }
}

@Test
func HTTPTransportValidatesBracketedHostsAndNetlocNormalization() throws {
    for url in [
        "http://[::1]/",
        "https://[2001:db8::1]:443/path",
        "http://[v1.address]/",
    ] {
        #expect(throws: Never.self) {
            try HttpTransport(url: url)
        }
    }
    for url in [
        "http://[abc]/",
        "http://[1:2:3]/",
        "http://[2001:db8::1::2]/",
        "http://example.com：80/",
    ] {
        #expect(throws: FlyrailValueError.self) {
            try HttpTransport(url: url)
        }
    }
}

@Test
func hookArtifactToolNamesUseExactUTF8Identity() throws {
    let command = try Command(argv: [.text("check")])
    _ = try HookArtifact(
        id: "hook",
        event: .beforeTool,
        command: command,
        tools: ["é", "e\u{301}"]
    )
    #expect(throws: FlyrailValueError.self) {
        try HookArtifact(
            id: "hook",
            event: .beforeTool,
            command: command,
            tools: ["é", "é"]
        )
    }
}

@Test
func configurationDependenciesAndAssetReferencesAreValidated() throws {
    let identity = try BundleIdentity(id: "team", version: "1")
    let asset = try SupportAsset(
        id: "runtime",
        family: .mcp,
        content: TreeContent(entries: [
            BundleEntry(path: "run.js", data: Data()),
            BundleEntry(path: "data"),
        ])
    )
    let server = try Artifact.mcp(MCPArtifact(
        id: "server",
        name: "server",
        transport: .stdio(Command(argv: [
            .asset(AssetRef(assetID: "runtime", path: "run.js")),
        ]))
    ))
    #expect(throws: FlyrailValueError.self) {
        try FlyrailBundle(identity: identity, artifacts: [server], assets: [asset])
    }
    let bundle = try FlyrailBundle(
        identity: identity,
        artifacts: [server],
        assets: [asset],
        dependencies: [Dependency(dependent: "server", required: "runtime")]
    )
    #expect(bundle.schemaVersion == 2)
}

@Test(.enabled(if: repositoryCheckout))
func packagedConfigurationAndHookFixturesMatchRoot() throws {
    for name in ["configurations", "hooks"] {
        let packaged = try Data(contentsOf: #require(Bundle.module.url(
            forResource: name,
            withExtension: "json",
            subdirectory: "Fixtures"
        )))
        #expect(try packaged == Data(contentsOf: repositoryFile("spec/fixtures/\(name).json")))
    }
}

@Test
func hookRuntimePackagesItsThreeResources() throws {
    let runtime = try HookRuntime()
    #expect(runtime.resources.map(\.path).sorted() == ["bridge.mts", "process.mts", "runner.mts"])
}

@Test(.enabled(if: repositoryCheckout))
func hookRuntimeResourcesMatchPythonAndRustCopies() throws {
    for entry in try HookRuntime().resources {
        let data = try #require(entry.data)
        for directory in ["python/src/flyrail/runtime", "rust/src/runtime"] {
            #expect(try data == Data(contentsOf: repositoryFile("\(directory)/\(entry.path)")))
        }
    }
}

private func configurationFixture() throws -> [String: Any] {
    let url = try #require(Bundle.module.url(
        forResource: "configurations",
        withExtension: "json",
        subdirectory: "Fixtures"
    ))
    return try #require(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
}
