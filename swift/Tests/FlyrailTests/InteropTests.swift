@testable import Flyrail
import FlyrailInteropSupport
import Foundation
import Testing

@Test
func interopRejectsMalformedCommandsBeforeWriting() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let state = root.url.appending(path: "state").path
    let destination = root.url.appending(path: "document").path
    let base = ["write", "--state", state, "--destination", destination]
    let usage = "usage: FlyrailInterop differential|render-hooks|encode|decode|write|install|update|inspect|" +
        "remove|recover|records|hold|observe [options]"
    let cases: [([String], String)] = [
        ([], usage), (["unknown"], usage),
        (["encode", "--type"], "unexpected interop arguments"),
        (["encode", "--type", "Receipt", "--type", "Receipt"], "unexpected interop arguments"),
        (["records", "--state"], "interop option requires a value"),
        (["inspect", "--state", state, "--fault-phase", "publish"],
         "unexpected or repeated interop argument: --fault-phase"),
        (base + ["--large", "-1"], "large payload size must be nonnegative"),
        (base + ["--large", "1048577"], "large payload size must not exceed 1048576"),
        (base + ["--mode", "unknown"], "invalid mode"),
        (base + ["--acquisition", "unknown"], "invalid destination or acquisition"),
        (
            base + ["--lock-timeout", "nan"],
            "invalid --lock-timeout: cannot convert float seconds to Duration: value is either too big or NaN",
        ),
        (
            base + ["--lock-timeout", "-1"],
            "invalid --lock-timeout: cannot convert float seconds to Duration: value is negative",
        ),
        (["write", "--state", state, "--destination", state + "/inner.md"],
         "state store overlaps a resource or authority"),
        (base + ["--fault-phase", "unknown"], "unknown transaction seam"),
        (base + ["--fail-index-completion", "--fault-phase", "publish"], "fault options cannot be combined"),
        (base + ["--fail-index-completion", "--barrier-timeout", "1"],
         "fault and barrier options cannot be combined"),
        (base + ["--barrier-phase", "publish"], "--barrier-phase requires --barrier-ready and --barrier-release"),
        (base + ["--barrier-ready", "ready"], "barrier options require --barrier-phase"),
        (base + ["--kind", "tree", "--section", "guide"], "tree resources cannot have a section"),
        (base + ["--destination", destination], "unexpected or repeated interop argument: --destination"),
        (["install", "--state", state, "--destination", destination, "--large", "-1"],
         "large payload size must be nonnegative"),
        (["update", "--state", state, "--destination", destination, "--mode", "unknown"], "invalid mode"),
        (["hold", "--state", state], "missing required argument: --destination"),
        (
            ["remove", "--state", state, "--lock-timeout", "nan"],
            "invalid --lock-timeout: cannot convert float seconds to Duration: value is either too big or NaN",
        ),
        (
            ["recover", "--state", state, "--lock-timeout", "-1"],
            "invalid --lock-timeout: cannot convert float seconds to Duration: value is negative",
        ),
        (base + ["--source", destination, "--version", "2", "--lock-timeout", "nan"],
         "--version cannot be combined with --source"),
    ]
    for (arguments, message) in cases {
        let result = try invokeInterop(arguments, input: Data(), root: root.url)
        #expect(result.status == 1, "\(arguments)")
        if ["write", "install", "update", "hold", "remove", "recover", "inspect", "records"].contains(arguments.first) {
            #expect(result.output == interopFailure(message), "\(arguments)")
            #expect(result.error == Data())
        } else {
            #expect(result.output == Data())
            #expect(String(decoding: result.error, as: UTF8.self) == message + "\n", "\(arguments)")
        }
        #expect(!FileManager.default.fileExists(atPath: state))
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
    let state = root.url.appending(path: "state").path
    let destination = root.url.appending(path: "project").path
    let fixture = try #require(Bundle.module.url(forResource: "interop-source-options", withExtension: "json",
                                                 subdirectory: "Fixtures"))
    let document = try #require(JSONSerialization.jsonObject(with: Data(contentsOf: fixture)) as? [String: Any])
    for pair in try #require(document["cases"] as? [[String]]) {
        try #require(pair.count == 2, "\(pair)")
        let (option, value) = (pair[0], pair[1])
        let result = try invokeInterop([
            "write", "--state", state, "--destination", destination, "--source", source.path, option, value,
        ], input: Data(), root: root.url)
        #expect(result.status == 1)
        #expect(result.output == interopFailure("\(option) cannot be combined with --source"))
        #expect(result.error == Data())
        #expect(!FileManager.default.fileExists(atPath: state))
        #expect(!FileManager.default.fileExists(atPath: destination))
    }
}

@Test
func interopChecksStoreAgainstSourceBeforeResolvingArtifacts() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let source = root.url.appending(path: "source")
    try FileManager.default.createDirectory(at: source, withIntermediateDirectories: false)
    try Data(#"{"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":"instruction","id":"guide","text":"safe"}]}"#
        .utf8).write(to: source.appending(path: "flyrail.json"))
    let real = root.url.appending(path: "real")
    try FileManager.default.createDirectory(at: real, withIntermediateDirectories: false)
    let link = root.url.appending(path: "link")
    try FileManager.default.createSymbolicLink(at: link, withDestinationURL: real)
    for command in ["write", "install", "update"] {
        let result = try invokeInterop([
            command, "--state", source.appending(path: "state").path,
            "--destination", link.appending(path: "project").path, "--source", source.path,
        ], input: Data(), root: root.url)
        #expect(result.status == 1, "\(command)")
        #expect(result.output == interopFailure("state store overlaps bundle source"), "\(command)")
        #expect(result.error == Data(), "\(command)")
    }
}

