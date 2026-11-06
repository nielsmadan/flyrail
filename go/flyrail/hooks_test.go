package flyrail

import (
	"bytes"
	"encoding/json"
	"errors"
	"slices"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/internal/hookruntime"
)

func hookCode(t *testing.T, err error) string {
	t.Helper()
	failure, ok := errors.AsType[*HookError](err)
	if !ok {
		t.Fatalf("error %v is not a *HookError", err)
	}
	return failure.Code
}

func hookText(t *testing.T, s string) HookText {
	t.Helper()
	return must(HookTextFromString(s))
}

func hookJSON(t *testing.T, source string) HookJSON {
	t.Helper()
	return must(ParseHookJSON([]byte(source)))
}

func nested(t *testing.T, value HookJSON, levels int) HookJSON {
	t.Helper()
	for range levels {
		value = must(HookArray(value))
	}
	return value
}

func TestHookRuntimeSnapshotsEmbeddedModules(t *testing.T) {
	runtime := must(NewHookRuntime("/opt/node/bin/node", ShellPOSIX))
	resources := runtime.Resources()
	files := hookruntime.Files()
	paths := make([]string, 0, len(resources))
	for index, entry := range resources {
		paths = append(paths, entry.Path)
		if !bytes.Equal(entry.Data, files[index].Data) || entry.Dir || entry.Executable {
			t.Errorf("resource %s does not hold the embedded module bytes", entry.Path)
		}
	}
	if diff := cmp.Diff([]string{"runner.mts", "bridge.mts", "process.mts"}, paths); diff != "" {
		t.Fatal(diff)
	}
	if runtime.Node() != "/opt/node/bin/node" || runtime.Shell() != ShellPOSIX {
		t.Fatalf("runtime = %q, %q", runtime.Node(), runtime.Shell())
	}
	resources[0].Data[0] ^= 0xff
	if !bytes.Equal(runtime.Resources()[0].Data, files[0].Data) {
		t.Fatal("Resources returned shared bytes")
	}
	none := must(NewHookRuntime("", ""))
	if none.Node() != "" || none.Shell() != "" || len(none.Resources()) != 3 {
		t.Fatalf("runtime without node = %q, %q, %d resources", none.Node(), none.Shell(), len(none.Resources()))
	}
	var missing *HookRuntime
	if missing.Node() != "" || missing.Shell() != "" || missing.Resources() != nil {
		t.Fatal("nil runtime accessors returned values")
	}
}

func TestHookRuntimeValidatesNodePath(t *testing.T) {
	for _, path := range []string{"/opt/node/bin/node", "/./node", "/usr/bin/node/", "/opt/nöde"} {
		if _, err := NewHookRuntime(path, ShellCmd); err != nil {
			t.Errorf("%q: %v", path, err)
		}
	}
	for _, path := range []string{"node", "./node", "/", "//", "/.", "/./", "/opt/../node", "/opt/no\x01de", "/opt/no\x00de", "/opt/\xffnode"} {
		_, err := NewHookRuntime(path, "")
		if err == nil || err.Error() != "hook runtime requires an absolute Node 24+ executable path" {
			t.Errorf("%q: %v", path, err)
		}
	}
	if _, err := NewHookRuntime("/opt/node", "zsh"); err == nil || err.Error() != "'zsh' is not a valid HookShell" {
		t.Fatalf("shell error = %v", err)
	}
	if shell, err := ParseHookShell("cmd"); err != nil || shell != ShellCmd {
		t.Fatalf("ParseHookShell = %q, %v", shell, err)
	}
}

func TestHookWireRecordsAreClosedBoundedAndCanonical(t *testing.T) {
	request := must(ParseHookRequest([]byte(`{"version":1,"event":"before-tool","tool":"Bash","input":{"command":"pwd"},"context":{}}`)))
	if got := string(request.Bytes()); got != `{"version":1,"event":"before-tool","context":{},"tool":"Bash","input":{"command":"pwd"}}` {
		t.Fatal(got)
	}
	_, err := ParseHookRequest([]byte(`{"version":1,"event":"stop","context":{},"extra":1}`))
	if err == nil || err.Error() != "flyrail-hook:input-fields" {
		t.Fatalf("extra field error = %v", err)
	}
	_, err = ParseHookJSON([]byte(`{"__proto__":{}}`))
	if err == nil || err.Error() != "flyrail-hook:json-key" {
		t.Fatalf("proto key error = %v", err)
	}
}

func TestHookResponseValidationMatchesRuntimeCodes(t *testing.T) {
	declared := []HookOutcome{OutcomeContinue, OutcomeBlock, OutcomeModifyInput}
	response := must(ParseHookResponse([]byte(`{"version":1,"outcome":"block","reason":"stop"}`), EventBeforeTool, declared))
	if response.Outcome() != OutcomeBlock {
		t.Fatalf("outcome = %q", response.Outcome())
	}
	if reason, ok := response.Value(); !ok || string(reason.Bytes()) != `"stop"` {
		t.Fatalf("value = %s, %v", reason.Bytes(), ok)
	}
	_, err := ParseHookResponse([]byte(`{"version":1,"outcome":"block"}`), EventBeforeTool, declared)
	if err == nil || err.Error() != "flyrail-hook:output-fields" {
		t.Fatalf("missing reason error = %v", err)
	}
}

