@testable import Flyrail
import Testing

@Test
func validationMatchesPortableRules() throws {
    try validateIdentifier("team-notes", label: "bundle id")
    #expect(throws: FlyrailValueError.self) {
        try validateIdentifier("con", label: "bundle id")
    }
    try validateVersion(" 1 ")
    #expect(throws: FlyrailValueError.self) {
        try validateVersion("\u{A0}")
    }
    try validateRelativePath("skills/café.md", label: "path")
    for path in ["", "../a", "a\\b", "con/file", "a/NUL.txt", "a/nul .txt", "cafe\u{301}.md"] {
        #expect(throws: Error.self, "must reject \(path)") {
            try validateRelativePath(path, label: "path")
        }
    }
}

@Test
func generatedFullCaseFoldUsesUnicodeSixteen() {
    #expect(unicodeCaseFoldingVersion == "16.0.0")
    #expect(portablePathKey("STRASSE") == "strasse")
    #expect(portablePathKey("straße") == "strasse")
    #expect(portablePathKey("ΣΊΣΥΦΟΣ") == "σίσυφοσ")
    #expect(portablePathKey("ﬁle") == "file")
    #expect(portablePathKey("cafe\u{301}") == "café")
}

@Test
func bundleIdentityUsesVerbatimUTF8() throws {
    let composed = try BundleIdentity(id: "team", version: "é")
    let decomposed = try BundleIdentity(id: "team", version: "e\u{301}")
    #expect(composed != decomposed)
}
