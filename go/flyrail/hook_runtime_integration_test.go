//go:build integration && (darwin || linux)

package flyrail

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

const (
	hookCallTimeout = 30 * time.Second
	hookWaitDelay   = 5 * time.Second
)

const hookToolNotice = "SKIP: hook runtime integration requires Node 24+, npm and Bun 1.4.2+"

func hookVersionsSupported(ci bool, node, npm, bun string) bool {
	if ci {
		return node == "24.21.0" && npm == "11.19.0" && bun == "1.4.2"
	}
	major, err := strconv.Atoi(strings.Split(node, ".")[0])
	if err != nil {
		return false
	}
	parts := []int{}
	for _, text := range strings.Split(bun, ".") {
		part, err := strconv.Atoi(text)
		if err != nil {
			return false
		}
		parts = append(parts, part)
	}
	return major >= 24 && slices.Compare(parts, []int{1, 4, 2}) >= 0
}

func TestHookRuntimeCIRequiresTheExactToolchain(t *testing.T) {
	cases := []struct {
		ci             bool
		node, npm, bun string
		supported      bool
	}{
		{true, "24.21.0", "11.19.0", "1.4.2", true},
		{true, "24.21.0", "11.18.0", "1.4.2", false},
		{true, "24.20.0", "11.19.0", "1.4.2", false},
		{true, "24.21.0", "11.19.0", "1.4.1", false},
		{false, "25.0.0", "10.0.0", "1.5.0", true},
		{false, "24.0.0", "10.0.0", "1.4.2", true},
		{false, "23.9.0", "10.0.0", "1.5.0", false},
		{false, "24.0.0", "10.0.0", "1.4.1", false},
	}
	for _, c := range cases {
		if got := hookVersionsSupported(c.ci, c.node, c.npm, c.bun); got != c.supported {
			t.Errorf("hookVersionsSupported(%v, %s, %s, %s) = %v, want %v", c.ci, c.node, c.npm, c.bun, got, c.supported)
		}
	}
}

func toolVersion(path string) (string, bool) {
	ctx, cancel := context.WithTimeout(context.Background(), hookCallTimeout)
	defer cancel()
	command := exec.CommandContext(ctx, path, "--version")
	command.WaitDelay = hookWaitDelay
	output, err := command.Output()
	if err != nil {
		return "", false
	}
	return strings.TrimPrefix(strings.TrimSpace(string(output)), "v"), true
}

func hookTools() (node, bun string, found bool) {
	paths := map[string]string{}
	versions := map[string]string{}
	for _, name := range []string{"node", "npm", "bun"} {
		path, err := exec.LookPath(name)
		if err != nil {
			return "", "", false
		}
		absolute, err := filepath.Abs(path)
		if err != nil {
			return "", "", false
		}
		version, ok := toolVersion(absolute)
		if !ok {
			return "", "", false
		}
		paths[name], versions[name] = absolute, version
	}
	_, ci := os.LookupEnv("CI")
	if !hookVersionsSupported(ci, versions["node"], versions["npm"], versions["bun"]) {
		return "", "", false
	}
	return paths["node"], paths["bun"], true
}

func requireHookTools(t *testing.T) (node, bun string) {
	t.Helper()
	node, bun, found := hookTools()
	if !found {
		if _, ci := os.LookupEnv("CI"); ci {
			t.Fatalf("%s; CI acceptance jobs may not skip", hookToolNotice)
		}
		t.Skip(hookToolNotice)
	}
	return node, bun
}

func hookPlatform() Platform {
	if runtime.GOOS == "darwin" {
		return PlatformMacOS
	}
	return PlatformLinux
}