func TestHookJSONMatchesJavaScriptNumberBytes(t *testing.T) {
	cases := []struct{ source, want string }{
		{"9007199254740991", "9007199254740991"},
		{"9007199254740992", "9007199254740992"},
		{"9007199254740993", "9007199254740992"},
		{"100000000000000000000", "100000000000000000000"},
		{"123456789012345678901", "123456789012345680000"},
		{"1e21", "1e+21"},
		{"1E21", "1e+21"},
		{"1.5e300", "1.5e+300"},
		{"1e-6", "0.000001"},
		{"1.5e-6", "0.0000015"},
		{"1e-7", "1e-7"},
		{"-1.25e-7", "-1.25e-7"},
		{"5e-324", "5e-324"},
		{"1.7976931348623157e308", "1.7976931348623157e+308"},
		{"0.30000000000000004", "0.30000000000000004"},
		{"1e-400", "0"},
		{"-1e-400", "0"},
		{"-0", "0"},
		{"-0.0", "0"},
		{"1.0", "1"},
		{"-12.50", "-12.5"},
		{"[1,2.5,{\"a\":-3e2}]", "[1,2.5,{\"a\":-300}]"},
	}
	for _, c := range cases {
		if got := string(hookJSON(t, c.source).Bytes()); got != c.want {
			t.Errorf("%s: got %s, want %s", c.source, got, c.want)
		}
	}
	tenth, fifth := 0.1, 0.2
	direct := []struct {
		value float64
		want  string
	}{
		{tenth + fifth, "0.30000000000000004"},
		{1e21, "1e+21"},
		{1e-7, "1e-7"},
		{5e-324, "5e-324"},
		{1.7976931348623157e308, "1.7976931348623157e+308"},
		{-123.456, "-123.456"},
	}
	for _, c := range direct {
		if got := jsNumber(c.value); got != c.want {
			t.Errorf("jsNumber(%v) = %s, want %s", c.value, got, c.want)
		}
	}
	for _, source := range []string{"1e400", "-1e400"} {
		if _, err := ParseHookJSON([]byte(source)); hookCode(t, err) != "json-number" {
			t.Errorf("%s: %v", source, err)
		}
	}
	for value, want := range map[int64]string{1: "1", -42: "-42", 9007199254740993: "9007199254740992", -9223372036854775808: "-9223372036854776000"} {
		if got := string(must(HookInt(value)).Bytes()); got != want {
			t.Errorf("HookInt(%d) = %s, want %s", value, got, want)
		}
	}
}

func TestHookJSONConstructorsEnforceDepthAndEncodedSize(t *testing.T) {
	exact := `"` + strings.Repeat("x", HookInputLimit-2) + `"`
	if got := len(hookJSON(t, exact).Bytes()); got != HookInputLimit {
		t.Fatalf("exact parse encodes %d bytes", got)
	}
	if got := len(must(HookString(hookText(t, strings.Repeat("x", HookInputLimit-2)))).Bytes()); got != HookInputLimit {
		t.Fatalf("exact string encodes %d bytes", got)
	}
	if _, err := ParseHookJSON([]byte(`"` + strings.Repeat("x", HookInputLimit-1) + `"`)); hookCode(t, err) != "input-limit" {
		t.Fatalf("over parse: %v", err)
	}
	if _, err := HookString(hookText(t, strings.Repeat("x", HookInputLimit-1))); hookCode(t, err) != "input-limit" {
		t.Fatalf("over string: %v", err)
	}
	parsed := strings.Repeat("[", HookJSONDepthLimit) + "0" + strings.Repeat("]", HookJSONDepthLimit)
	hookJSON(t, parsed)
	if _, err := ParseHookJSON([]byte("[" + parsed + "]")); hookCode(t, err) != "json-depth" {
		t.Fatalf("depth 65 parse: %v", err)
	}
	constructed := nested(t, HookNull(), HookJSONDepthLimit)
	if _, err := HookArray(constructed); hookCode(t, err) != "json-depth" {
		t.Fatalf("depth 65 constructor: %v", err)
	}
	if _, err := HookObject(HookMember{Key: "value", Value: constructed}); hookCode(t, err) != "json-depth" {
		t.Fatalf("depth 65 object: %v", err)
	}
}

func TestHookJSONDeepInputsAreBoundedBeforeRecursiveParsing(t *testing.T) {
	for _, depth := range []int{65, 513} {
		source := strings.Repeat("[", depth) + "0" + strings.Repeat("]", depth)
		if _, err := ParseHookJSON([]byte(source)); hookCode(t, err) != "json-depth" {
			t.Errorf("depth %d: %v", depth, err)
		}
		request := `{"version":1,"event":"stop","context":{"value":` + source + `}}`
		if _, err := ParseHookRequest([]byte(request)); hookCode(t, err) != "json-depth" {
			t.Errorf("request depth %d: %v", depth, err)
		}
	}
	prefix, suffix := strings.Repeat("[", 513), strings.Repeat("]", 513)
	source := prefix + strings.Repeat(" ", HookInputLimit-len(prefix)-len(suffix)-1) + "0" + suffix
	if len(source) != HookInputLimit {
		t.Fatalf("source has %d bytes", len(source))
	}
	if _, err := ParseHookJSON([]byte(source)); hookCode(t, err) != "json-depth" {
		t.Fatalf("padded deep input: %v", err)
	}
	if _, err := ParseHookJSON([]byte(source[:len(source)-1])); hookCode(t, err) != "invalid-json" {
		t.Fatalf("truncated deep input: %v", err)
	}
}

