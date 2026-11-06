package flyrail

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"math"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

func encoded(t *testing.T, v Value) string {
	t.Helper()
	b, err := EncodeValue(v)
	if err != nil {
		t.Fatalf("EncodeValue(%v) = %v", v, err)
	}
	return string(b)
}

func must[T any](value T, err error) T {
	if err != nil {
		panic(err)
	}
	return value
}

func TestEncodesScalarsWithPythonTags(t *testing.T) {
	cases := []struct {
		value    Value
		expected string
	}{
		{Null{}, `["null"]`},
		{Bool(true), `["boolean",true]`},
		{Int(-17), `["integer","-17"]`},
		{Int(math.MaxInt64), `["integer","9223372036854775807"]`},
		{Int(math.MinInt64), `["integer","-9223372036854775808"]`},
		{must(NewFloat(1.5)), `["float","0x1.8000000000000p+0"]`},
		{must(NewFloat(math.Copysign(0, -1))), `["float","-0x0.0p+0"]`},
		{Float{}, `["float","0x0.0p+0"]`},
		{must(NewFloat(math.SmallestNonzeroFloat64)), `["float","0x0.0000000000001p-1022"]`},
		{must(NewFloat(math.Float64frombits(0x000F_FFFF_FFFF_FFFF))), `["float","0x0.fffffffffffffp-1022"]`},
		{must(NewString("é")), "[\"string\",\"é\"]"},
		{String{}, `["string",""]`},
		{mustDate(t, 2026, 1, 2), `["date","2026-01-02"]`},
		{mustTime(t, 1, 2, 3, 0), `["time","01:02:03.000000"]`},
		{must(NewDateTimeWithOffset(mustDate(t, 2026, 1, 2), mustTime(t, 5, 2, 3, 0), 5*3_600)),
			`["datetime","2026-01-02T00:02:03.000000+00:00"]`},
		{NewLocalDateTime(mustDate(t, 2026, 1, 2), mustTime(t, 3, 4, 5, 6)), `["datetime","2026-01-02T03:04:05.000006"]`},
		{Array{}, `["array",[]]`},
		{Object{}, `["object",[]]`},
	}
	for _, c := range cases {
		if got := encoded(t, c.value); got != c.expected {
			t.Errorf("EncodeValue = %s, want %s", got, c.expected)
		}
	}
}

func TestWritesNonASCIIAndDeleteCharactersLiterally(t *testing.T) {
	value := must(NewString("\u007f\u00ad\u2028"))
	if got := encoded(t, value); got != "[\"string\",\"\u007f\u00ad\u2028\"]" {
		t.Fatalf("EncodeValue = %q", got)
	}
}

func TestKeepsObjectMembersSortedByKeyBytes(t *testing.T) {
	object := must(NewObject(
		ObjectMember{Key: "é", Value: Int(1)},
		ObjectMember{Key: "b", Value: Int(2)},
		ObjectMember{Key: "a", Value: Int(3)},
	))
	var keys []string
	for key := range object.All() {
		keys = append(keys, key)
	}
	if diff := cmp.Diff([]string{"a", "b", "é"}, keys); diff != "" {
		t.Errorf("keys (-want +got):\n%s", diff)
	}
	if value, ok := object.Get("b"); !ok || value != Int(2) {
		t.Errorf("Get(b) = %v, %v", value, ok)
	}
	if object.Len() != 3 {
		t.Errorf("Len() = %d", object.Len())
	}
}

