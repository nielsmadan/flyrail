package flyrail

import (
	"errors"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func errorText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func TestLayoutFailuresReportThePythonMessages(t *testing.T) {
	cases := []struct{ layout, expected string }{
		{`{"version":1,"headers":[5]}`, "invalid TOML restoration header entry"},
		{`{"version":1,"headers":[[1,2,3]]}`, "invalid TOML restoration header entry"},
		{`{"version":1,"headers":[[5,"a"]]}`, "invalid TOML restoration path"},
		{`{"version":1,"headers":[[[],"a"]]}`, "invalid TOML restoration path"},
		{`{"version":1,"headers":[[[1.5],"a"]]}`, "invalid TOML restoration path"},
		{`{"version":1,"headers":[[[-1],"a"]]}`, "invalid TOML restoration path"},
		{`{"version":1,"headers":[[[true],"a"]]}`, "invalid TOML restoration path"},
		{`{"version":1,"headers":[[["a"],5]]}`, "invalid TOML restoration header"},
		{`{"version":1,"headers":[[["a"],"a."]]}`, "invalid TOML restoration header"},
		{`{"version":1,"headers":[[["a"],"a"],[["a"],"b"]]}`, "duplicate TOML restoration path"},
		{`{"version":1,"headers":[[["a"],"a"],[["a"],5]]}`, "duplicate TOML restoration path"},
		{`{"version":2,"headers":[]}`, "invalid TOML restoration layout"},
		{`{"version":true,"headers":[]}`, "invalid TOML restoration layout"},
		{`{"version":1000000000000000000000,"headers":[]}`, "invalid TOML restoration layout"},
		{`{"version":1,"extra":[]}`, "invalid TOML restoration layout"},
		{`not json`, "invalid TOML restoration layout"},
	}
	for _, c := range cases {
		_, err := layoutHeaders([]byte(c.layout))
		if got := errorText(err); got != c.expected {
			t.Errorf("layout %s: got %q, want %q", c.layout, got, c.expected)
		}
	}
}

func TestLayoutAcceptsFieldAndIndexPaths(t *testing.T) {
	headers, err := layoutHeaders([]byte(`{"version":1,"headers":[[["a"],"a.b"],[["a",0],"a.b"]]}`))
	if err != nil {
		t.Fatal(err)
	}
	want := []layoutHeader{
		{path: []layoutPart{{field: "a"}}, name: "a.b"},
		{path: []layoutPart{{field: "a"}, {index: 0, isIndex: true}}, name: "a.b"},
	}
	if diff := cmp.Diff(want, headers, cmp.AllowUnexported(layoutHeader{}, layoutPart{})); diff != "" {
		t.Errorf("headers (-want +got):\n%s", diff)
	}
	if empty, err := layoutHeaders([]byte(`{"version":1,"headers":[]}`)); err != nil || len(empty) != 0 {
		t.Errorf("empty layout = %v, %v", empty, err)
	}
}

func TestHeaderPathParsesTOMLKeys(t *testing.T) {
	cases := []struct {
		name string
		want []string
		err  string
	}{
		{"a.b", []string{"a", "b"}, ""},
		{" a . \"b\\u0041\\t\" . 'c d' ", []string{"a", "bA\t", "c d"}, ""},
		{`"\U0001F600"`, []string{"\U0001F600"}, ""},
		{`"\ud800"`, nil, "invalid TOML restoration header"},
		{`"a\q"`, nil, "invalid TOML restoration header"},
		{`'unterminated`, nil, "invalid TOML restoration header"},
		{"a..b", nil, "invalid TOML restoration header"},
		{"a b", nil, "invalid TOML restoration header"},
		{"", nil, "invalid TOML restoration header"},
		{"'tab\there'", nil, "invalid TOML restoration header"},
	}
	for _, c := range cases {
		got, err := headerPath(c.name)
		if errorText(err) != c.err || !cmp.Equal(got, c.want) {
			t.Errorf("headerPath(%q) = %q, %v; want %q, %q", c.name, got, err, c.want, c.err)
		}
	}
}

func snapshotBytes(t *testing.T, text string) SelectionSnapshot {
	t.Helper()
	return must(NewSelectionSnapshot(NewSnapshotBytes([]byte(text)), []byte(text), []byte(text), nil, nil))
}

func typedSnapshot(t *testing.T, value Value, layout string) SelectionSnapshot {
	t.Helper()
	return must(NewSelectionSnapshot(must(NewSnapshotTyped(value)), []byte("x"), []byte("x"), nil, []byte(layout)))
}

func TestOwnedSelectionValidatesLikeRust(t *testing.T) {
	section := must(NewClaim(OwnershipSection, SubtreeSelector("guide")))
	tomlClaim := must(NewClaim(OwnershipStructured, must(NewSelector(must(NewKey("tool")), must(NewKey("servers"))))))
	env := must(NewObject(ObjectMember{Key: "env", Value: must(NewObject())}))
	layout := `{"version":1,"headers":[[["env"],"tool.servers.env"]]}`
	text := snapshotBytes(t, "x\n")
	baseline := snapshotBytes(t, "old\n")
	cases := []struct {
		name   string
		params OwnedSelectionParams
		err    string
	}{
		{"section", OwnedSelectionParams{Claim: section, Installed: text, Disposition: DispositionCreated, Separator: []byte("\n")}, ""},
		{"toml layout", OwnedSelectionParams{Claim: tomlClaim, Format: FormatTOML, Installed: typedSnapshot(t, env, layout), Disposition: DispositionAdopted}, ""},
		{"section with format", OwnedSelectionParams{Claim: section, Format: FormatJSON, Installed: text, Disposition: DispositionCreated}, "shared claim and document format disagree"},
		{"structured without format", OwnedSelectionParams{Claim: tomlClaim, Installed: text, Disposition: DispositionCreated}, "shared claim and document format disagree"},
		{"file claim", OwnedSelectionParams{Claim: must(NewClaim(OwnershipFile, WholeSelector{})), Installed: text, Disposition: DispositionCreated}, "shared claim and document format disagree"},
		{"takeover without baseline", OwnedSelectionParams{Claim: section, Installed: text, Disposition: DispositionTakenOver}, "only takeover ownership has a baseline"},
		{"baseline without takeover", OwnedSelectionParams{Claim: section, Installed: text, Disposition: DispositionCreated, Baseline: &baseline}, "only takeover ownership has a baseline"},
		{"bytes for structured", OwnedSelectionParams{Claim: tomlClaim, Format: FormatTOML, Installed: text, Disposition: DispositionCreated}, "selection snapshot and document format disagree"},
		{"layout outside TOML", OwnedSelectionParams{Claim: tomlClaim, Format: FormatJSON, Installed: typedSnapshot(t, env, layout), Disposition: DispositionCreated}, "restoration layout requires a TOML claim"},
		{"layout header mismatch", OwnedSelectionParams{Claim: tomlClaim, Format: FormatTOML, Installed: typedSnapshot(t, env, `{"version":1,"headers":[[["env"],"tool.env"]]}`), Disposition: DispositionCreated}, "TOML restoration header does not match its destination"},
		{"separator on structured", OwnedSelectionParams{Claim: tomlClaim, Format: FormatTOML, Installed: typedSnapshot(t, env, ""), Disposition: DispositionCreated, Separator: []byte("\n")}, "section separators require a text claim"},
		{"zero claim", OwnedSelectionParams{Installed: text, Disposition: DispositionCreated}, "ownership requires a Claim"},
		{"zero snapshot", OwnedSelectionParams{Claim: section, Disposition: DispositionCreated}, "ownership requires an installed snapshot and disposition"},
		{"unknown format", OwnedSelectionParams{Claim: tomlClaim, Format: "yaml", Installed: text, Disposition: DispositionCreated}, "'yaml' is not a valid DocumentFormat"},
	}
	for _, c := range cases {
		_, err := NewOwnedSelection(c.params)
		if got := errorText(err); got != c.err {
			t.Errorf("%s: error %q, want %q", c.name, got, c.err)
		}
	}
}

func TestSelectionSnapshotChecksLayoutAgainstItsValue(t *testing.T) {
	scalar := must(NewString("x"))
	list := must(NewArray(must(NewObject())))
	value := must(NewObject(ObjectMember{Key: "env", Value: scalar}, ObjectMember{Key: "list", Value: list}))
	cases := []struct{ layout, err string }{
		{`{"version":1,"headers":[[["missing"],"missing"]]}`, "invalid TOML restoration path"},
		{`{"version":1,"headers":[[["env"],"env"]]}`, "TOML restoration header requires an object"},
		{`{"version":1,"headers":[[["list",1],"list"]]}`, "invalid TOML restoration path"},
		{`{"version":1,"headers":[[["list",0],"list"]]}`, ""},
	}
	for _, c := range cases {
		_, err := NewSelectionSnapshot(must(NewSnapshotTyped(value)), nil, nil, nil, []byte(c.layout))
		if got := errorText(err); got != c.err {
			t.Errorf("layout %s: error %q, want %q", c.layout, got, c.err)
		}
	}
	_, err := NewSelectionSnapshot(NewSnapshotBytes(nil), nil, nil, nil, []byte(`{"version":1,"headers":[[["a"],"a"]]}`))
	if got := errorText(err); got != "invalid TOML restoration path" {
		t.Errorf("bytes snapshot with layout error %q", got)
	}
	if _, err := NewSelectionSnapshot(nil, nil, nil, nil, nil); err == nil {
		t.Error("nil snapshot value accepted")
	}
}

func TestArrayPositionAnchorsMustDiffer(t *testing.T) {
	anchor := must(NewMember(must(NewString("a")), []string{"id"}))
	same := must(NewMember(must(NewString("a")), []string{"id"}))
	_, err := NewArrayPosition(&anchor, &same, nil)
	if got := errorText(err); got != "array position anchors must be distinct" {
		t.Errorf("error %q", got)
	}
	position := must(NewArrayPosition(&anchor, nil, []byte("\n  ")))
	if previous, ok := position.Previous(); !ok || !previous.equal(anchor) {
		t.Errorf("previous = %#v, %v", previous, ok)
	}
	if _, ok := position.Following(); ok {
		t.Error("following anchor reported")
	}
}

func TestEditingRecordsCopyTheirBytes(t *testing.T) {
	prefix := []byte("ab")
	position := must(NewArrayPosition(nil, nil, prefix))
	prefix[0] = 'x'
	returned := position.Prefix()
	returned[1] = 'y'
	if got := string(position.Prefix()); got != "ab" {
		t.Errorf("prefix = %q", got)
	}
	syntax := []byte("text")
	snapshot := must(NewSelectionSnapshot(NewSnapshotBytes(syntax), syntax, syntax, &position, nil))
	syntax[0] = 'X'
	value, _ := snapshot.Value().(SnapshotBytes)
	value.Bytes()[0] = 'Y'
	if got := string(snapshot.Syntax()) + string(snapshot.OwnedSyntax()) + string(value.Bytes()); got != "texttexttext" {
		t.Errorf("snapshot bytes = %q", got)
	}
	empty := []byte(" ")
	provenance := must(NewDocumentProvenance(DocumentProvenanceParams{EmptyDocument: empty, HasEmptyDocument: true}))
	empty[0] = 'x'
	if got, ok := provenance.EmptyDocument(); !ok || string(got) != " " {
		t.Errorf("empty document = %q, %v", got, ok)
	}
}

func TestDocumentProvenanceValidatesLikeRust(t *testing.T) {
	schemaKey := must(NewClaim(OwnershipStructured, must(NewSelector(must(NewKey("$schema"))))))
	scalar := must(NewSelectionSnapshot(must(NewSnapshotTyped(must(NewString("s")))), nil, nil, nil, nil))
	field := must(NewOwnedSelection(OwnedSelectionParams{Claim: schemaKey, Format: FormatJSON, Installed: scalar, Disposition: DispositionCreated}))
	adopted := must(NewOwnedSelection(OwnedSelectionParams{Claim: schemaKey, Format: FormatJSON, Installed: scalar, Disposition: DispositionAdopted}))
	container := NewCreatedContainer(must(NewSelector(must(NewKey("a")))), []byte("{}"))
	digest := strings.Repeat("a", 64)
	cases := []struct {
		name   string
		params DocumentProvenanceParams
		err    string
	}{
		{"complete", DocumentProvenanceParams{Containers: []CreatedContainer{container}, TrailingSeparators: []string{digest}, SchemaFields: []OwnedSelection{field}, SchemaEmptyDocument: []byte(" \t\n\r\v\f"), HasSchemaEmptyDocument: true}, ""},
		{"adopted schema field", DocumentProvenanceParams{SchemaFields: []OwnedSelection{adopted}}, "schema provenance requires created top-level scalar fields"},
		{"duplicate schema field", DocumentProvenanceParams{SchemaFields: []OwnedSelection{field, field}}, "duplicate schema provenance"},
		{"schema empty document", DocumentProvenanceParams{SchemaEmptyDocument: []byte(" x"), HasSchemaEmptyDocument: true}, "schema empty-document evidence must be whitespace bytes"},
		{"duplicate container", DocumentProvenanceParams{Containers: []CreatedContainer{container, container}}, "duplicate container provenance"},
		{"zero container", DocumentProvenanceParams{Containers: []CreatedContainer{{}}}, "container provenance requires a selector and immutable syntax"},
		{"separator digest", DocumentProvenanceParams{TrailingSeparators: []string{strings.ToUpper(digest)}}, "separator evidence must be SHA-256 digests"},
	}
	for _, c := range cases {
		_, err := NewDocumentProvenance(c.params)
		if got := errorText(err); got != c.err {
			t.Errorf("%s: error %q, want %q", c.name, got, c.err)
		}
	}
}

func TestZeroDocumentProvenanceIsRustDefault(t *testing.T) {
	for _, vector := range stateVectors(t) {
		if vector.name == "generated-provenance-default" {
			if got := encodeRecord(DocumentProvenance{}); string(got) != string(vector.data) {
				t.Errorf("zero provenance encodes as %s, want %s", got, vector.data)
			}
			return
		}
	}
	t.Fatal("generated-provenance-default vector is missing")
}

func TestStateEnumsParseRustSpellings(t *testing.T) {
	if got := must(ParseDisposition("taken-over")); got != DispositionTakenOver {
		t.Errorf("ParseDisposition = %q", got)
	}
	if got := must(ParseResourceKind("skill-container")); got != ResourceSkillContainer {
		t.Errorf("ParseResourceKind = %q", got)
	}
	for _, c := range []struct {
		err  error
		want string
	}{
		{errorOf(ParseDisposition("taken_over")), "'taken_over' is not a valid Disposition"},
		{errorOf(ParseResourceKind("file")), "'file' is not a valid ResourceKind"},
		{errorOf(ParseOwnership("whole")), "'whole' is not a valid Ownership"},
	} {
		if failure, ok := errors.AsType[*ValueError](c.err); !ok || failure.Message != c.want {
			t.Errorf("error %v, want %q", c.err, c.want)
		}
	}
}
