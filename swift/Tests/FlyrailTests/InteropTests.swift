@testable import Flyrail
import FlyrailInteropSupport
import Foundation
import Testing

@Test
func interopRejectsMalformedCommandsBeforeWriting() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let index = root.url.appending(path: "index").path
    let destination = root.url.appending(path: "document").path
    let base = ["write", "--index", index, "--destination", destination]
    let usage = "usage: FlyrailInterop differential|render-hooks|encode|decode|write|inspect|remove|recover|" +
        "records|hold|observe [options]"
    let cases: [([String], String)] = [
        ([], usage), (["unknown"], usage),
        (["encode", "--type"], "unexpected interop arguments"),
        (["encode", "--type", "Receipt", "--type", "Receipt"], "unexpected interop arguments"),
        (["records", "--index"], "interop option requires a value"),
        (["inspect", "--index", index, "--fault-phase", "publish"],
         "unexpected or repeated interop argument: --fault-phase"),
        (base + ["--large", "-1"], "large payload size must be between 0 and 1048576"),
        (base + ["--large", "1048577"], "large payload size must be between 0 and 1048576"),
        (base + ["--mode", "unknown"], "invalid mode"),
        (base + ["--acquisition", "unknown"], "invalid destination or acquisition"),
        (base + ["--lock-timeout", "nan"], "timeout must be finite and nonnegative"),
        (base + ["--lock-timeout", "-1"], "timeout must be finite and nonnegative"),
        (base + ["--fault-phase", "unknown"], "unknown transaction seam"),
        (base + ["--fail-index-completion", "--fault-phase", "publish"], "fault options cannot be combined"),
        (base + ["--fail-index-completion", "--barrier-timeout", "1"],
         "fault and barrier options cannot be combined"),
        (base + ["--barrier-phase", "publish"], "--barrier-phase requires --barrier-ready and --barrier-release"),
        (base + ["--barrier-ready", "ready"], "barrier options require --barrier-phase"),
        (base + ["--kind", "tree", "--section", "guide"], "tree resources cannot have a section"),
        (base + ["--destination", destination], "unexpected or repeated interop argument: --destination"),
    ]
    for (arguments, message) in cases {
        let result = try invokeInterop(arguments, input: Data(), root: root.url)
        #expect(result.status == 1, "\(arguments)")
        #expect(result.output == Data())
        #expect(String(decoding: result.error, as: UTF8.self) == message + "\n", "\(arguments)")
        #expect(!FileManager.default.fileExists(atPath: index))
        #expect(!FileManager.default.fileExists(atPath: destination))
    }
}

@Test
func interopRejectsSyntheticOptionsWithSourceBeforeWriting() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let source = root.url.appending(path: "source")
    try FileManager.default.createDirectory(at: source, withIntermediateDirectories: false)
    try Data(#"{"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":"instruction","id":"guide","text":"safe"}]}"#
        .utf8).write(to: source.appending(path: "flyrail.json"))
    let index = root.url.appending(path: "index").path
    let destination = root.url.appending(path: "project").path
    let fixture = try #require(Bundle.module.url(forResource: "interop-source-options", withExtension: "json",
                                                 subdirectory: "Fixtures"))
    let document = try #require(JSONSerialization.jsonObject(with: Data(contentsOf: fixture)) as? [String: Any])
    for pair in try #require(document["cases"] as? [[String]]) {
        try #require(pair.count == 2, "\(pair)")
        let (option, value) = (pair[0], pair[1])
        let result = try invokeInterop([
            "write", "--index", index, "--destination", destination, "--source", source.path, option, value,
        ], input: Data(), root: root.url)
        #expect(result.status == 1)
        #expect(result.output == Data())
        #expect(String(decoding: result.error, as: UTF8.self) == "\(option) cannot be combined with --source\n")
        #expect(!FileManager.default.fileExists(atPath: index))
        #expect(!FileManager.default.fileExists(atPath: destination))
    }
}

@Test
func interopBoundsInputAndCanonicalizesState() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let record = try StateCodec.encode(DocumentProvenance())
    let result = try invokeInterop(["encode", "--type", "DocumentProvenance"], input: record, root: root.url)
    #expect(result.status == 0)
    #expect(result.output == record)
    #expect(result.error == Data())
    for (input, message) in [
        (Data(repeating: 32, count: JSONLimits.default.maximumInputBytes + 1), "JSON input exceeds the byte limit"),
        (Data(#"{"type":"Unknown","fields":{}}"#.utf8), "unknown state record"),
    ] {
        let failure = try invokeInterop(["encode"], input: input, root: root.url)
        #expect(failure.status == 1)
        #expect(failure.output == Data())
        #expect(String(decoding: failure.error, as: UTF8.self) == message + "\n")
    }
}

private func invokeInterop(
    _ arguments: [String], input: Data, root: URL,
) throws -> (status: Int32, output: Data, error: Data) {
    let inputURL = root.appending(path: "stdin")
    let outputURL = root.appending(path: "stdout")
    let errorURL = root.appending(path: "stderr")
    try input.write(to: inputURL)
    try Data().write(to: outputURL)
    try Data().write(to: errorURL)
    let standardInput = try FileHandle(forReadingFrom: inputURL)
    let standardOutput = try FileHandle(forWritingTo: outputURL)
    let standardError = try FileHandle(forWritingTo: errorURL)
    defer {
        try? standardInput.close()
        try? standardOutput.close()
        try? standardError.close()
    }
    let status = runInterop(arguments: arguments, standardInput: standardInput,
                            standardOutput: standardOutput, standardError: standardError)
    return try (status, Data(contentsOf: outputURL), Data(contentsOf: errorURL))
}
