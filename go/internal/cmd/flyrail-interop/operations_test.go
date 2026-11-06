package main

import (
	"bufio"
	"bytes"
	"encoding/base64"
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func runWithInput(input string, args ...string) (code int, stdout, stderr string) {
	var out, errOut bytes.Buffer
	code = run(args, strings.NewReader(input), &out, &errOut)
	return code, out.String(), errOut.String()
}

func decodeLine(t *testing.T, line string) map[string]any {
	t.Helper()
	var value map[string]any
	if err := json.Unmarshal([]byte(line), &value); err != nil {
		t.Fatalf("%v: %q", err, line)
	}
	return value
}

func errorMessage(t *testing.T, stdout string) string {
	t.Helper()
	detail, _ := decodeLine(t, stdout)["error"].(map[string]any)
	message, _ := detail["message"].(string)
	return message
}

func sourceOptionCases(t *testing.T) [][2]string {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "..", "..", "..", "spec", "fixtures", "interop-source-options.json"))
	if os.IsNotExist(err) {
		if os.Getenv("FLYRAIL_REQUIRE_FIXTURES") == "1" {
			t.Fatal("fixture interop-source-options.json is required but missing")
		}
		t.Skip("skipping: spec fixture interop-source-options.json is not available")
	}
	if err != nil {
		t.Fatal(err)
	}
	var document struct{ Cases [][2]string }
	if err := json.Unmarshal(data, &document); err != nil {
		t.Fatal(err)
	}
	return document.Cases
}

func TestWriteRejectsGeneratedOptionsWithSourceBeforeTouchingTheStore(t *testing.T) {
	root := tempRoot(t)
	source := filepath.Join(root, "source")
	if err := os.Mkdir(source, 0o700); err != nil {
		t.Fatal(err)
	}
	writeText(t, filepath.Join(source, "flyrail.json"),
		`{"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":"instruction","id":"guide","text":"safe"}]}`)
	state, project := filepath.Join(root, "state"), filepath.Join(root, "project")
	cases := sourceOptionCases(t)
	if len(cases) == 0 {
		t.Fatal("fixture lists no source option cases")
	}
	for _, option := range append(cases, [2]string{"--bundle-id", "other"}) {
		code, stdout, stderr := runDriver("write", "--state", state, "--destination", project, "--source", source, option[0], option[1])
		want := option[0] + " cannot be combined with --source"
		if option[0] == "--bundle-id" {
			want = "source bundle id must match --bundle-id"
		}
		if code != 1 || errorMessage(t, stdout) != want || stderr != "" {
			t.Errorf("%v: got exit %d, stdout %q, stderr %q", option, code, stdout, stderr)
		}
		if _, err := os.Lstat(state); !os.IsNotExist(err) {
			t.Fatalf("%v: state directory exists (%v)", option, err)
		}
	}
}

func TestOptionParsingRejectsMalformedArguments(t *testing.T) {
	cases := []struct {
		args []string
		want string
	}{
		{[]string{"write", "--state", "a", "--state", "b"}, "duplicate option: --state"},
		{[]string{"remove", "--replace-modified", "--replace-modified"}, "duplicate option: --replace-modified"},
		{[]string{"inspect", "--state"}, "missing value for --state"},
		{[]string{"inspect", "--state", "--bundle-id"}, "missing value for --state"},
		{[]string{"recover", "--state", "s", "--payload", "x"}, "unknown option: --payload"},
		{[]string{"records", "positional"}, usage},
		{[]string{"inspect"}, "missing --state"},
		{[]string{"observe", "--destination", "/tmp/x", "--repeat", "0"}, "repeat must be between 1 and 1000"},
		{[]string{"inspect", "--state", "s", "--lock-timeout", "-1"}, "invalid --lock-timeout: must be finite and nonnegative"},
	}
	for _, tc := range cases {
		code, stdout, _ := runDriver(tc.args...)
		if code != 1 || errorMessage(t, stdout) != tc.want {
			t.Errorf("%v: got exit %d, stdout %q, want message %q", tc.args, code, stdout, tc.want)
		}
	}
}

func TestWriteRejectsInvalidSeamCombinations(t *testing.T) {
	root := tempRoot(t)
	base := []string{"write", "--state", filepath.Join(root, "state"), "--destination", filepath.Join(root, "owned")}
	cases := []struct {
		extra []string
		want  string
	}{
		{[]string{"--fail-index-completion", "--barrier-phase", "prepare"}, "fault and barrier options cannot be combined"},
		{[]string{"--barrier-phase", "prepare"}, "--barrier-phase requires --barrier-ready and --barrier-release"},
		{[]string{"--barrier-ready", "r"}, "--barrier-ready and --barrier-release require --barrier-phase"},
		{[]string{"--barrier-timeout", "1"}, "--barrier-timeout requires --barrier-phase"},
		{[]string{"--barrier-phase", "backup", "--barrier-ready", "r", "--barrier-release", "s"}, "invalid --barrier-phase: backup"},
		{[]string{"--large", "1048577"}, "large payload size must not exceed 1048576"},
		{[]string{"--large", "-1"}, "large payload size must be nonnegative"},
		{[]string{"--kind", "tree", "--section", "guide"}, "tree resources cannot have a section"},
		{[]string{"--agent", "codex"}, "--agent and --node require --source"},
	}
	for _, tc := range cases {
		code, stdout, _ := runDriver(append(append([]string{}, base...), tc.extra...)...)
		if code != 1 || errorMessage(t, stdout) != tc.want {
			t.Errorf("%v: got exit %d, stdout %q, want %q", tc.extra, code, stdout, tc.want)
		}
	}
}

