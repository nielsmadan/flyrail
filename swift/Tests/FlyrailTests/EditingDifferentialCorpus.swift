import Foundation

func editingDifferentialCases() throws -> [[String: Any]] {
    let fixtures = try editFixture()
    let cases = fixtures["cases"] as? [[String: Any]] ?? []
    return cases + editingOwnershipCases() + editingDiagnosticCases() + tomlDifferentialCases() +
        schemaDifferentialCases()
}

private func editingSet(_ selector: [Any], _ value: Any, acquisition: String = "conflict") -> [String: Any] {
    ["action": "set", "selector": selector, "value": value, "acquisition": acquisition]
}

private func editingRetire(_ selector: [Any]) -> [String: Any] {
    ["action": "retire", "selector": selector]
}

private func editingMember(_ name: String) -> [String: Any] {
    ["identity": name, "key": ["name"]]
}

// swiftlint:disable:next function_body_length
private func editingOwnershipCases() -> [[String: Any]] {
    let hooksA: [Any] = ["hooks", editingMember("a")]
    let hooksB: [Any] = ["hooks", editingMember("b")]
    let hooksC: [Any] = ["hooks", editingMember("c")]
    var moveA = editingSet(hooksA, ["name": "a", "run": 2], acquisition: "takeover")
    moveA["placement"] = ["position": "last"]
    var moveC = editingSet(hooksC, ["name": "c", "run": 3], acquisition: "takeover")
    moveC["placement"] = ["position": "first"]
    let sections: [String: String] = ["start": "<begin>", "end": "<end>"]
    return [
        [
            "name": "object-retirement-preserves-order-provenance",
            "format": "jsonc",
            "before": "\u{FEFF}{\r\n \"z\":0, /* left */ \"a\": 1e0,\r\n \"b\": \"ü😀\", \"tail\":false,\r\n}\r\n",
            "steps": [
                ["requests": [
                    editingSet(["a"], 2, acquisition: "takeover"),
                    editingSet(["b"], "changed", acquisition: "takeover"),
                    editingSet(["created", "nested", "leaf"], true),
                ]],
                ["requests": [
                    editingSet(["a"], 3),
                    editingSet(["b"], "again"),
                ]],
                ["requests": [editingRetire(["b"]), editingRetire(["a"])]],
                ["requests": [editingRetire(["created", "nested", "leaf"])]],
            ],
        ],
        [
            "name": "batched-array-restores-anchors-prefix-and-baselines",
            "format": "jsonc",
            "before": "{\"hooks\":[\n /* a */ {\"name\":\"a\",\"run\":1},\n {\"name\":\"b\"},\n /* c */ {\"name\":\"c\"},\n]}",
            "steps": [
                ["requests": [moveA, moveC, editingSet(hooksB, ["name": "b"], acquisition: "adopt")]],
                ["requests": [editingRetire(hooksC), editingRetire(hooksA)]],
                ["requests": [editingRetire(hooksB)]],
            ],
        ],
        [
            "name": "partial-container-retirement",
            "format": "json",
            "before": NSNull(),
            "steps": [
                ["requests": [
                    editingSet(["outer", "inner", "x"], 1),
                    editingSet(["outer", "keep"], true),
                ]],
                ["requests": [editingRetire(["outer", "inner", "x"])]],
                ["requests": [editingRetire(["outer", "keep"])]],
            ],
        ],
        [
            "name": "conflict-rolls-back-created-container-provenance",
            "format": "jsonc",
            "before": "{}",
            "steps": [
                ["requests": [
                    editingSet(["generated", "leaf"], 1),
                    [
                        "action": "set", "selector": ["date"],
                        "semantic_value": ["date", "2026-01-02"],
                    ],
                ]],
            ],
        ],
        [
            "name": "placement-conflict-rolls-back-takeover",
            "format": "jsonc",
            "before": "{\"hooks\":[{\"name\":\"a\"}]}",
            "steps": [
                ["requests": [
                    [
                        "action": "set", "selector": hooksA, "value": ["name": "a", "run": 2],
                        "acquisition": "takeover",
                        "placement": ["position": "before", "anchor": editingMember("missing")],
                    ],
                    editingSet(["other", "leaf"], 1),
                ]],
            ],
        ],
        [
            "name": "section-created-separator-crlf",
            "format": "text",
            "before": "\u{FEFF}foreign\r\n",
            "steps": [
                ["action": "set", "selector": sections, "text": "one\ntwo", "marker": "custom"],
                ["action": "set", "selector": sections, "text": "new", "marker": "custom"],
                ["action": "retire", "selector": sections],
            ],
        ],
        [
            "name": "section-takeover-and-restoration",
            "format": "text",
            "before": "<begin>\r\noriginal\r\n<end>\r\nforeign",
            "steps": [
                [
                    "action": "set", "selector": sections, "text": "replacement",
                    "marker": "custom", "acquisition": "takeover",
                ],
                ["action": "retire", "selector": sections],
            ],
        ],
        [
            "name": "json-adoption-follows-semantic-ownership",
            "format": "json",
            "before": "{\"x\":1}",
            "steps": [
                editingSet(["x"], 1, acquisition: "adopt"),
                [
                    "observed": "{ \"x\" : 1 }",
                    "requests": [editingSet(["x"], 2)],
                ],
                ["requests": [editingRetire(["x"])]],
            ],
        ],
        [
            "name": "jsonc-adoption-detects-syntax-modification",
            "format": "jsonc",
            "before": "{\"x\":1.0}",
            "steps": [
                [
                    "action": "set",
                    "selector": ["x"],
                    "semantic_value": ["float", "0x1.0000000000000p+0"],
                    "acquisition": "adopt",
                ],
                ["observed": "{\"x\":1e0}", "requests": [editingSet(["x"], 2)]],
            ],
        ],
        [
            "name": "empty-request-preserves-absence",
            "format": "json",
            "before": NSNull(),
            "steps": [["requests": []]],
        ],
    ]
}