func renderedHookRuntime(t *testing.T, node string) string {
	t.Helper()
	root := tempRoot(t)
	hookRuntime := must(NewHookRuntime(node, ShellPOSIX))
	bundle := hookBundle(t, hookManifest("start", "session-start", "continue"))
	rendered := must(Render(bundle, hookContext(t, AgentClaude, root, false, hookPlatform(), hookRuntime)))
	index := slices.IndexFunc(rendered.Artifacts(), func(artifact RenderedArtifact) bool {
		_, tree := artifact.Content().(TreeContent)
		return tree
	})
	if index < 0 {
		t.Fatal("rendered hooks have no runtime tree")
	}
	artifact := rendered.Artifacts()[index]
	entries := artifact.Content().(TreeContent).Entries()
	written := map[string][]byte{}
	for _, entry := range entries {
		path := filepath.Join(artifact.Destination(), entry.Path)
		if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, entry.Data, 0o600); err != nil {
			t.Fatal(err)
		}
		written[entry.Path] = entry.Data
	}
	for _, resource := range hookRuntime.Resources() {
		if !bytes.Equal(written[resource.Path], resource.Data) {
			t.Fatalf("rendered %s differs from the embedded runtime", resource.Path)
		}
	}
	if _, ok := written["config.mts"]; !ok || len(written) != len(hookRuntime.Resources())+1 {
		t.Fatalf("rendered runtime tree holds %d entries", len(written))
	}
	return artifact.Destination()
}

type hookHarnessResult map[string]any

func invokeHookHarness(t *testing.T, binary string, request map[string]any) hookHarnessResult {
	t.Helper()
	encoded, err := json.Marshal(request)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(t.Context(), hookCallTimeout)
	defer cancel()
	command := exec.CommandContext(ctx, binary, filepath.Join("..", "..", "rust", "tests", "hook_runtime_harness.mjs"))
	command.WaitDelay = hookWaitDelay
	command.Stdin = bytes.NewReader(encoded)
	var stdout, stderr bytes.Buffer
	command.Stdout, command.Stderr = &stdout, &stderr
	if err := command.Run(); err != nil {
		if ctx.Err() != nil {
			t.Fatalf("hook harness exceeded %s: %v: %s", hookCallTimeout, err, stderr.String())
		}
		t.Fatalf("hook harness failed: %v: %s", err, stderr.String())
	}
	if stderr.Len() != 0 {
		t.Fatalf("hook harness stderr: %s", stderr.String())
	}
	decoder := json.NewDecoder(&stdout)
	decoder.UseNumber()
	var result hookHarnessResult
	if err := decoder.Decode(&result); err != nil {
		t.Fatalf("parse hook harness output: %v", err)
	}
	return result
}