func TestHookJSONSemanticErrorsFollowTokenOrderAfterSyntax(t *testing.T) {
	deep := strings.Repeat("[", 65) + "0" + strings.Repeat("]", 65)
	cases := []struct{ source, code string }{
		{`{"__proto__":0,"deep":` + deep + `}`, "json-key"},
		{`{"value":1e400,"deep":` + deep + `}`, "json-number"},
		{`{"a":0,"a":1,"deep":` + deep + `}`, "json-key"},
		{`{"deep":` + deep + `,"__proto__":0}`, "json-depth"},
		{`{"deep":` + deep + `,"value":1e400}`, "json-depth"},
		{`{"deep":` + deep + `,"a":0,"a":1}`, "json-depth"},
		{`{"__proto__":0,"deep":` + deep, "invalid-json"},
		{`[{"a":1},{"a":2},{"prototype":3}]`, "json-key"},
	}
	for _, c := range cases {
		if _, err := ParseHookJSON([]byte(c.source)); hookCode(t, err) != c.code {
			t.Errorf("%.40s: %v, want %s", c.source, err, c.code)
		}
	}
}

func TestHookJSONRejectsMalformedSyntax(t *testing.T) {
	for _, source := range []string{
		"", " ", "01", "-", "1.", "1e", "1e+", ".5", "+1", "tru", "nul", "True", "[1,]", "{\"a\":1,}", "{\"a\" 1}", "{1:2}",
		"[1 2]", "\"abc", "\"a\x01\"", `"\x"`, `"\u12"`, `"\u12g4"`, "\xef\xbb\xbf1", "\"\xff\"", "1 2", "[", "{", "}", "NaN", "Infinity",
	} {
		if _, err := ParseHookJSON([]byte(source)); hookCode(t, err) != "invalid-json" {
			t.Errorf("%q: %v", source, err)
		}
	}
	for _, c := range []struct{ source, want string }{
		{" \t\r\n[ 1 , true , false , null , \"x\" ] ", `[1,true,false,null,"x"]`},
		{`"\"\\\/\b\f\n\r\t\u0001\u007f"`, `"\"\\/\b\f\n\r\t\u0001` + "\x7f" + `"`},
		{`{"a":{},"b":[]}`, `{"a":{},"b":[]}`},
	} {
		if got := string(hookJSON(t, c.source).Bytes()); got != c.want {
			t.Errorf("%q: got %q, want %q", c.source, got, c.want)
		}
	}
}

func TestHookJSONPreservesJavaScriptUTF16Strings(t *testing.T) {
	emoji := `"` + string(rune(0x1f600)) + `"`
	cases := []struct{ source, want string }{
		{`"\ud800"`, `"\ud800"`},
		{`"\udfff"`, `"\udfff"`},
		{`"\uDBFF\uDFFF"`, `"` + string(rune(0x10ffff)) + `"`},
		{`"\ud83d\ude00"`, emoji},
		{emoji, emoji},
		{`"\ude00\ud83d"`, `"\ude00\ud83d"`},
		{`{"\ud800":"\udfff"}`, `{"\ud800":"\udfff"}`},
		{`"\u2028` + string(rune(0x2029)) + `"`, `"` + string(rune(0x2028)) + string(rune(0x2029)) + `"`},
	}
	for _, c := range cases {
		if got := string(hookJSON(t, c.source).Bytes()); got != c.want {
			t.Errorf("%s: got %q, want %q", c.source, got, c.want)
		}
	}
	lone := hookJSON(t, `"\ud800"`)
	if _, ok := lone.UTF8(); ok {
		t.Fatal("lone surrogate converted to UTF-8")
	}
	if text, ok := lone.Text(); !ok || !slices.Equal(text.Units(), []uint16{0xd800}) {
		t.Fatalf("units = %v", text.Units())
	}
	if text, ok := hookJSON(t, `"a\u00e9"`).UTF8(); !ok || text != "a"+string(rune(0xe9)) {
		t.Fatalf("UTF8 = %q, %v", text, ok)
	}
	for _, source := range []string{`{"\ud800":1,"\uD800":2}`, `{"constructor":1}`} {
		if _, err := ParseHookJSON([]byte(source)); hookCode(t, err) != "json-key" {
			t.Errorf("%s: %v", source, err)
		}
	}
}

func TestHookTypedPacketsPreserveJavaScriptUTF16Text(t *testing.T) {
	cases := []struct {
		escaped, canonical string
		units              []uint16
	}{
		{`\ud800`, `\ud800`, []uint16{0xd800}},
		{`\udfff`, `\udfff`, []uint16{0xdfff}},
		{`\ud83d\ude00`, string(rune(0x1f600)), []uint16{0xd83d, 0xde00}},
	}
	for _, c := range cases {
		request := must(ParseHookRequest([]byte(`{"version":1,"event":"prompt","context":{},"input":"` + c.escaped + `"}`)))
		input, _ := request.Input()
		if text, ok := input.Text(); !ok || !slices.Equal(text.Units(), c.units) {
			t.Errorf("%s prompt units = %v", c.escaped, text.Units())
		}
		if got, want := string(request.Bytes()), `{"version":1,"event":"prompt","context":{},"input":"`+c.canonical+`"}`; got != want {
			t.Errorf("prompt bytes = %s, want %s", got, want)
		}

		request = must(ParseHookRequest([]byte(`{"version":1,"event":"before-tool","context":{},"tool":"` + c.escaped + `","input":{}}`)))
		if tool, ok := request.Tool(); !ok || !slices.Equal(tool.Units(), c.units) {
			t.Errorf("%s tool units = %v", c.escaped, tool.Units())
		}
		if got, want := string(request.Bytes()), `{"version":1,"event":"before-tool","context":{},"tool":"`+c.canonical+`","input":{}}`; got != want {
			t.Errorf("tool bytes = %s, want %s", got, want)
		}

		for _, pair := range [][2]string{{"block", "reason"}, {"ask", "reason"}, {"context", "text"}} {
			source := `{"version":1,"outcome":"` + pair[0] + `","` + pair[1] + `":"` + c.escaped + `"}`
			response := must(ParseHookResponse([]byte(source), EventBeforeTool, []HookOutcome{OutcomeBlock, OutcomeAsk, OutcomeContext}))
			value, _ := response.Value()
			if text, ok := value.Text(); !ok || !slices.Equal(text.Units(), c.units) {
				t.Errorf("%s %s units = %v", c.escaped, pair[0], text.Units())
			}
			if got, want := string(response.Bytes()), `{"version":1,"outcome":"`+pair[0]+`","`+pair[1]+`":"`+c.canonical+`"}`; got != want {
				t.Errorf("response bytes = %s, want %s", got, want)
			}
		}

		effects := must(ParseHookEffects([]byte(`{"contexts":["` + c.escaped + `"],"decision":"block","reason":"` + c.escaped + `"}`)))
		reason, _ := effects.Reason()
		if !slices.Equal(effects.Contexts()[0].Units(), c.units) || !slices.Equal(reason.Units(), c.units) {
			t.Errorf("%s effects units = %v, %v", c.escaped, effects.Contexts()[0].Units(), reason.Units())
		}
		if got, want := string(effects.Bytes()), `{"contexts":["`+c.canonical+`"],"decision":"block","reason":"`+c.canonical+`"}`; got != want {
			t.Errorf("effects bytes = %s, want %s", got, want)
		}
	}
}

