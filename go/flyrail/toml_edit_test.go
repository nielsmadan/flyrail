package flyrail

import (
	"strings"
	"testing"
	"unicode/utf8"
)

func TestTOMLEditNativeTimePrecisionPreservesBytes(t *testing.T) {
	cases := []struct{ token, canonical string }{
		{"00:00:00.123456789", `["time","00:00:00.123456"]`},
		{"2000-01-01T00:00:00.123456789", `["datetime","2000-01-01T00:00:00.123456"]`},
		{"2000-01-01T00:00:00.123456789Z", `["datetime","2000-01-01T00:00:00.123456+00:00"]`},
		{"2000-01-01 01:00:00.123456789+01:00", `["datetime","2000-01-01T00:00:00.123456+00:00"]`},
	}
	for _, c := range cases {
		source := "x=" + c.token + "\n"
		document := mustTOML(t, source)
		value := must(document.native(keyPath("x")))
		if got := string(must(EncodeValue(value))); got != c.canonical {
			t.Errorf("%s: native %s, want %s", c.token, got, c.canonical)
		}
		if err := document.put(keyPath("y"), Int(1), nil, nil); err != nil {
			t.Fatal(err)
		}
		if got := document.text(); got != source+"y = 1\n" {
			t.Errorf("%s: put text %q", c.token, got)
		}
		if err := document.delete(keyPath("y")); err != nil {
			t.Fatal(err)
		}
		if got := document.text(); got != source {
			t.Errorf("%s: delete text %q", c.token, got)
		}
	}
}

func TestTOMLEditInlineFieldInsertionRoundTripsClosingTrivia(t *testing.T) {
	for _, gap := range []string{"", " ", "   ", "\t \t"} {
		source := "x={a=1" + gap + "}\nforeign=2\n"
		document := mustTOML(t, source)
		if err := document.put(keyPath("x", "b"), Int(3), nil, nil); err != nil {
			t.Fatal(err)
		}
		if got, want := document.text(), "x={a=1,"+gap+"b = 3}\nforeign=2\n"; got != want {
			t.Errorf("gap %q: put %q, want %q", gap, got, want)
		}
		if err := document.delete(keyPath("x", "b")); err != nil {
			t.Fatal(err)
		}
		if got := document.text(); got != source {
			t.Errorf("gap %q: delete %q", gap, got)
		}
	}
}

func TestTOMLEditSelectedTableSyntaxStripsOnlyItsHeaderTrivia(t *testing.T) {
	document := mustTOML(t, "  [x] # header\ncommand = 1\n[x . env] # nested\nX=2\n")
	if got := string(must(document.syntax(keyPath("x")))); got != "[value]\ncommand = 1\n[value. env] # nested\nX=2\n" {
		t.Errorf("syntax %q", got)
	}
	dotted := mustTOML(t, "x.command = 1\nx.other = 2\n")
	if got := string(must(dotted.ownedSyntax(keyPath("x")))); got != "command = 1\nother = 2\n" {
		t.Errorf("owned syntax %q", got)
	}
}

func TestTOMLEditDetachedCommentsKeepOriginalIndentation(t *testing.T) {
	for _, source := range []string{"  x=1 # foreign\n", "  [x] # foreign\na=1\n"} {
		document := mustTOML(t, source)
		if got := document.text(); got != source {
			t.Errorf("parse %q", got)
		}
		if err := document.delete(keyPath("x")); err != nil {
			t.Fatal(err)
		}
		if got := document.text(); got != "   # foreign\n" {
			t.Errorf("%q: delete %q", source, got)
		}
	}
}

func TestTOMLEditForeignNonFiniteValuesStayEditableButUnownable(t *testing.T) {
	document := mustTOML(t, "x=1\nforeign=inf\n")
	if err := document.put(keyPath("x"), Int(2), nil, nil); err != nil {
		t.Fatal(err)
	}
	if got := document.text(); got != "x=2\nforeign=inf\n" {
		t.Errorf("put %q", got)
	}
	if _, err := document.syntax(keyPath("foreign")); errorText(err) != "floats must be finite" {
		t.Errorf("syntax error %v", err)
	}
}

func TestTOMLEditGeneratedValuesFollowTomlkit(t *testing.T) {
	inner := must(NewObject(ObjectMember{Key: "name", Value: must(NewString("check"))}))
	env := must(NewObject(ObjectMember{Key: "X", Value: must(NewString("é\x01\""))}))
	value := must(NewObject(
		ObjectMember{Key: "checks", Value: must(NewArray(inner))},
		ObjectMember{Key: "command", Value: must(NewString("new"))},
		ObjectMember{Key: "env", Value: env},
		ObjectMember{Key: "quoted key", Value: must(NewFloat(2.5e20))},
	))
	cases := []struct{ source, want string }{
		{"", "[x]\ncommand = \"new\"\n\"quoted key\" = 2.5e+20\n[[x.checks]]\nname = \"check\"\n\n[x.env]\nX = \"é\\u0001\\\"\"\n"},
		{"x = 1 # keep\n", "x = {command = \"new\", \"quoted key\" = 2.5e+20, checks = [{name = \"check\"}], env = {X = \"é\\u0001\\\"\"}} # keep\n"},
		{"a.b = 1\n", "a.b = 1\n[x]\ncommand = \"new\"\n\"quoted key\" = 2.5e+20\n[[x.checks]]\nname = \"check\"\n\n[x.env]\nX = \"é\\u0001\\\"\"\n"},
	}
	for _, c := range cases {
		document := mustTOML(t, c.source)
		if err := document.put(keyPath("x"), value, nil, nil); err != nil {
			t.Fatalf("%q: %v", c.source, err)
		}
		if got := document.text(); got != c.want {
			t.Errorf("%q: put %q, want %q", c.source, got, c.want)
		}
	}
}