private func editingDiagnosticCases() -> [[String: Any]] {
    let documents = [
        "", "// no root", "1 2", "[]]", "{\"a\":1} trailing",
        "[", "[01]", "[00]", "[1.]", "[1e]", "[0x10]", "[1 2]", "[truex]",
        "[Infinityx]", "[NaNx]", "[-NaN]", "[-Infinit]", "[-.5]", "[,]", "[tru]", "[+1]", "[.5]",
        "{\"a\":}", "{\"a\":1,,}", "{\"a\"}", "{\"a\" 1}", "{1:2}", "{01:2}",
        "{[1]:2}", "{[]:2}", "{[1,2]:2}", "{[[1]]:2}", "{{}:1}", "{[1]:}",
        "{\"a\":1, [2]:3}", "{[1]\"a\":2}", "{[1],:2}", "{[1:2}", "{[}:1}",
        "[\"a\" \"b\"]", "[\"a\\qb\"]", "[\"a\\u12\"]", "[\"a\\u00ZZ\"]",
        "[\"a\\u+123\"]", "[\"abc", "[\"a\\", "[\"a\nb\"]",
        "{\"a\":NaN}", "{\"a\":1e400}", "{\"a\":-Infinity}",
        "{\"a\":9223372036854775808}", "{\"a\":-9223372036854775809}",
        "[\"\\ud800\"]", "[\"\\udc00\"]", "[\"\\ud800abc\"]",
        "[\"\\ud800\\ud800\"]", "{\"\\ud800\":1}", "{/* c */}",
        "{\"a\":1,}", "{\"a\":1,\"a\":2}", "{\"a\":", "{\"a\":1", "{\"a\":1,",
        "[1,", "[1", "{", "{true:1}", "[1,]",
        "{\n  \"a\": tru\n}", "{\n\"ü😀e\u{301}\": tru}", "[\"ü😀\\q\"]",
        "{/*oops", "{\"a\" /* unterminated", "[1, /* nope",
    ]
    let malformed = ["80", "ff", "c0af", "c280e282", "e2", "e282", "e28241",
                     "f09080", "f0908041", "eda080", "e08080", "f4908080", "efbbbf41e282"]
    return ["json", "jsonc"].flatMap { format in
        documents.enumerated().map { index, document in
            [
                "name": "\(format)-diagnostic-\(index)", "format": format, "before": document,
                "steps": [editingSet(["new"], 1)],
            ] as [String: Any]
        }
    } + malformed.enumerated().map { index, hex in
        [
            "name": "utf8-diagnostic-\(index)", "format": "json", "before_hex": hex,
            "steps": [editingSet(["new"], 1)],
        ] as [String: Any]
    }
}