func TestWriteInspectRecordsAndRemoveRoundTrip(t *testing.T) {
	root := tempRoot(t)
	state, destination := filepath.Join(root, "state"), filepath.Join(root, "AGENTS.md")
	writeText(t, destination, "baseline\n")
	code, stdout, _ := runDriver("write", "--state", state, "--destination", destination, "--large", "30", "--acquisition", "takeover")
	result := decodeLine(t, stdout)
	if code != 0 || result["status"] != "applied" || result["error"] != nil {
		t.Fatalf("write: exit %d, %s", code, stdout)
	}
	data, err := os.ReadFile(destination)
	if err != nil {
		t.Fatal(err)
	}
	if want := "flyrail-interop-payload\nflyrai"; string(data) != want {
		t.Errorf("payload %q, want %q", data, want)
	}

	_, stdout, _ = runDriver("inspect", "--state", state)
	observation := decodeLine(t, stdout)
	want := map[string]any{
		"bundle_id": "team", "is_current": true, "pending": false, "version": "1", "error": nil,
		"resources": []any{map[string]any{
			"destination":    destination,
			"claims":         []any{map[string]any{"artifact_id": "guide", "disposition": "current"}},
			"error":          nil,
			"recovery_paths": []any{},
		}},
	}
	if diff := cmp.Diff(want, observation); diff != "" {
		t.Errorf("inspect (-want +got):\n%s", diff)
	}

	_, stdout, _ = runDriver("records", "--state", state)
	var records struct {
		Records []struct{ Key, Type, Bytes, Canonical string }
	}
	if err := json.Unmarshal([]byte(stdout), &records); err != nil {
		t.Fatal(err)
	}
	types := map[string]int{}
	for _, record := range records.Records {
		types[record.Type]++
		if record.Bytes != record.Canonical {
			t.Errorf("%s: stored bytes are not canonical", record.Key)
		}
	}
	if diff := cmp.Diff(map[string]int{"Index": 1, "ResourceRef": 1, "Receipt": 1}, types); diff != "" {
		t.Errorf("record types (-want +got):\n%s", diff)
	}

	code, stdout, _ = runDriver("remove", "--state", state)
	if code != 0 || decodeLine(t, stdout)["status"] != "applied" {
		t.Fatalf("remove: exit %d, %s", code, stdout)
	}
	if data, _ := os.ReadFile(destination); string(data) != "baseline\n" {
		t.Errorf("restored %q", data)
	}
}

func TestEncodeTranscodesStoredRecordsAndChecksTheirType(t *testing.T) {
	root := tempRoot(t)
	state, destination := filepath.Join(root, "state"), filepath.Join(root, "owned")
	if code, stdout, _ := runDriver("write", "--state", state, "--destination", destination); code != 0 {
		t.Fatalf("write: %s", stdout)
	}
	_, stdout, _ := runDriver("records", "--state", state)
	var records struct {
		Records []struct{ Type, Bytes string }
	}
	if err := json.Unmarshal([]byte(stdout), &records); err != nil {
		t.Fatal(err)
	}
	for _, record := range records.Records {
		raw, err := base64.StdEncoding.DecodeString(record.Bytes)
		if err != nil {
			t.Fatal(err)
		}
		for _, args := range [][]string{{"encode"}, {"encode", "--type", record.Type}} {
			code, stdout, stderr := runWithInput(string(raw), args...)
			if code != 0 || stdout != string(raw) || stderr != "" {
				t.Errorf("%v %s: exit %d, stdout %q, stderr %q", args, record.Type, code, stdout, stderr)
			}
		}
		code, stdout, stderr := runWithInput(string(raw), "encode", "--type", "Journal")
		if code != 1 || stdout != "" || stderr != "metadata record has the wrong type\n" {
			t.Errorf("%s as Journal: exit %d, stdout %q, stderr %q", record.Type, code, stdout, stderr)
		}
	}
}

func TestEncodeReportsFailuresOnStandardError(t *testing.T) {
	code, stdout, stderr := runWithInput(strings.Repeat(" ", maxStateInput+1), "encode")
	if code != 1 || stdout != "" || stderr != "JSON input exceeds the byte limit\n" {
		t.Errorf("got exit %d, stdout %q, stderr %q", code, stdout, stderr)
	}
}