func TestHookTextUsesJavaScriptTrimAndTextEncoderSize(t *testing.T) {
	if _, err := ContextResponse(HookTextFromUTF16([]uint16{0xfeff, 0x3000})); hookCode(t, err) != "context" {
		t.Fatalf("blank context: %v", err)
	}
	if _, err := BlockResponse(HookText{}); hookCode(t, err) != "reason" {
		t.Fatalf("empty reason: %v", err)
	}
	if _, err := AskResponse(HookTextFromUTF16([]uint16{0x0085})); err != nil {
		t.Fatalf("U+0085 is not JavaScript whitespace: %v", err)
	}
	exact := HookTextFromUTF16(slices.Repeat([]uint16{0xd800}, HookTextLimit/3))
	must(ContextResponse(exact))
	over := HookTextFromUTF16(slices.Repeat([]uint16{0xd800}, HookTextLimit/3+1))
	if _, err := ContextResponse(over); hookCode(t, err) != "context" {
		t.Fatalf("over-limit context: %v", err)
	}
	pairs := HookTextFromUTF16(slices.Repeat([]uint16{0xd83d, 0xde00}, HookTextLimit/4))
	must(ContextResponse(pairs))
	if _, err := ContextResponse(HookTextFromUTF16(slices.Repeat([]uint16{0xd83d, 0xde00}, HookTextLimit/4+1))); hookCode(t, err) != "context" {
		t.Fatalf("over-limit pairs: %v", err)
	}
}

func TestHookTextConversions(t *testing.T) {
	units := []uint16{0x61, 0xd800}
	text := HookTextFromUTF16(units)
	units[0] = 0x62
	if got := text.Units(); !slices.Equal(got, []uint16{0x61, 0xd800}) {
		t.Fatalf("units = %v", got)
	}
	if _, ok := text.UTF8(); ok {
		t.Fatal("lone surrogate text has UTF-8")
	}
	scalar := hookText(t, "x"+string(rune(0x1f600)))
	if s, ok := scalar.UTF8(); !ok || s != "x"+string(rune(0x1f600)) || !slices.Equal(scalar.Units(), []uint16{0x78, 0xd83d, 0xde00}) {
		t.Fatalf("scalar text = %q, %v, %v", s, ok, scalar.Units())
	}
	if s, ok := (HookText{}).UTF8(); !ok || s != "" || !(HookText{}).IsEmpty() {
		t.Fatal("zero text is not empty UTF-8")
	}
	if _, err := HookTextFromString("\xff"); err == nil || err.Error() != "hook text must contain Unicode scalar values" {
		t.Fatalf("invalid UTF-8: %v", err)
	}
	if _, err := HookObject(HookMember{Key: "\xff", Value: HookNull()}); hookCode(t, err) != "json-key" {
		t.Fatalf("invalid key: %v", err)
	}
	object := must(HookObject(
		HookMember{Key: "b", Value: HookBool(true)},
		HookMember{Key: "a", Value: HookNull()},
		HookMember{Key: "c", Value: HookJSON{}},
	))
	if got := string(object.Bytes()); got != `{"b":true,"a":null,"c":null}` || !object.IsObject() {
		t.Fatalf("object = %s", got)
	}
	if _, err := HookObject(HookMember{Key: "a", Value: HookNull()}, HookMember{Key: "a", Value: HookNull()}); hookCode(t, err) != "json-key" {
		t.Fatalf("duplicate key: %v", err)
	}
	var failure *HookError
	if failure.Error() != "" || (&HookError{Code: "x"}).Error() != "flyrail-hook:x" {
		t.Fatal("HookError text")
	}
}

