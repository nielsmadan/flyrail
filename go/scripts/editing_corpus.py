"""Build the editing-record corpus: spec/fixtures/edits.json cases plus the cross-language ownership, diagnostic, TOML and schema cases."""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def editing_set(selector, value, acquisition="conflict"):
    return {"action": "set", "selector": selector, "value": value, "acquisition": acquisition}


def editing_retire(selector):
    return {"action": "retire", "selector": selector}


def editing_member(name):
    return {"identity": name, "key": ["name"]}


def ownership_cases():
    hooks_a = ["hooks", editing_member("a")]
    hooks_b = ["hooks", editing_member("b")]
    hooks_c = ["hooks", editing_member("c")]
    move_a = editing_set(hooks_a, {"name": "a", "run": 2}, "takeover") | {"placement": {"position": "last"}}
    move_c = editing_set(hooks_c, {"name": "c", "run": 3}, "takeover") | {"placement": {"position": "first"}}
    sections = {"start": "<begin>", "end": "<end>"}
    return [
        {
            "name": "object-retirement-preserves-order-provenance",
            "format": "jsonc",
            "before": "\ufeff{\r\n \"z\":0, /* left */ \"a\": 1e0,\r\n \"b\": \"ü😀\", \"tail\":false,\r\n}\r\n",
            "steps": [
                {"requests": [
                    editing_set(["a"], 2, "takeover"),
                    editing_set(["b"], "changed", "takeover"),
                    editing_set(["created", "nested", "leaf"], True),
                ]},
                {"requests": [editing_set(["a"], 3), editing_set(["b"], "again")]},
                {"requests": [editing_retire(["b"]), editing_retire(["a"])]},
                {"requests": [editing_retire(["created", "nested", "leaf"])]},
            ],
        },
        {
            "name": "batched-array-restores-anchors-prefix-and-baselines",
            "format": "jsonc",
            "before": "{\"hooks\":[\n /* a */ {\"name\":\"a\",\"run\":1},\n {\"name\":\"b\"},\n /* c */ {\"name\":\"c\"},\n]}",
            "steps": [
                {"requests": [move_a, move_c, editing_set(hooks_b, {"name": "b"}, "adopt")]},
                {"requests": [editing_retire(hooks_c), editing_retire(hooks_a)]},
                {"requests": [editing_retire(hooks_b)]},
            ],
        },
        {
            "name": "partial-container-retirement",
            "format": "json",
            "before": None,
            "steps": [
                {"requests": [editing_set(["outer", "inner", "x"], 1), editing_set(["outer", "keep"], True)]},
                {"requests": [editing_retire(["outer", "inner", "x"])]},
                {"requests": [editing_retire(["outer", "keep"])]},
            ],
        },
        {
            "name": "conflict-rolls-back-created-container-provenance",
            "format": "jsonc",
            "before": "{}",
            "steps": [
                {"requests": [
                    editing_set(["generated", "leaf"], 1),
                    {"action": "set", "selector": ["date"], "semantic_value": ["date", "2026-01-02"]},
                ]},
            ],
        },
        {
            "name": "placement-conflict-rolls-back-takeover",
            "format": "jsonc",
            "before": "{\"hooks\":[{\"name\":\"a\"}]}",
            "steps": [
                {"requests": [
                    {
                        "action": "set", "selector": hooks_a, "value": {"name": "a", "run": 2},
                        "acquisition": "takeover",
                        "placement": {"position": "before", "anchor": editing_member("missing")},
                    },
                    editing_set(["other", "leaf"], 1),
                ]},
            ],
        },
        {
            "name": "section-created-separator-crlf",
            "format": "text",
            "before": "\ufefforeign\r\n",
            "steps": [
                {"action": "set", "selector": sections, "text": "one\ntwo", "marker": "custom"},
                {"action": "set", "selector": sections, "text": "new", "marker": "custom"},
                {"action": "retire", "selector": sections},
            ],
        },
        {
            "name": "section-takeover-and-restoration",
            "format": "text",
            "before": "<begin>\r\noriginal\r\n<end>\r\nforeign",
            "steps": [
                {
                    "action": "set", "selector": sections, "text": "replacement",
                    "marker": "custom", "acquisition": "takeover",
                },
                {"action": "retire", "selector": sections},
            ],
        },
        {
            "name": "json-adoption-follows-semantic-ownership",
            "format": "json",
            "before": "{\"x\":1}",
            "steps": [
                editing_set(["x"], 1, "adopt"),
                {"observed": "{ \"x\" : 1 }", "requests": [editing_set(["x"], 2)]},
                {"requests": [editing_retire(["x"])]},
            ],
        },
        {
            "name": "jsonc-adoption-detects-syntax-modification",
            "format": "jsonc",
            "before": "{\"x\":1.0}",
            "steps": [
                {
                    "action": "set", "selector": ["x"],
                    "semantic_value": ["float", "0x1.0000000000000p+0"], "acquisition": "adopt",
                },
                {"observed": "{\"x\":1e0}", "requests": [editing_set(["x"], 2)]},
            ],
        },
        {"name": "empty-request-preserves-absence", "format": "json", "before": None, "steps": [{"requests": []}]},
    ]