func TestHoldReservesTheResourceLockUntilTheBarrier(t *testing.T) {
	root := tempRoot(t)
	state, destination, barrierPath := filepath.Join(root, "state"), filepath.Join(root, "owned"), filepath.Join(root, "barrier")
	reader, writer := io.Pipe()
	done := make(chan int, 1)
	go func() {
		done <- run([]string{"hold", "--state", state, "--destination", destination, "--barrier", barrierPath},
			strings.NewReader(""), writer, io.Discard)
		_ = writer.Close()
	}()
	lines := bufio.NewScanner(reader)
	var got []string
	if lines.Scan() {
		got = append(got, lines.Text())
	}
	code, stdout, _ := runDriver("hold", "--state", state, "--destination", destination, "--barrier", barrierPath)
	if busy := errorMessage(t, stdout); code != 1 || !strings.HasPrefix(busy, "state store lock is busy: resource/") {
		t.Errorf("competing hold: exit %d, stdout %q", code, stdout)
	}
	writeText(t, barrierPath, "release\n")
	for lines.Scan() {
		got = append(got, lines.Text())
	}
	if code := <-done; code != 0 {
		t.Fatalf("hold exited %d", code)
	}
	if diff := cmp.Diff([]string{`{"status":"reserved"}`, `{"status":"admitted"}`}, got); diff != "" {
		t.Errorf("hold output (-want +got):\n%s", diff)
	}
}

func TestObserveReportsTheRevisionAndDescriptorStability(t *testing.T) {
	root := tempRoot(t)
	destination := filepath.Join(root, "owned")
	writeText(t, destination, "data")
	code, stdout, stderr := runDriver("observe", "--destination", destination, "--repeat", "3")
	lines := strings.Split(strings.TrimSuffix(stdout, "\n"), "\n")
	if code != 0 || len(lines) != 3 || stderr != "" {
		t.Fatalf("got exit %d, stdout %q, stderr %q", code, stdout, stderr)
	}
	observed := decodeLine(t, lines[0])
	resource, _ := observed["resource"].(map[string]any)
	revision, _ := observed["revision"].(map[string]any)
	if resource["type"] != "ResourceRef" || revision["type"] != "Revision" || observed["error"] != nil {
		t.Errorf("observation %s", lines[0])
	}

	_, stdout, _ = runDriver("observe", "--destination", filepath.Join(root, "missing"), "--kind", "tree")
	revision, _ = decodeLine(t, stdout)["revision"].(map[string]any)
	fields, _ := revision["fields"].(map[string]any)
	if diff := cmp.Diff(map[string]any{"nodes": []any{}}, fields); diff != "" {
		t.Errorf("missing destination revision (-want +got):\n%s", diff)
	}
}

func TestFailIndexCompletionLeavesAPartialUpdate(t *testing.T) {
	root := tempRoot(t)
	state, destination := filepath.Join(root, "state"), filepath.Join(root, "owned")
	if code, stdout, _ := runDriver("write", "--state", state, "--destination", destination, "--payload", "first"); code != 0 {
		t.Fatalf("write: %s", stdout)
	}
	code, stdout, _ := runDriver("update", "--state", state, "--destination", destination, "--version", "2",
		"--payload", "second", "--fail-index-completion")
	result := decodeLine(t, stdout)
	want := map[string]any{"code": "io_error", "message": "injected index-completion fault", "path": nil}
	if diff := cmp.Diff(want, result["error"]); code != 0 || result["status"] != "partial" || diff != "" {
		t.Errorf("exit %d, status %v, error (-want +got):\n%s", code, result["status"], diff)
	}
	if data, _ := os.ReadFile(destination); string(data) != "second" {
		t.Errorf("destination %q", data)
	}
}

func TestBarrierTimeoutIsReportedAsATimeoutNotAnInjectedFault(t *testing.T) {
	root := tempRoot(t)
	release := filepath.Join(root, "release")
	code, stdout, _ := runDriver("write", "--state", filepath.Join(root, "state"), "--destination", filepath.Join(root, "owned"),
		"--barrier-phase", "prepare", "--barrier-ready", filepath.Join(root, "ready"), "--barrier-release", release,
		"--barrier-timeout", "0.05")
	result := decodeLine(t, stdout)
	detail, _ := result["error"].(map[string]any)
	if code != 0 || result["status"] != "failed" || detail["code"] != "io_error" ||
		detail["message"] != "interop barrier timed out" || detail["path"] != release {
		t.Errorf("exit %d, stdout %s", code, stdout)
	}
}

func TestBarrierReadyFileFailureIsReportedAsAnIOError(t *testing.T) {
	root := tempRoot(t)
	ready := filepath.Join(root, "missing", "ready")
	code, stdout, _ := runDriver("write", "--state", filepath.Join(root, "state"), "--destination", filepath.Join(root, "owned"),
		"--barrier-phase", "prepare", "--barrier-ready", ready, "--barrier-release", filepath.Join(root, "release"),
		"--barrier-timeout", "0.05")
	result := decodeLine(t, stdout)
	detail, _ := result["error"].(map[string]any)
	if message, _ := detail["message"].(string); code != 0 || result["status"] != "failed" || detail["code"] != "io_error" ||
		detail["path"] != ready || strings.Contains(message, "injected") {
		t.Errorf("exit %d, stdout %s", code, stdout)
	}
}