func TestHookAssembledPacketsAreRevalidated(t *testing.T) {
	dataObject := func(n int) HookJSON {
		return must(HookObject(HookMember{Key: "data", Value: must(HookString(hookText(t, strings.Repeat("x", n))))}))
	}
	baseline := len(must(NewHookRequest(EventStop, dataObject(0), nil, nil, nil)).Bytes())
	if got := len(must(NewHookRequest(EventStop, dataObject(HookInputLimit-baseline), nil, nil, nil)).Bytes()); got != HookInputLimit {
		t.Fatalf("exact request encodes %d bytes", got)
	}
	if _, err := NewHookRequest(EventStop, dataObject(HookInputLimit-baseline+1), nil, nil, nil); hookCode(t, err) != "input-limit" {
		t.Fatalf("over request: %v", err)
	}
	baseline = len(must(ModifyInputResponse(EventBeforeTool, dataObject(0))).Bytes())
	if got := len(must(ModifyInputResponse(EventBeforeTool, dataObject(HookInputLimit-baseline))).Bytes()); got != HookInputLimit {
		t.Fatalf("exact response encodes %d bytes", got)
	}
	if _, err := ModifyInputResponse(EventBeforeTool, dataObject(HookInputLimit-baseline+1)); hookCode(t, err) != "input-limit" {
		t.Fatalf("over response: %v", err)
	}
	deep := must(HookObject(HookMember{Key: "nested", Value: nested(t, HookNull(), HookJSONDepthLimit-1)}))
	if _, err := NewHookRequest(EventStop, deep, nil, nil, nil); hookCode(t, err) != "json-depth" {
		t.Fatalf("deep request: %v", err)
	}
	if _, err := ModifyInputResponse(EventBeforeTool, deep); hookCode(t, err) != "json-depth" {
		t.Fatalf("deep response: %v", err)
	}
}

func TestHookCanonicalRecordsMatchAuthoritativeInsertionOrder(t *testing.T) {
	tool := hookText(t, "Bash")
	input := hookJSON(t, `{"command":"pwd"}`)
	result := HookBool(true)
	request := must(NewHookRequest(EventAfterTool, hookJSON(t, `{"session":"s"}`), &tool, &input, &result))
	if got := string(request.Bytes()); got != `{"version":1,"event":"after-tool","context":{"session":"s"},"tool":"Bash","input":{"command":"pwd"},"result":true}` {
		t.Fatal(got)
	}
	if request.Event() != EventAfterTool || string(request.Context().Bytes()) != `{"session":"s"}` {
		t.Fatal("request accessors")
	}
	if value, ok := request.Result(); !ok || string(value.Bytes()) != "true" {
		t.Fatal("request result")
	}

	effects := must(ParseHookEffects([]byte(`{"input":{"command":"changed"},"contexts":["notice"],"decision":"ask","reason":"why"}`)))
	if got := string(effects.Bytes()); got != `{"contexts":["notice"],"input":{"command":"changed"},"decision":"ask","reason":"why"}` {
		t.Fatal(got)
	}
	if effects.Decision() != OutcomeAsk {
		t.Fatalf("decision = %q", effects.Decision())
	}
	if got := string((HookEffects{}).Bytes()); got != `{"contexts":[]}` {
		t.Fatalf("zero effects = %s", got)
	}

	handler := must(NewHookHandler(HookHandlerParams{
		ID:            "handler",
		Event:         EventBeforeTool,
		Argv:          []string{"node", "handler.mjs"},
		Environment:   map[string]EnvironmentValue{"TOKEN": EnvText("value"), "API": must(NewEnvRef("API_KEY"))},
		Cwd:           "/work",
		TimeoutMillis: 1000,
		Outcomes:      []HookOutcome{OutcomeContinue, OutcomeBlock},
		Tools:         []string{"Bash", "Agent"},
	}))
	config := must(NewHookConfig("test", []HookHandler{handler}))
	want := `{"host":"test","handlers":[{"id":"handler","event":"before-tool","argv":["node","handler.mjs"],"cwd":"/work",` +
		`"env":{"API":{"ref":"API_KEY"},"TOKEN":"value"},"timeout_ms":1000,"outcomes":["block","continue"],"tools":["Agent","Bash"]}]}`
	if got := string(config.Bytes()); got != want {
		t.Fatalf("config = %s", got)
	}
}

func TestHookRequestFailureCodes(t *testing.T) {
	cases := []struct{ source, code string }{
		{`[]`, "host-input"},
		{`{"version":1,"event":"stop","context":{},"Tool":"x"}`, "input-fields"},
		{`{"event":"stop","context":{}}`, "input-version"},
		{`{"version":2,"event":"stop","context":{}}`, "input-version"},
		{`{"version":"1","event":"stop","context":{}}`, "input-version"},
		{`{"version":1.0,"event":"stop","context":{}}`, ""},
		{`{"version":1e0,"event":"stop","context":{}}`, ""},
		{`{"version":1,"event":"Stop","context":{}}`, "input-event"},
		{`{"version":1,"event":"\ud800","context":{}}`, "input-event"},
		{`{"version":1,"event":"stop"}`, "host-input"},
		{`{"version":1,"event":"stop","context":[]}`, "host-input"},
		{`{"version":1,"event":"before-tool","context":{},"tool":1,"input":{}}`, "host-tool"},
		{`{"version":1,"event":"before-tool","context":{},"tool":"","input":{}}`, "host-tool"},
		{`{"version":1,"event":"before-tool","context":{},"input":{}}`, "host-tool"},
		{`{"version":1,"event":"before-tool","context":{},"tool":"Bash"}`, "host-arguments"},
		{`{"version":1,"event":"after-tool","context":{},"tool":"Bash","input":{}}`, "host-result"},
		{`{"version":1,"event":"prompt","context":{}}`, "host-prompt"},
		{`{"version":1,"event":"prompt","context":{},"input":{}}`, "host-prompt"},
		{`{"version":1,"event":"stop","context":{},"tool":"Bash"}`, "input-fields"},
		{`{"version":1,"event":"session-start","context":{},"input":{}}`, "input-fields"},
		{`{"version":1,"event":"before-tool","context":{},"tool":"Bash","input":{},"result":1}`, "input-fields"},
		{`{"version":1,"event":"prompt","context":{},"input":"x","tool":"Bash"}`, "input-fields"},
		{`{"version":1,"event":"stop","context":{},"context":{}}`, "json-key"},
		{`{"version":1,`, "invalid-json"},
	}
	for _, c := range cases {
		_, err := ParseHookRequest([]byte(c.source))
		if c.code == "" {
			if err != nil {
				t.Errorf("%s: %v", c.source, err)
			}
			continue
		}
		if got := hookCode(t, err); got != c.code {
			t.Errorf("%s: %s, want %s", c.source, got, c.code)
		}
	}
	if _, err := ParseHookRequest(make([]byte, HookInputLimit+1)); hookCode(t, err) != "input-limit" {
		t.Fatalf("over-limit request: %v", err)
	}
	if _, err := NewHookRequest("unknown", hookJSON(t, `{}`), nil, nil, nil); hookCode(t, err) != "input-event" {
		t.Fatalf("unknown event: %v", err)
	}
	if _, err := NewHookRequest(EventStop, HookJSON{}, nil, nil, nil); hookCode(t, err) != "host-input" {
		t.Fatalf("null context: %v", err)
	}
	prompt := must(HookString(hookText(t, "hi")))
	request := must(NewHookRequest(EventPrompt, hookJSON(t, `{}`), nil, &prompt, nil))
	if _, ok := request.Tool(); ok {
		t.Fatal("prompt request has a tool")
	}
	if value, ok := request.Input(); !ok || string(value.Bytes()) != `"hi"` {
		t.Fatal("prompt input")
	}
}