@Test
func interopDurationsMatchRustRange() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let state = root.stateURL.path
    let tooBig = "invalid --lock-timeout: cannot convert float seconds to Duration: value is either too big or NaN"
    let big = try invokeInterop(["inspect", "--state", state, "--lock-timeout", "1e20"], input: Data(), root: root.url)
    #expect(big.status == 0)
    let write = try invokeInterop(
        ["write", "--state", state, "--destination", root.url.appending(path: "document").path,
         "--lock-timeout", "2e9"],
        input: Data(), root: root.url,
    )
    #expect(write.status == 0)
    #expect(try stateField(JSONParser.parse(write.output), "status") == .string("applied"))
    let rejected = try invokeInterop(
        ["remove", "--state", state, "--lock-timeout", "1e20"], input: Data(), root: root.url,
    )
    #expect(rejected.status == 1)
    #expect(rejected.output == interopFailure(tooBig))
    let inspected = try invokeInterop(["inspect", "--state", state, "--lock-timeout", "-1"], input: Data(),
                                      root: root.url)
    #expect(inspected.status == 0)
    #expect(inspected.error == Data())
    let destination = root.url.appending(path: "held").path
    let barrier = root.url.appending(path: "barrier").path
    let hold = ["hold", "--state", state, "--destination", destination, "--barrier", barrier]
    let both = try invokeInterop(
        hold + ["--lock-timeout", "-1", "--wait-timeout", "nan"],
        input: Data(),
        root: root.url,
    )
    #expect(both.output == interopFailure(
        "invalid --wait-timeout: cannot convert float seconds to Duration: value is either too big or NaN",
    ))
    let large = try invokeInterop(hold + ["--wait-timeout", "1e19"], input: Data(), root: root.url)
    #expect(large.status == 1)
    #expect(large.output == Data(#"{"status":"reserved"}"#.utf8) + Data([10]) +
        interopFailure("invalid --wait-timeout: duration is too large"))
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

@Test
func interopRecordsListStoreRecords() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let state = root.stateURL.path
    let destination = try root.child("document")
    let written = try invokeInterop(["write", "--state", state, "--destination", destination.displayString],
                                    input: Data(), root: root.url)
    #expect(written.status == 0)
    #expect(try stateField(JSONParser.parse(written.output), "status") == .string("applied"))
    let listed = try invokeInterop(["records", "--state", state], input: Data(), root: root.url)
    #expect(listed.status == 0)
    #expect(listed.error == Data())
    let records = try stateArray(stateField(JSONParser.parse(listed.output), "records"))
    let resource = try "resource/" + StateKey.destination(destination)
    #expect(try records.map { try stateField($0, "key") } == [
        .string(InstallationIndex.key(bundleID: "team", target: root.target())),
        .string(resource + "/authority"), .string(resource + "/receipt"),
    ])
    #expect(try records.map { try stateField($0, "type") } == [
        .string("Index"), .string("ResourceRef"), .string("Receipt"),
    ])
    for record in records {
        #expect(try stateField(record, "bytes") == stateField(record, "canonical"))
    }
}

@Test
func interopHoldExcludesWritersInItsStore() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let state = root.stateURL.path
    let destination = try root.child("document").displayString
    let barrier = root.url.appending(path: "barrier")
    let pipe = Pipe()
    let output = pipe.fileHandleForWriting
    let finished = DispatchSemaphore(value: 0)
    let arguments = ["hold", "--state", state, "--destination", destination, "--barrier", barrier.path]
    let thread = Thread {
        _ = runInterop(arguments: arguments, standardInput: FileHandle.nullDevice, standardOutput: output,
                       standardError: output)
        finished.signal()
    }
    thread.start()
    var released = false
    defer {
        if !released {
            FileManager.default.createFile(atPath: barrier.path, contents: Data())
            _ = finished.wait(timeout: .now() + 10)
        }
    }
    let reserved = try interopLine(pipe.fileHandleForReading, deadline: ContinuousClock.now.advanced(by: .seconds(10)))
    #expect(reserved == #"{"status":"reserved"}"#)
    let blocked = try invokeInterop(["write", "--state", state, "--destination", destination, "--lock-timeout", "0.01"],
                                    input: Data(), root: root.url)
    let error = try stateField(JSONParser.parse(blocked.output), "error")
    #expect(try stateField(error, "code") == .string("busy"))
    #expect(!FileManager.default.fileExists(atPath: destination))
    #expect(FileManager.default.createFile(atPath: barrier.path, contents: Data()))
    released = true
    #expect(finished.wait(timeout: .now() + 10) == .success)
    let admitted = try interopLine(pipe.fileHandleForReading, deadline: ContinuousClock.now.advanced(by: .seconds(10)))
    #expect(admitted == #"{"status":"admitted"}"#)
}

private func interopLine(_ handle: FileHandle, deadline: ContinuousClock.Instant) throws -> String {
    var data = Data()
    while !data.contains(10) {
        let remaining = deadline - ContinuousClock.now
        guard remaining > .zero else { throw FlyrailValueError("interop output timed out") }
        let (seconds, attoseconds) = remaining.components
        let milliseconds = Int32(clamping: seconds * 1000 + attoseconds / 1_000_000_000_000_000 + 1)
        var descriptor = pollfd(fd: handle.fileDescriptor, events: Int16(POLLIN), revents: 0)
        guard poll(&descriptor, 1, milliseconds) >= 0 else { throw FlyrailValueError("interop output poll failed") }
        if descriptor.revents != 0 {
            data.append(handle.availableData)
        }
    }
    return String(decoding: data.prefix { $0 != 10 }, as: UTF8.self)
}

private func interopFailure(_ message: String) -> Data {
    Data(#"{"error":{"code":null,"message":"\#(message)","path":null}}"#.utf8) + Data([10])
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
