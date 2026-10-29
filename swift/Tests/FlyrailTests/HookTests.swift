@testable import Flyrail
import Foundation
import Testing

@Test
func hookTextPreservesUnpairedUTF16Surrogates() throws {
    for (source, expected) in [
        (#""\ud800""#, #""\ud800""#),
        (#""\udfff""#, #""\udfff""#),
        (#""\ud83d\ude00""#, #""😀""#),
    ] {
        let value = try HookJSON(data: Data(source.utf8))
        #expect(String(decoding: value.data, as: UTF8.self) == expected)
    }
    let lone = try HookJSON(data: Data(#""\ud800""#.utf8))
    #expect(lone.text?.string == nil)
    #expect(lone.text?.utf16CodeUnits == [0xD800])
}

@Test
func hookRequestsAndResponsesAreClosedAndCanonical() throws {
    let request = try HookRequest(data: Data(
        #"{"version":1,"event":"before-tool","tool":"Bash","input":{"command":"pwd"},"context":{}}"#.utf8
    ))
    #expect(String(decoding: request.data, as: UTF8.self) ==
        #"{"version":1,"event":"before-tool","context":{},"tool":"Bash","input":{"command":"pwd"}}"#)
    #expect(throws: HookFailure.self) {
        try HookRequest(data: Data(
            #"{"version":1,"event":"stop","context":{},"extra":true}"#.utf8
        ))
    }
    let response = try HookResponse(
        data: Data(#"{"version":1,"outcome":"block","reason":"stop"}"#.utf8),
        event: .beforeTool,
        declared: [.continue, .block]
    )
    #expect(response.outcome == .block)
    #expect(response.data == Data(#"{"version":1,"outcome":"block","reason":"stop"}"#.utf8))
}

@Test
func hookJSONRejectsReservedDuplicateAndDeepObjects() throws {
    for source in [
        #"{"__proto__":{}}"#,
        #"{"same":1,"same":2}"#,
        #"{"constructor":1}"#,
    ] {
        #expect(throws: HookFailure.self) {
            try HookJSON(data: Data(source.utf8))
        }
    }
    let deep = String(repeating: "[", count: 65) + "0" + String(repeating: "]", count: 65)
    #expect(throws: HookFailure.self) {
        try HookJSON(data: Data(deep.utf8))
    }
}

@Test
func hookJSONPreservesEarlierSemanticFailure() throws {
    let deep = String(repeating: "[", count: 65) + "0" + String(repeating: "]", count: 65)
    for (prefix, code) in [
        (#""__proto__":0"#, "json-key"),
        (#""same":0,"same":1"#, "json-key"),
        (#""number":1e999"#, "json-number"),
    ] {
        do {
            _ = try HookJSON(data: Data("{\(prefix),\"deep\":\(deep)}".utf8))
            Issue.record("expected \(code)")
        } catch let failure as HookFailure {
            #expect(failure.code == code)
        }
    }
}

@Test
func hookJSONUsesJavaScriptNumberEncoding() throws {
    for (source, expected) in [
        ("9007199254740991", "9007199254740991"),
        ("9007199254740992", "9007199254740992"),
        ("9007199254740993", "9007199254740992"),
        ("100000000000000000000", "100000000000000000000"),
        ("1e21", "1e+21"),
        ("5e-324", "5e-324"),
        ("-0", "0"),
        ("1.01", "1.01"),
    ] {
        let value = try HookJSON(data: Data(source.utf8))
        #expect(String(decoding: value.data, as: UTF8.self) == expected)
    }
}

@Test
func hookConfigSortsHandlersAndBoundsEvents() throws {
    let cwd = try ResourcePath("/tmp/project")
    let handler = try HookHandler(
        id: "check",
        event: .beforeTool,
        argv: ["check"],
        environment: [],
        cwd: cwd,
        timeoutMilliseconds: 10000,
        outcomes: [.continue],
        tools: []
    )
    let config = try HookConfig(host: "claude", handlers: [handler])
    #expect(String(decoding: config.data, as: UTF8.self).contains(#""host":"claude""#))
    #expect(throws: FlyrailValueError.self) {
        try HookConfig(host: "claude", handlers: Array(repeating: handler, count: 65))
    }
}

@Test
func hookHandlerToolNamesUseExactUTF8Identity() throws {
    func handler(tools: [String]) throws -> HookHandler {
        try HookHandler(
            id: "check",
            event: .beforeTool,
            argv: ["check"],
            environment: [],
            cwd: ResourcePath("/tmp/project"),
            timeoutMilliseconds: 10000,
            outcomes: [.continue],
            tools: tools
        )
    }

    _ = try handler(tools: ["é", "e\u{301}"])
    #expect(throws: FlyrailValueError.self) {
        try handler(tools: ["é", "é"])
    }
}

@Test
func hookRuntimeRejectsEveryParentPathComponent() throws {
    _ = try HookRuntime(node: ResourcePath("/tmp/node"))
    for path in ["/tmp/../node", "/tmp/.."] {
        #expect(throws: FlyrailValueError.self) {
            try HookRuntime(node: ResourcePath(path))
        }
    }
}

@Test
func hookFixtureWireRecordsParse() throws {
    let url = try #require(Bundle.module.url(
        forResource: "hooks",
        withExtension: "json",
        subdirectory: "Fixtures"
    ))
    let fixture = try #require(
        JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
    )
    for item in fixture["cases"] as? [[String: Any]] ?? [] {
        let eventName = try #require(item["event"] as? String)
        let event = try #require(HookEvent(rawValue: eventName))
        let response = try JSONSerialization.data(
            withJSONObject: #require(item["response"] as? [String: Any])
        )
        _ = try HookResponse(data: response, event: event, declared: HookOutcome.allCases)

        var request = item["canonical"] as? [String: Any] ?? [:]
        request["version"] = 1
        request["event"] = eventName
        request["context"] = item["context"] as? [String: Any] ?? [:]
        let parsed = try HookRequest(data: JSONSerialization.data(withJSONObject: request))
        #expect(parsed.event == event)
    }
}