func TestHookResponseFailureCodes(t *testing.T) {
	all := []HookOutcome{OutcomeContinue, OutcomeBlock, OutcomeAsk, OutcomeModifyInput, OutcomeContext}
	cases := []struct {
		source   string
		event    HookEvent
		declared []HookOutcome
		code     string
	}{
		{`{"version":1,"outcome":"continue"}`, EventStop, all, ""},
		{`not json`, EventStop, all, "invalid-output"},
		{`{"a":1,"a":2}`, EventStop, all, "invalid-output"},
		{`[]`, EventStop, all, "undeclared-outcome"},
		{`{"outcome":"continue"}`, EventStop, all, "undeclared-outcome"},
		{`{"version":1,"outcome":"block","reason":"x"}`, EventStop, []HookOutcome{OutcomeContinue}, "undeclared-outcome"},
		{`{"version":1,"outcome":"stop"}`, EventStop, all, "undeclared-outcome"},
		{`{"version":1,"outcome":"continue","reason":"x"}`, EventStop, all, "output-fields"},
		{`{"version":1,"outcome":"block","text":"x"}`, EventStop, all, "output-fields"},
		{`{"version":1,"outcome":"block","reason":1}`, EventStop, all, "reason"},
		{`{"version":1,"outcome":"ask","reason":" "}`, EventStop, all, "reason"},
		{`{"version":1,"outcome":"context","text":""}`, EventStop, all, "context"},
		{`{"version":1,"outcome":"modify-input","input":"x"}`, EventBeforeTool, all, "replacement"},
		{`{"version":1,"outcome":"modify-input","input":{}}`, EventPrompt, all, "replacement"},
		{`{"version":1,"outcome":"modify-input","input":"x"}`, EventPrompt, all, ""},
		{`{"version":1,"outcome":"modify-input","input":{"command":"x"}}`, EventBeforeTool, all, ""},
	}
	for _, c := range cases {
		response, err := ParseHookResponse([]byte(c.source), c.event, c.declared)
		if c.code == "" {
			if err != nil || !bytes.Equal(response.Bytes(), []byte(c.source)) {
				t.Errorf("%s: %s, %v", c.source, response.Bytes(), err)
			}
			continue
		}
		if got := hookCode(t, err); got != c.code {
			t.Errorf("%s: %s, want %s", c.source, got, c.code)
		}
	}
	oversized := append([]byte(`{"version":1,"outcome":"continue"}`), bytes.Repeat([]byte(" "), HookOutputLimit)...)
	if _, err := ParseHookResponse(oversized, EventStop, all); hookCode(t, err) != "stdout-limit" {
		t.Fatalf("oversized: %v", err)
	}
	if _, err := ParseHookResponse([]byte(`{"version":1,"outcome":"continue"}`), "later", all); hookCode(t, err) != "input-event" {
		t.Fatalf("unknown event: %v", err)
	}
	if response := ContinueResponse(); response.Outcome() != OutcomeContinue || string(response.Bytes()) != `{"version":1,"outcome":"continue"}` {
		t.Fatalf("continue = %s", response.Bytes())
	}
	if _, ok := ContinueResponse().Value(); ok {
		t.Fatal("continue has a value")
	}
}

