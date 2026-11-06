package jsonx

import (
	"bytes"
	"encoding/json/jsontext"
	"errors"
	"io"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func parseMessage(t *testing.T, text string) string {
	t.Helper()
	_, err := Parse([]byte(text))
	var failure *Error
	if !errors.As(err, &failure) {
		t.Fatalf("Parse(%q) = %v, want *Error", text, err)
	}
	return failure.Message
}

func TestParseReencodesNestedDocuments(t *testing.T) {
	text := `{"a":[1,2.5,"x",null,true,{"b":"é"}],"c":{}}`
	value, err := Parse([]byte(text))
	if err != nil {
		t.Fatal(err)
	}
	encoded := Encode(value)
	if string(encoded) != text {
		t.Fatalf("Encode = %s, want %s", encoded, text)
	}
	reparsed, err := Parse(encoded)
	if err != nil || !Equal(reparsed, value) {
		t.Fatalf("reparse = %v, %v", reparsed, err)
	}
}

func TestParseReportsReferenceMessages(t *testing.T) {
	cases := []struct{ text, message string }{
		{``, "JSON contains an unexpected character"},
		{`{`, "JSON object keys must be strings"},
		{`[1,]`, "JSON contains an unexpected character"},
		{`{"a"}`, "JSON object is missing a colon"},
		{`{"a":1 "b":2}`, "JSON object is missing a separator"},
		{`[1 2]`, "JSON array is missing a separator"},
		{`01`, "JSON number has a leading zero"},
		{`1.`, "JSON number has an empty fraction"},
		{`1e`, "JSON number has an empty exponent"},
		{`-`, "JSON number is malformed"},
		{`tru`, "JSON contains an invalid literal"},
		{`nul`, "JSON contains an invalid literal"},
		{`NaN`, "non-JSON numeric constant: NaN"},
		{`[NaN]`, "non-JSON numeric constant: NaN"},
		{`[Infinity]`, "non-JSON numeric constant: Infinity"},
		{`[-Infinity]`, "non-JSON numeric constant: -Infinity"},
		{`Nope`, "JSON contains an unexpected character"},
		{`"a" "b"`, "JSON has trailing content"},
		{`[1] x`, "JSON has trailing content"},
		{"\"\x01\"", "JSON string contains a control character"},
		{`"abc`, "JSON string is unterminated"},
		{`"\`, "JSON escape is unterminated"},
		{`"\x"`, "JSON contains an invalid escape"},
		{`"\u12zz"`, "JSON unicode escape is not hexadecimal"},
		{`"\u1`, "JSON unicode escape is truncated"},
		{`"\ud800"`, "JSON escape contains an unpaired surrogate"},
		{`"\udc00"`, "JSON escape contains an unpaired surrogate"},
		{`"\ud800\ud800"`, "JSON escape contains an unpaired surrogate"},
		{"\"\xff\"", "JSON string contains invalid UTF-8"},
		{"\"\xed\xa0\x80\"", "JSON string contains invalid UTF-8"},
		{"\"\xe2\x82\"", "JSON string contains invalid UTF-8"},
		{`{"a":1,"a":2}`, "duplicate JSON field: a"},
		{`{"a":{"b":1,"b":2}}`, "duplicate JSON field: b"},
		{`{"a":1,"a":[1,]}`, "JSON contains an unexpected character"},
		{`{"a":1,"\u0061":2}`, "duplicate JSON field: a"},
	}
	for _, c := range cases {
		if got := parseMessage(t, c.text); got != c.message {
			t.Errorf("Parse(%q) message = %q, want %q", c.text, got, c.message)
		}
	}
}

func TestParseLimitsNestingDepth(t *testing.T) {
	accepted := strings.Repeat("[", MaxDepth+1) + strings.Repeat("]", MaxDepth+1)
	if _, err := Parse([]byte(accepted)); err != nil {
		t.Fatalf("Parse(%d nested arrays) = %v", MaxDepth+1, err)
	}
	rejected := strings.Repeat("[", MaxDepth+2) + strings.Repeat("]", MaxDepth+2)
	if got := parseMessage(t, rejected); got != "JSON nesting is too deep" {
		t.Fatalf("Parse(%d nested arrays) message = %q", MaxDepth+2, got)
	}
}

func TestParsePairsSurrogateEscapesIntoScalars(t *testing.T) {
	value, err := Parse([]byte(`"\ud83d\ude00"`))
	text, ok := value.AsString()
	if err != nil || !ok || text != "\U0001F600" {
		t.Fatalf("Parse = %q, %v", text, err)
	}
}

func TestParseKeepsNumbersAsReferenceKinds(t *testing.T) {
	value, err := Parse([]byte(`[1e400,-1e400,9223372036854775807,9223372036854775808,-9223372036854775809,-0,1e-400]`))
	if err != nil {
		t.Fatal(err)
	}
	items, _ := value.Items()
	if f, ok := items[0].AsFloat(); !ok || !math.IsInf(f, 1) {
		t.Errorf("1e400 = %v, want +Inf", items[0])
	}
	if f, ok := items[1].AsFloat(); !ok || !math.IsInf(f, -1) {
		t.Errorf("-1e400 = %v, want -Inf", items[1])
	}
	if i, ok := items[2].AsInt(); !ok || i != math.MaxInt64 {
		t.Errorf("max int = %v", items[2])
	}
	if text, ok := items[3].BigIntText(); !ok || text != "9223372036854775808" {
		t.Errorf("beyond int64 = %v", items[3])
	}
	if i, ok := items[5].AsInt(); !ok || i != 0 {
		t.Errorf("-0 = %v, want integer 0", items[5])
	}
	if f, ok := items[6].AsFloat(); !ok || f != 0 {
		t.Errorf("1e-400 = %v, want 0", items[6])
	}
	if got := string(Encode(Array(items[3], items[4]))); got != "[9223372036854775808,-9223372036854775809]" {
		t.Errorf("Encode big integers = %s", got)
	}
}

func TestPythonFloatRepr(t *testing.T) {
	cases := []struct {
		bits     uint64
		expected string
	}{
		{0x0000000000000000, "0.0"},
		{0x8000000000000000, "-0.0"},
		{0x3FF0000000000000, "1.0"},
		{0xBFF0000000000000, "-1.0"},
		{0x4004000000000000, "2.5"},
		{0x3FB999999999999A, "0.1"},
		{0x3FD5555555555555, "0.3333333333333333"},
		{0x430C6BF526340000, "1000000000000000.0"},
		{0x4341C37937E08000, "1e+16"},
		{0x4376345785D8A000, "1e+17"},
		{0x3F1A36E2EB1C432D, "0.0001"},
		{0x3EE4F8B588E368F1, "1e-05"},
		{0x3E7AD7F29ABCAF48, "1e-07"},
		{0x437B69B4BA630F35, "1.2345678901234568e+17"},
		{0x0000000000000001, "5e-324"},
		{0x7FEFFFFFFFFFFFFF, "1.7976931348623157e+308"},
		{0x0010000000000000, "2.2250738585072014e-308"},
		{0x4059000000000000, "100.0"},
		{0x3FE0000000000000, "0.5"},
		{0x54B249AD2594C37D, "1e+100"},
		{0xAB2BFF2EE48E0530, "-1e-100"},
		{0x4132D687E4189375, "1234567.891"},
		{0x444B1AE4D6E2EF50, "1e+21"},
		{0x400921FB54442D11, "3.14159265358979"},
		{0x7FF8000000000000, "NaN"},
		{0x7FF0000000000000, "Infinity"},
		{0xFFF0000000000000, "-Infinity"},
	}
	for _, c := range cases {
		value := math.Float64frombits(c.bits)
		if got := string(Encode(Float(value))); got != c.expected {
			t.Errorf("Encode(%#x) = %s, want %s", c.bits, got, c.expected)
		}
	}
}

func TestEncodeEscapesLikePython(t *testing.T) {
	text := "a\u0001b\"\\\b\f\n\r\t\u007fé\U0001F600"
	if got := string(Encode(String(text))); got != "\"a\\u0001b\\\"\\\\\\b\\f\\n\\r\\t\u007fé\U0001F600\"" {
		t.Errorf("Encode = %s", got)
	}
	if got := string(EncodePythonCompact(String(text))); got != `"a\u0001b\"\\\b\f\n\r\t`+"\u007f"+`\u00e9\ud83d\ude00"` {
		t.Errorf("EncodePythonCompact = %s", got)
	}
	document := Object(Member{Key: "k", Value: Array(Int(1), Bool(false), Null())}, Member{Key: "é", Value: Float(1.5)})
	if got := string(EncodePython(document)); got != `{"k": [1, false, null], "\u00e9": 1.5}` {
		t.Errorf("EncodePython = %s", got)
	}
}

func TestPythonRepr(t *testing.T) {
	cases := []struct {
		value    Value
		expected string
	}{
		{String("plain"), `'plain'`},
		{String("it's"), `"it's"`},
		{String(`it's "x"`), `'it\'s "x"'`},
		{String("a\\b\n\r\t\x01\x7fé"), `'a\\b\n\r\t\x01\x7fé'`},
		{Array(Null(), Bool(true), Bool(false), Int(-3), Float(0.5), BigInt("99999999999999999999")), `[None, True, False, -3, 0.5, 99999999999999999999]`},
		{Object(Member{Key: "a", Value: Array()}, Member{Key: "b", Value: Object()}), `{'a': [], 'b': {}}`},
	}
	for _, c := range cases {
		if got := PythonRepr(c.value); got != c.expected {
			t.Errorf("PythonRepr = %s, want %s", got, c.expected)
		}
	}
}

func jsontextAccepts(data []byte) bool {
	decoder := jsontext.NewDecoder(bytes.NewReader(data))
	if _, err := decoder.ReadValue(); err != nil {
		return false
	}
	_, err := decoder.ReadToken()
	return errors.Is(err, io.EOF)
}

func seedDocuments(f *testing.F) {
	for _, seed := range []string{
		``, `null`, `true`, `[1,2.5,-0,1e400]`, `{"a":{"b":[]}}`, `{"a":1,"a":2}`, `"\ud83d\ude00"`,
		`"\ud800"`, "\"\xff\"", `[NaN]`, `01`, `[1,]`, `{"a" 1}`, ` [ ] `, `9223372036854775808`,
	} {
		f.Add([]byte(seed))
	}
	matches, _ := filepath.Glob(filepath.Join("..", "..", "..", "spec", "fixtures", "*.json"))
	for _, match := range matches {
		if data, err := os.ReadFile(match); err == nil {
			f.Add(data)
		}
	}
}

func FuzzParse(f *testing.F) {
	seedDocuments(f)
	f.Fuzz(func(t *testing.T, data []byte) {
		value, err := Parse(data)
		var failure *Error
		if err != nil && !errors.As(err, &failure) {
			t.Fatalf("Parse error %T, want *Error", err)
		}
		if err == nil && !bytes.Contains(Encode(value), []byte("Infinity")) {
			again, err := Parse(Encode(value))
			if err != nil || !bytes.Equal(Encode(again), Encode(value)) {
				t.Fatalf("Encode(Parse(%q)) does not round-trip: %v", data, err)
			}
		}
		// The reference reader rejects leading whitespace and nests 512 levels; jsontext allows both.
		if (failure != nil && failure.Message == "JSON nesting is too deep") || bytes.IndexAny(data[:min(len(data), 1)], " \t\r\n") == 0 {
			return
		}
		if accepted := err == nil; accepted != jsontextAccepts(data) {
			t.Fatalf("Parse(%q) accepted=%v, jsontext disagrees (%v)", data, accepted, err)
		}
	})
}
