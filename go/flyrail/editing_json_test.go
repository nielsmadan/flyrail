package flyrail

import (
	"bytes"
	"fmt"
	"math"
	"slices"
	"strings"
	"testing"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

func jsonNative(t *testing.T, text string, comments bool) Value {
	t.Helper()
	document, err := parseJSONDocument(text, comments)
	if err != nil {
		t.Fatalf("parse %q: %v", text, err)
	}
	value, err := document.native(nil)
	if err != nil {
		t.Fatal(err)
	}
	return value
}

func jsonRejection(text string, comments bool) string {
	_, err := parseJSONDocument(text, comments)
	return errorText(err)
}

func mustJSONDocument(t *testing.T, text string, comments bool) *jsonDocument {
	t.Helper()
	document, err := parseJSONDocument(text, comments)
	if err != nil {
		t.Fatal(err)
	}
	return document
}

func expectJSONText(t *testing.T, label string, document *jsonDocument, want string) {
	t.Helper()
	if document.text() != want {
		t.Errorf("%s: text %q, want %q", label, document.text(), want)
	}
}

func TestJSONDocumentParsesScalarsAndContainers(t *testing.T) {
	inner := must(NewObject(ObjectMember{Key: "b", Value: String{text: "é"}}))
	items := must(NewArray(Int(1), must(NewFloat(2.5)), String{text: "x"}, Null{}, Bool(true), inner))
	rows := []struct {
		text string
		want Value
	}{
		{`{"a":[1,2.5,"x",null,true,{"b":"é"}],"c":{}}`, must(NewObject(
			ObjectMember{Key: "a", Value: items}, ObjectMember{Key: "c", Value: must(NewObject())}))},
		{"[-0,0e0,1E+5,-0.0,1e-05]", must(NewArray(Int(0), must(NewFloat(0)), must(NewFloat(100_000)),
			must(NewFloat(negativeZero())), must(NewFloat(1e-5))))},
		{`{"b":2,"a":1}`, must(NewObject(ObjectMember{Key: "a", Value: Int(1)}, ObjectMember{Key: "b", Value: Int(2)}))},
		{`"😀"`, String{text: "😀"}},
		{`"\ud83d\ude00"`, String{text: "😀"}},
	}
	for _, row := range rows {
		if got := jsonNative(t, row.text, false); !ValuesEqual(got, row.want) {
			t.Errorf("%q parsed as %#v", row.text, got)
		}
	}
}

func negativeZero() float64 { return math.Copysign(0, -1) }

func TestJSONDocumentRejectsMalformedDocumentsWithReferenceMessages(t *testing.T) {
	rows := []struct{ text, message string }{
		{"", "missing JSON value"},
		{"// no root", "Expecting value: line 1 column 1 (char 0)"},
		{"1 2", "unexpected data after JSON document"},
		{"[]]", "unexpected data after JSON document"},
		{`{"a":1} trailing`, "unexpected data after JSON document"},
		{"[", "unterminated or malformed JSON container"},
		{"[01]", "unterminated or malformed JSON container"},
		{"[00]", "unterminated or malformed JSON container"},
		{"[1.]", "unterminated or malformed JSON container"},
		{"[1e]", "unterminated or malformed JSON container"},
		{"[0x10]", "unterminated or malformed JSON container"},
		{"[1 2]", "unterminated or malformed JSON container"},
		{"[truex]", "unterminated or malformed JSON container"},
		{"[Infinityx]", "floats must be finite"},
		{"[NaNx]", "floats must be finite"},
		{"[-NaN]", "Expecting value: line 1 column 2 (char 1)"},
		{"[-Inf]", "Expecting value: line 1 column 2 (char 1)"},
		{"[-.5]", "Expecting value: line 1 column 2 (char 1)"},
		{"[,]", "Expecting value: line 1 column 2 (char 1)"},
		{"[tru]", "Expecting value: line 1 column 2 (char 1)"},
		{"[+1]", "Expecting value: line 1 column 2 (char 1)"},
		{"[.5]", "Expecting value: line 1 column 2 (char 1)"},
		{`{"a":}`, "Expecting value: line 1 column 6 (char 5)"},
		{`{"a":1,,}`, "Expecting value: line 1 column 8 (char 7)"},
		{`{"a"}`, "missing JSON object colon"},
		{`{"a" 1}`, "missing JSON object colon"},
		{"{1:2}", "JSON object keys must be strings"},
		{"{01:2}", "JSON object keys must be strings"},
		{"{[1]:2}", "JSON object keys must be strings"},
		{"{[]:2}", "JSON object keys must be strings"},
		{"{[1,2]:2}", "JSON object keys must be strings"},
		{"{[[1]]:2}", "JSON object keys must be strings"},
		{"{{}:1}", "JSON object keys must be strings"},
		{"{[1]:}", "JSON object keys must be strings"},
		{`{"a":1, [2]:3}`, "JSON object keys must be strings"},
		{`{[1]"a":2}`, "JSON object keys must be strings"},
		{"{[1],:2}", "JSON object keys must be strings"},
		{"{[1:2}", "unterminated or malformed JSON container"},
		{"{[}:1}", "Expecting value: line 1 column 3 (char 2)"},
		{`["a" "b"]`, "unterminated or malformed JSON container"},
		{`["a\qb"]`, `Invalid \escape: line 1 column 4 (char 3)`},
		{`["a\u12"]`, `Invalid \uXXXX escape: line 1 column 5 (char 4)`},
		{`["a\u00ZZ"]`, `Invalid \uXXXX escape: line 1 column 5 (char 4)`},
		{`["abc`, "Unterminated string starting at: line 1 column 2 (char 1)"},
		{`["a\`, "Unterminated string starting at: line 1 column 2 (char 1)"},
		{"[\"a\nb\"]", "Invalid control character at: line 1 column 4 (char 3)"},
		{`{"a":NaN}`, "floats must be finite"},
		{`{"a":1e400}`, "floats must be finite"},
		{`{"a":-Infinity}`, "floats must be finite"},
		{`{"a":9223372036854775808}`, "integers must fit signed 64 bits"},
		{`{"a":-9223372036854775809}`, "integers must fit signed 64 bits"},
		{`["\ud800"]`, "scalar must contain Unicode scalar values"},
		{`["\udc00"]`, "scalar must contain Unicode scalar values"},
		{`["\ud800abc"]`, "scalar must contain Unicode scalar values"},
		{`["\ud800\ud800"]`, "scalar must contain Unicode scalar values"},
		{`{"\ud800":1}`, "scalar must contain Unicode scalar values"},
		{"{/* c */}", "Expecting value: line 1 column 2 (char 1)"},
		{`{"a":1,}`, "JSON trailing commas are unsupported"},
		{`{"a":1,"a":2}`, "duplicate JSON object key"},
		{`{"a":`, "missing JSON value"},
		{`{"a":1`, "unterminated or malformed JSON container"},
		{`{"a":1,`, "unterminated or malformed JSON container"},
		{"[1,", "unterminated or malformed JSON container"},
		{"[1", "unterminated or malformed JSON container"},
		{"{", "unterminated or malformed JSON container"},
		{"{true:1}", "JSON object keys must be strings"},
		{"[1,]", "JSON trailing commas are unsupported"},
		{"{\n  \"a\": tru\n}", "Expecting value: line 2 column 8 (char 9)"},
		{"[\"é\" x]", "unterminated or malformed JSON container"},
		{"[\"é\", x]", "Expecting value: line 1 column 7 (char 6)"},
	}
	for _, row := range rows {
		if got := jsonRejection(row.text, false); got != row.message {
			t.Errorf("%q: error %q, want %q", row.text, got, row.message)
		}
	}
}

func TestJSONCAcceptsCommentsAndTrailingCommas(t *testing.T) {
	for _, text := range []string{`{"a":1,}`, `{"a":1 /* c */}`, "[1 /* c */, 2,]", "{\"a\" // k\n:1}", "/*header*/{}", "{} // tail"} {
		if _, err := parseJSONDocument(text, true); err != nil {
			t.Errorf("%q: %v", text, err)
		}
	}
	rows := []struct {
		text     string
		comments bool
		message  string
	}{
		{"{/*oops", true, "unterminated JSONC comment"},
		{"{/* c */[1]:2}", true, "JSON object keys must be strings"},
		{`{"a":1,}`, false, "JSON trailing commas are unsupported"},
	}
	for _, row := range rows {
		if got := jsonRejection(row.text, row.comments); got != row.message {
			t.Errorf("%q: error %q, want %q", row.text, got, row.message)
		}
	}
}

func TestJSONDocumentRejectsExcessiveNestingLikeTheReference(t *testing.T) {
	_, err := parseJSONDocument(strings.Repeat("[", 986), false)
	if failure, ok := asValueError(err); !ok || !failure.nesting || failure.Message != documentNesting {
		t.Errorf("deep error %v", err)
	}
	if _, err := parseJSONDocument(strings.Repeat("[", 985)+strings.Repeat("]", 985), false); err != nil {
		t.Errorf("shallow error %v", err)
	}
}

func TestEditDocumentRejectsHugeKeyPositionNesting(t *testing.T) {
	content := structuredKeys(t, FormatJSONC, Int(1), "x")
	for _, raw := range []string{
		strings.Repeat("{", 2_500_000),
		`{"a":1,` + strings.Repeat("{", 2_500_000),
		`{"a":1,` + strings.Repeat("[", 2_500_000),
	} {
		if got := setStructuredError(t, []byte(raw), true, content, AcquireConflict); got != documentNesting {
			t.Errorf("%.12q: error %q, want %q", raw, got, documentNesting)
		}
	}
}

func TestJSONDocumentPutsInsertsDeletesAndRelocatesMembers(t *testing.T) {
	x, y := []pathPart{segmentKey("x")}, []pathPart{segmentKey("y")}
	document := mustJSONDocument(t, `{"x": 1 /* foreign */}`, true)
	if err := document.put(x, Int(2), nil, []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "put", document, `{"x": 2 /* foreign */}`)
	if err := document.put(y, String{text: "new"}, nil, []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "put new key", document, `{"x": 2, /* foreign */"y":"new"}`)
	if err := document.delete(x); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "delete", document, `{  /* foreign */"y":"new"}`)
	want := must(NewObject(ObjectMember{Key: "y", Value: String{text: "new"}}))
	if got := must(document.native(nil)); !ValuesEqual(got, want) {
		t.Errorf("native %#v", got)
	}

	array := mustJSONDocument(t, "[1, 2, 3]", false)
	if err := array.insert(nil, 0, Int(0), nil, nil, []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "insert first", array, "[0,1, 2, 3]")
	if err := array.relocate([]pathPart{segmentIndex(3)}, 0, Int(3), nil, nil, []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "relocate", array, "[3,0,1, 2 ]")
	checks := []struct {
		label string
		got   []byte
		want  string
	}{
		{"prefix", must(array.positionPrefix([]pathPart{segmentIndex(2)})), ""},
		{"syntax", must(array.syntax([]pathPart{segmentIndex(2)})), "1"},
		{"layout", must(array.layout(nil)), ""},
		{"empty", must(array.emptySyntax(nil)), "[3,0,1, 2 ]"},
	}
	for _, check := range checks {
		if string(check.got) != check.want {
			t.Errorf("%s: %q, want %q", check.label, check.got, check.want)
		}
	}

	middle := mustJSONDocument(t, "[1, 2]", false)
	if err := middle.insert(nil, 1, Int(9), nil, nil, []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "insert middle", middle, "[1,9, 2]")
	if err := middle.insert(nil, 9, Int(7), nil, nil, []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "append", middle, "[1,9, 2,7]")

	trailing := mustJSONDocument(t, "[1, 2,]", true)
	if err := trailing.insert(nil, 2, Int(3), nil, nil, []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "append after trailing comma", trailing, "[1, 2,3,]")
	empty := mustJSONDocument(t, "{ }", false)
	if err := empty.put(x, Int(1), nil, []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "first member", empty, `{ "x":1}`)
	if err := empty.put(nil, Int(5), []byte("  5"), []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "root syntax", empty, "  5")
}

func TestJSONDocumentRejectsMissingPathsAndInvalidSyntax(t *testing.T) {
	document := mustJSONDocument(t, `{"x":[1]}`, false)
	rows := []struct {
		label string
		err   error
		want  string
	}{
		{"missing native", errorOf(document.native([]pathPart{segmentKey("y")})), "document selector path is missing"},
		{"root delete", document.delete(nil), "document selector path is missing"},
		{"missing delete", document.delete([]pathPart{segmentKey("y")}), "document selector path is missing"},
		{"key prefix", errorOf(document.positionPrefix([]pathPart{segmentKey("x")})), "array position requires a member"},
		{"bad syntax", document.put([]pathPart{segmentKey("x")}, Int(1), []byte{0xff}, []byte{}),
			"'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"},
		{"unparsable syntax", document.put([]pathPart{segmentKey("x")}, Int(1), []byte("1 2"), []byte{}),
			"unterminated or malformed JSON container"},
		{"key under array", document.put([]pathPart{segmentKey("x"), segmentKey("a")}, Int(1), nil, []byte{}),
			"document selector path is missing"},
	}
	for _, row := range rows {
		if got := errorText(row.err); got != row.want {
			t.Errorf("%s: error %q, want %q", row.label, got, row.want)
		}
	}
}

func TestJSONRelocationPrefixMustMatchOriginalTrivia(t *testing.T) {
	two := []pathPart{segmentIndex(1)}
	document := mustJSONDocument(t, "[1, /* two */ 2]", true)
	err := document.relocate(two, 0, Int(2), nil, []byte(" /* wrong */ "), []byte{})
	if got := errorText(err); got != "original array position trivia changed" {
		t.Errorf("mismatch error %q", got)
	}
	expectJSONText(t, "after mismatch", document, "[1 /* two */ ]")
	matched := mustJSONDocument(t, "[1, /* two */ 2]", true)
	if err := matched.relocate(two, 0, Int(2), nil, []byte{}, []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "empty prefix", matched, "[2,1 /* two */ ]")
	restored := mustJSONDocument(t, "[ /* zero */ 1, /* two */ 2]", true)
	if err := restored.relocate(two, 1, Int(2), nil, []byte(" /* two */ "), []byte{}); err != nil {
		t.Fatal(err)
	}
	expectJSONText(t, "prefix restore", restored, "[ /* zero */ 1, /* two */ 2]")
}

func TestJSONValuesUsePythonJSONDumpsSpelling(t *testing.T) {
	document := mustJSONDocument(t, "{}", false)
	object := must(NewObject(
		ObjectMember{Key: "text", Value: String{text: "line\nbreak\u0001\"é"}},
		ObjectMember{Key: "float", Value: must(NewFloat(negativeZero()))},
		ObjectMember{Key: "big", Value: must(NewFloat(1e16))},
		ObjectMember{Key: "int", Value: Int(-12)},
		ObjectMember{Key: "bool", Value: Bool(false)},
		ObjectMember{Key: "null", Value: Null{}},
		ObjectMember{Key: "list", Value: must(NewArray(Bool(true), must(NewArray())))},
	))
	if err := document.put([]pathPart{segmentKey("k")}, object, nil, []byte{}); err != nil {
		t.Fatal(err)
	}
	want := `{"k":{"big":1e+16,"bool":false,"float":-0.0,"int":-12,"list":[true,[]],"null":null,"text":"line\nbreak\u0001\"é"}}`
	expectJSONText(t, "encoded", document, want)
}

func TestJSONTemporalScalarsAreNotSerializable(t *testing.T) {
	date := must(NewDate(2026, 1, 2))
	clock := must(NewTime(1, 2, 3, 0))
	document := mustJSONDocument(t, `{"x":1}`, false)
	rows := []struct {
		value   Value
		message string
	}{
		{date, "Object of type date is not JSON serializable"},
		{clock, "Object of type time is not JSON serializable"},
		{NewLocalDateTime(date, clock), "Object of type datetime is not JSON serializable"},
	}
	for _, row := range rows {
		err := document.put([]pathPart{segmentKey("x")}, row.value, nil, []byte{})
		if failure, ok := asValueError(err); !ok || !failure.pythonTypeError || failure.Message != row.message {
			t.Errorf("error %v, want type error %q", err, row.message)
		}
	}
	expectJSONText(t, "unchanged", document, `{"x":1}`)
}

func setStructured(t *testing.T, data []byte, exists bool, content StructuredContent, acquisition Acquisition) DocumentEdit {
	t.Helper()
	return mustEdit(t, data, exists, []DocumentRequest{mustSetRequest(t, content, acquisition)}, nil, DocumentProvenance{})
}

func setStructuredError(t *testing.T, data []byte, exists bool, content StructuredContent, acquisition Acquisition) string {
	t.Helper()
	_, err := EditDocument(data, exists, []DocumentRequest{mustSetRequest(t, content, acquisition)}, nil, DocumentProvenance{})
	return errorText(err)
}

func structuredParts(t *testing.T, format DocumentFormat, value Value, parts ...SelectorPart) StructuredContent {
	t.Helper()
	return must(NewStructuredContent(format, must(NewSelector(parts...)), value, nil))
}

func TestMalformedJSONEditorInputIsRejected(t *testing.T) {
	rows := []struct {
		format  DocumentFormat
		raw     string
		message string
	}{
		{FormatJSON, `{"a":1,"a":2}`, "duplicate JSON object key"},
		{FormatJSON, `{"a":NaN}`, "floats must be finite"},
		{FormatJSON, `{"a":1e400}`, "floats must be finite"},
		{FormatJSON, `{"a":9223372036854775808}`, "integers must fit signed 64 bits"},
		{FormatJSON, `{"a":1,}`, "JSON trailing commas are unsupported"},
		{FormatJSON, "{/* c */}", "Expecting value: line 1 column 2 (char 1)"},
		{FormatJSONC, `{"a":1,"a":1}`, "duplicate JSON object key"},
		{FormatJSONC, "{/*oops", "unterminated JSONC comment"},
		{FormatJSONC, `{"a":1} trailing`, "unexpected data after JSON document"},
		{FormatJSONC, "{1:2}", "JSON object keys must be strings"},
		{FormatJSONC, `{"a" 1}`, "missing JSON object colon"},
		{FormatJSONC, "[", "unterminated or malformed JSON container"},
		{FormatJSONC, "[]]", "unexpected data after JSON document"},
		{FormatJSONC, `{"a":"\ud800"}`, "scalar must contain Unicode scalar values"},
		{FormatJSONC, "// no root", "missing JSON value"},
		{FormatJSON, "{\"a\":\"\xff\"}", "'utf-8' codec can't decode byte 0xff in position 6: invalid start byte"},
		{FormatJSON, strings.Repeat("[", 1200) + strings.Repeat("]", 1200), documentNesting},
	}
	for _, row := range rows {
		content := structuredKeys(t, row.format, Int(1), "mcp", "x")
		if got := setStructuredError(t, []byte(row.raw), true, content, AcquireConflict); got != row.message {
			t.Errorf("%s %q: error %q, want %q", row.format, row.raw, got, row.message)
		}
	}
}

func TestJSONSelectorPathsMustMatchTheDocument(t *testing.T) {
	member := must(NewMember(String{text: "a"}, nil))
	rows := []struct {
		raw     string
		content StructuredContent
		message string
	}{
		{`{"x":1}`, structuredKeys(t, FormatJSON, Int(1), "x", "y"), "object key selector crosses a non-object"},
		{`{"x":{}}`, structuredParts(t, FormatJSON, Int(1), must(NewKey("x")), member), "member selector requires an array"},
		{"{}", structuredParts(t, FormatJSON, Int(1), member), "member selector requires an array"},
		{"1", structuredKeys(t, FormatJSON, Int(1), "x"), "object key selector crosses a non-object"},
	}
	for _, row := range rows {
		if got := setStructuredError(t, []byte(row.raw), true, row.content, AcquireConflict); got != row.message {
			t.Errorf("%q: error %q, want %q", row.raw, got, row.message)
		}
	}
	missing := setStructured(t, []byte(`{"x":[]}`), true,
		structuredParts(t, FormatJSON, Int(1), must(NewKey("x")), member, must(NewKey("y"))), AcquireConflict)
	if missing.Applicable() || missing.Observations[0].Message != "intermediate array member is missing" {
		t.Errorf("missing member observations %+v", missing.Observations)
	}
	expectAfter(t, "missing member", missing, []byte(`{"x":[]}`))
}

func TestJSONOwnershipComparesSemanticallyButJSONCComparesComments(t *testing.T) {
	value := must(NewObject(ObjectMember{Key: "v", Value: Int(1)}))
	first := setStructured(t, []byte(`{"x":{"v":1}}`), true, structuredKeys(t, FormatJSON, value, "x"), AcquireAdopt)
	expectAfter(t, "json whitespace", retireAll(t, first, []byte(`{"x":{ "v" : 1 }}`), true), []byte("{}"))

	commented := setStructured(t, []byte(`{"x":{"v":1}}`), true, structuredKeys(t, FormatJSONC, value, "x"), AcquireAdopt)
	edited := []byte(`{"x":{"v":1 /* changed */}}`)
	result := retireAll(t, commented, edited, true)
	if result.Observations[0].Status != EditModified || !ownershipEqual(result.Ownership, commented.Ownership) {
		t.Errorf("jsonc comment change: %v", statuses(result))
	}
	expectAfter(t, "jsonc comment change", result, edited)
}

func TestJSONCKeyTriviaIsForeign(t *testing.T) {
	rows := []struct{ raw, want string }{
		{`{"x": 1 /* foreign */}`, `{  /* foreign */}`},
		{`{"x" /* before colon */ : /* before value */ 1, "foreign":2}`, `{ /* before colon */  /* before value */  "foreign":2}`},
	}
	for _, row := range rows {
		first := setStructured(t, []byte(row.raw), true, structuredKeys(t, FormatJSONC, Int(1), "x"), AcquireAdopt)
		expectAfter(t, row.raw, retireAll(t, first, first.After, true), []byte(row.want))
	}
}

func TestJSONCreatedDocumentsAreRemovedWithTheirLastOwner(t *testing.T) {
	for _, format := range []DocumentFormat{FormatJSON, FormatJSONC} {
		first := setStructured(t, nil, false, structuredKeys(t, format, Null{}, "x"), AcquireConflict)
		expectAfter(t, string(format), first, []byte(`{"x":null}`))
		typed, ok := first.Ownership[0].Installed().Value().(SnapshotTyped)
		if !ok || !ValuesEqual(typed.Value(), Null{}) {
			t.Errorf("%s: installed %#v", format, first.Ownership[0].Installed().Value())
		}
		if removed := retireAll(t, first, first.After, true); removed.AfterExists {
			t.Errorf("%s: removed document still exists: %q", format, removed.After)
		}
	}
	member := must(NewMember(Int(1), nil))
	array := setStructured(t, nil, false, structuredParts(t, FormatJSON, Int(1), member), AcquireConflict)
	expectAfter(t, "root array", array, []byte("[1]"))
	if removed := retireAll(t, array, array.After, true); removed.AfterExists {
		t.Errorf("root array still exists: %q", removed.After)
	}
}

func TestJSONTemporalValuesConflict(t *testing.T) {
	date := must(NewDate(2026, 1, 2))
	clock := must(NewTime(1, 2, 3, 0))
	for _, format := range []DocumentFormat{FormatJSON, FormatJSONC} {
		for _, value := range []Value{date, clock, NewLocalDateTime(date, clock)} {
			result := setStructured(t, nil, false, structuredKeys(t, format, value, "x"), AcquireConflict)
			observation := result.Observations[0]
			if result.Applicable() || result.AfterExists || len(result.Ownership) != 0 ||
				observation.Status != EditConflict || observation.Current != nil ||
				!strings.HasSuffix(observation.Message, "is not JSON serializable") {
				t.Errorf("%s %T: %+v", format, value, result)
			}
		}
	}
}

func deepJSON(depth int, alternate bool) string {
	value := "1"
	for level := range depth {
		if alternate && level%2 == 1 {
			value = "[" + value + "]"
		} else {
			value = `{"a":` + value + "}"
		}
	}
	return value
}

func deepJSONArray(depth int) string {
	return strings.Repeat("[", depth) + "1" + strings.Repeat("]", depth)
}

func deepValue(t *testing.T, depth int, array bool) Value {
	t.Helper()
	var value Value = Int(1)
	for range depth {
		if array {
			value = must(NewArray(value))
		} else {
			value = must(NewObject(ObjectMember{Key: "a", Value: value}))
		}
	}
	return value
}

func TestJSONNestingMatchesTheReferenceRecursionBudget(t *testing.T) {
	content := structuredKeys(t, FormatJSON, Int(2), "a")
	snapshots := []struct {
		value    string
		accepted bool
	}{
		{deepJSON(197, false), true}, {deepJSON(198, false), false},
		{deepJSONArray(247), true}, {deepJSONArray(248), false},
		{deepJSON(219, true), true}, {deepJSON(220, true), false},
	}
	for index, row := range snapshots {
		raw := []byte(`{"a":` + row.value + "}")
		if row.accepted {
			expectAfter(t, fmt.Sprint("snapshot ", index), setStructured(t, raw, true, content, AcquireTakeover), []byte(`{"a":2}`))
		} else if got := setStructuredError(t, raw, true, content, AcquireTakeover); got != documentNesting {
			t.Errorf("snapshot %d: error %q", index, got)
		}
	}
	parses := []struct {
		chain    string
		accepted bool
	}{
		{deepJSONArray(984), true}, {deepJSONArray(985), false},
		{deepJSON(984, false), true}, {deepJSON(985, false), false},
	}
	for index, row := range parses {
		raw := []byte(`{"a":1,"b":` + row.chain + "}")
		if row.accepted {
			want := []byte(`{"a":2,"b":` + row.chain + "}")
			expectAfter(t, fmt.Sprint("parse ", index), setStructured(t, raw, true, content, AcquireTakeover), want)
		} else if got := setStructuredError(t, raw, true, content, AcquireTakeover); got != documentNesting {
			t.Errorf("parse %d: error %q", index, got)
		}
	}
	requested := []struct {
		value Value
		want  string
	}{
		{deepValue(t, 197, false), `{"x":` + deepJSON(197, false) + "}"},
		{deepValue(t, 198, false), ""},
		{deepValue(t, 247, true), `{"x":` + deepJSONArray(247) + "}"},
		{deepValue(t, 248, true), ""},
	}
	for index, row := range requested {
		created := structuredKeys(t, FormatJSON, row.value, "x")
		if row.want != "" {
			expectAfter(t, fmt.Sprint("requested ", index), setStructured(t, nil, false, created, AcquireConflict), []byte(row.want))
		} else if got := setStructuredError(t, nil, false, created, AcquireConflict); got != documentNesting {
			t.Errorf("requested %d: error %q", index, got)
		}
	}
}

func FuzzJSONDocumentNoOpEdit(f *testing.F) {
	for _, seed := range []string{
		`{"x": 1 /* foreign */}`, "[1, /* two */ 2,]", `{"a":[1,2.5,"x",null,true,{"b":"é"}],"c":{}}`,
		`{"\ud83d\ude00":"\u00e9\n"}`, "// c\n{\"a\" // k\n:1}", `{"a":1,"a":2}`, "[[[[]]]]", `{"a":-0.0e+1}`,
		strings.Repeat("{", 5000), `{"a":1,` + strings.Repeat("{", 5000),
	} {
		f.Add([]byte(seed), true)
		f.Add([]byte(seed), false)
	}
	if data, err := readFixture("edits.json"); err == nil {
		if fixtures, err := jsonx.Parse(data); err == nil {
			cases, _ := fixtures.Get("cases")
			items, _ := cases.Items()
			for _, item := range items {
				format, _ := editRecordText(item, "format")
				if before, ok := editRecordText(item, "before"); ok && (format == "json" || format == "jsonc") {
					f.Add([]byte(before), format == "jsonc")
				}
			}
		}
	}
	f.Fuzz(func(t *testing.T, data []byte, comments bool) {
		format := FormatJSON
		if comments {
			format = FormatJSONC
		}
		text, err := decodeUTF8(data)
		if err != nil || bytes.IndexByte(data, 0) >= 0 {
			return
		}
		document, err := parseJSONDocument(text, comments)
		if err != nil {
			if _, ok := asValueError(err); !ok {
				t.Fatalf("parse error %T, want *ValueError", err)
			}
			return
		}
		object, ok := document.root.value.(Object)
		if !ok || object.Len() == 0 {
			return
		}
		name, value := object.members[0].Key, object.members[0].Value
		path := []pathPart{segmentKey(name)}
		rewritten := document.clone()
		if err := rewritten.put(path, value, must(document.syntax(path)), []byte{}); err != nil || rewritten.text() != text {
			t.Fatalf("rewriting %q with its own syntax gave %q: %v", name, rewritten.text(), err)
		}
		key, err := NewKey(name)
		if err != nil {
			return
		}
		content, err := NewStructuredContent(format, must(NewSelector(key)), value, nil)
		if err != nil {
			return
		}
		edit, err := EditDocument(data, true, []DocumentRequest{mustSetRequest(t, content, AcquireAdopt)}, nil, DocumentProvenance{})
		if err != nil {
			if failure, ok := asValueError(err); !ok || !failure.nesting {
				t.Fatalf("adopting %q: %v", name, err)
			}
			return
		}
		if !edit.Applicable() || !edit.AfterExists || !bytes.Equal(edit.After, data) {
			t.Fatalf("adopting %q changed %q into %q (%v)", name, data, edit.After, statuses(edit))
		}
		retained := mustEdit(t, edit.After, true, nil, edit.Ownership, edit.Provenance)
		if !retained.AfterExists || !bytes.Equal(retained.After, data) {
			t.Fatalf("observing %q changed it into %q", data, retained.After)
		}
	})
}

func hookEntry(t *testing.T, name string, v int) Value {
	t.Helper()
	return must(NewObject(ObjectMember{Key: "name", Value: String{text: name}}, ObjectMember{Key: "v", Value: Int(int64(v))}))
}

func hookContent(t *testing.T, format DocumentFormat, value Value, identity Value, key []string, position *Position) StructuredContent {
	t.Helper()
	parts := []SelectorPart{must(NewKey("hooks"))}
	if identity != nil {
		parts = append(parts, must(NewMember(identity, key)))
	}
	var placement *Placement
	if position != nil {
		built := must(NewPlacement(*position, nil))
		placement = &built
	}
	return must(NewStructuredContent(format, must(NewSelector(parts...)), value, placement))
}

func swapAB(text []byte) []byte {
	swapped := bytes.ReplaceAll(text, []byte(`"a"`), []byte(`"temp"`))
	swapped = bytes.ReplaceAll(swapped, []byte(`"b"`), []byte(`"a"`))
	return bytes.ReplaceAll(swapped, []byte(`"temp"`), []byte(`"b"`))
}

func baselineOf(t *testing.T, edit DocumentEdit) SelectionSnapshot {
	t.Helper()
	baseline, ok := edit.Ownership[0].Baseline()
	if !ok {
		t.Fatal("ownership has no baseline")
	}
	return baseline
}

func TestJSONTakeoverKeepsTheFirstSyntaxBaselineAcrossUpdates(t *testing.T) {
	rows := []struct {
		format DocumentFormat
		raw    string
	}{
		{FormatJSON, `{"foreign":true,"mcp":{"server":{"command":"old"}}}`},
		{FormatJSONC, `{ /* top */ "mcp": { "server": {"command" : "old" /* owned */} }, "foreign":true }`},
	}
	object := func(command string) Value {
		return must(NewObject(ObjectMember{Key: "command", Value: String{text: command}}))
	}
	for _, row := range rows {
		content := structuredKeys(t, row.format, object("new"), "mcp", "server")
		conflict := setStructured(t, []byte(row.raw), true, content, AcquireConflict)
		if got := statuses(conflict); len(got) != 1 || got[0] != EditConflict {
			t.Errorf("%s conflict statuses %v", row.format, got)
		}
		first := setStructured(t, []byte(row.raw), true, content, AcquireTakeover)
		if !first.Applicable() {
			t.Fatalf("%s takeover not applicable", row.format)
		}
		newer := structuredKeys(t, row.format, object("newer"), "mcp", "server")
		second := mustEdit(t, first.After, true, []DocumentRequest{mustSetRequest(t, newer, AcquireConflict)}, first.Ownership, first.Provenance)
		third := mustEdit(t, second.After, true, []DocumentRequest{mustSetRequest(t, content, AcquireConflict)}, second.Ownership, second.Provenance)
		if !baselineOf(t, third).equal(baselineOf(t, first)) {
			t.Errorf("%s: baseline changed across updates", row.format)
		}
		removed := retireAll(t, third, third.After, true)
		expectAfter(t, string(row.format)+" retire", removed, []byte(row.raw))
		again := retireAll(t, removed, removed.After, true)
		if len(again.Ownership) != 0 {
			t.Errorf("%s: ownership after second retirement %v", row.format, again.Ownership)
		}
	}
}

const hookRaw = `{"hooks":[{"name":"a"},{"name":"x","v":1},{"name":"b"}]}`

func TestJSONArrayTakeoverRestoresNativeIdentityPayloadAndOriginalPosition(t *testing.T) {
	last := PositionLast
	for _, format := range []DocumentFormat{FormatJSON, FormatJSONC} {
		identity := String{text: "x"}
		first := setStructured(t, []byte(hookRaw), true,
			hookContent(t, format, hookEntry(t, "x", 2), identity, []string{"name"}, &last), AcquireTakeover)
		if !first.Applicable() {
			t.Fatalf("%s: takeover not applicable", format)
		}
		next := hookContent(t, format, hookEntry(t, "x", 3), identity, []string{"name"}, nil)
		second := mustEdit(t, first.After, true, []DocumentRequest{mustSetRequest(t, next, AcquireConflict)}, first.Ownership, first.Provenance)
		if !baselineOf(t, second).equal(baselineOf(t, first)) {
			t.Errorf("%s: baseline changed by update", format)
		}
		expectAfter(t, string(format)+" retire", retireAll(t, second, second.After, true), []byte(hookRaw))
		swapped := swapAB(second.After)
		blocked := retireAll(t, second, swapped, true)
		if blocked.Applicable() || !bytes.Equal(blocked.After, swapped) || !ownershipEqual(blocked.Ownership, second.Ownership) {
			t.Errorf("%s: swapped neighbours retired: %v", format, statuses(blocked))
		}
	}
}

func TestJSONArrayRestoreWithSurvivingAnchorAndMissingPositionConflict(t *testing.T) {
	for _, format := range []DocumentFormat{FormatJSON, FormatJSONC} {
		first := setStructured(t, []byte(hookRaw), true,
			hookContent(t, format, hookEntry(t, "x", 2), String{text: "x"}, []string{"name"}, nil), AcquireTakeover)
		missing := bytes.ReplaceAll(first.After, []byte(`{"name":"a"},`), nil)
		result := retireAll(t, first, missing, true)
		want := bytes.ReplaceAll([]byte(hookRaw), []byte(`{"name":"a"},`), nil)
		if !result.Applicable() {
			t.Errorf("%s: dropped neighbour blocked retirement", format)
		}
		expectAfter(t, string(format)+" dropped neighbour", result, want)
		renamed := bytes.ReplaceAll(bytes.ReplaceAll(first.After, []byte(`"a"`), []byte(`"c"`)), []byte(`"b"`), []byte(`"d"`))
		blocked := retireAll(t, first, renamed, true)
		if blocked.Applicable() || !ownershipEqual(blocked.Ownership, first.Ownership) {
			t.Errorf("%s: renamed neighbours retired: %v", format, statuses(blocked))
		}
	}
}

func TestJSONCreatedContainerWithUserWhitespaceIsRetained(t *testing.T) {
	first := setStructured(t, nil, false, structuredKeys(t, FormatJSONC, Int(1), "mcp", "x"), AcquireConflict)
	foreign := bytes.ReplaceAll(first.After, []byte(`{"x"`), []byte(`{  "x"`))
	result := retireAll(t, first, foreign, true)
	expectAfter(t, "retained whitespace", result, []byte(`{"mcp":{  }}`))
	if !result.Provenance.equal(DocumentProvenance{}) {
		t.Errorf("provenance %v, want empty", result.Provenance)
	}
}

func TestJSONArrayMovesRestoreOriginalTrivia(t *testing.T) {
	raw := `{"hooks":[ {"name":"a"}, /* x */ {"name":"x"}, /* b */ {"name":"b"} ]}`
	for _, selected := range []string{"a", "x", "b"} {
		for _, position := range []Position{PositionFirst, PositionLast} {
			content := hookContent(t, FormatJSONC, hookEntry(t, selected, 2), String{text: selected}, []string{"name"}, &position)
			first := setStructured(t, []byte(raw), true, content, AcquireTakeover)
			if !first.Applicable() {
				t.Fatalf("%s %s: takeover not applicable", selected, position)
			}
			result := retireAll(t, first, first.After, true)
			if !result.Applicable() {
				t.Errorf("%s %s: retirement not applicable", selected, position)
			}
			expectAfter(t, selected+" "+string(position), result, []byte(raw))
		}
	}
}

func TestJSONChangedRestorationGapPreservesForeignRevision(t *testing.T) {
	raw := `{"hooks":[{"name":"a"}, /* original */ {"name":"x"},{"name":"b"}]}`
	last := PositionLast
	first := setStructured(t, []byte(raw), true,
		hookContent(t, FormatJSONC, hookEntry(t, "x", 2), String{text: "x"}, []string{"name"}, &last), AcquireTakeover)
	if !first.Applicable() {
		t.Fatal("takeover not applicable")
	}
	changed := bytes.ReplaceAll(first.After, []byte("original"), []byte("changed"))
	result := retireAll(t, first, changed, true)
	if got := statuses(result); len(got) != 1 || got[0] != EditConflict {
		t.Errorf("statuses %v, want conflict", got)
	}
	if !bytes.Equal(result.After, changed) || !ownershipEqual(result.Ownership, first.Ownership) {
		t.Errorf("foreign revision not preserved: %q", result.After)
	}
}

func TestJSONDocumentsRequireCompleteRoots(t *testing.T) {
	content := structuredKeys(t, FormatJSON, Int(1), "x")
	if got := setStructuredError(t, []byte("1"), true, content, AcquireConflict); got != "object key selector crosses a non-object" {
		t.Errorf("scalar root error %q", got)
	}
	first := setStructured(t, nil, false, content, AcquireConflict)
	bad := structuredParts(t, FormatJSON, Int(3), must(NewKey("bad")), must(NewMember(Int(4), nil)))
	requests := []DocumentRequest{
		mustSetRequest(t, structuredKeys(t, FormatJSON, Int(2), "x"), AcquireConflict),
		mustSetRequest(t, bad, AcquireConflict),
	}
	changed := mustEdit(t, first.After, true, requests, first.Ownership, first.Provenance)
	if changed.Applicable() || !bytes.Equal(changed.After, first.After) || !ownershipEqual(changed.Ownership, first.Ownership) {
		t.Errorf("failed batch changed the document: %q %v", changed.After, statuses(changed))
	}
}

func TestJSONAnchorNeighboursMatchTheReferenceRecursionBudget(t *testing.T) {
	named := structuredParts(t, FormatJSON, must(NewObject(ObjectMember{Key: "name", Value: String{text: "c"}})),
		must(NewKey("hooks")), must(NewMember(String{text: "b"}, []string{"name"})))
	whole := structuredParts(t, FormatJSON, String{text: "c"},
		must(NewKey("hooks")), must(NewMember(String{text: "b"}, nil)))
	withNeighbour := func(value string) string { return `{"hooks":[` + value + `,{"name":"b"}]}` }
	withWholeNeighbour := func(value string) string { return `{"hooks":[` + value + `,"b"]}` }
	withNamedNeighbour := func(value string) string { return `{"hooks":[{"name":` + value + `},{"name":"b"}]}` }
	rows := []struct {
		document string
		content  StructuredContent
		nesting  bool
	}{
		{withNeighbour(deepJSON(196, false)), named, false},
		{withNeighbour(deepJSON(197, false)), named, true},
		{withNeighbour(deepJSON(198, false)), named, true},
		{withNeighbour(deepJSONArray(246)), named, false},
		{withNeighbour(deepJSONArray(247)), named, true},
		{withWholeNeighbour(deepJSON(196, false)), whole, false},
		{withWholeNeighbour(deepJSON(197, false)), whole, true},
		{withWholeNeighbour(deepJSONArray(246)), whole, false},
		{withWholeNeighbour(deepJSONArray(247)), whole, true},
		{withNamedNeighbour(deepJSON(195, false)), named, false},
		{withNamedNeighbour(deepJSON(196, false)), named, true},
		{`{"hooks":[{"name":{"nested":1}},{"name":"b"}]}`, named, false},
	}
	for index, row := range rows {
		if row.nesting {
			if got := setStructuredError(t, []byte(row.document), true, row.content, AcquireTakeover); got != documentNesting {
				t.Errorf("row %d: error %q, want %q", index, got, documentNesting)
			}
			continue
		}
		edit := setStructured(t, []byte(row.document), true, row.content, AcquireTakeover)
		if edit.Applicable() || !bytes.Equal(edit.After, []byte(row.document)) {
			t.Errorf("row %d: edit applied or changed the document", index)
		}
		if edit.Observations[0].Status != EditConflict || edit.Observations[0].Message != "generated array member does not match its selector" {
			t.Errorf("row %d: observation %v %q", index, edit.Observations[0].Status, edit.Observations[0].Message)
		}
	}
}

func TestJSONStructuredEditResultsAndInputsAreImmutable(t *testing.T) {
	array := func(n int64) Value { return must(NewObject(ObjectMember{Key: "x", Value: must(NewArray(Int(n)))})) }
	first := setStructured(t, nil, false, structuredKeys(t, FormatJSON, array(1), "mcp"), AcquireConflict)
	ownership := slices.Clone(first.Ownership)
	provenance := first.Provenance
	expectAfter(t, "first", first, []byte(`{"mcp":{"x":[1]}}`))
	update := mustSetRequest(t, structuredKeys(t, FormatJSON, array(2), "mcp"), AcquireConflict)
	second := mustEdit(t, first.After, true, []DocumentRequest{update}, first.Ownership, first.Provenance)
	expectAfter(t, "second", second, []byte(`{"mcp":{"x":[2]}}`))
	expectAfter(t, "first unchanged", first, []byte(`{"mcp":{"x":[1]}}`))
	if !ownershipEqual(first.Ownership, ownership) || !first.Provenance.equal(provenance) {
		t.Error("update changed the previous result")
	}
}