func TestConstructorsRejectInvalidInputWithReferenceMessages(t *testing.T) {
	cases := []struct {
		err     error
		message string
	}{
		{errorOf(NewObject(ObjectMember{Key: "a", Value: Int(1)}, ObjectMember{Key: "a", Value: Int(2)})), "duplicate object key"},
		{errorOf(NewFloat(math.NaN())), "floats must be finite"},
		{errorOf(NewFloat(math.Inf(1))), "floats must be finite"},
		{errorOf(NewFloat(math.Inf(-1))), "floats must be finite"},
		{errorOf(NewString("\xed\xa0\x80")), "scalar must contain Unicode scalar values"},
		{errorOf(NewObject(ObjectMember{Key: "\xff", Value: Null{}})), "object key must contain Unicode scalar values"},
		{errorOf(NewArray(nil)), "values must be Scalar, ObjectValue or ArrayValue"},
		{errorOf(NewArray(new(Bool))), "values must be Scalar, ObjectValue or ArrayValue"},
		{errorOf(NewArray(Date{})), "date year must be 1-9999"},
		{errorOf(NewObject(ObjectMember{Key: "d", Value: NewUTCDateTime(Date{}, Time{})})), "date year must be 1-9999"},
		{errorOf(EncodeValue(nil)), "values must be Scalar, ObjectValue or ArrayValue"},
		{errorOf(EncodeValue(Date{})), "date year must be 1-9999"},
	}
	for _, c := range cases {
		var invalid *ValueError
		if !errors.As(c.err, &invalid) || invalid.Message != c.message {
			t.Errorf("error = %v, want *ValueError %q", c.err, c.message)
		}
	}
	if got := must(NewFloat(math.SmallestNonzeroFloat64)).Float64(); got != math.SmallestNonzeroFloat64 {
		t.Errorf("subnormal = %v", got)
	}
}

func TestBoundsValueNesting(t *testing.T) {
	var value Value = Null{}
	for range maxValueDepth {
		value = must(NewArray(value))
	}
	_, err := NewArray(value)
	var invalid *ValueError
	if !errors.As(err, &invalid) || invalid.Message != "value nesting is too deep" || !invalid.nesting {
		t.Fatalf("NewArray beyond %d levels = %v", maxValueDepth, err)
	}
}

func TestArraysAndObjectsCopyTheirInputs(t *testing.T) {
	items := []Value{Int(1), Int(2)}
	array := must(NewArray(items...))
	items[0] = Int(9)
	returned := array.Items()
	returned[1] = Int(9)
	if diff := cmp.Diff([]Value{Int(1), Int(2)}, array.Items()); diff != "" || array.Len() != 2 {
		t.Errorf("array items (-want +got):\n%s", diff)
	}
	members := []ObjectMember{{Key: "b", Value: Int(1)}, {Key: "a", Value: Int(2)}}
	object := must(NewObject(members...))
	members[0].Value = Int(9)
	if value, _ := object.Get("b"); value != Int(1) {
		t.Errorf("object member b = %v", value)
	}
}

func TestDistinguishesScalarTypesWithoutCoercion(t *testing.T) {
	zero := must(NewFloat(0))
	negativeZero := must(NewFloat(math.Copysign(0, -1)))
	one := must(NewFloat(1))
	text := must(NewString("1"))
	unequal := [][2]Value{{Int(1), Bool(true)}, {Int(1), one}, {Int(1), text}, {zero, negativeZero}, {Null{}, Array{}}}
	for _, pair := range unequal {
		if ValuesEqual(pair[0], pair[1]) {
			t.Errorf("ValuesEqual(%v, %v) = true", pair[0], pair[1])
		}
	}
	half := must(NewFloat(0.5))
	if !ValuesEqual(half, must(NewFloat(0.5))) {
		t.Error("ValuesEqual(0.5, 0.5) = false")
	}
	utc := NewUTCDateTime(mustDate(t, 2026, 1, 1), mustTime(t, 0, 0, 0, 0))
	shifted := must(NewDateTimeWithOffset(mustDate(t, 2026, 1, 1), mustTime(t, 1, 0, 0, 0), 3_600))
	if !ValuesEqual(utc, shifted) {
		t.Error("equal instants with different offsets differ")
	}
	if ValuesEqual(utc, NewLocalDateTime(mustDate(t, 2026, 1, 1), mustTime(t, 0, 0, 0, 0))) {
		t.Error("UTC and local datetimes compare equal")
	}
	first := must(NewObject(ObjectMember{Key: "b", Value: Int(2)}, ObjectMember{Key: "a", Value: Int(1)}))
	again := must(NewObject(ObjectMember{Key: "a", Value: Int(1)}, ObjectMember{Key: "b", Value: Int(2)}))
	if !ValuesEqual(first, again) {
		t.Error("member order changes object equality")
	}
}

func TestRejectsSemanticBytesThatAreNotUTF8(t *testing.T) {
	_, err := DecodeValue([]byte{0xFF, 0xFE})
	if err == nil || err.Error() != "semantic values must be UTF-8" {
		t.Fatalf("DecodeValue = %v", err)
	}
}

