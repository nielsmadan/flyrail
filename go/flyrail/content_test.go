package flyrail

import (
	"errors"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func valueMessage(t *testing.T, err error) string {
	t.Helper()
	failure, ok := errors.AsType[*ValueError](err)
	if !ok {
		t.Fatalf("error %T %v is not a *ValueError", err, err)
	}
	return failure.Message
}

func errorOf[T any](_ T, err error) error {
	return err
}

func TestTreeContentInfersParentsAndSnapshotsEntries(t *testing.T) {
	entries := []BundleEntry{{Path: "scripts/run", Data: []byte("run"), Executable: true}, dir("empty")}
	tree := must(NewTreeContent(entries))
	entries[0].Data[0] = 'X'

	want := []BundleEntry{dir("empty"), dir("scripts"), {Path: "scripts/run", Data: []byte("run"), Executable: true}}
	if diff := cmp.Diff(want, tree.Entries()); diff != "" {
		t.Errorf("entries (-want +got):\n%s", diff)
	}
	tree.Entries()[2].Data[0] = 'Y'
	if string(tree.Entries()[2].Data) != "run" {
		t.Errorf("accessor exposed internal data: %q", tree.Entries()[2].Data)
	}
	if got := len((TreeContent{}).Entries()); got != 0 {
		t.Errorf("zero tree has %d entries", got)
	}
}

func TestTreeContentRejectsDuplicateCollidingAndShadowedEntries(t *testing.T) {
	cases := []struct {
		entries []BundleEntry
		message string
	}{
		{[]BundleEntry{dir("a"), dir("a")}, "duplicate entry path"},
		{[]BundleEntry{file("a/x", "x"), file("A/y", "y")}, "portable entry path collision"},
		{[]BundleEntry{file("a", "file"), file("a/b", "child")}, "entry file/directory collision"},
		{[]BundleEntry{{Path: "a", Dir: true, Executable: true}}, "a directory cannot have executable file intent"},
		{[]BundleEntry{file("../a", "x")}, "entry path must be an unambiguous relative path"},
	}
	for _, tc := range cases {
		if got := valueMessage(t, errorOf(NewTreeContent(tc.entries))); got != tc.message {
			t.Errorf("NewTreeContent(%v) = %q, want %q", tc.entries, got, tc.message)
		}
	}
}

func TestSectionsDeriveSelectorsFromMarkersOrCustomBoundaries(t *testing.T) {
	section := must(NewSectionContent("guide", "text", nil))
	want := must(NewSectionBoundaries("<!-- flyrail:guide:start -->", "<!-- flyrail:guide:end -->"))
	if section.Selector() != want {
		t.Errorf("Selector() = %+v", section.Selector())
	}
	custom := must(NewSectionBoundaries("<!-- >>> owned >>> -->", "<!-- <<< owned <<< -->"))
	first := must(NewSectionContent("first", "text", &custom))
	second := must(NewSectionContent("second", "text", &custom))
	if first.Selector() != second.Selector() || first.Selector() != custom {
		t.Errorf("custom selectors %+v, %+v", first.Selector(), second.Selector())
	}
	if boundaries, ok := first.Boundaries(); !ok || boundaries != custom {
		t.Errorf("Boundaries() = %+v, %t", boundaries, ok)
	}
}

func TestSectionsRejectInvalidBoundariesAndText(t *testing.T) {
	boundaries := must(NewSectionBoundaries("start", "end"))
	cases := []struct {
		err     error
		message string
	}{
		{errorOf(NewSectionBoundaries("same", "same")), "section boundaries must be distinct"},
		{errorOf(NewSectionBoundaries("", "end")), "section boundaries must be nonblank single lines without NUL"},
		{errorOf(NewSectionBoundaries("\n", "end")), "section boundaries must be nonblank single lines without NUL"},
		{errorOf(NewSectionBoundaries("a\x00b", "end")), "section boundaries must be nonblank single lines without NUL"},
		{errorOf(NewSectionBoundaries("a", "b\r")), "section boundaries must be nonblank single lines without NUL"},
		{errorOf(NewSectionContent("x", "text\x00", nil)), "section text cannot contain NUL or Flyrail markers"},
		{errorOf(NewSectionContent("x", "<!-- flyrail:x:start -->", nil)), "section text cannot contain NUL or Flyrail markers"},
		{errorOf(NewSectionContent("x", "start\n", &boundaries)), "section text cannot contain its boundary lines"},
		{errorOf(NewSectionContent("x", "a end", &boundaries)), "section text cannot contain its boundary lines"},
		{errorOf(NewSectionContent("x", "text", &SectionBoundaries{})), "section boundaries must be nonblank single lines without NUL"},
		{errorOf(NewSectionContent("x", "\xff", nil)), "section text must contain Unicode scalar values"},
		{errorOf(NewSectionContent("Bad", "text", nil)), "section marker must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
	}
	for index, tc := range cases {
		if got := valueMessage(t, tc.err); got != tc.message {
			t.Errorf("case %d: %q, want %q", index, got, tc.message)
		}
	}
	if _, err := NewSectionContent("x", "start here\nend here", &boundaries); err != nil {
		t.Error(err)
	}
	if _, err := NewSectionBoundaries(" start ", "end"); err != nil {
		t.Error(err)
	}
}

func TestPythonLinesSplitLikeStrSplitlines(t *testing.T) {
	cases := map[string][]string{
		"a\r\nb\rc\vd\fe\x1cg\x1dh\x1ei\u0085j k l": {"a", "b", "c", "d", "e", "g", "h", "i", "j", "k", "l"},
		"":      nil,
		"plain": {"plain"},
		"a\r\n": {"a"},
		"a\n\n": {"a", ""},
	}
	for text, want := range cases {
		if diff := cmp.Diff(want, pythonLines(text)); diff != "" {
			t.Errorf("pythonLines(%q) (-want +got):\n%s", text, diff)
		}
	}
}

func TestSelectorsAcceptKeysAndIdentifiedMembers(t *testing.T) {
	member := must(NewMember(must(NewString("review")), []string{"name"}))
	selector := must(NewSelector(must(NewKey("hooks")), member))
	parts := selector.Parts()
	if len(parts) != 2 || parts[0] != SelectorPart(must(NewKey("hooks"))) {
		t.Fatalf("parts = %v", parts)
	}
	if got, ok := parts[1].(Member); !ok || !got.equal(member) {
		t.Errorf("member part = %v", parts[1])
	}
	object := must(NewObject(ObjectMember{Key: "a", Value: Int(1)}))
	cases := []struct {
		err     error
		message string
	}{
		{errorOf(NewSelector()), "a selector must select a key or member"},
		{errorOf(NewKey("")), "selector keys must be nonempty"},
		{errorOf(NewMember(object, []string{"name"})), "native member identity must be a scalar"},
		{errorOf(NewMember(Int(1), []string{""})), "selector keys must be nonempty"},
		{errorOf(NewSelector(Key{})), "selector keys must be nonempty"},
		{errorOf(NewSelector(Member{})), "values must be Scalar, ObjectValue or ArrayValue"},
		{errorOf(NewSelector(nil)), "selector parts must be Key or Member values"},
		{errorOf(NewMember(nil, nil)), "values must be Scalar, ObjectValue or ArrayValue"},
	}
	for index, tc := range cases {
		if got := valueMessage(t, tc.err); got != tc.message {
			t.Errorf("case %d: %q, want %q", index, got, tc.message)
		}
	}
	if keyless := must(NewMember(object, nil)); len(keyless.Key()) != 0 {
		t.Errorf("keyless member key = %v", keyless.Key())
	}
}

func TestPlacementAnchorsOnlyBeforeAndAfter(t *testing.T) {
	anchor := must(NewMember(must(NewString("anchor")), nil))
	if (Placement{}).Position() != PositionLast {
		t.Errorf("zero placement position = %q", (Placement{}).Position())
	}
	for _, position := range []Position{PositionBefore, PositionAfter} {
		placement := must(NewPlacement(position, &anchor))
		if got, ok := placement.Anchor(); !ok || !got.equal(anchor) || placement.Position() != position {
			t.Errorf("%s placement = %+v", position, placement)
		}
		if got := valueMessage(t, errorOf(NewPlacement(position, nil))); got != "before/after placement requires exactly one member anchor" {
			t.Errorf("%s without anchor: %q", position, got)
		}
	}
	for _, position := range []Position{PositionFirst, PositionLast} {
		if got := valueMessage(t, errorOf(NewPlacement(position, &anchor))); got != "before/after placement requires exactly one member anchor" {
			t.Errorf("%s with anchor: %q", position, got)
		}
	}
	if got := valueMessage(t, errorOf(NewPlacement("middle", nil))); got != "'middle' is not a valid Position" {
		t.Errorf("unknown position: %q", got)
	}
}

func TestStructuredContentPlacesOnlyArrayMembers(t *testing.T) {
	member := must(NewMember(must(NewString("review")), []string{"name"}))
	later := must(NewMember(must(NewString("later")), []string{"name"}))
	memberSelector := must(NewSelector(must(NewKey("hooks")), member))
	keySelector := must(NewSelector(must(NewKey("hooks"))))
	before := must(NewPlacement(PositionBefore, &later))
	content := must(NewStructuredContent(FormatJSONC, memberSelector, Int(1), &before))
	if placement, ok := content.Placement(); !ok || placement.Position() != PositionBefore || content.Format() != FormatJSONC {
		t.Errorf("content = %+v", content)
	}
	self := must(NewPlacement(PositionBefore, &member))
	cases := []struct {
		err     error
		message string
	}{
		{errorOf(NewStructuredContent(FormatJSON, keySelector, Int(1), &Placement{})), "placement is only valid for an array member"},
		{errorOf(NewStructuredContent(FormatJSON, memberSelector, Int(1), &self)), "a member cannot be its own insertion anchor"},
		{errorOf(NewStructuredContent(FormatJSON, Selector{}, Int(1), nil)), "a selector must select a key or member"},
		{errorOf(NewStructuredContent("yaml", keySelector, Int(1), nil)), "'yaml' is not a valid DocumentFormat"},
		{errorOf(NewStructuredContent(FormatJSON, keySelector, nil, nil)), "values must be Scalar, ObjectValue or ArrayValue"},
	}
	for index, tc := range cases {
		if got := valueMessage(t, tc.err); got != tc.message {
			t.Errorf("case %d: %q, want %q", index, got, tc.message)
		}
	}
	dated := must(NewStructuredContent(FormatTOML, keySelector, mustDate(t, 2026, 1, 2), nil))
	if _, ok := dated.Placement(); ok {
		t.Error("absent placement reported as present")
	}
}

func TestDocumentSchemasScaffoldStructuredContent(t *testing.T) {
	schema := must(NewDocumentSchema(FormatJSONC, "version", Int(1)))
	content := must(schema.Content())
	if content.Format() != FormatJSONC || !ValuesEqual(content.Value(), Int(1)) {
		t.Errorf("content = %+v", content)
	}
	if parts := content.Selector().Parts(); len(parts) != 1 || parts[0] != SelectorPart(must(NewKey("version"))) {
		t.Errorf("selector = %v", parts)
	}
	wantModel := `["object",[["fields",["object",[["format",["string","jsonc"]],["key",["string","version"]],` +
		`["value",["object",[["fields",["object",[["value",["integer","1"]]]]],["type",["string","Scalar"]]]]]]]],["type",["string","DocumentSchema"]]]]`
	if got := encoded(t, schema.modelValue()); got != wantModel {
		t.Errorf("model = %s", got)
	}
	if got := valueMessage(t, errorOf(NewDocumentSchema(FormatTOML, "version", Int(1)))); got != "document schema scaffolds require JSON or JSONC" {
		t.Errorf("TOML schema: %q", got)
	}
	if got := valueMessage(t, errorOf(NewDocumentSchema(FormatJSON, "", Int(1)))); got != "selector keys must be nonempty" {
		t.Errorf("empty key: %q", got)
	}
}

func TestFileContentCopiesBytesAndValidatesMode(t *testing.T) {
	data := []byte("hi")
	content := must(NewFileContent(data, ModeReadable))
	data[0] = 'X'
	content.Data()[1] = 'Y'
	if string(content.Data()) != "hi" || content.Mode() != ModeReadable {
		t.Errorf("content = %q %q", content.Data(), content.Mode())
	}
	if got := valueMessage(t, errorOf(NewFileContent(nil, ""))); got != "'' is not a valid FileMode" {
		t.Errorf("empty mode: %q", got)
	}
}

func TestContentModelValuesMatchRustRecords(t *testing.T) {
	anchor := must(NewMember(Int(7), nil))
	placement := must(NewPlacement(PositionAfter, &anchor))
	selector := must(NewSelector(must(NewKey("hooks")), must(NewMember(must(NewString("x")), []string{"name"}))))
	structured := must(NewStructuredContent(FormatJSON, selector, must(NewArray(Bool(true))), &placement))
	want := `["object",[["fields",["object",[` +
		`["format",["string","json"]],` +
		`["placement",["object",[["fields",["object",[["anchor",["object",[["fields",["object",[` +
		`["identity",["object",[["fields",["object",[["value",["integer","7"]]]]],["type",["string","Scalar"]]]]],` +
		`["key",["array",[]]]]]],["type",["string","Member"]]]]],["position",["string","after"]]]]],["type",["string","Placement"]]]]],` +
		`["selector",["object",[["fields",["object",[["parts",["array",[` +
		`["object",[["fields",["object",[["name",["string","hooks"]]]]],["type",["string","Key"]]]],` +
		`["object",[["fields",["object",[["identity",["object",[["fields",["object",[["value",["string","x"]]]]],["type",["string","Scalar"]]]]],` +
		`["key",["array",[["string","name"]]]]]]],["type",["string","Member"]]]]]]]]]],["type",["string","Selector"]]]]],` +
		`["value",["object",[["fields",["object",[["items",["array",[["object",[["fields",["object",[["value",["boolean",true]]]]],` +
		`["type",["string","Scalar"]]]]]]]]]],["type",["string","ArrayValue"]]]]]` +
		`]]],["type",["string","StructuredContent"]]]]`
	if got := encoded(t, modelContent(structured)); got != want {
		t.Errorf("structured model:\n got %s\nwant %s", got, want)
	}
	file := must(NewFileContent([]byte{0, 0xff}, ModePrivate))
	wantFile := `["object",[["fields",["object",[["data",["object",[["bytes",["string","00ff"]]]]],["mode",["string","private"]]]]],["type",["string","FileContent"]]]]`
	if got := encoded(t, modelContent(file)); got != wantFile {
		t.Errorf("file model: %s", got)
	}
}