func TestHookEffectsFailureCodes(t *testing.T) {
	long := strings.Repeat("x", HookTextLimit/2)
	cases := []struct{ source, code string }{
		{`{"contexts":[]}`, ""},
		{`{"contexts":["` + long + `","` + long[1:] + `"]}`, ""},
		{`{"contexts":["` + long + `","` + long + `"]}`, "context-limit"},
		{`{`, "invalid-output"},
		{`[]`, "output-fields"},
		{`{}`, "output-fields"},
		{`{"contexts":{}}`, "output-fields"},
		{`{"contexts":[],"extra":1}`, "output-fields"},
		{`{"contexts":[1]}`, "context"},
		{`{"contexts":[" "]}`, "context-limit"},
		{`{"contexts":[],"decision":"continue","reason":"x"}`, "undeclared-outcome"},
		{`{"contexts":[],"decision":"block","reason":""}`, "reason"},
		{`{"contexts":[],"decision":"block"}`, "output-fields"},
		{`{"contexts":[],"reason":"x"}`, "output-fields"},
	}
	for _, c := range cases {
		_, err := ParseHookEffects([]byte(c.source))
		if c.code == "" {
			if err != nil {
				t.Errorf("%.40s: %v", c.source, err)
			}
			continue
		}
		if got := hookCode(t, err); got != c.code {
			t.Errorf("%.40s: %s, want %s", c.source, got, c.code)
		}
	}
	if _, err := ParseHookEffects(make([]byte, HookOutputLimit+1)); hookCode(t, err) != "invalid-output" {
		t.Fatalf("oversized effects: %v", err)
	}
}