func TestWritesPythonFloatHexSpellings(t *testing.T) {
	cases := map[float64]string{
		0:                       "0x0.0p+0",
		math.Copysign(0, -1):    "-0x0.0p+0",
		1:                       "0x1.0000000000000p+0",
		math.Float64frombits(1): "0x0.0000000000001p-1022",
		math.MaxFloat64:         "0x1.fffffffffffffp+1023",
		-2.5:                    "-0x1.4000000000000p+1",
	}
	for value, expected := range cases {
		if got := floatHex(value); got != expected {
			t.Errorf("floatHex(%v) = %s, want %s", value, got, expected)
		}
	}
}

func TestRoundTripsFloatHexAcrossTheExponentRange(t *testing.T) {
	samples := []float64{0, math.Copysign(0, -1), 1, -2.5, 0x1p-1022, math.MaxFloat64, math.Float64frombits(1),
		math.Float64frombits(0x000F_FFFF_FFFF_FFFF), 1e-300, 1e300, 0.1, math.Pi}
	for _, sample := range samples {
		text := floatHex(sample)
		parsed, err := floatFromHex(text)
		if err != nil || math.Float64bits(parsed) != math.Float64bits(sample) {
			t.Errorf("floatFromHex(%s) = %v, %v", text, parsed, err)
		}
	}
	messages := map[string]string{
		"1.0":                   "float hexadecimal literal is malformed",
		"0x":                    "float hexadecimal literal is malformed",
		"0x1.0p":                "float hexadecimal literal is malformed",
		"0x1.g":                 "float hexadecimal literal is malformed",
		"0x1.00000000000000000": "float hexadecimal literal has too many digits",
		"0x1p99999999999":       "float exponent is out of range",
		"0x1p+1024":             "float hexadecimal literal is out of range",
	}
	for text, message := range messages {
		if _, err := floatFromHex(text); err == nil || err.Error() != message {
			t.Errorf("floatFromHex(%s) = %v, want %q", text, err, message)
		}
	}
}

func TestRoundTripsTheEncodingsPythonProduces(t *testing.T) {
	canonical := []string{
		`["null"]`,
		`["boolean",true]`,
		`["boolean",false]`,
		`["integer","0"]`,
		`["integer","-1"]`,
		`["integer","9223372036854775807"]`,
		`["integer","-9223372036854775808"]`,
		`["float","0x0.0p+0"]`,
		`["float","-0x0.0p+0"]`,
		`["float","0x1.921fb54442d18p+1"]`,
		`["float","0x1.56e1fc2f8f359p-997"]`,
		`["float","0x1.7e43c8800759cp+996"]`,
		`["float","0x0.0000000000001p-1022"]`,
		`["float","0x1.fffffffffffffp+1023"]`,
		`["float","-0x1.c0302807c0200p+12"]`,
		`["string",""]`,
		`["string","a\tb\nc"]`,
		`["string","\u0001"]`,
		"[\"string\",\"\U0001F600\"]",
		`["string"," "]`,
		`["date","2026-01-02"]`,
		`["date","0001-01-01"]`,
		`["date","9999-12-31"]`,
		`["time","00:00:00.000000"]`,
		`["time","23:59:59.999999"]`,
		`["datetime","2026-01-02T03:04:05.000006"]`,
		`["datetime","2026-01-02T00:02:03.000000+00:00"]`,
		`["datetime","2025-12-31T23:00:00.000000+00:00"]`,
		`["object",[]]`,
		`["array",[]]`,
		"[\"object\",[[\"A\",[\"integer\",\"4\"]],[\"a\",[\"integer\",\"2\"]],[\"b\",[\"integer\",\"1\"]],[\"é\",[\"integer\",\"3\"]]]]",
		`["array",[["integer","1"],["object",[["x",["array",[["null"],["boolean",true]]]]]]]]`,
		`["object",[["a",["integer","1"]],["b",["array",[["boolean",true],["null"]]]]]]`,
	}
	for _, text := range canonical {
		value, err := DecodeValue([]byte(text))
		if err != nil {
			t.Errorf("DecodeValue(%s) = %v", text, err)
			continue
		}
		if got := encoded(t, value); got != text {
			t.Errorf("EncodeValue(DecodeValue(%s)) = %s", text, got)
		}
	}
}