func (r hookHarnessResult) decoded(t *testing.T, key string) []byte {
	t.Helper()
	text, ok := r[key].(string)
	if !ok {
		t.Fatalf("harness result has no %s: %v", key, r)
	}
	data, err := base64.StdEncoding.DecodeString(text)
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func base64Text(text string) string {
	return base64.StdEncoding.EncodeToString([]byte(text))
}

func runRequest(root string, handler map[string]any) map[string]any {
	return map[string]any{
		"action": "run", "runtime": root,
		"config": map[string]any{"host": "test", "handlers": []any{handler}}, "input": beforeToolInput(),
	}
}

func runHandler(root string, argv []string, timeout int) map[string]any {
	return map[string]any{
		"id": "handler", "event": "before-tool", "argv": argv, "env": map[string]any{}, "cwd": root,
		"timeout_ms": timeout, "outcomes": []string{"continue", "block", "ask", "context", "modify-input"},
		"tools": []string{},
	}
}

func beforeToolInput() map[string]any {
	return map[string]any{"version": 1, "event": "before-tool", "tool": "Bash", "input": map[string]any{}, "context": map[string]any{}}
}

func unorderedJSON(t *testing.T, value jsonx.Value) any {
	t.Helper()
	decoder := json.NewDecoder(bytes.NewReader(jsonx.Encode(value)))
	decoder.UseNumber()
	var result any
	if err := decoder.Decode(&result); err != nil {
		t.Fatal(err)
	}
	return result
}

func hookField(t *testing.T, value jsonx.Value, key string) jsonx.Value {
	t.Helper()
	found, ok := value.Get(key)
	if !ok {
		t.Fatalf("fixture value has no %q", key)
	}
	return found
}

func expectedPacket(t *testing.T, item jsonx.Value, root string) []byte {
	t.Helper()
	host, _ := hookField(t, item, "host").AsString()
	context, _ := hookField(t, item, "context").Members()
	order := map[string][]string{
		"pi":       {"tool_call_id", "source"},
		"opencode": {"session_id", "tool_call_id"},
		"copilot":  {"cwd", "session_id", "timestamp", "stop_hook_active", "stop_reason", "tool_call_id"},
	}
	if keys, ok := order[host]; ok {
		ordered := []jsonx.Member{}
		switch host {
		case "pi":
			ordered = append(ordered, jsonx.Member{Key: "cwd", Value: jsonx.String(root)},
				jsonx.Member{Key: "execution_mode", Value: jsonx.String("print")},
				jsonx.Member{Key: "has_ui", Value: jsonx.Bool(false)})
		case "opencode":
			ordered = append(ordered, jsonx.Member{Key: "cwd", Value: jsonx.String(root)})
		}
		object := jsonx.Object(context...)
		for _, key := range keys {
			if value, ok := object.Get(key); ok {
				ordered = append(ordered, jsonx.Member{Key: key, Value: value})
			}
		}
		context = ordered
	}
	packet := []jsonx.Member{
		{Key: "version", Value: jsonx.Int(1)},
		{Key: "event", Value: hookField(t, item, "event")},
		{Key: "context", Value: jsonx.Object(context...)},
	}
	canonical := hookField(t, item, "canonical")
	for _, key := range []string{"tool", "input", "result"} {
		if value, ok := canonical.Get(key); ok {
			packet = append(packet, jsonx.Member{Key: key, Value: value})
		}
	}
	return jsonx.Encode(jsonx.Object(packet...))
}

func TestAllSharedProtocolVectorsExecuteThroughTheRenderedRuntime(t *testing.T) {
	node, bun := requireHookTools(t)
	data := fixture(t, "hooks.json")
	var raw struct {
		Cases []json.RawMessage `json:"cases"`
	}
	if err := json.Unmarshal(data, &raw); err != nil {
		t.Fatal(err)
	}
	if len(raw.Cases) != 58 {
		t.Fatalf("hooks.json has %d cases, want 58", len(raw.Cases))
	}
	root := renderedHookRuntime(t, node)
	capture := filepath.Join(root, "capture.json")
	for _, rawCase := range raw.Cases {
		item := must(jsonx.Parse(rawCase))
		name, _ := hookField(t, item, "name").AsString()
		host, _ := hookField(t, item, "host").AsString()
		binary := node
		if host == "opencode" {
			binary = bun
		}
		result := invokeHookHarness(t, binary, map[string]any{
			"action": "case", "runtime": root, "capture": capture, "cwd": root, "node": node, "case": rawCase,
		})
		if diff := cmp.Diff(unorderedJSON(t, hookField(t, item, "expected")), result["observed"]); diff != "" {
			t.Errorf("%s: native output (-want +got):\n%s", name, diff)
		}
		if got, want := result.decoded(t, "packetBase64"), expectedPacket(t, item, root); !bytes.Equal(got, want) {
			t.Errorf("%s: canonical packet\n got %s\nwant %s", name, got, want)
		}
	}
}

func TestNodeAndBunEnforceProtocolAndProcessBounds(t *testing.T) {
	node, bun := requireHookTools(t)
	root := renderedHookRuntime(t, node)
	for _, binary := range []string{node, bun} {
		for _, c := range []struct{ data, error string }{
			{"eyJhIjoxLCJhIjoyfQ==", "flyrail-hook:json-key"},
			{"eyJfX3Byb3RvX18iOnt9fQ==", "flyrail-hook:json-key"},
			{"eyJ4IjoxZTQwMH0=", "flyrail-hook:json-number"},
			{"/w==", "flyrail-hook:invalid-json"},
		} {
			result := invokeHookHarness(t, binary, map[string]any{"action": "parse", "runtime": root, "data": c.data})
			if result["error"] != c.error {
				t.Errorf("%s parse %s: %v, want %s", binary, c.data, result, c.error)
			}
		}
		for _, c := range []struct{ script, error string }{
			{"process.exit(7)", "flyrail-hook:exit"},
			{"process.kill(process.pid,'SIGTERM')", "flyrail-hook:exit"},
			{"setInterval(()=>{},1000)", "flyrail-hook:timeout"},
			{"process.stdout.write('x'.repeat(65537))", "flyrail-hook:stdout-limit"},
			{"process.stderr.write('x'.repeat(16385))", "flyrail-hook:stderr-limit"},
			{"process.stdout.write(Buffer.from([255]))", "flyrail-hook:invalid-output"},
			{"process.stdout.write('{')", "flyrail-hook:invalid-output"},
			{`process.stdout.write('{"version":1,"outcome":"continue","extra":true}')`, "flyrail-hook:output-fields"},
			{`process.stdout.write('{"version":1,"outcome":"block"}')`, "flyrail-hook:output-fields"},
			{`process.stdout.write('{"version":1,"outcome":"block","reason":""}')`, "flyrail-hook:reason"},
			{`process.stdout.write('{"version":1,"outcome":"context","text":""}')`, "flyrail-hook:context"},
			{`process.stdout.write('{"version":1,"outcome":"modify-input","input":[]}')`, "flyrail-hook:replacement"},
			{`process.stdout.write('{"version":2,"outcome":"continue"}')`, "flyrail-hook:undeclared-outcome"},
			{`process.stdout.write('{"version":1,"outcome":"permit"}')`, "flyrail-hook:undeclared-outcome"},
			{`process.stdout.write('{"version":1,"version":1,"outcome":"continue"}')`, "flyrail-hook:invalid-output"},
			{`process.stdout.write('{"version":1,"outcome":"continue"}');process.stderr.write(Buffer.from([255]))`, "flyrail-hook:invalid-output"},
		} {
			timeout := 2000
			if strings.HasPrefix(c.script, "setInterval") {
				timeout = 20
			}
			result := invokeHookHarness(t, binary, runRequest(root, runHandler(root, []string{node, "-e", c.script}, timeout)))
			if result["error"] != c.error {
				t.Errorf("%s run %s: %v, want %s", binary, c.script, result, c.error)
			}
		}
		for _, change := range []func(map[string]any){
			func(handler map[string]any) {
				handler["argv"] = []string{filepath.Join(root, "missing-private-command")}
			},
			func(handler map[string]any) { handler["cwd"] = filepath.Join(root, "missing-private-cwd") },
		} {
			handler := runHandler(root, []string{node, "-e", ""}, 2000)
			handler["outcomes"] = []string{"continue"}
			change(handler)
			result := invokeHookHarness(t, binary, runRequest(root, handler))
			if diff := cmp.Diff(hookHarnessResult{"error": "flyrail-hook:spawn"}, result); diff != "" {
				t.Errorf("%s spawn failure (-want +got):\n%s", binary, diff)
			}
		}
		handler := runHandler(root, []string{node, "-e", `process.stdout.write('{"version":1,"outcome":"continue"}')`}, 1000)
		handler["outcomes"] = []string{"continue"}
		request := runRequest(root, handler)
		request["clockStep"] = 150001
		deadline := invokeHookHarness(t, binary, request)
		if diff := cmp.Diff(hookHarnessResult{"error": "flyrail-hook:event-timeout"}, deadline); diff != "" {
			t.Errorf("%s event deadline (-want +got):\n%s", binary, diff)
		}
		for _, c := range []struct {
			heldOpen bool
			error    string
		}{{false, "flyrail-hook:input-limit"}, {true, "flyrail-hook:stdin-timeout"}} {
			result := invokeHookHarness(t, binary, map[string]any{"action": "readInput", "runtime": root, "heldOpen": c.heldOpen})
			if diff := cmp.Diff(hookHarnessResult{"error": c.error}, result); diff != "" {
				t.Errorf("%s readInput heldOpen=%v (-want +got):\n%s", binary, c.heldOpen, diff)
			}
		}
		marker := filepath.Join(root, "descendant-"+filepath.Base(binary))
		markerJSON := must(json.Marshal(marker))
		child := "setTimeout(()=>require('node:fs').writeFileSync(" + string(markerJSON) + ",'ran'),500)"
		childJSON := must(json.Marshal(child))
		script := "require('node:child_process').spawn(process.execPath,['-e'," + string(childJSON) +
			`],{stdio:'inherit'});process.stdout.write('{"version":1,"outcome":"continue"}');process.exit(0)`
		handler = runHandler(root, []string{node, "-e", script}, 1000)
		handler["outcomes"] = []string{"continue"}
		descendant := invokeHookHarness(t, binary, runRequest(root, handler))
		want := hookHarnessResult{"result": map[string]any{"contexts": []any{}}}
		if diff := cmp.Diff(want, descendant); diff != "" {
			t.Errorf("%s descendant run (-want +got):\n%s", binary, diff)
		}
		time.Sleep(600 * time.Millisecond)
		if _, err := os.Lstat(marker); !os.IsNotExist(err) {
			t.Errorf("%s: descendant pipe holder survived cleanup (%v)", binary, err)
		}
		handler = runHandler(root, []string{node, "-e",
			"process.stdout.write(JSON.stringify({version:1,outcome:'context',text:JSON.stringify([process.env.__proto__,process.env.FOUND,process.cwd()])}))"}, 1000)
		handler["env"] = map[string]any{"__proto__": "literal", "FOUND": map[string]any{"ref": "PATH"}}
		handler["outcomes"] = []string{"context"}
		handler["tools"] = []string{"Bash"}
		environment := invokeHookHarness(t, binary, runRequest(root, handler))
		contexts, _ := environment["result"].(map[string]any)["contexts"].([]any)
		if len(contexts) != 1 {
			t.Fatalf("%s environment run: %v", binary, environment)
		}
		var captured []string
		if err := json.Unmarshal([]byte(contexts[0].(string)), &captured); err != nil {
			t.Fatal(err)
		}
		if diff := cmp.Diff([]string{"literal", os.Getenv("PATH"), root}, captured); diff != "" {
			t.Errorf("%s handler environment (-want +got):\n%s", binary, diff)
		}
	}
}

func TestGoUTF16StringBytesMatchTheRenderedNodeRuntime(t *testing.T) {
	node, _ := requireHookTools(t)
	root := renderedHookRuntime(t, node)
	for _, source := range []string{
		`"\ud800"`,
		`"\udfff"`,
		`"\ud83d\ude00"`,
		`{"\ud800":"\udfff"}`,
		`{"\ud83d\ude00":"pair"}`,
	} {
		result := invokeHookHarness(t, node, map[string]any{"action": "parse", "runtime": root, "data": base64Text(source)})
		parsed, err := ParseHookJSON([]byte(source))
		if err != nil {
			t.Fatalf("%s: %v", source, err)
		}
		if got := result.decoded(t, "bytesBase64"); !bytes.Equal(parsed.Bytes(), got) {
			t.Errorf("%s: Go %q, runtime %q", source, parsed.Bytes(), got)
		}
	}
}

func TestTypedUTF16PacketsMatchTheRenderedNodeRuntime(t *testing.T) {
	node, _ := requireHookTools(t)
	root := renderedHookRuntime(t, node)
	declared := []HookOutcome{OutcomeContext, OutcomeBlock, OutcomeAsk, OutcomeModifyInput}
	for _, escaped := range []string{`\ud800`, `\udfff`, `\ud83d\ude00`} {
		for _, c := range []struct {
			input, final string
			event        HookEvent
		}{
			{`{"version":1,"event":"prompt","context":{},"input":"` + escaped + `"}`, "block", EventPrompt},
			{`{"version":1,"event":"before-tool","context":{},"tool":"` + escaped + `","input":{}}`, "ask", EventBeforeTool},
		} {
			var responses []string
			if c.final == "block" {
				responses = append(responses, `{"version":1,"outcome":"modify-input","input":"`+escaped+`"}`)
			}
			responses = append(responses,
				`{"version":1,"outcome":"context","text":"`+escaped+`"}`,
				`{"version":1,"outcome":"`+c.final+`","reason":"`+escaped+`"}`)
			encoded := make([]string, len(responses))
			for index, response := range responses {
				encoded[index] = base64Text(response)
			}
			result := invokeHookHarness(t, node, map[string]any{
				"action": "typed", "runtime": root, "cwd": root, "node": node,
				"input": base64Text(c.input), "responses": encoded,
			})
			if _, failed := result["error"]; failed {
				t.Fatalf("%s: %v", c.input, result)
			}
			request, err := ParseHookRequest([]byte(c.input))
			if err != nil {
				t.Fatalf("%s: %v", c.input, err)
			}
			if got := result.decoded(t, "inputBase64"); !bytes.Equal(request.Bytes(), got) {
				t.Errorf("%s: Go request %q, runtime %q", c.input, request.Bytes(), got)
			}
			runtimeResponses, _ := result["responseBase64"].([]any)
			if len(runtimeResponses) != len(responses) {
				t.Fatalf("%s: runtime responses %v", c.input, runtimeResponses)
			}
			for index, response := range responses {
				parsed, err := ParseHookResponse([]byte(response), c.event, declared)
				if err != nil {
					t.Fatalf("%s: %v", response, err)
				}
				got, err := base64.StdEncoding.DecodeString(runtimeResponses[index].(string))
				if err != nil {
					t.Fatal(err)
				}
				if !bytes.Equal(parsed.Bytes(), got) {
					t.Errorf("%s: Go response %q, runtime %q", response, parsed.Bytes(), got)
				}
			}
			runtimeEffects := result.decoded(t, "effectsBase64")
			effects, err := ParseHookEffects(runtimeEffects)
			if err != nil {
				t.Fatalf("%s: effects %q: %v", c.input, runtimeEffects, err)
			}
			if !bytes.Equal(effects.Bytes(), runtimeEffects) {
				t.Errorf("%s: Go effects %q, runtime %q", c.input, effects.Bytes(), runtimeEffects)
			}
		}
	}
}

func TestMixedSemanticErrorPrecedenceMatchesTheRenderedNodeRuntime(t *testing.T) {
	node, _ := requireHookTools(t)
	root := renderedHookRuntime(t, node)
	deep := strings.Repeat("[", 65) + "0" + strings.Repeat("]", 65)
	for _, source := range []string{
		`{"__proto__":0,"deep":` + deep + `}`,
		`{"a":0,"a":1,"deep":` + deep + `}`,
		`{"value":1e400,"deep":` + deep + `}`,
		`{"deep":` + deep + `,"__proto__":0}`,
		`{"deep":` + deep + `,"a":0,"a":1}`,
		`{"deep":` + deep + `,"value":1e400}`,
		`{"__proto__":0,"deep":` + deep,
	} {
		result := invokeHookHarness(t, node, map[string]any{"action": "parse", "runtime": root, "data": base64Text(source)})
		_, err := ParseHookJSON([]byte(source))
		if err == nil {
			t.Fatalf("%s: Go accepted the document", source)
		}
		if result["error"] != err.Error() {
			t.Errorf("%s: Go %q, runtime %v", source, err.Error(), result["error"])
		}
	}
}

func TestGoNumberBytesMatchTheRenderedNodeRuntime(t *testing.T) {
	node, _ := requireHookTools(t)
	root := renderedHookRuntime(t, node)
	for _, source := range []string{
		"9007199254740991", "9007199254740992", "9007199254740993", "100000000000000000000",
		"1e21", "5e-324", "-0", "-0.0",
	} {
		result := invokeHookHarness(t, node, map[string]any{"action": "parse", "runtime": root, "data": base64Text(source)})
		parsed, err := ParseHookJSON([]byte(source))
		if err != nil {
			t.Fatalf("%s: %v", source, err)
		}
		if got := result.decoded(t, "bytesBase64"); !bytes.Equal(parsed.Bytes(), got) {
			t.Errorf("%s: Go %q, runtime %q", source, parsed.Bytes(), got)
		}
	}
}
