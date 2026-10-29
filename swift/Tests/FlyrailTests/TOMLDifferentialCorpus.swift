import Foundation

func tomlDifferentialCases() -> [[String: Any]] {
    let sources = [
        "foreign=1", "[foreign]\nx=1\n", "first=1\n[foreign]\nx=1\n",
        "a.b=1\nforeign=2\n", "# comment only\n", "# no final newline",
        "foreign=inf\nother=nan\nx=1\n", "x=1\nforeign=2\n",
        "x = { nested = 1 } # foreign\nother=3\n", "[x]\nnested=1\n[foreign]\ny=2\n",
        "x.command=\"old\"\nx.env.X=\"before\"\nforeign=1\n",
        "x.command=\"old\"\n# between\n\nx.env.X=\"before\"\nforeign=1\n",
        "[x.env] # child\nX=\"old\"\n[x] # parent\ncommand=\"old\"\n# gap\n[foreign]\nv=1\n",
        "[x.env] # child\nX=\"old\"\n[x] # parent\ncommand=\"old\"\n[outer]\nv=1\n",
        "[x] # header\ncommand=\"old\"\n[x . env] # nested\nX=\"before\"\n# gap\n",
    ]
    let value: [String: Any] = ["command": "new", "env": ["X": "new"], "checks": [["name": "check"]]]
    var cases = sources.enumerated().map { index, source in
        [
            "name": "toml-layout-\(index)", "format": "toml", "before": source,
            "steps": [
                tomlSet(["x"], value, acquisition: "takeover"),
                tomlSet(["x"], ["env": ["Y": "updated"]]),
                tomlRetire(["x"]),
            ],
        ] as [String: Any]
    }
    let hooks =
        "[[hooks]] # a\nname=\"a\"\n[[hooks]] # x\nname=\"x\"\n# gap x\n[[hooks]] # y\nname=\"y\"\n# gap y\n[[hooks]] # b\nname=\"b\"\n"
    for source in [hooks, "hooks=[ {name=\"a\"}, # x\n {name=\"x\"}, # y\n {name=\"y\"}, # b\n {name=\"b\"} ]\n"] {
        for reverse in [false, true] {
            let x: [Any] = ["hooks", ["identity": "x", "key": ["name"]]]
            let y: [Any] = ["hooks", ["identity": "y", "key": ["name"]]]
            var first = tomlSet(x, ["name": "x", "env": ["X": "y"]], acquisition: "takeover")
            first["placement"] = ["position": "last"]
            var second = tomlSet(y, ["name": "y", "env": ["X": "y"]], acquisition: "takeover")
            second["placement"] = ["position": "first"]
            cases.append([
                "name": "toml-joint-\(cases.count)", "format": "toml", "before": source,
                "steps": [
                    ["requests": [first, second]],
                    ["requests": reverse ? [tomlRetire(y), tomlRetire(x)] : [tomlRetire(x), tomlRetire(y)]],
                ],
            ])
        }
    }
    for whole in [false, true] {
        let selector: [Any] = whole ? ["hooks"] : ["hooks", ["identity": "a", "key": ["name"]]]
        let payload: [String: Any] = ["name": "a", "env#[]": ["X": "new"], "checks": [["name": "new"]]]
        cases.append([
            "name": "toml-nested-quoted-aot-\(whole)", "format": "toml",
            "before": "[[hooks]]\nname=\"a\"\n[hooks.\"env#[]\"]\nX=\"old\"\n[[hooks.checks]]\nname=\"check\"\n",
            "steps": [
                tomlSet(selector, whole ? [payload] : payload, acquisition: "takeover"),
                tomlRetire(selector),
            ],
        ])
    }
    for source in [NSNull(), "# foreign\n[mcp]\n", "mcp.foo.command=\"old\"\nforeign=1\n# end\n"] as [Any] {
        for selector in [["mcp", "test"], ["mcp", "foo", "env"], ["mcp", "other"]] {
            cases.append([
                "name": "toml-generated-\(cases.count)", "format": "toml", "before": source,
                "steps": [tomlSet(selector, value, acquisition: "takeover"), tomlRetire(selector)],
            ])
        }
    }
    let malformed = [
        "x=1\nx=2\n", "x=[1 2]", "x=[1,,2]", "x=01", "x=1.", "x=1e",
        "x=\"abc", "x=\"a\\q\"", "x", "[a", "x=", "x=1\r", "x=truex",
        "x=2026-02-30", "x=inf\n", "x=nan\n",
        "x.command=1\nforeign=1\nx.env.X=2\n",
    ]
    cases += malformed.enumerated().map { index, source in
        [
            "name": "toml-diagnostic-\(index)", "format": "toml", "before": source,
            "steps": [tomlSet(["x"], 2, acquisition: "takeover")],
        ]
    }
    return cases + tomlReviewRegressionCases()
}

private func tomlSet(_ selector: [Any], _ value: Any, acquisition: String = "conflict") -> [String: Any] {
    ["action": "set", "selector": selector, "value": value, "acquisition": acquisition]
}

private func tomlRetire(_ selector: [Any]) -> [String: Any] {
    ["action": "retire", "selector": selector]
}

func schemaDifferentialCases() -> [[String: Any]] {
    let schema: [String: Any] = ["key": "version", "value": 1]
    let request: [String: Any] = ["action": "set", "selector": ["x"], "value": 1]
    var cases: [[String: Any]] = []
    for format in ["json", "jsonc"] {
        for source in [
            NSNull(),
            " \r\n",
            "{}",
            "{\"version\":1}",
            "{\"version\":1.0}",
            "{\"version\":2}",
            "{/* foreign */}",
        ] as [Any] {
            cases.append([
                "name": "schema-scaffold-\(cases.count)", "format": format, "before": source,
                "schema": true,
                "steps": [
                    ["requests": [request], "schemas": [schema], "validate": true],
                    ["requests": [["action": "retire", "selector": ["x"]]], "schemas": [], "validate": true],
                ],
            ])
        }
    }
    for requirements in [[schema, schema], [schema, ["key": "version", "value": 2]]] {
        cases.append([
            "name": "schema-duplicates-\(cases.count)", "format": "json", "before": NSNull(), "schema": true,
            "steps": [["requests": [], "schemas": requirements, "validate": true]],
        ])
    }
    cases.append([
        "name": "schema-app-owned-overlap", "format": "json", "before": NSNull(), "schema": true,
        "steps": [["requests": [["action": "set", "selector": ["version"], "value": 1]],
                   "schemas": [schema], "validate": true]],
    ])
    cases.append([
        "name": "schema-blank-only", "format": "json", "before": " \r\n", "schema": true,
        "steps": [
            ["requests": [], "schemas": [schema], "validate": true],
            ["requests": [], "schemas": [], "validate": true],
        ],
    ])
    return cases
}