func TestRejectsTheRecordsPythonRejects(t *testing.T) {
	rejected := map[string]string{
		`["integer","01"]`:                                "semantic scalar encoding is not canonical",
		`["integer","-0"]`:                                "semantic scalar encoding is not canonical",
		`["integer","+1"]`:                                "invalid literal for int() with base 10: '+1'",
		`["integer"," 1"]`:                                "invalid literal for int() with base 10: ' 1'",
		`["integer","1_0"]`:                               "invalid literal for int() with base 10: '1_0'",
		`["integer","9223372036854775808"]`:               "integers must fit signed 64 bits",
		`["float","0x1.8p+0"]`:                            "semantic scalar encoding is not canonical",
		`["float","1.0"]`:                                 "float hexadecimal literal is malformed",
		`["float","nan"]`:                                 "float hexadecimal literal is malformed",
		`["float","inf"]`:                                 "float hexadecimal literal is malformed",
		`["float","0X1.0000000000000P+0"]`:                "semantic scalar encoding is not canonical",
		`["date","2026-1-02"]`:                            "date must be written as YYYY-MM-DD",
		`["date","20260102"]`:                             "date must be written as YYYY-MM-DD",
		`["time","01:02:03"]`:                             "semantic scalar encoding is not canonical",
		`["time","01:02:03.0"]`:                           "semantic scalar encoding is not canonical",
		`["datetime","2026-01-02T00:02:03+00:00"]`:        "semantic scalar encoding is not canonical",
		`["datetime","2026-01-02T05:02:03.000000+05:00"]`: "semantic scalar encoding is not canonical",
		`["datetime","2026-01-02T00:02:03.000000Z"]`:      "semantic scalar encoding is not canonical",
		`["datetime","2026-01-02 00:02:03.000000"]`:       "datetime must separate date and time with T",
		`["boolean","true"]`:                              "unknown semantic scalar kind",
		`["boolean",1]`:                                   "semantic scalar payload has the wrong type",
		`["null",1]`:                                      "semantic scalar payload has the wrong type",
		`["nope","1"]`:                                    "unknown semantic scalar kind",
		`["integer"]`:                                     "semantic value has an invalid field count",
		`[]`:                                              "semantic values must be tagged arrays",
		`["string",1]`:                                    "semantic scalar payload has the wrong type",
		`["object",[["a",["integer","1"]],["a",["integer","2"]]]]`: "duplicate object key",
		`["array",["integer","1"]]`:                                "semantic values must be tagged arrays",
		`["object",[["a"]]]`:                                       "semantic object members must be name/value pairs",
		`["object",[[]]]`:                                          "semantic object members must be name/value pairs",
		`["object",[[1,["null"]]]]`:                                "semantic object members must be name/value pairs",
		`["null",null,null]`:                                       "semantic value has an invalid field count",
		`[1]`:                                                      "semantic values must be tagged arrays",
		`"notalist"`:                                               "semantic values must be tagged arrays",
		`{"a":1}`:                                                  "semantic values must be tagged arrays",
		`["string","a"]x`:                                          "JSON has trailing content",
		`["string","a","a"]`:                                       "semantic value has an invalid field count",
		`{"a":1,"a":2}`:                                            "duplicate JSON field: a",
	}
	for text, message := range rejected {
		_, err := DecodeValue([]byte(text))
		var invalid *ValueError
		if !errors.As(err, &invalid) || invalid.Message != message {
			t.Errorf("DecodeValue(%s) = %v, want %q", text, err, message)
		}
	}
	unsorted, err := DecodeValue([]byte(`["object",[["b",["integer","1"]],["a",["integer","2"]]]]`))
	if err != nil {
		t.Fatal(err)
	}
	if got := encoded(t, unsorted); got != `["object",[["a",["integer","2"]],["b",["integer","1"]]]]` {
		t.Errorf("unsorted object encodes as %s", got)
	}
}