func TestHookHandlerValidation(t *testing.T) {
	valid := func() HookHandlerParams {
		return HookHandlerParams{ID: "h", Event: EventBeforeTool, Argv: []string{"node"}, Cwd: "/work", TimeoutMillis: 1, Outcomes: []HookOutcome{OutcomeContinue}}
	}
	cases := []struct {
		edit    func(*HookHandlerParams)
		message string
	}{
		{func(p *HookHandlerParams) { p.ID = "Bad" }, "artifact id must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
		{func(p *HookHandlerParams) { p.Event = "later" }, "'later' is not a valid HookEvent"},
		{func(p *HookHandlerParams) { p.Argv = nil }, "argv requires a command"},
		{func(p *HookHandlerParams) { p.Argv = []string{"node", "a\x00"} }, "command arguments cannot contain NUL"},
		{func(p *HookHandlerParams) { p.Argv = []string{"\xff"} }, "command argument must contain Unicode scalar values"},
		{func(p *HookHandlerParams) { p.Argv = []string{""} }, "command cannot be empty"},
		{func(p *HookHandlerParams) { p.Cwd = "work" }, "render routes must be absolute Paths"},
		{func(p *HookHandlerParams) { p.Cwd = "" }, "render routes must be absolute Paths"},
		{func(p *HookHandlerParams) { p.Cwd = "/work/../x" }, "render routes must be unambiguous paths without NUL"},
		{func(p *HookHandlerParams) { p.TimeoutMillis = 0 }, "hook timeout must be 1-300000 integer milliseconds"},
		{func(p *HookHandlerParams) { p.TimeoutMillis = 300_001 }, "hook timeout must be 1-300000 integer milliseconds"},
		{func(p *HookHandlerParams) { p.Outcomes = nil }, "hook requires declared HookOutcome values"},
		{func(p *HookHandlerParams) { p.Outcomes = []HookOutcome{"maybe"} }, "'maybe' is not a valid HookOutcome"},
		{func(p *HookHandlerParams) { p.Outcomes = []HookOutcome{OutcomeAsk, OutcomeAsk} }, "hook outcomes must be unique"},
		{func(p *HookHandlerParams) { p.Tools = []string{""} }, "tool names must be nonempty and cannot contain NUL"},
		{func(p *HookHandlerParams) { p.Tools = []string{"a", "a"} }, "tool names must be unique"},
		{func(p *HookHandlerParams) { p.Event, p.Tools = EventStop, []string{"a"} }, "tool selection requires a tool event"},
		{func(p *HookHandlerParams) { p.Environment = map[string]EnvironmentValue{"1BAD": EnvText("x")} }, "invalid environment reference"},
		{func(p *HookHandlerParams) { p.Environment = map[string]EnvironmentValue{"A": EnvText("x\x00")} }, "environment values cannot contain NUL"},
		{func(p *HookHandlerParams) { p.Environment = map[string]EnvironmentValue{"A": EnvRef{}} }, "invalid environment reference"},
		{func(p *HookHandlerParams) { p.Environment = map[string]EnvironmentValue{"A": nil} }, "environment values must be EnvText or EnvRef values"},
	}
	for _, c := range cases {
		params := valid()
		c.edit(&params)
		if _, err := NewHookHandler(params); err == nil || err.Error() != c.message {
			t.Errorf("got %v, want %s", err, c.message)
		}
	}
	params := valid()
	params.Argv = []string{"node", "x"}
	handler := must(NewHookHandler(params))
	params.Argv[1] = "changed"
	if diff := cmp.Diff([]string{"node", "x"}, handler.Argv()); diff != "" {
		t.Fatal(diff)
	}
	if handler.ID() != "h" || handler.Event() != EventBeforeTool || handler.Cwd() != "/work" || handler.TimeoutMillis() != 1 || len(handler.Tools()) != 0 || len(handler.Environment()) != 0 {
		t.Fatal("handler accessors")
	}
}

func TestHookConfigValidation(t *testing.T) {
	handler := func(id string, event HookEvent, timeout int) HookHandler {
		return must(NewHookHandler(HookHandlerParams{ID: id, Event: event, Argv: []string{"node"}, Cwd: "/w", TimeoutMillis: timeout, Outcomes: []HookOutcome{OutcomeContinue}}))
	}
	config := must(NewHookConfig("claude", []HookHandler{handler("b", EventStop, 1), handler("a", EventStop, 1)}))
	ids := []string{}
	for _, h := range config.Handlers() {
		ids = append(ids, h.ID())
	}
	if diff := cmp.Diff([]string{"a", "b"}, ids); diff != "" || config.Host() != "claude" {
		t.Fatal(diff)
	}
	many := make([]HookHandler, 0, HookHandlerLimit+1)
	for index := range HookHandlerLimit + 1 {
		many = append(many, handler("h"+strings.Repeat("x", index%3)+string(rune('a'+index%26)), EventPrompt, 1))
	}
	if _, err := NewHookConfig("x", many[:HookHandlerLimit]); err != nil {
		t.Fatal(err)
	}
	for _, c := range []struct {
		host     string
		handlers []HookHandler
		message  string
	}{
		{" \t\u3000", nil, "hook host must be a nonblank string"},
		{"\xff", nil, "hook host must contain Unicode scalar values"},
		{"x", many, "hook event cannot have more than 64 handlers"},
		{"x", []HookHandler{handler("a", EventStop, 200_000), handler("b", EventStop, 100_001)}, "hook event timeout sum cannot exceed 300000 milliseconds"},
		{"x", []HookHandler{{}}, "hook handlers must be HookHandler values"},
	} {
		if _, err := NewHookConfig(c.host, c.handlers); err == nil || err.Error() != c.message {
			t.Errorf("got %v, want %s", err, c.message)
		}
	}
	if _, err := NewHookConfig("x", []HookHandler{handler("a", EventStop, 200_000), handler("b", EventPrompt, 200_000)}); err != nil {
		t.Fatalf("per-event sums: %v", err)
	}
}

type hookVector struct {
	Name      string                     `json:"name"`
	Event     HookEvent                  `json:"event"`
	Response  json.RawMessage            `json:"response"`
	Canonical map[string]json.RawMessage `json:"canonical"`
	Context   json.RawMessage            `json:"context"`
}

func compactJSON(t *testing.T, raw json.RawMessage) string {
	t.Helper()
	var buffer bytes.Buffer
	if err := json.Compact(&buffer, raw); err != nil {
		t.Fatal(err)
	}
	return buffer.String()
}

func TestHookFixtureVectors(t *testing.T) {
	var document struct {
		Version int          `json:"version"`
		Cases   []hookVector `json:"cases"`
	}
	if err := json.Unmarshal(fixture(t, "hooks.json"), &document); err != nil {
		t.Fatal(err)
	}
	if document.Version != 1 || len(document.Cases) == 0 {
		t.Fatalf("hooks.json version %d with %d cases", document.Version, len(document.Cases))
	}
	for _, c := range document.Cases {
		t.Run(c.Name, func(t *testing.T) {
			canonical := `{"version":1,"event":"` + string(c.Event) + `","context":` + compactJSON(t, c.Context)
			reordered := `{`
			for _, name := range []string{"tool", "input", "result"} {
				if value, ok := c.Canonical[name]; ok {
					canonical += `,"` + name + `":` + compactJSON(t, value)
					reordered += `"` + name + `":` + compactJSON(t, value) + `,`
				}
			}
			canonical += `}`
			reordered += `"context":` + compactJSON(t, c.Context) + `,"event":"` + string(c.Event) + `","version":1}`
			for _, source := range []string{canonical, reordered} {
				request, err := ParseHookRequest([]byte(source))
				if err != nil {
					t.Fatalf("%s: %v", source, err)
				}
				if got := string(request.Bytes()); got != canonical {
					t.Fatalf("request = %s, want %s", got, canonical)
				}
			}
			response := compactJSON(t, c.Response)
			parsed, err := ParseHookResponse([]byte(response), c.Event, hookOutcomes)
			if err != nil {
				t.Fatalf("%s: %v", response, err)
			}
			if got := string(parsed.Bytes()); got != response {
				t.Fatalf("response = %s, want %s", got, response)
			}
			var outcome struct {
				Outcome HookOutcome `json:"outcome"`
			}
			if err := json.Unmarshal(c.Response, &outcome); err != nil || parsed.Outcome() != outcome.Outcome {
				t.Fatalf("outcome = %q, want %q (%v)", parsed.Outcome(), outcome.Outcome, err)
			}
		})
	}
}

func FuzzParseHookJSON(f *testing.F) {
	for _, seed := range []string{
		`{"version":1,"event":"before-tool","context":{},"tool":"Bash","input":{"command":"pwd"}}`,
		`{"version":1,"outcome":"block","reason":"\ud800"}`,
		`{"contexts":["\ud83d\ude00"],"decision":"ask","reason":"why"}`,
		`[1e21,-0,5e-324,1.7976931348623157e308,0.30000000000000004]`,
		`{"__proto__":1}`, `"\u0000\u001f\u2028"`, `[[[[[]]]]]`, `1e400`, `{"a":1,"a":2}`,
	} {
		f.Add([]byte(seed))
	}
	if data, err := readFixture("hooks.json"); err == nil {
		f.Add(data)
	}
	f.Fuzz(func(t *testing.T, data []byte) {
		value, err := ParseHookJSON(data)
		_, requestErr := ParseHookRequest(data)
		_, responseErr := ParseHookResponse(data, EventPrompt, hookOutcomes)
		_, effectsErr := ParseHookEffects(data)
		for _, failure := range []error{err, requestErr, responseErr, effectsErr} {
			if failure != nil {
				if _, ok := errors.AsType[*HookError](failure); !ok {
					t.Fatalf("error %v is not a *HookError", failure)
				}
			}
		}
		if err != nil {
			return
		}
		encoded := value.Bytes()
		if len(encoded) > HookInputLimit {
			return
		}
		again, err := ParseHookJSON(encoded)
		if err != nil {
			t.Fatalf("re-parse of %q: %v", encoded, err)
		}
		if !bytes.Equal(again.Bytes(), encoded) {
			t.Fatalf("encoding is not stable: %q then %q", encoded, again.Bytes())
		}
	})
}