func TestTOMLEditMembersOfArraysAndArraysOfTables(t *testing.T) {
	member := must(NewObject(ObjectMember{Key: "name", Value: must(NewString("z"))}))
	inline := mustTOML(t, "hooks = [ {name = \"a\"}, # x\n {name = \"b\"} ]\n")
	if err := inline.insert(keyPath("hooks"), 1, member, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if got, want := inline.text(), "hooks = [ {name = \"a\"},{name = \"z\"}, # x\n {name = \"b\"} ]\n"; got != want {
		t.Errorf("array insert %q, want %q", got, want)
	}
	if got := string(must(inline.positionPrefix(keyPath("hooks", 2)))); got != " # x\n " {
		t.Errorf("position prefix %q", got)
	}
	if err := inline.delete(keyPath("hooks", 1)); err != nil {
		t.Fatal(err)
	}
	if got, want := inline.text(), "hooks = [ {name = \"a\"}, # x\n {name = \"b\"} ]\n"; got != want {
		t.Errorf("array delete %q, want %q", got, want)
	}
	tables := mustTOML(t, "[[hooks]] # a\nname = \"a\"\n[[hooks]] # b\nname = \"b\"\n")
	if err := tables.relocate(keyPath("hooks", 1), 0, must(NewObject(ObjectMember{Key: "name", Value: must(NewString("b"))})), nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if got, want := tables.text(), "[[hooks]] # b\nname = \"b\"\n[[hooks]] # a\nname = \"a\"\n"; got != want {
		t.Errorf("relocate %q, want %q", got, want)
	}
	if err := tables.delete(keyPath("hooks", 0)); err != nil {
		t.Fatal(err)
	}
	if got, want := tables.text(), " # b\n[[hooks]] # a\nname = \"a\"\n"; got != want {
		t.Errorf("member delete %q, want %q", got, want)
	}
}

func TestTOMLEditOutOfOrderTablesMoveAsFragments(t *testing.T) {
	source := "[x.env] # child\nX=\"old\"\n[x] # parent\ncommand=\"old\"\n# gap\n[foreign]\nv=1\n"
	document := mustTOML(t, source)
	if err := document.contiguous(keyPath("x")); err != nil {
		t.Fatal(err)
	}
	if got, want := string(must(document.ownedSyntax(keyPath("x")))), "[x.env] # child\nX=\"old\"\n[value]\ncommand=\"old\"\n"; got != want {
		t.Errorf("owned syntax %q, want %q", got, want)
	}
	if _, err := document.emptySyntax(keyPath("x")); errorText(err) != "a noncontiguous TOML table cannot be pruned" {
		t.Errorf("empty syntax error %v", err)
	}
	split := mustTOML(t, "[x]\na=1\n[y]\nb=2\n[x.c]\nd=3\n")
	if err := split.contiguous(keyPath("x")); errorText(err) != tomlNoncontiguous {
		t.Errorf("contiguity error %v", err)
	}
}

func FuzzTOMLDocumentEdits(f *testing.F) {
	for _, seed := range []string{
		"", "x = 1\n", "[x]\na = 1\n", "x.a = 1\nforeign = 2\n", "x = {a = 1}\n", "[[hooks]]\nname = \"a\"\n",
		"[x.env]\nX = 1\n[x]\nc = 2\n", "hooks = [{name = \"a\"}, # c\n]\n", "# only\n", "x = 1", "[x]\r\na = 1\r\n",
	} {
		f.Add(seed, "x", int64(2))
		f.Add(seed, "y", int64(-1))
	}
	f.Fuzz(func(t *testing.T, source, name string, number int64) {
		if !utf8.ValidString(source) || !utf8.ValidString(name) || strings.ContainsRune(source, 0) {
			return
		}
		document, err := parseTOMLDocument(source)
		if err != nil {
			return
		}
		path := keyPath(name)
		if err := document.put(path, Int(number), nil, nil); err != nil {
			if _, ok := asValueError(err); !ok {
				t.Fatalf("put error %T", err)
			}
			return
		}
		if _, err := parseTOMLDocument(document.text()); err != nil {
			t.Fatalf("published text does not parse: %v", err)
		}
		value, err := document.native(path)
		if err != nil || !ValuesEqual(value, Int(number)) {
			t.Fatalf("published value %v, %v", value, err)
		}
		if _, err := document.syntax(path); err != nil {
			if _, ok := asValueError(err); !ok {
				t.Fatalf("syntax error %T", err)
			}
		}
		if err := document.delete(path); err != nil {
			if _, ok := asValueError(err); !ok {
				t.Fatalf("delete error %T", err)
			}
		}
	})
}

func TestTOMLEditInlineTableNestingMatchesTomlkitLimit(t *testing.T) {
	nested := func(depth int) string {
		return "x = " + strings.Repeat("{a = ", depth) + "1" + strings.Repeat("}", depth) + "\n"
	}
	content := structuredKeys(t, FormatTOML, Int(1), "z")
	for _, depth := range []int{52, 100} {
		source := nested(depth)
		edit, err := EditDocument([]byte(source), true, []DocumentRequest{mustSetRequest(t, content, AcquireConflict)}, nil, DocumentProvenance{})
		if err != nil {
			t.Fatalf("depth %d: %v", depth, err)
		}
		if got := string(edit.After); got != source+"z = 1\n" {
			t.Errorf("depth %d: after %q", depth, got)
		}
	}
	got := setStructuredError(t, []byte(nested(101)), true, content, AcquireConflict)
	if want := "TOML value nested more than 100 levels deep at line 1 col 504"; got != want {
		t.Errorf("depth 101: error %q, want %q", got, want)
	}
}