func TestValueFromJSONMapsPlainDocuments(t *testing.T) {
	document, err := jsonx.Parse([]byte(`{"b":[1,2.5,"x",null,true],"a":{}}`))
	if err != nil {
		t.Fatal(err)
	}
	value, err := valueFromJSON(document)
	if err != nil {
		t.Fatal(err)
	}
	expected := `["object",[["a",["object",[]]],["b",["array",[["integer","1"],["float","0x1.4000000000000p+1"],["string","x"],["null"],["boolean",true]]]]]]`
	if got := encoded(t, value); got != expected {
		t.Errorf("valueFromJSON encodes as %s", got)
	}
	for text, message := range map[string]string{
		`[9223372036854775808]`: "integers must fit signed 64 bits",
		`[1e400]`:               "floats must be finite",
	} {
		document, _ := jsonx.Parse([]byte(text))
		if _, err := valueFromJSON(document); err == nil || err.Error() != message {
			t.Errorf("valueFromJSON(%s) = %v, want %q", text, err, message)
		}
	}
}

func TestMatchesTheSharedSemanticValueFixtures(t *testing.T) {
	document, err := jsonx.Parse(fixture(t, "configurations.json"))
	if err != nil {
		t.Fatal(err)
	}
	casesValue, _ := document.Get("semantic_values")
	cases, _ := casesValue.Items()
	if len(cases) < 10 {
		t.Fatalf("semantic_values has %d cases", len(cases))
	}
	for _, c := range cases {
		record, _ := c.Get("record")
		hexValue, _ := c.Get("encoding_hex")
		expectedHex, _ := hexValue.AsString()
		digestValue, _ := c.Get("sha256")
		expectedDigest, _ := digestValue.AsString()
		value, err := valueFromRecord(record)
		if err != nil {
			t.Errorf("valueFromRecord(%s) = %v", jsonx.Encode(record), err)
			continue
		}
		b := must(EncodeValue(value))
		digest := sha256.Sum256(b)
		if hex.EncodeToString(b) != expectedHex || hex.EncodeToString(digest[:]) != expectedDigest {
			t.Errorf("encoding %x, want %s", b, expectedHex)
		}
	}
}

func TestRoundTripsTheDifferentialValueCorpus(t *testing.T) {
	scanner := bufio.NewScanner(bytes.NewReader(fixture(t, "differential.tsv")))
	scanner.Buffer(nil, 16<<20)
	count := 0
	for scanner.Scan() {
		fields := strings.Split(scanner.Text(), "\t")
		if fields[0] != "value" {
			continue
		}
		raw, err := hex.DecodeString(fields[2])
		if err != nil {
			t.Fatal(err)
		}
		value, err := DecodeValue(raw)
		if err != nil {
			t.Errorf("%s: DecodeValue = %v", fields[1], err)
			continue
		}
		if got := must(EncodeValue(value)); !bytes.Equal(got, raw) {
			t.Errorf("%s: EncodeValue = %s, want %s", fields[1], got, raw)
		}
		count++
	}
	if count < 20 {
		t.Fatalf("differential corpus has %d value rows", count)
	}
}

func FuzzDecodeValue(f *testing.F) {
	for _, seed := range []string{`["null"]`, `["float","-0x0.0p+0"]`, `["object",[["b",["integer","1"]],["a",["null"]]]]`,
		`["datetime","2026-01-02T05:02:03.000000+05:00"]`, `["integer","01"]`, `["time","23:59:59.999999"]`} {
		f.Add([]byte(seed))
	}
	if data, err := readFixture("differential.tsv"); err == nil {
		for line := range strings.SplitSeq(string(data), "\n") {
			fields := strings.Split(line, "\t")
			if len(fields) == 3 && fields[0] == "value" {
				if raw, err := hex.DecodeString(fields[2]); err == nil {
					f.Add(raw)
				}
			}
		}
	}
	f.Fuzz(func(t *testing.T, data []byte) {
		value, err := DecodeValue(data)
		if err != nil {
			if _, ok := errors.AsType[*ValueError](err); !ok {
				t.Fatalf("DecodeValue error %T, want *ValueError", err)
			}
			return
		}
		first, err := EncodeValue(value)
		if err != nil {
			t.Fatalf("EncodeValue after decode: %v", err)
		}
		again, err := DecodeValue(first)
		if err != nil || !ValuesEqual(again, value) {
			t.Fatalf("decode(encode(v)) = %v, %v", again, err)
		}
		second, _ := EncodeValue(again)
		if !bytes.Equal(first, second) {
			t.Fatalf("encoding is unstable: %s != %s", first, second)
		}
	})
}
