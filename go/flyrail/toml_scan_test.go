package flyrail

import (
	"fmt"
	"math"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

func tomlFixtureDocuments(t testing.TB, data []byte) ([]string, map[string]string) {
	t.Helper()
	fixtures, err := jsonx.Parse(data)
	if err != nil {
		t.Fatal(err)
	}
	var documents []string
	cases, _ := fixtures.Get("cases")
	items, _ := cases.Items()
	for _, item := range items {
		if format, _ := editRecordText(item, "format"); format != "toml" {
			continue
		}
		if before, ok := editRecordText(item, "before"); ok {
			documents = append(documents, before)
		}
		steps, _ := item.Get("steps")
		records, _ := steps.Items()
		for _, step := range records {
			for _, field := range []string{"expected", "observed"} {
				if text, ok := editRecordText(step, field); ok {
					documents = append(documents, text)
				}
			}
		}
	}
	invalid := map[string]string{}
	rejected, _ := fixtures.Get("invalid")
	records, _ := rejected.Items()
	for _, item := range records {
		if format, _ := editRecordText(item, "format"); format == "toml" {
			name, _ := editRecordText(item, "name")
			invalid[name], _ = editRecordText(item, "document")
		}
	}
	return documents, invalid
}

func TestTOMLFixtureDocumentsRoundTrip(t *testing.T) {
	documents, invalid := tomlFixtureDocuments(t, fixture(t, "edits.json"))
	if len(documents) < 30 {
		t.Fatalf("edits.json has only %d TOML documents", len(documents))
	}
	for _, source := range documents {
		document, err := parseTOMLDocument(source)
		if err != nil {
			t.Errorf("parse %q: %v", source, err)
			continue
		}
		if document.text() != source {
			t.Errorf("round trip of %q gave %q", source, document.text())
		}
	}
	_, err := parseTOMLDocument(invalid["duplicate-toml-key"])
	if got := errorText(err); got != `Key "x" already exists. at line 2 col 0` {
		t.Errorf("duplicate-toml-key: %q", got)
	}
}

func TestTOMLMalformedDocumentsReportReferenceDiagnostics(t *testing.T) {
	cases := []struct{ source, diagnostic string }{
		{"x={}\nx.y=2\n", `Key "x" already exists. at line 2 col 0`},
		{"x=1\n[x]\n", `Key "x" already exists. at line 2 col 0`},
		{"[a]\n[a]\n", `Key "a" already exists. at line 2 col 0`},
		{"\"a\\\"é\" = 1\na\"é = 2\n", `Unexpected character: '"' at line 2 col 1`},
		{"\"a\\\"é\" = 1\n'a\"é' = 2\n", `Key "a"é" already exists. at line 2 col 0`},
		{"x=1_e2\n", "Invalid number at line 1 col 6"},
		{"x=1e_2\n", "Invalid number at line 1 col 6"},
		{"x=\"\\u+123\"\n", "Invalid unicode value at line 1 col 4"},
		{"x=\"\\uD800\"\n", "Invalid unicode value at line 1 col 4"},
		{"x=2000-01-01T12:34:56+01\n", "Invalid number at line 1 col 24"},
		{"x=2000-01-01T25:34:56\n", "Invalid datetime at line 1 col 21"},
		{"x=25:00:00\n", "Invalid time at line 1 col 10"},
		{"x=[1 2]", "Unexpected character: '2' at line 1 col 5"},
		{"x=[1,,2]", "Unexpected character: ',' at line 1 col 5"},
		{"x=01", "Invalid number at line 1 col 4"},
		{"x=1.", "Invalid number at line 1 col 4"},
		{"x=1e", "Invalid number at line 1 col 4"},
		{"x=\"abc", "Unexpected end of file at line 1 col 6"},
		{"x=\"a\\q\"", "Invalid character 'q' in string at line 1 col 5"},
		{"x", `Unexpected character: '\x00' at line 1 col 1`},
		{"[a", "Unexpected end of file at line 1 col 2"},
		{"x=", `Unexpected character: '\x00' at line 1 col 2`},
		{"x=truex", "Unexpected character: 'x' at line 1 col 6"},
		{"x=2026-02-30", "Invalid date at line 1 col 12"},
		{"=1\n", "Empty key at line 1 col 0"},
		{"[ ]\n", "Empty table name at line 1 col 2"},
		{"x=\"é\" é\n", "Unexpected character: 'é' at line 1 col 6"},
		{"a b = 1\n", `Invalid key "a b" at line 1 col 4`},
		{"x = 1 = 2\n", "Unexpected character: '=' at line 1 col 6"},
		{"x = 1\r", `Control characters (codes less than 0x1f and 0x7f) are not allowed in comments, use \u000d instead at line 1 col 0`},
		{"x = 1\n\r", `Control characters (codes less than 0x1f and 0x7f) are not allowed in documents, use \u000d instead at line 2 col 0`},
		{"x = 1 # a\x1fb\n", `Control characters (codes less than 0x1f and 0x7f) are not allowed in comments, use \u001f instead at line 1 col 9`},
		{"x = \"a\x7f\"\n", `Control characters (codes less than 0x1f and 0x7f) are not allowed in strings, use \u007f instead at line 1 col 6`},
		{"x = \"\"\"a\x01\"\"\"\n", `Control characters (codes less than 0x1f and 0x7f) are not allowed in strings, use \u0001 instead at line 1 col 8`},
		{"x = '''a\rb'''\n", `Control characters (codes less than 0x1f and 0x7f) are not allowed in strings, use \u000d instead at line 2 col 0`},
		{"x = \"\"\"a\\ b\"\"\"\n", "Invalid character 'b' in string at line 1 col 10"},
		{"x = \"\"\"a\"\"\"\"\"\"\n", "Invalid character '\\n' in string at line 1 col 14"},
		{"x = {a=1 b=2}\n", "Unexpected character: 'b' at line 1 col 9"},
		{"x = {a=1,,b=2}\n", "Unexpected character: ',' at line 1 col 9"},
		{"x = {,}\n", "Unexpected character: ',' at line 1 col 5"},
		{"x = {a.b=1, a=2}\n", `Key "a" already exists.`},
		{"x = {a={b=1}, a.c=2}\n", `Key "a" already exists.`},
		{"x = .5\n", "Invalid number at line 1 col 6"},
		{"x = +.5\n", "Invalid number at line 1 col 7"},
		{"x = 1.e5\n", "Invalid number at line 1 col 8"},
		{"x = 0E5\n", "Invalid number at line 1 col 7"},
		{"x = +-5\n", "Invalid number at line 1 col 7"},
		{"x = 0x+1\n", "Invalid number at line 1 col 8"},
		{"x = -0x1\n", "Invalid number at line 1 col 8"},
		{"x = 9223372036854775808\n", "Invalid number at line 1 col 23"},
		{"x = +1:00:00\n", "Invalid time at line 1 col 12"},
		{"x = 07:32:00Z\n", "Invalid time at line 1 col 13"},
		{"x = 2020-01-01T07:32:60\n", "Invalid datetime at line 1 col 23"},
		{"[a b]\n", `Invalid key "a b" at line 1 col 4`},
		{"[[a]\n", `Unexpected character: '\n' at line 1 col 4`},
		{"[a]]\n", "Unexpected character: ']' at line 1 col 3"},
		{"a.b = 1\n[a]\n", "Redefinition of an existing table at line 2 col 0"},
		{"c.a = 1\n[c.a]\n", `Key "a" already exists. at line 2 col 0`},
		{"[a]\nb = 1\n[a.b]\n", `Key "b" already exists.`},
		{"[a]\n[a.b]\n[a.b]\n", `Key "b" already exists.`},
		{"[a]\n[a.b]\n[[a.b]]\n", `Key "b" already exists.`},
		{"x = \"é\n", `Control characters (codes less than 0x1f and 0x7f) are not allowed in strings, use \u000a instead at line 1 col 6`},
	}
	for _, c := range cases {
		_, err := parseTOMLDocument(c.source)
		if got := errorText(err); got != c.diagnostic {
			t.Errorf("parse %q: got %q, want %q", c.source, got, c.diagnostic)
		}
	}
}

func TestTOMLOutOfOrderPartsAreCheckedWhenReadBack(t *testing.T) {
	cases := []struct{ source, message string }{
		{"[[t.a.z]]\n[t.a]\n[t.a.z.q]\n", `Key "z" already exists.`},
		{"[t]\n[[t.a.x.p]]\n[t.a]\n[u]\n[t.a.z]\n", `Key "a" already exists.`},
		{"[t]\n[t.a.z]\n[t . a]\n\n[[u]]\n[t.a.y]\n", `Key "a" already exists.`},
	}
	for _, c := range cases {
		_, err := parseTOMLDocument(c.source)
		if got, _, _ := strings.Cut(errorText(err), " at line"); got != c.message {
			t.Errorf("parse %q: %v, want %q", c.source, err, c.message)
		}
	}
}

func deepTOMLKey(depth int) string {
	return strings.TrimSuffix(strings.Repeat("a.", depth), ".")
}

func TestTOMLNestingIsBoundedAtTheReferenceDepth(t *testing.T) {
	for _, depth := range []int{100, 101} {
		documents := []struct{ source, message string }{
			{fmt.Sprintf("[%s]\nx=1\n", deepTOMLKey(depth)), "TOML key nested more than 100 levels deep"},
			{fmt.Sprintf("%s=1\n", deepTOMLKey(depth)), "TOML key nested more than 100 levels deep"},
			{fmt.Sprintf("x=%s1%s\n", strings.Repeat("[", depth), strings.Repeat("]", depth)), "TOML value nested more than 100 levels deep"},
			{fmt.Sprintf("x=%s1%s\n", strings.Repeat("{a=", depth), strings.Repeat("}", depth)), "TOML value nested more than 100 levels deep"},
		}
		for _, document := range documents {
			parsed, err := parseTOMLDocument(document.source)
			if depth == 100 {
				if err != nil || parsed.text() != document.source {
					t.Errorf("depth %d: %v", depth, err)
				}
				continue
			}
			if got, _, _ := strings.Cut(errorText(err), " at line"); got != document.message {
				t.Errorf("depth %d of %.20q: got %q", depth, document.source, errorText(err))
			}
		}
	}
	compound := fmt.Sprintf("x=%s1%s\n", strings.Repeat(fmt.Sprintf("{%s=", deepTOMLKey(100)), 100), strings.Repeat("}", 100))
	if _, err := parseTOMLDocument(compound); !strings.Contains(errorText(err), "nested more than 100 levels deep") {
		t.Errorf("compound dotted inline nesting: %v", err)
	}
	headers := strings.Repeat("[a]\n", 1) + fmt.Sprintf("[%s]\n%s=1\n", deepTOMLKey(100), deepTOMLKey(100))
	if parsed, err := parseTOMLDocument(headers); err != nil || parsed.text() != headers {
		t.Errorf("deep dotted key under a deep header: %v", err)
	}
}

func mustTOML(t *testing.T, source string) *tomlDocument {
	t.Helper()
	document, err := parseTOMLDocument(source)
	if err != nil {
		t.Fatalf("parse %q: %v", source, err)
	}
	if document.text() != source {
		t.Fatalf("round trip of %q gave %q", source, document.text())
	}
	return document
}

func keyPath(names ...any) []pathPart {
	path := make([]pathPart, len(names))
	for index, name := range names {
		if position, ok := name.(int); ok {
			path[index] = segmentIndex(position)
		} else {
			path[index] = segmentKey(name.(string))
		}
	}
	return path
}

func TestTOMLNativeValuesFollowTheReferenceGrammar(t *testing.T) {
	date := must(NewDate(2000, 1, 1))
	at := func(hour, minute, second, microsecond int) Time {
		return must(NewTime(hour, minute, second, microsecond))
	}
	cases := []struct {
		raw  string
		want Value
	}{
		{"0x10", Int(16)},
		{"0o17", Int(15)},
		{"0b101", Int(5)},
		{"0xdead_BEEF", Int(0xDEADBEEF)},
		{"+1_000", Int(1000)},
		{"-9223372036854775808", Int(math.MinInt64)},
		{"-0", Int(0)},
		{"1e5", rawFloat(100000)},
		{"0e5", rawFloat(0)},
		{"-1.5E-3", rawFloat(-0.0015)},
		{"1_000.000_1", rawFloat(1000.0001)},
		{"1e400", rawFloat(math.Inf(1))},
		{"-inf", rawFloat(math.Inf(-1))},
		{"-nan", rawFloat(math.NaN())},
		{"true", Bool(true)},
		{`"é\u00e9\U0001F600\x41\e"`, String{text: "éé😀A\x1b"}},
		{`'C:\path'`, String{text: `C:\path`}},
		{"\"\"\"\nline \\\n   next\"\"\"\"\"", String{text: `line next""`}},
		{"'''a''''", String{text: "a'"}},
		{"\"\"\"a\\   \n  b\"\"\"", String{text: "ab"}},
		{"2000-01-01", date},
		{"00:00:00.123456789", at(0, 0, 0, 123456)},
		{"07:32", at(7, 32, 0, 0)},
		{"2000-01-01T00:00:00.123456789", NewLocalDateTime(date, at(0, 0, 0, 123456))},
		{"2000-01-01t00:00:00.5z", NewUTCDateTime(date, at(0, 0, 0, 500000))},
		{"2000-01-01 01:00:00.123456789+01:00", NewUTCDateTime(date, at(0, 0, 0, 123456))},
		{"2000-01-01 07:32", NewLocalDateTime(date, at(7, 32, 0, 0))},
		{"[1, # one\r\n 2,]", must(NewArray(Int(1), Int(2)))},
		{"{a.b = 1, a.c = 2, d = {}}", must(NewObject(
			ObjectMember{Key: "a", Value: must(NewObject(ObjectMember{Key: "b", Value: Int(1)}, ObjectMember{Key: "c", Value: Int(2)}))},
			ObjectMember{Key: "d", Value: must(NewObject())},
		))},
		{"{\n  a = 1, # first\n  b = 2,\n}", must(NewObject(ObjectMember{Key: "a", Value: Int(1)}, ObjectMember{Key: "b", Value: Int(2)}))},
	}
	for _, c := range cases {
		source := "x=" + c.raw + "\n"
		document := mustTOML(t, source)
		got, err := document.native(keyPath("x"))
		if err != nil || !ValuesEqual(got, c.want) {
			t.Errorf("native %s = %#v, %v; want %#v", c.raw, got, err, c.want)
		}
	}
}

func TestTOMLNativeResolvesTablesAndPaths(t *testing.T) {
	source := "# head\r\na.b = 1\r\n[t] # c\r\n  x = [1, 2] # foreign\r\n[t.sub]\r\ny = 'z'\r\n[[arr]]\r\nn = 1\r\n[[arr]]\r\nn = 2\r\n[arr.inner]\r\nm = true\r\n"
	document := mustTOML(t, source)
	if document.newline != "\r\n" {
		t.Errorf("newline %q", document.newline)
	}
	cases := []struct {
		path []pathPart
		want Value
	}{
		{keyPath("a", "b"), Int(1)},
		{keyPath("t", "x", 1), Int(2)},
		{keyPath("t", "sub", "y"), String{text: "z"}},
		{keyPath("arr", 1, "inner", "m"), Bool(true)},
		{keyPath("arr", 0), must(NewObject(ObjectMember{Key: "n", Value: Int(1)}))},
	}
	for _, c := range cases {
		got, err := document.native(c.path)
		if err != nil || !ValuesEqual(got, c.want) {
			t.Errorf("native %v = %#v, %v; want %#v", c.path, got, err, c.want)
		}
	}
	failures := []struct {
		path    []pathPart
		message string
	}{
		{keyPath("missing"), "document selector path is missing"},
		{keyPath("t", "x", 2), "document selector path is missing"},
		{keyPath("a", "b", "c"), "TOML selector crosses a scalar"},
		{keyPath("t", 0), "TOML selector crosses a scalar"},
		{keyPath(0), "TOML selector crosses a scalar"},
	}
	for _, c := range failures {
		if _, err := document.native(c.path); errorText(err) != c.message {
			t.Errorf("native %v: %v, want %q", c.path, err, c.message)
		}
	}
}

func TestTOMLLayoutRecordsMatchThePythonReference(t *testing.T) {
	nested := "[mcp.test] # header\ncommand=\"old\"\n[mcp . test . env] # nested\nX=\"before\"\n# gap\n"
	aot := "[[hooks]] # header\nname=\"a\"\n[hooks . env] # nested\nX=\"before\"\n# gap\n[[hooks]]\nname=\"b\"\n"
	reversed := "[mcp.foo.env]\nX=\"old\"\n[mcp.foo]\ncommand=\"old\"\n"
	cases := []struct {
		source string
		path   []pathPart
		want   string
	}{
		{nested, keyPath("mcp", "test"), `{"version":1,"headers":[[["env"],"mcp . test . env"]]}`},
		{nested, keyPath("mcp"), `{"version":1,"headers":[[["test"],"mcp.test"],[["test","env"],"mcp . test . env"]]}`},
		{aot, keyPath("hooks"), `{"version":1,"headers":[[[0],"hooks"],[[0,"env"],"hooks . env"],[[1],"hooks"]]}`},
		{aot, keyPath("hooks", 0), `{"version":1,"headers":[[["env"],"hooks . env"]]}`},
		{aot, keyPath("hooks", 1), ``},
		{reversed, keyPath("mcp", "foo"), `{"version":1,"headers":[[["env"],"mcp.foo.env"]]}`},
		{reversed, keyPath("mcp"), `{"version":1,"headers":[[["foo","env"],"mcp.foo.env"],[["foo"],"mcp.foo"]]}`},
		{"a.b.c = 1\n[a.d]\nx=1\n", keyPath("a"), `{"version":1,"headers":[[["d"],"a.d"]]}`},
		{"x = 1\n", keyPath("x"), ``},
	}
	for _, c := range cases {
		document := mustTOML(t, c.source)
		got, err := document.layout(c.path)
		if err != nil || string(got) != c.want {
			t.Errorf("layout %v of %q = %q, %v; want %q", c.path, c.source, got, err, c.want)
			continue
		}
		headers, err := layoutHeaders(got)
		if err != nil {
			t.Errorf("layout %q does not validate: %v", got, err)
		}
		for _, header := range headers {
			if _, err := headerPath(header.name); err != nil {
				t.Errorf("header %q: %v", header.name, err)
			}
		}
	}
	if _, err := mustTOML(t, nested).layout(keyPath("missing")); errorText(err) != "TOML selector path is missing" {
		t.Errorf("missing layout path: %v", err)
	}
}

func TestTOMLLayoutLocatesSplitTables(t *testing.T) {
	document := mustTOML(t, "[a.x]\nv=1\n[b]\nw=2\n[a.y]\nz=3\n")
	located, err := locateTOML(&document.root, keyPath("a"))
	if err != nil || located.kind != tomlLocatedTables || len(located.tables) != 2 {
		t.Fatalf("split table located as %+v, %v", located.kind, err)
	}
	got, err := document.layout(keyPath("a"))
	if want := `{"version":1,"headers":[[["x"],"a.x"],[["y"],"a.y"]]}`; err != nil || string(got) != want {
		t.Errorf("split layout %q, %v", got, err)
	}
	value, err := document.native(keyPath("a"))
	want := must(NewObject(
		ObjectMember{Key: "x", Value: must(NewObject(ObjectMember{Key: "v", Value: Int(1)}))},
		ObjectMember{Key: "y", Value: must(NewObject(ObjectMember{Key: "z", Value: Int(3)}))},
	))
	if err != nil || !ValuesEqual(value, want) {
		t.Errorf("split native %#v, %v", value, err)
	}
}

func tomlObject(pairs ...any) Value {
	members := make([]ObjectMember, 0, len(pairs)/2)
	for index := 0; index < len(pairs); index += 2 {
		members = append(members, ObjectMember{Key: pairs[index].(string), Value: pairs[index+1].(Value)})
	}
	return must(NewObject(members...))
}

func tomlArray(items ...Value) Value {
	return must(NewArray(items...))
}

func parseTOMLText(t *testing.T, source, printed string) *tomlDocument {
	t.Helper()
	document, err := parseTOMLDocument(source)
	if err != nil {
		t.Fatalf("parse %q: %v", source, err)
	}
	if document.text() != printed {
		t.Fatalf("text of %q = %q, want %q", source, document.text(), printed)
	}
	return document
}

func TestTOMLOutOfOrderArrayOfTablesFailsLikeTheReference(t *testing.T) {
	hooks := "[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[mcp_servers.fs]\ncommand = \"npx\"\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\n"
	env := "[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[mcp_servers.fs]\ncommand = \"npx\"\n[hooks.PreToolUse.env]\nX = \"1\"\n"
	for _, source := range []string{hooks, env} {
		document := parseTOMLText(t, source, source)
		for _, path := range [][]pathPart{nil, keyPath("hooks")} {
			if _, err := document.native(path); errorText(err) != `Key "PreToolUse" already exists.` {
				t.Errorf("native %v of %q: %v", path, source, err)
			}
		}
		if _, err := document.layout(keyPath("hooks")); errorText(err) != `Key "PreToolUse" already exists.` {
			t.Errorf("layout of hooks in %q: %v", source, err)
		}
		servers, err := document.native(keyPath("mcp_servers"))
		if want := tomlObject("fs", tomlObject("command", String{text: "npx"})); err != nil || !ValuesEqual(servers, want) {
			t.Errorf("native mcp_servers of %q = %#v, %v", source, servers, err)
		}
		layout, err := document.layout(keyPath("mcp_servers"))
		if want := `{"version":1,"headers":[[["fs"],"mcp_servers.fs"]]}`; err != nil || string(layout) != want {
			t.Errorf("layout mcp_servers of %q = %q, %v", source, layout, err)
		}
	}
	document := parseTOMLText(t, "b.'b' = 1\n[b . a . b]\n[['b'.'b'.b]]\n", "b.'b' = 1\n[b . a . b]\n[[b .'b'.b]]\n")
	for _, path := range [][]pathPart{nil, keyPath("b", "a")} {
		if _, err := document.native(path); errorText(err) != `Key "b" already exists.` {
			t.Errorf("native %v of a scalar extended by an array of tables: %v", path, err)
		}
	}
}

func TestTOMLOutOfOrderArrayOfTablesRegroupsLikeTheReference(t *testing.T) {
	member := tomlObject
	cases := []struct {
		source, printed string
		want            Value
		layout          string
	}{
		{
			"[[h.P]]\nm = 1\n[[x]]\n[[h.P.hooks]]\nt = 1\n",
			"[[h.P]]\nm = 1\n[[h.P.hooks]]\nt = 1\n[[x]]\n",
			tomlObject("P", tomlArray(member("m", Int(1), "hooks", tomlArray(member("t", Int(1)))))),
			`{"version":1,"headers":[[["P",0],"h.P"],[["P",0,"hooks",0],"h.P.hooks"]]}`,
		},
		{
			"[[h.P]]\nm = 1\n[[x]]\n[[h.P]]\nm = 3\n",
			"[[h.P]]\nm = 1\n[[h.P]]\nm = 3\n[[x]]\n",
			tomlObject("P", tomlArray(member("m", Int(1)), member("m", Int(3)))),
			`{"version":1,"headers":[[["P",0],"h.P"],[["P",1],"h.P"]]}`,
		},
		{
			"[[h.P]]\nm = 1\n[[x]]\n[[h.P.hooks]]\nt = 1\n[[h.P]]\nm = 2\n",
			"[[h.P]]\nm = 1\n[[h.P.hooks]]\nt = 1\n[[h.P]]\nm = 2\n[[x]]\n",
			tomlObject("P", tomlArray(member("m", Int(1), "hooks", tomlArray(member("t", Int(1)))), member("m", Int(2)))),
			`{"version":1,"headers":[[["P",0],"h.P"],[["P",0,"hooks",0],"h.P.hooks"],[["P",1],"h.P"]]}`,
		},
	}
	for _, c := range cases {
		document := parseTOMLText(t, c.source, c.printed)
		got, err := document.native(keyPath("h"))
		if err != nil || !ValuesEqual(got, c.want) {
			t.Errorf("native h of %q = %#v, %v", c.source, got, err)
		}
		root, err := document.native(nil)
		if want := tomlObject("h", c.want, "x", tomlArray(member())); err != nil || !ValuesEqual(root, want) {
			t.Errorf("native root of %q = %#v, %v", c.source, root, err)
		}
		layout, err := document.layout(keyPath("h"))
		if err != nil || string(layout) != c.layout {
			t.Errorf("layout h of %q = %q, %v", c.source, layout, err)
		}
	}
	skills := "[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[[skills]]\nname = \"s\"\n[[hooks.PreToolUse]]\nmatcher = \"Edit\"\n"
	document := parseTOMLText(t, skills, "[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[[hooks.PreToolUse]]\nmatcher = \"Edit\"\n[[skills]]\nname = \"s\"\n")
	layout, err := document.layout(keyPath("hooks"))
	if want := `{"version":1,"headers":[[["PreToolUse",0],"hooks.PreToolUse"],[["PreToolUse",1],"hooks.PreToolUse"]]}`; err != nil || string(layout) != want {
		t.Errorf("layout of hooks split by skills = %q, %v", layout, err)
	}
}

func TestTOMLSplitArrayOfTablesUnderATableKeepsTheReferenceLayout(t *testing.T) {
	source := "[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[mcp_servers.fs]\ncommand = \"npx\"\n[[hooks.PreToolUse]]\nmatcher = \"Edit\"\n"
	document := parseTOMLText(t, source, source)
	hooks, err := document.native(keyPath("hooks"))
	want := tomlObject("PreToolUse", tomlArray(tomlObject("matcher", String{text: "Bash"}), tomlObject("matcher", String{text: "Edit"})))
	if err != nil || !ValuesEqual(hooks, want) {
		t.Errorf("native hooks = %#v, %v", hooks, err)
	}
	layout, err := document.layout(keyPath("hooks"))
	if want := `{"version":1,"headers":[[["PreToolUse",0],"hooks.PreToolUse"],[["PreToolUse",0],"hooks.PreToolUse"]]}`; err != nil || string(layout) != want {
		t.Errorf("layout hooks = %q, %v", layout, err)
	}
}

func TestTOMLLocateResolvesDottedKeysInsideInlineTables(t *testing.T) {
	document := mustTOML(t, "x = {a.b = 1}\n")
	located, err := locateTOML(&document.root, keyPath("x", "a"))
	if err != nil || located.kind != tomlLocatedInline {
		t.Fatalf("x.a located as %v, %v", located.kind, err)
	}
	leaf, err := locateTOML(&document.root, keyPath("x", "a", "b"))
	if err != nil || leaf.kind != tomlLocatedItem || !ValuesEqual(leaf.item.value, Int(1)) {
		t.Fatalf("x.a.b located as %+v, %v", leaf, err)
	}
	for _, c := range []struct {
		path []pathPart
		want Value
	}{
		{keyPath("x", "a"), tomlObject("b", Int(1))},
		{keyPath("x", "a", "b"), Int(1)},
	} {
		got, err := document.native(c.path)
		if err != nil || !ValuesEqual(got, c.want) {
			t.Errorf("native %v = %#v, %v", c.path, got, err)
		}
	}
	servers := mustTOML(t, "[mcp_servers]\nfs = { command = \"npx\", env.TOKEN = \"t\" }\n")
	for _, c := range []struct {
		path []pathPart
		want Value
	}{
		{keyPath("mcp_servers", "fs", "env", "TOKEN"), String{text: "t"}},
		{keyPath("mcp_servers", "fs", "env"), tomlObject("TOKEN", String{text: "t"})},
		{keyPath("mcp_servers", "fs"), tomlObject("command", String{text: "npx"}, "env", tomlObject("TOKEN", String{text: "t"}))},
	} {
		got, err := servers.native(c.path)
		if err != nil || !ValuesEqual(got, c.want) {
			t.Errorf("native %v = %#v, %v", c.path, got, err)
		}
		layout, err := servers.layout(c.path)
		if err != nil || len(layout) != 0 {
			t.Errorf("layout %v = %q, %v", c.path, layout, err)
		}
	}
	if _, err := servers.layout(keyPath("mcp_servers", "fs", "env", "TOKEN", "x")); errorText(err) != "TOML selector crosses a scalar" {
		t.Errorf("selector through an inline string: %v", err)
	}
}

func TestTOMLDateWhitespaceFollowsTheReferencePrinter(t *testing.T) {
	date := must(NewDate(1979, 5, 27))
	cases := []struct {
		source, printed string
		want            Value
	}{
		{"x = [1979-05-27 ]\n", "x = [1979-05-27]\n", tomlArray(date)},
		{"x = {b = 1979-05-27 , c = 1}\n", "x = {b = 1979-05-27, c = 1}\n", tomlObject("b", date, "c", Int(1))},
		{"x = [1979-05-27 07:32 ]\n", "x = [1979-05-27 07:32]\n", tomlArray(NewLocalDateTime(date, must(NewTime(7, 32, 0, 0))))},
		{"x = [ 1979-05-27 # c\n]\n", "x = [ 1979-05-27# c\n]\n", tomlArray(date)},
		{"x = 1979-05-27 \t# c\n", "x = 1979-05-27 # c\n", date},
		{"x = {b.c = 1979-05-27 , d = 1}\n", "x = {b.c = 1979-05-27 , d = 1}\n", tomlObject("b", tomlObject("c", date), "d", Int(1))},
		{"x = 1979-05-27 07:32:00 \n", "x = 1979-05-27 07:32:00 \n", NewLocalDateTime(date, must(NewTime(7, 32, 0, 0)))},
		{"x = [[1979-05-27 ], 1979-05-27 ]\n", "x = [[1979-05-27], 1979-05-27]\n", tomlArray(tomlArray(date), date)},
	}
	for _, c := range cases {
		document := parseTOMLText(t, c.source, c.printed)
		got, err := document.native(keyPath("x"))
		if err != nil || !ValuesEqual(got, c.want) {
			t.Errorf("native of %q = %#v, %v", c.source, got, err)
		}
	}
	document := parseTOMLText(t, "x = [1, 1979-05-27 , 2]\n", "x = [1, 1979-05-27, 2]\n")
	located, err := locateTOML(&document.root, keyPath("x"))
	if err != nil {
		t.Fatal(err)
	}
	if want := [][2]int{{1, 2}, {4, 14}, {16, 17}}; !slices.Equal(located.item.spans, want) || !slices.Equal(located.item.commas, []int{2, 14}) {
		t.Errorf("spans %v, commas %v of %q", located.item.spans, located.item.commas, located.item.raw)
	}
}

func TestTOMLInterleavedTablesParseInLinearTime(t *testing.T) {
	var root, nested strings.Builder
	nested.WriteString("[t]\n")
	for index := range 6000 {
		fmt.Fprintf(&root, "[a.x%d]\nv=1\n[b.y%d]\nw=2\n", index, index)
		fmt.Fprintf(&nested, "[t.a.x%d]\nv=1\n[t.b.y%d]\nw=2\n", index, index)
	}
	for _, source := range []string{root.String(), nested.String()} {
		start := time.Now()
		document, err := parseTOMLDocument(source)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := document.native(nil); err != nil {
			t.Fatal(err)
		}
		if elapsed := time.Since(start); elapsed > 3*time.Second {
			t.Errorf("%d-byte interleaved document took %v", len(source), elapsed)
		}
	}
}

func TestTOMLRoundTripsForeignLayouts(t *testing.T) {
	for _, source := range []string{
		"",
		"\n\n",
		"   ",
		"# only a comment",
		"x=1",
		"  x=1 # foreign\n",
		"  [x] # foreign\na=1\n",
		"a.b.c = 1\n# gap\na.b.d = 2\n\na.e = 3\n",
		"[a]\nx = 1\n[a.b.c]\ny = 2\n[a.b]\nz = 3\n",
		"[[a]]\n[[a.b]]\nx = 1\n[[a.b]]\n[[a]]\n[[a.b]]\n",
		"[x]\n[x.y]\n[[x.z]]\n\n\n[w]\n",
		"x = { a = 1, b = { c = [1, {d = 2}] } }\n",
		"x = [\n  1, # one\n  [2, 3],\n  { a = \"b\" },\n]\n",
		"s1 = \"\"\"\r\nline\r\n\"\"\"\r\ns2 = '''x\ny'''\r\n",
		"t.\"quoted key\".'lit' = 1\n\"\" = 2\n",
		"x = 2020-01-01 # c\ny = 1979-05-27 07:32:00Z # d\n",
		"[ spaced . header ]   # c  \n\tk = \"v\"\n",
	} {
		mustTOML(t, source)
	}
}

func regroupedArrayOfTables(source, printed string) (regrouped, exact bool) {
	if sameHeaderBlocks(source, printed, false) {
		return true, true
	}
	return sameHeaderBlocks(source, printed, true), false
}

var tomlDateTail = regexp.MustCompile(`(\d{4}-\d\d-\d\d(?:[Tt ]\d\d:\d\d:\d\d(?:\.\d+)?(?:[Zz]|[+-]\d\d:\d\d)?)?)[\s\x0b\x1c-\x1f\x85\p{Z}]+([,\]#\r\n])`)

func dropTOMLDateTails(text string) string {
	return tomlDateTail.ReplaceAllString(text, "$1$2")
}

func TestTOMLDateTailWhitespaceModel(t *testing.T) {
	for _, source := range []string{
		"x = [1979-05-27 ]\n", "x = 1979-05-27 \t# c\n", "x = [1979-05-27 07:32:00 , 1]\n", "x = [1979-05-27 \t# c\n]\n", "x = [1979-05-27 \n]\n",
	} {
		document, err := parseTOMLDocument(source)
		if err != nil {
			t.Fatalf("parse %q: %v", source, err)
		}
		if document.text() == source {
			t.Errorf("%q printed unchanged", source)
		}
		if dropTOMLDateTails(document.text()) != dropTOMLDateTails(source) {
			t.Errorf("%q printed %q, which differs beyond date-tail whitespace", source, document.text())
		}
	}
	for source, printed := range map[string]string{
		"x = [\"1979-05-27 \"]\n": "x = [\"1979-05-27\"]\n",
		"x = [1979-05-27 1]\n":    "x = [1979-05-271]\n",
		"x = [1979-05-27 ]\n":     "x = [1979-05-27 2]\n",
	} {
		if dropTOMLDateTails(source) == dropTOMLDateTails(printed) {
			t.Errorf("%q and %q compare equal", source, printed)
		}
	}
}

func nonBlank(text string) string {
	return strings.Join(strings.Fields(text), "")
}

func sameHeaderBlocks(source, printed string, inserted bool) bool {
	sourceBlocks, printedBlocks := tomlHeaderBlocks(source), tomlHeaderBlocks(printed)
	var arrays [][]string
	remaining := map[string]int{}
	for _, block := range sourceBlocks[1:] {
		remaining[block.text]++
		if block.names != nil {
			arrays = append(arrays, block.names)
		}
	}
	if len(arrays) == 0 || sourceBlocks[0].text != printedBlocks[0].text {
		return false
	}
	for index, block := range printedBlocks[1:] {
		if remaining[block.text] > 0 {
			remaining[block.text]--
			continue
		}
		copied := index+2 < len(printedBlocks) && strings.Count(block.text, "\n") == 1 && block.trivia == printedBlocks[index+2].trivia
		implicit := inserted && (block.bare || copied) && slices.ContainsFunc(arrays, func(array []string) bool {
			return len(array) > len(block.names) && slices.Equal(array[:len(block.names)], block.names)
		})
		if !implicit {
			return false
		}
	}
	for _, count := range remaining {
		if count != 0 {
			return false
		}
	}
	return true
}

type tomlBlock struct {
	text   string
	names  []string
	bare   bool
	trivia string
}

func tomlHeaderBlocks(text string) []tomlBlock {
	lines := []string{""}
	for _, line := range strings.SplitAfter(text, "\n") {
		if strings.HasPrefix(strings.TrimLeft(line, " \t"), "[") {
			lines = append(lines, "")
		}
		lines[len(lines)-1] += line
	}
	blocks := []tomlBlock{{text: lines[0]}}
	for _, block := range lines[1:] {
		if !strings.HasSuffix(block, "\n") {
			block += "\n"
		}
		blocks = append(blocks, arrayHeaderBlock(block))
	}
	return blocks
}

func arrayHeaderBlock(block string) tomlBlock {
	start := strings.Index(block, "[[")
	if start < 0 || strings.TrimLeft(block[:start], " \t") != "" {
		return tomlBlock{text: block}
	}
	end := start + 2
	for end < len(block) && block[end] != ']' && block[end] != '\n' {
		if quote := block[end]; quote == '"' || quote == '\'' {
			for end++; end < len(block) && block[end] != quote && block[end] != '\n'; end++ {
				if quote == '"' && block[end] == '\\' {
					end++
				}
			}
		}
		end++
	}
	end = min(end, len(block))
	scanner := &tomlScanner{src: block[start+2 : end], line: 1}
	key, err := parseTOMLKey(scanner)
	if err != nil || !scanner.end() {
		return tomlBlock{text: block}
	}
	names := key.names()
	return tomlBlock{
		text:   block[:start+2] + strings.Join(names, "\x00") + block[end:],
		names:  names,
		bare:   block[end:] == "]]\n",
		trivia: strings.SplitAfter(block[end:], "\n")[0],
	}
}

func TestTOMLRegroupingModelAcceptsOnlyMovedHeaderBlocks(t *testing.T) {
	cases := []struct {
		source, printed string
		want            bool
	}{
		{"[[h.P]]\nm = 1\n[[x]]\n[[h.P]]\nm = 3\n", "[[h.P]]\nm = 1\n[[h.P]]\nm = 3\n[[x]]\n", true},
		{"x = [[1]]\n", "x = [[1]]\n\n", false},
		{"x = [[1, 2], [3]]\n", "x = [[2, 1], [3]]\n", false},
		{"[[a]]\nb = 12\n", "[[a]]\nb = 21\n", false},
		{"[[a]]\nb = 1\n[c]\n", "[[a]]\n[c]\nb = 1\n", false},
		{"[[0 ]]\n[[0]]\n", "[[0 ]]\n[[0 ]]\n", true},
		{"[[\"\t\" ]]\n[[\"\t\"]]\n", "[[\"\t\" ]]\n[[\"\t\" ]]\n", true},
		{"b.'b' = 1\n[b . a . b]\n[['b'.'b'.b]]\n", "b.'b' = 1\n[b . a . b]\n[[b .'b'.b]]\n", true},
		{"[[a]]\n[b . c]\n", "[[a]]\n[b.c]\n", false},
		{"[[0]]\n[[0]]\n[[0.0.0]]\n[[0.0.0]]\n", "[[0]]\n[[0]]\n[[0.0]]\n[[0.0.0]]\n[[0.0]]\n[[0.0.0]]\n", true},
		{"[[0]]\n[[0]]\n[[0.0.0]]#c\n[[0.0.0]]\n", "[[0]]\n[[0]]\n[[0.0]]#c\n[[0.0.0]]#c\n[[0.0]]\n[[0.0.0]]\n", true},
		{"[[0]]\n[[0]]\n[[0.0.0]]#c\n[[0.0.0]]\n", "[[0]]\n[[0]]\n[[0.0]]#d\n[[0.0.0]]#c\n[[0.0]]\n[[0.0.0]]\n", false},
		{"[[0]]\n[[0]]\n[[0.0.0]]\n", "[[0]]\n[[0]]\n[[1]]\n[[0.0.0]]\n", false},
	}
	for _, c := range cases {
		if got, _ := regroupedArrayOfTables(c.source, c.printed); got != c.want {
			t.Errorf("regrouped(%q, %q) = %v", c.source, c.printed, got)
		}
	}
}

func FuzzTOMLDocumentRoundTrip(f *testing.F) {
	for _, seed := range []string{
		"x = 1\n", "[a]\nb = 2\n[[c]]\nd = [1, {e = 3}]\n", "a.b.c = 1\n# gap\na.b.d = 2\n",
		"x = \"\"\"\nmulti\\\n  line\"\"\"\n", "[mcp . test . env] # nested\nX=\"before\"\n",
		"x = 2020-01-01 07:32:00Z\ny = 07:32\n", "x = {a.b = 1, a.c = [1, 2,]}\n",
		"x = [1979-05-27 ]\n", "x = [1979-05-27 ]", "0=[0001-01-01 \n]\n", "0=[0001-01-01 \v]\n", "x = 1979-05-27 \t# c\n", "[a.b]\n[a]\n[a.b.c]\n",
		strings.Repeat("[", 200), strings.Repeat("a.", 200) + "a=1\n", "x=" + strings.Repeat("{a=", 200),
	} {
		f.Add(seed)
	}
	if data, err := readFixture("edits.json"); err == nil {
		documents, invalid := tomlFixtureDocuments(f, data)
		for _, document := range documents {
			f.Add(document)
		}
		for _, document := range invalid {
			f.Add(document)
		}
	}
	f.Fuzz(func(t *testing.T, source string) {
		if !utf8.ValidString(source) {
			return
		}
		document, err := parseTOMLDocument(source)
		if err != nil {
			if _, ok := asValueError(err); !ok {
				t.Fatalf("parse error %T, want *ValueError", err)
			}
			return
		}
		native, err := document.native(nil)
		if err != nil {
			if _, ok := asValueError(err); !ok {
				t.Fatalf("native error %T, want *ValueError", err)
			}
		}
		if document.text() == source {
			return
		}
		if !strings.HasSuffix(source, "\n") {
			// tomlkit can move an unterminated last line without a newline; the terminated twin carries the regrouping checks.
			twin, err := parseTOMLDocument(source + "\n")
			if err != nil {
				t.Fatalf("%q parses but not with a final newline: %v", source, err)
			}
			if regrouped, _ := regroupedArrayOfTables(dropTOMLDateTails(source+"\n"), dropTOMLDateTails(twin.text())); dropTOMLDateTails(twin.text()) != dropTOMLDateTails(source+"\n") && !regrouped {
				t.Fatalf("round trip of %q gave %q", source+"\n", twin.text())
			}
			if nonBlank(twin.text()) != nonBlank(document.text()) {
				t.Fatalf("%q printed %q, its terminated twin %q", source, document.text(), twin.text())
			}
			if again, err := twin.native(nil); (err == nil) != (native != nil) || err == nil && !ValuesEqual(again, native) {
				t.Fatalf("terminated twin of %q reads %#v, %v; source read %#v", source, again, err, native)
			}
			return
		}
		normalized, printed := dropTOMLDateTails(source), dropTOMLDateTails(document.text())
		if printed == normalized {
			return
		}
		regrouped, exact := regroupedArrayOfTables(normalized, printed)
		if !regrouped {
			t.Fatalf("round trip of %q gave %q", source, document.text())
		}
		if err != nil || !exact {
			return
		}
		reparsed, err := parseTOMLDocument(document.text())
		if err != nil {
			t.Fatalf("regrouped text %q does not parse: %v", document.text(), err)
		}
		if reparsed.text() != document.text() {
			// tomlkit itself reads some regrouped texts differently, and then prints them differently too.
			return
		}
		if again, err := reparsed.native(nil); err != nil || !ValuesEqual(again, native) {
			t.Fatalf("regrouped text %q reads %#v, %v; source read %#v", document.text(), again, err, native)
		}
	})
}