DIAGNOSTIC_DOCUMENTS = [
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
    "{\n  \"a\": tru\n}", "{\n\"ü😀e\u0301\": tru}", "[\"ü😀\\q\"]",
    "{/*oops", "{\"a\" /* unterminated", "[1, /* nope",
]
MALFORMED_UTF8 = ["80", "ff", "c0af", "c280e282", "e2", "e282", "e28241",
                  "f09080", "f0908041", "eda080", "e08080", "f4908080", "efbbbf41e282"]


def diagnostic_cases():
    cases = [
        {"name": f"{format}-diagnostic-{index}", "format": format, "before": document,
         "steps": [editing_set(["new"], 1)]}
        for format in ("json", "jsonc")
        for index, document in enumerate(DIAGNOSTIC_DOCUMENTS)
    ]
    return cases + [
        {"name": f"utf8-diagnostic-{index}", "format": "json", "before_hex": raw, "steps": [editing_set(["new"], 1)]}
        for index, raw in enumerate(MALFORMED_UTF8)
    ]


TOML_LAYOUT_SOURCES = [
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
TOML_MALFORMED = [
    "x=1\nx=2\n", "x=[1 2]", "x=[1,,2]", "x=01", "x=1.", "x=1e",
    "x=\"abc", "x=\"a\\q\"", "x", "[a", "x=", "x=1\r", "x=truex",
    "x=2026-02-30", "x=inf\n", "x=nan\n",
    "x.command=1\nforeign=1\nx.env.X=2\n",
]


def toml_cases():
    value = {"command": "new", "env": {"X": "new"}, "checks": [{"name": "check"}]}
    cases = [
        {
            "name": f"toml-layout-{index}", "format": "toml", "before": source,
            "steps": [
                editing_set(["x"], value, "takeover"),
                editing_set(["x"], {"env": {"Y": "updated"}}),
                editing_retire(["x"]),
            ],
        }
        for index, source in enumerate(TOML_LAYOUT_SOURCES)
    ]
    hooks = (
        "[[hooks]] # a\nname=\"a\"\n[[hooks]] # x\nname=\"x\"\n# gap x\n[[hooks]] # y\nname=\"y\"\n"
        "# gap y\n[[hooks]] # b\nname=\"b\"\n"
    )
    for source in (hooks, "hooks=[ {name=\"a\"}, # x\n {name=\"x\"}, # y\n {name=\"y\"}, # b\n {name=\"b\"} ]\n"):
        for reverse in (False, True):
            x = ["hooks", {"identity": "x", "key": ["name"]}]
            y = ["hooks", {"identity": "y", "key": ["name"]}]
            first = editing_set(x, {"name": "x", "env": {"X": "y"}}, "takeover") | {"placement": {"position": "last"}}
            second = editing_set(y, {"name": "y", "env": {"X": "y"}}, "takeover") | {"placement": {"position": "first"}}
            retire = [editing_retire(y), editing_retire(x)] if reverse else [editing_retire(x), editing_retire(y)]
            cases.append({
                "name": f"toml-joint-{len(cases)}", "format": "toml", "before": source,
                "steps": [{"requests": [first, second]}, {"requests": retire}],
            })
    for whole in (False, True):
        selector = ["hooks"] if whole else ["hooks", {"identity": "a", "key": ["name"]}]
        payload = {"name": "a", "env#[]": {"X": "new"}, "checks": [{"name": "new"}]}
        cases.append({
            "name": f"toml-nested-quoted-aot-{'true' if whole else 'false'}", "format": "toml",
            "before": "[[hooks]]\nname=\"a\"\n[hooks.\"env#[]\"]\nX=\"old\"\n[[hooks.checks]]\nname=\"check\"\n",
            "steps": [editing_set(selector, [payload] if whole else payload, "takeover"), editing_retire(selector)],
        })
    for source in (None, "# foreign\n[mcp]\n", "mcp.foo.command=\"old\"\nforeign=1\n# end\n"):
        for selector in (["mcp", "test"], ["mcp", "foo", "env"], ["mcp", "other"]):
            cases.append({
                "name": f"toml-generated-{len(cases)}", "format": "toml", "before": source,
                "steps": [editing_set(selector, value, "takeover"), editing_retire(selector)],
            })
    cases += [
        {"name": f"toml-diagnostic-{index}", "format": "toml", "before": source,
         "steps": [editing_set(["x"], 2, "takeover")]}
        for index, source in enumerate(TOML_MALFORMED)
    ]
    return cases + toml_review_cases()


def toml_row(name, source, selector, value, acquisition):
    return {
        "name": name, "format": "toml", "before": source,
        "steps": [editing_set(selector, value, acquisition), editing_retire(selector)],
    }


def toml_review_cases():
    cases = [
        toml_row("toml-review-dotted-retire", "x.a=1\nforeign=2\n", ["x", "a"], 1, "adopt"),
        toml_row("toml-review-deep-dotted-retire", "x.a.b=1\nforeign=2\n", ["x", "a", "b"], 1, "adopt"),
        toml_row("toml-review-dotted-restore-gap", "x.a=1\n# owned between\n\nx.b=2\nforeign=3\n",
                 ["x"], {"a": 5, "b": 6}, "takeover"),
        toml_row("toml-review-mixed-prefix", "x.a=1\n\"x\".b=2\nforeign=3\n", ["x"], {"a": 5, "b": 6}, "takeover"),
        toml_row("toml-review-detach-indented-scalar", "  x=1 # foreign\n", ["x"], 1, "adopt"),
        toml_row("toml-review-detach-indented-table", "  [x] # foreign\na=1\n", ["x"], {"a": 1}, "adopt"),
        toml_row("toml-review-table-relative", "[mcp]\nfoo.command=1\nfoo.other=2\n",
                 ["mcp", "foo"], {"command": 3}, "takeover"),
        toml_row("toml-review-aot-relative", "[[hooks]]\nname=\"a\"\nfoo.command=1\nfoo.other=2\n",
                 ["hooks", {"identity": "a", "key": ["name"]}, "foo"], {"command": 3}, "takeover"),
    ]
    cases += [
        toml_row(f"toml-review-inline-gap-{index}", f"x={{a=1{gap}}}\nforeign=2\n", ["x", "b"], 3, "conflict")
        for index, gap in enumerate(("", " ", "   ", "\t \t"))
    ]
    malformed = ["x={}\nx.y=2\n", "x=1_e2\n", "x=1e_2\n", "x=\"\\u+123\"\n", "x=2000-01-01T12:34:56+01\n"]
    cases += [
        toml_row(f"toml-review-invalid-foreign-{index}", source, ["y"], 1, "conflict")
        for index, source in enumerate(malformed)
    ]
    temporals = [
        ("00:00:00.123456789", "time", "00:00:00.123456"),
        ("2000-01-01T00:00:00.123456789", "datetime", "2000-01-01T00:00:00.123456"),
        ("2000-01-01T00:00:00.123456789Z", "datetime", "2000-01-01T00:00:00.123456+00:00"),
        ("2000-01-01 01:00:00.123456789+01:00", "datetime", "2000-01-01T00:00:00.123456+00:00"),
    ]
    for index, (token, kind, semantic) in enumerate(temporals):
        cases.append(toml_row(f"toml-review-native-precision-{index}", f"x={token}\n", ["y"], 1, "conflict"))
        cases.append({
            "name": f"toml-review-selected-precision-{index}", "format": "toml", "before": f"x={token}\n",
            "steps": [
                {"action": "set", "selector": ["x"], "semantic_value": [kind, semantic], "acquisition": "adopt"},
                editing_retire(["x"]),
            ],
        })
    prefix = "{" + ".".join(["a"] * 10) + "="
    compounded = "x=" + prefix * 4 + "1" + "}" * 4 + "\n"
    cases.append(toml_row("toml-review-compounded-depth", compounded, ["y"], 1, "conflict"))
    return cases


def schema_cases():
    schema = {"key": "version", "value": 1}
    request = {"action": "set", "selector": ["x"], "value": 1}
    cases = []
    for format in ("json", "jsonc"):
        for source in (None, " \r\n", "{}", "{\"version\":1}", "{\"version\":1.0}", "{\"version\":2}", "{/* foreign */}"):
            cases.append({
                "name": f"schema-scaffold-{len(cases)}", "format": format, "before": source, "schema": True,
                "steps": [
                    {"requests": [request], "schemas": [schema], "validate": True},
                    {"requests": [{"action": "retire", "selector": ["x"]}], "schemas": [], "validate": True},
                ],
            })
    for requirements in ([schema, schema], [schema, {"key": "version", "value": 2}]):
        cases.append({
            "name": f"schema-duplicates-{len(cases)}", "format": "json", "before": None, "schema": True,
            "steps": [{"requests": [], "schemas": requirements, "validate": True}],
        })
    cases.append({
        "name": "schema-app-owned-overlap", "format": "json", "before": None, "schema": True,
        "steps": [{"requests": [{"action": "set", "selector": ["version"], "value": 1}],
                   "schemas": [schema], "validate": True}],
    })
    cases.append({
        "name": "schema-blank-only", "format": "json", "before": " \r\n", "schema": True,
        "steps": [
            {"requests": [], "schemas": [schema], "validate": True},
            {"requests": [], "schemas": [], "validate": True},
        ],
    })
    return cases


def editing_corpus():
    fixtures = json.loads((ROOT / "spec/fixtures/edits.json").read_text(encoding="utf-8"))
    return fixtures["cases"] + ownership_cases() + diagnostic_cases() + toml_cases() + schema_cases()


if __name__ == "__main__":
    sys.stdout.write(json.dumps(editing_corpus(), ensure_ascii=False, sort_keys=True))
