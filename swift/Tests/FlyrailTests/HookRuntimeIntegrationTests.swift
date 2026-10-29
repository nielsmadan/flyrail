@testable import Flyrail
import Foundation
import Testing

@Test(.enabled(if: ProcessInfo.processInfo.environment["FLYRAIL_SWIFT_INTEGRATION"] == "1"))
func packagedHookRuntimeExecutesSharedHarnesses() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let runtime = root.url.appending(path: "runtime")
    try FileManager.default.createDirectory(at: runtime, withIntermediateDirectories: false)
    let snapshot = try HookRuntime()
    #expect(snapshot.resources.count == 3)
    for entry in snapshot.resources {
        try #require(entry.data).write(to: runtime.appending(path: entry.path))
    }
    var wires: [[String: Any]] = []
    for escaped in [#"\ud800"#, #"\udfff"#, #"\ud83d\ude00"#] {
        let input = Data(
            #"{"version":1,"event":"prompt","context":{},"input":"\#(escaped)"}"#.utf8,
        )
        let responses = [
            #"{"version":1,"outcome":"modify-input","input":"\#(escaped)"}"#,
            #"{"version":1,"outcome":"context","text":"\#(escaped)"}"#,
            #"{"version":1,"outcome":"block","reason":"\#(escaped)"}"#,
        ].map { Data($0.utf8) }
        let effects = Data(
            #"{"contexts":["\#(escaped)"],"input":"\#(escaped)","decision":"block","reason":"\#(escaped)"}"#.utf8,
        )
        try wires.append([
            "input": input.base64EncodedString(),
            "responses": responses.map { $0.base64EncodedString() },
            "inputBase64": HookRequest(data: input).data.base64EncodedString(),
            "responseBase64": responses.map {
                try HookResponse(data: $0, event: .prompt, declared: HookOutcome.allCases).data.base64EncodedString()
            },
            "effectsBase64": HookEffects(data: effects).data.base64EncodedString(),
        ])
    }
    let wireURL = root.url.appending(path: "wires.json")
    try JSONSerialization.data(withJSONObject: wires).write(to: wireURL)
    let process = Process()
    let output = Pipe()
    process.executableURL = try repositoryFile("python/.venv/bin/python")
    process.arguments = try [
        "-B", repositoryFile("swift/scripts/check_hook_runtime.py").path,
        "--runtime", runtime.path, "--wires", wireURL.path,
    ]
    process.standardOutput = output
    process.standardError = output
    try process.run()
    let text = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    process.waitUntilExit()
    #expect(process.terminationStatus == 0, "\(text)")
    print(text)
}
