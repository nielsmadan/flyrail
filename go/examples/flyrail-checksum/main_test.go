package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"testing/iotest"
	"time"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail"
	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

const mcpURL = "https://checksum.example.invalid/mcp"

func tempRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	return root
}

func runCLI(args ...string) (code int, stdout, stderr string) {
	var out, errOut bytes.Buffer
	code = run(context.Background(), args, &out, &errOut)
	return code, out.String(), errOut.String()
}

func runReport(t *testing.T, want int, args ...string) map[string]any {
	t.Helper()
	code, stdout, stderr := runCLI(args...)
	if code != want {
		t.Fatalf("%v: exit %d, want %d\nstdout: %s\nstderr: %s", args, code, want, stdout, stderr)
	}
	var value map[string]any
	if err := json.Unmarshal([]byte(stdout), &value); err != nil {
		t.Fatalf("%v: %v\n%s", args, err, stdout)
	}
	return value
}

func field(t *testing.T, value any, path ...any) any {
	t.Helper()
	for _, step := range path {
		switch key := step.(type) {
		case string:
			object, ok := value.(map[string]any)
			if !ok {
				t.Fatalf("%v is not an object at %q", value, key)
			}
			value = object[key]
		case int:
			array, ok := value.([]any)
			if !ok || key >= len(array) {
				t.Fatalf("%v has no index %d", value, key)
			}
			value = array[key]
		}
	}
	return value
}

func keys(t *testing.T, value any) []string {
	t.Helper()
	object, ok := value.(map[string]any)
	if !ok {
		t.Fatalf("%v is not an object", value)
	}
	names := make([]string, 0, len(object))
	for name := range object {
		names = append(names, name)
	}
	slices.Sort(names)
	return names
}

type boundedReader struct {
	remaining int
	largest   int
}

func (r *boundedReader) Read(buffer []byte) (int, error) {
	if len(buffer) > r.largest {
		return 0, errors.New("oversized read")
	}
	if r.remaining == 0 {
		return 0, io.EOF
	}
	count := min(len(buffer), r.remaining)
	for i := range count {
		buffer[i] = 0x5a
	}
	r.remaining -= count
	return count, nil
}

func TestDigestReaderUsesBoundedReads(t *testing.T) {
	sum, err := sha256Reader(&boundedReader{remaining: 32 << 20, largest: 64 << 10})
	if err != nil {
		t.Fatal(err)
	}
	if want := "371036ccdfc733fa30542a26a4f276536147c906d1570f0becc1d6f8b868c311"; sum != want {
		t.Fatalf("digest = %s, want %s", sum, want)
	}
}

func TestDigestReaderPropagatesErrorsAfterPartialInput(t *testing.T) {
	failure := errors.New("reader failed")
	_, err := sha256Reader(io.MultiReader(strings.NewReader("partial"), iotest.ErrReader(failure)))
	if !errors.Is(err, failure) {
		t.Fatalf("err = %v, want %v", err, failure)
	}
}

func TestValueOptionsRejectFollowingOptionsWithoutWrites(t *testing.T) {
	root := tempRoot(t)
	t.Chdir(root)
	options := []string{"--target", "--state", "--project", "--agent", "--node", "--mcp-command", "--mcp-arg", "--mcp-url", "--guidance", "--asset-root"}
	for _, option := range options {
		args := []string{"config", "status", option, "--replace-modified"}
		if option == "--target" || option == "--state" {
			args = []string{"ai", "install", option, "--replace-modified"}
		}
		code, stdout, stderr := runCLI(args...)
		if code != 2 || stdout != "" || !strings.Contains(stderr, option+" requires a value") {
			t.Errorf("%s: exit %d, stdout %q, stderr %q", option, code, stdout, stderr)
		}
	}
	entries, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 0 {
		t.Fatalf("usage errors wrote %v", entries)
	}
}

func TestConfigUsageErrors(t *testing.T) {
	project := tempRoot(t)
	required := []string{"--project", project, "--agent", "claude"}
	cases := []struct {
		args    []string
		message string
	}{
		{[]string{"config"}, "missing config action"},
		{[]string{"config", "remove"}, "unknown config action 'remove'"},
		{[]string{"config", "status", "--agent", "claude"}, "--project is required"},
		{[]string{"config", "status", "--project", project}, "--agent is required"},
		{[]string{"config", "status", "--project", project, "--agent", "robot"}, "'robot' is not a valid Agent"},
		{append([]string{"config", "status"}, required...), "--mcp-command is required"},
		{append([]string{"config", "status", "--mcp-command", "x"}, required...), "--mcp-url is required"},
		{append([]string{"config", "uninstall", "--guidance", "x"}, required...), "uninstall accepts only project, agent, and replacement options"},
		{append([]string{"config", "status", "--mcp-command", "x", "--mcp-url", mcpURL, "--replace-modified"}, required...), "status does not accept --replace-modified"},
		{append([]string{"config", "status", "extra"}, required...), "unknown option 'extra'"},
		{[]string{"ai", "status", "--target", project, "--state", project, "--replace-modified"}, "status does not accept --replace-modified"},
		{[]string{"checksum"}, "invalid command or arguments for 'checksum'"},
	}
	for _, tc := range cases {
		code, stdout, stderr := runCLI(tc.args...)
		if code != 2 || stdout != "" || !strings.Contains(stderr, tc.message) || !strings.Contains(stderr, "Usage:") {
			t.Errorf("%v: exit %d, stdout %q, stderr %q", tc.args, code, stdout, stderr)
		}
	}
}

func TestConfigPathsMatchStrictFalseResolution(t *testing.T) {
	root := tempRoot(t)
	actual := filepath.Join(root, "actual")
	child := filepath.Join(actual, "child")
	if err := os.MkdirAll(child, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("actual/child", filepath.Join(root, "link")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("actual/missing", filepath.Join(root, "broken")); err != nil {
		t.Fatal(err)
	}
	cases := []struct{ input, want string }{
		{root + "/link/../future", filepath.Join(actual, "future")},
		{root + "/broken/suffix", filepath.Join(actual, "missing", "suffix")},
		{filepath.Join(actual, "ordinary", "missing"), filepath.Join(actual, "ordinary", "missing")},
		{filepath.Join(root, "link"), child},
	}
	for _, tc := range cases {
		value := runReport(t, 1, "config", "status", "--project", tc.input, "--agent", "claude",
			"--mcp-command", "checksum-mcp", "--mcp-url", mcpURL)
		if got := field(t, value, "project"); got != tc.want {
			t.Errorf("%s: project = %v, want %s", tc.input, got, tc.want)
		}
		wantContext := []any{
			[]any{"agent", "claude"},
			[]any{"root", tc.want},
			[]any{"scope", "project"},
			[]any{"surface", "cli"},
		}
		if diff := cmp.Diff(wantContext, field(t, value, "observation", "target", "context")); diff != "" {
			t.Errorf("%s: context (-want +got):\n%s", tc.input, diff)
		}
		routing, _ := field(t, value, "observation", "target", "routing_context").([]any)
		for _, pair := range []any{[]any{"root", tc.want}, []any{"execution", tc.want}} {
			if !slices.ContainsFunc(routing, func(item any) bool { return cmp.Equal(item, pair) }) {
				t.Errorf("%s: routing context %v lacks %v", tc.input, routing, pair)
			}
		}
		if got, want := field(t, value, "observation", "target", "store"), tc.want+"/.cache/example-checksum-config"; got != want {
			t.Errorf("%s: store = %v, want %s", tc.input, got, want)
		}
		if got := field(t, value, "observation", "render_digest"); got != nil {
			t.Errorf("%s: render_digest = %v", tc.input, got)
		}
		if field(t, value, "configured_current") != false || field(t, value, "recorded_current") != false {
			t.Errorf("%s: current flags = %v, %v", tc.input, value["configured_current"], value["recorded_current"])
		}
	}
}

func nullable(value string) any {
	if value == "" {
		return nil
	}
	return value
}

func sortedPairs(values map[string]string) []any {
	pairs := []any{}
	for _, key := range slices.Sorted(maps.Keys(values)) {
		pairs = append(pairs, []any{key, values[key]})
	}
	return pairs
}

func jsonFields(value any) []string {
	typ := reflect.TypeOf(value)
	names := make([]string, 0, typ.NumField())
	for i := range typ.NumField() {
		names = append(names, strings.Split(typ.Field(i).Tag.Get("json"), ",")[0])
	}
	slices.Sort(names)
	return names
}

func TestReportShapeTracksPythonDataclassFields(t *testing.T) {
	python := os.Getenv("FLYRAIL_DIFFERENTIAL_PYTHON")
	if python == "" {
		t.Skip("SKIP: report shape comparison requires FLYRAIL_DIFFERENTIAL_PYTHON")
	}
	if !filepath.IsAbs(python) {
		python = filepath.Join("..", "..", python)
	}
	script := `
import dataclasses, json
from flyrail import (InstallationObservation, InstallationResult, InstallationTarget,
                     TargetInspection, TargetResult)
from flyrail._inventory import InventoryEntry
from flyrail._resource_models import ResourceRef
from flyrail.configuration import ClaimObservation, ResourceObservation, ResourceResult
from flyrail.observations import Installation, Observation
classes = (TargetInspection, TargetResult, Observation, Installation, InventoryEntry,
           InstallationTarget, InstallationObservation, ResourceObservation, ClaimObservation,
           InstallationResult, ResourceResult, ResourceRef)
print(json.dumps({c.__name__: sorted(f.name for f in dataclasses.fields(c)) for c in classes},
                 sort_keys=True))
`
	output, err := exec.Command(python, "-c", script).Output()
	if err != nil {
		t.Fatalf("Python shape probe: %v", err)
	}
	var actual map[string][]string
	if err := json.Unmarshal(output, &actual); err != nil {
		t.Fatal(err)
	}
	reports := map[string][]string{
		"ClaimObservation":        jsonFields(claimJSON{}),
		"Installation":            jsonFields(installationJSON{}),
		"InstallationObservation": jsonFields(installationObservationJSON{}),
		"InstallationResult":      {"error", "notices", "observation", "resources", "status"},
		"InstallationTarget":      jsonFields(installationTargetJSON{}),
		"InventoryEntry":          jsonFields(entryJSON{}),
		"Observation":             jsonFields(observationJSON{}),
		"ResourceObservation":     jsonFields(resourceObservationJSON{}),
		"ResourceRef":             jsonFields(resourceRefJSON{}),
		"ResourceResult":          jsonFields(resourceResultJSON{}),
		"TargetInspection":        slices.DeleteFunc(jsonFields(inspectionJSON{}), func(name string) bool { return name == "is_current" }),
		"TargetResult":            slices.DeleteFunc(jsonFields(resultJSON{}), func(name string) bool { return name == "is_current" }),
	}
	if diff := cmp.Diff(reports, actual); diff != "" {
		t.Errorf("Python fields differ from the report shape (-report +python):\n%s", diff)
	}
}

func TestVersionDigestUsageAndEmbeddedResources(t *testing.T) {
	root := tempRoot(t)
	sampleFile := filepath.Join(root, "sample.bin")
	if err := os.WriteFile(sampleFile, []byte("\x00one\ntwo"), 0o600); err != nil {
		t.Fatal(err)
	}
	cases := []struct {
		args   []string
		code   int
		stdout string
	}{
		{[]string{"--version"}, 0, "flyrail-checksum 2.1.0\n"},
		{[]string{"digest", sampleFile}, 0, "f8407b660c5c1645065501e7eaa5ba03868a156609cb45314d4ad01e3838414a\n"},
		{[]string{"digest", filepath.Join(root, "missing")}, 2, ""},
		{[]string{"ai", "install"}, 2, ""},
	}
	for _, tc := range cases {
		code, stdout, stderr := runCLI(tc.args...)
		if code != tc.code || stdout != tc.stdout {
			t.Errorf("%v: exit %d stdout %q, want %d %q (stderr %q)", tc.args, code, stdout, tc.code, tc.stdout, stderr)
		}
	}

	bundle, err := packagedBundle()
	if err != nil {
		t.Fatal(err)
	}
	if bundle.ID() != "example-checksum" {
		t.Errorf("bundle ID = %s", bundle.ID())
	}
	entries := map[string]flyrail.BundleEntry{}
	for _, entry := range bundle.Entries() {
		entries[entry.Path] = entry
	}
	if got := entries["checksum-review/assets/sample.bin"].Data; !bytes.Equal(got, []byte("\x00\x01\x02\n\r\x7f\x80\xfe\xffFLYRAIL\x00")) {
		t.Errorf("sample.bin = %q", got)
	}
	if !entries["checksum-review/scripts/verify_checksum.py"].Executable {
		t.Error("verify_checksum.py is not executable")
	}
	if _, ok := entries["checksum-review/scratch/README.txt"]; !ok {
		t.Errorf("scratch placeholder missing from %v", slices.Sorted(maps.Keys(entries)))
	}
}

func TestNonUTF8PathsWorkAndNonUTF8TextIsRejected(t *testing.T) {
	root := tempRoot(t)
	file := filepath.Join(root, "sample-\xff.bin")
	err := os.WriteFile(file, []byte("payload"), 0o600)
	switch {
	case err == nil:
		code, stdout, stderr := runCLI("digest", file)
		if code != 0 || stdout != "239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5\n" {
			t.Errorf("digest: exit %d stdout %q stderr %q", code, stdout, stderr)
		}
	case runtime.GOOS == "darwin" && errors.Is(err, syscall.EILSEQ):
		t.Logf("SKIP: filesystem rejects non-UTF-8 file names (%v); still checking non-UTF-8 text", err)
	case errors.Is(err, fs.ErrPermission) && os.Getenv("CI") == "":
		t.Skip("SKIP: active sandbox rejects non-UTF-8 filesystem names")
	default:
		t.Fatal(err)
	}
	if code, _, stderr := runCLI("digest", filepath.Join(root, "missing-\xff")); code != 2 || stderr == "" {
		t.Errorf("missing non-UTF-8 path: exit %d stderr %q", code, stderr)
	}
	code, _, stderr := runCLI("config", "status", "--project", root, "--agent", "bad-\xff")
	if code != 2 || !strings.Contains(stderr, "is not a valid Agent") {
		t.Errorf("non-UTF-8 agent: exit %d stderr %q", code, stderr)
	}
}

func TestDigestStreamsABoundedGeneratedFile(t *testing.T) {
	file := filepath.Join(tempRoot(t), "large.bin")
	if err := os.WriteFile(file, bytes.Repeat([]byte{0x5a}, 32<<20), 0o600); err != nil {
		t.Fatal(err)
	}
	code, stdout, stderr := runCLI("digest", file)
	if code != 0 || stdout != "371036ccdfc733fa30542a26a4f276536147c906d1570f0becc1d6f8b868c311\n" {
		t.Fatalf("digest: exit %d stdout %q stderr %q", code, stdout, stderr)
	}
}

func TestPackagedLifecycleIsRepeatableMultiTargetAndSourceIndependent(t *testing.T) {
	root := tempRoot(t)
	first := filepath.Join(root, "first")
	second := filepath.Join(root, "second")
	state := filepath.Join(root, "state")
	command := func(action string, extra ...string) []string {
		return append([]string{"ai", action, "--target", first, "--target", second, "--state", state}, extra...)
	}

	status := runReport(t, 1, command("status")...)
	if got := field(t, status, "results", 0, "observation", "state"); got != "absent" {
		t.Errorf("status state = %v", got)
	}
	if diff := cmp.Diff([]string{"alias_of", "is_current", "observation", "root", "target"}, keys(t, field(t, status, "results", 0))); diff != "" {
		t.Errorf("inspection keys (-want +got):\n%s", diff)
	}
	wantObservation := []string{"conflicts", "content_matches", "error", "installations", "installed", "modifications",
		"recorded_content_matches", "recovery_paths", "state", "version_matches"}
	if diff := cmp.Diff(wantObservation, keys(t, field(t, status, "results", 0, "observation"))); diff != "" {
		t.Errorf("observation keys (-want +got):\n%s", diff)
	}

	installed := runReport(t, 0, command("install")...)
	if got := field(t, installed, "results", 0, "status"); got != "applied" {
		t.Errorf("install = %v", got)
	}
	bundle, err := packagedBundle()
	if err != nil {
		t.Fatal(err)
	}
	st, err := store.NewDirectoryStore(context.Background(), state)
	if err != nil {
		t.Fatal(err)
	}
	target, err := flyrail.DirectoryTarget(first)
	if err != nil {
		t.Fatal(err)
	}
	inspected, err := flyrail.Inspect(context.Background(), bundle, []flyrail.Target{target}, st)
	if err != nil {
		t.Fatal(err)
	}
	expected := inspected[0].Observation.Installed
	if expected == nil || len(expected.Entries) == 0 {
		t.Fatalf("inspection found no installation: %+v", inspected[0].Observation)
	}
	entries := make([]any, 0, len(expected.Entries))
	sawFile, sawDir := false, false
	for _, entry := range expected.Entries {
		item := map[string]any{"path": entry.Path, "size": nil, "sha256": nil, "executable": entry.Executable}
		if entry.Dir {
			sawDir = true
		} else {
			sawFile = true
			item["size"] = float64(entry.Size)
			item["sha256"] = entry.SHA256
		}
		entries = append(entries, item)
	}
	if !sawFile || !sawDir {
		t.Fatalf("installation needs a file and a directory entry: %+v", expected.Entries)
	}
	wantInstalled := map[string]any{
		"bundle_id":      expected.BundleID,
		"version":        expected.Version,
		"content_digest": expected.ContentDigest,
		"transaction_id": expected.TransactionID,
		"entries":        entries,
	}
	if diff := cmp.Diff(wantInstalled, field(t, installed, "results", 0, "observation", "installed")); diff != "" {
		t.Errorf("installed (-library +report):\n%s", diff)
	}
	if diff := cmp.Diff([]string{"bundle_id", "content_digest", "entries", "transaction_id", "version"}, keys(t, wantInstalled)); diff != "" {
		t.Errorf("installation keys (-want +got):\n%s", diff)
	}
	if got := field(t, runReport(t, 0, command("install")...), "results", 1, "status"); got != "unchanged" {
		t.Errorf("repeat install = %v", got)
	}

	script := filepath.Join(first, "checksum-review", "scripts", "verify_checksum.py")
	info, err := os.Stat(script)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o755 {
		t.Errorf("verify_checksum.py mode = %v", info.Mode().Perm())
	}

	if err := os.WriteFile(filepath.Join(first, "checksum-review", "SKILL.md"), []byte("local edit"), 0o600); err != nil {
		t.Fatal(err)
	}
	modified := runReport(t, 1, command("update")...)
	if got := field(t, modified, "results", 0, "error", "code"); got != "modified" {
		t.Errorf("update error code = %v", got)
	}
	wantModification := map[string]any{"bundle_id": "example-checksum", "kind": "content_changed", "path": "checksum-review"}
	if diff := cmp.Diff(wantModification, field(t, modified, "results", 0, "observation", "modifications", 0)); diff != "" {
		t.Errorf("modification (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff([]any{}, field(t, modified, "results", 0, "recovery_paths")); diff != "" {
		t.Errorf("recovery paths (-want +got):\n%s", diff)
	}
	if got := field(t, runReport(t, 0, command("update", "--replace-modified")...), "results", 0, "status"); got != "applied" {
		t.Errorf("replacing update = %v", got)
	}
	if got := field(t, runReport(t, 0, command("uninstall")...), "results", 0, "status"); got != "applied" {
		t.Errorf("uninstall = %v", got)
	}
	if _, err := os.Lstat(first); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("%s still present after uninstall: %v", first, err)
	}
}

func node24() (string, bool) {
	path, err := exec.LookPath("node")
	if err != nil {
		return "", false
	}
	output, err := exec.Command(path, "--version").Output()
	if err != nil {
		return "", false
	}
	major, err := strconv.Atoi(strings.Split(strings.TrimPrefix(strings.TrimSpace(string(output)), "v"), ".")[0])
	return path, err == nil && major >= 24
}

func TestCombinedConfigurationExecutesHookAndCoversEveryAgent(t *testing.T) {
	if testing.Short() {
		t.Skip("SKIP: combined configuration runs hooks; integration tier only")
	}
	node, ok := node24()
	if !ok {
		const notice = "SKIP: combined configuration example requires Node 24+"
		if os.Getenv("CI") != "" {
			t.Fatalf("%s; CI may not skip", notice)
		}
		t.Skip(notice)
	}
	ctx := context.Background()
	root := tempRoot(t)
	project := filepath.Join(root, "project")
	if err := os.Mkdir(project, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(project, "CLAUDE.md"), []byte("# Existing\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	alias := filepath.Join(root, "project-alias")
	if err := os.Symlink(project, alias); err != nil {
		t.Fatal(err)
	}
	command := []string{"config", "status", "--project", alias, "--agent", "claude", "--node", node,
		"--mcp-command", "checksum-mcp", "--mcp-arg", "--", "--stdio", "--mcp-arg", "second", "--mcp-url", mcpURL}
	if got := field(t, runReport(t, 1, command...), "configured_current"); got != false {
		t.Errorf("initial configured_current = %v", got)
	}
	command[1] = "update"
	installed := runReport(t, 0, command...)

	bundle, err := configurationBundle(guidance, []string{"checksum-mcp", "--stdio", "second"}, mcpURL)
	if err != nil {
		t.Fatal(err)
	}
	canonicalNode, err := filepath.EvalSymlinks(node)
	if err != nil {
		t.Fatal(err)
	}
	hookRuntime, err := flyrail.NewHookRuntime(canonicalNode, flyrail.ShellPOSIX)
	if err != nil {
		t.Fatal(err)
	}
	target, err := flyrail.ProjectTarget(flyrail.AgentClaude, project)
	if err != nil {
		t.Fatal(err)
	}
	rc, err := flyrail.NewRenderContext(target, platform(), flyrail.RenderOptions{HookRuntime: hookRuntime})
	if err != nil {
		t.Fatal(err)
	}
	st, err := store.NewDirectoryStore(ctx, filepath.Join(project, ".cache", "example-checksum-config"))
	if err != nil {
		t.Fatal(err)
	}
	installation, err := rc.Installation(st, nil)
	if err != nil {
		t.Fatal(err)
	}
	rendered, err := flyrail.Render(bundle, rc)
	if err != nil {
		t.Fatal(err)
	}
	expected, err := flyrail.InspectInstallation(ctx, configBundleID, installation)
	if err != nil {
		t.Fatal(err)
	}
	if expected.BundleDigest != bundle.ContentDigest() || expected.RenderDigest != rendered.ContentDigest() {
		t.Errorf("recorded digests %s %s, want %s %s", expected.BundleDigest, expected.RenderDigest, bundle.ContentDigest(), rendered.ContentDigest())
	}
	for _, name := range []string{"configured_current", "recorded_current"} {
		if got := field(t, installed, name); got != true {
			t.Errorf("%s = %v", name, got)
		}
	}
	if got := field(t, installed, "activation"); got != "not-checked" {
		t.Errorf("activation = %v", got)
	}
	if expected.Target == nil || expected.Err != nil {
		t.Fatalf("recorded observation: %+v", expected)
	}
	location, hasLocation := expected.Target.Store().Location()
	var wantStore any
	if hasLocation {
		wantStore = location
	}
	wantResources := make([]any, 0, len(expected.Resources))
	for _, resource := range expected.Resources {
		claims := make([]any, 0, len(resource.Claims))
		for _, claim := range resource.Claims {
			claims = append(claims, map[string]any{
				"claim_id": claim.ClaimID, "bundle_id": claim.BundleID, "artifact_id": claim.ArtifactID,
				"version": claim.Version, "bundle_digest": claim.BundleDigest, "render_digest": claim.RenderDigest,
				"status": string(claim.Status),
			})
		}
		wantResources = append(wantResources, map[string]any{
			"destination":     resource.Destination,
			"claims":          claims,
			"revision_digest": nullable(resource.RevisionDigest),
			"error":           nil,
			"recovery_paths":  []any{},
		})
	}
	wantObservation := map[string]any{
		"bundle_id": expected.BundleID,
		"target": map[string]any{
			"store":           wantStore,
			"context":         sortedPairs(expected.Target.Context()),
			"routing_context": sortedPairs(expected.Target.RoutingContext()),
		},
		"is_current":    expected.Current,
		"pending":       expected.Pending,
		"version":       nullable(expected.Version),
		"bundle_digest": nullable(expected.BundleDigest),
		"render_digest": nullable(expected.RenderDigest),
		"error":         nil,
		"resources":     wantResources,
	}
	if diff := cmp.Diff(wantObservation, field(t, installed, "observation")); diff != "" {
		t.Errorf("observation (-library +report):\n%s", diff)
	}
	wantSummary := map[string]any{"is_current": true, "pending": false, "version": bundle.Version(), "render_digest": rendered.ContentDigest()}
	gotSummary := map[string]any{}
	for name := range wantSummary {
		gotSummary[name] = field(t, installed, "observation", name)
	}
	if diff := cmp.Diff(wantSummary, gotSummary); diff != "" {
		t.Errorf("observation summary (-want +got):\n%s", diff)
	}
	wantKeys := []string{"bundle_digest", "bundle_id", "error", "is_current", "pending", "render_digest", "resources", "target", "version"}
	if diff := cmp.Diff(wantKeys, keys(t, field(t, installed, "observation"))); diff != "" {
		t.Errorf("observation keys (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff([]string{"context", "routing_context", "store"}, keys(t, field(t, installed, "observation", "target"))); diff != "" {
		t.Errorf("target keys (-want +got):\n%s", diff)
	}
	routing, _ := field(t, installed, "observation", "target", "routing_context").([]any)
	for _, pair := range []any{[]any{"root", project}, []any{"agent", "claude"}} {
		if !slices.ContainsFunc(routing, func(item any) bool { return cmp.Equal(item, pair) }) {
			t.Errorf("routing context %v lacks %v", routing, pair)
		}
	}
	for _, resource := range field(t, installed, "observation", "resources").([]any) {
		if diff := cmp.Diff([]string{"claims", "destination", "error", "recovery_paths", "revision_digest"}, keys(t, resource)); diff != "" {
			t.Errorf("resource observation keys (-want +got):\n%s", diff)
		}
	}

	preview, err := flyrail.Preview(ctx, bundle, rendered, installation, flyrail.PreviewOptions{})
	if err != nil {
		t.Fatal(err)
	}
	resources := field(t, installed, "resources").([]any)
	if len(resources) != len(preview.Resources()) {
		t.Fatalf("%d resource results, preview plans %d", len(resources), len(preview.Resources()))
	}
	for index, plan := range preview.Resources() {
		want := map[string]any{"destination": plan.Resource().Destination, "kind": string(plan.Resource().Kind), "schema_version": 2.0}
		if diff := cmp.Diff(want, field(t, resources[index], "resource")); diff != "" {
			t.Errorf("resource %d (-want +got):\n%s", index, diff)
		}
		if diff := cmp.Diff([]string{"error", "recovery_paths", "resource", "status"}, keys(t, resources[index])); diff != "" {
			t.Errorf("resource %d keys (-want +got):\n%s", index, diff)
		}
	}
	skills := filepath.Join(project, ".claude", "skills")
	index := slices.IndexFunc(resources, func(item any) bool { return field(t, item, "resource", "destination") == skills })
	if index < 0 {
		t.Fatalf("no skill container among %v", resources)
	}
	wantSkills := map[string]any{
		"resource":       map[string]any{"destination": skills, "kind": "skill-container", "schema_version": 2.0},
		"status":         "applied",
		"error":          nil,
		"recovery_paths": []any{},
	}
	if diff := cmp.Diff(wantSkills, resources[index]); diff != "" {
		t.Errorf("skill container result (-want +got):\n%s", diff)
	}
	if prerequisites, _ := field(t, installed, "prerequisites").([]any); len(prerequisites) < 3 {
		t.Errorf("prerequisites = %v", prerequisites)
	}
	for _, path := range []string{".claude/skills/checksum-review/SKILL.md", "CLAUDE.md", ".mcp.json", ".claude/settings.json", ".flyrail-assets"} {
		if _, err := os.Stat(filepath.Join(project, path)); err != nil {
			t.Errorf("%s: %v", path, err)
		}
	}

	var mcp map[string]any
	readJSON(t, filepath.Join(project, ".mcp.json"), &mcp)
	wantMCP := map[string]any{
		"args":    []any{"--stdio", "second"},
		"env":     map[string]any{"CHECKSUM_MCP_TOKEN": "${CHECKSUM_MCP_TOKEN}"},
		"headers": map[string]any{"Authorization": "Bearer ${CHECKSUM_HTTP_TOKEN}"},
	}
	gotMCP := map[string]any{
		"args":    field(t, mcp, "mcpServers", "checksum-local", "args"),
		"env":     field(t, mcp, "mcpServers", "checksum-local", "env"),
		"headers": field(t, mcp, "mcpServers", "checksum-remote", "headers"),
	}
	if diff := cmp.Diff(wantMCP, gotMCP); diff != "" {
		t.Errorf(".mcp.json (-want +got):\n%s", diff)
	}

	var settings map[string]any
	readJSON(t, filepath.Join(project, ".claude", "settings.json"), &settings)
	hook := field(t, settings, "hooks", "PreToolUse", 0, "hooks", 0)
	hookCommand, _ := field(t, hook, "command").(string)
	var hookArgs []string
	for _, arg := range field(t, hook, "args").([]any) {
		hookArgs = append(hookArgs, arg.(string))
	}
	hookCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(hookCtx, hookCommand, hookArgs...)
	cmd.Dir = project
	cmd.Stdin = strings.NewReader(`{"tool_name":"Bash","tool_input":{}}`)
	var hookOut, hookErr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &hookOut, &hookErr
	if err := cmd.Run(); err != nil || hookErr.Len() != 0 {
		t.Fatalf("hook: %v\nstderr: %s", err, hookErr.String())
	}
	var hookResult any
	if err := json.Unmarshal(hookOut.Bytes(), &hookResult); err != nil {
		t.Fatalf("hook output %q: %v", hookOut.String(), err)
	}
	wantHook := map[string]any{"hookSpecificOutput": map[string]any{
		"hookEventName":     "PreToolUse",
		"additionalContext": "Verify a downloaded file with flyrail-checksum digest FILE before using it.",
	}}
	if diff := cmp.Diff(wantHook, hookResult); diff != "" {
		t.Errorf("hook output (-want +got):\n%s", diff)
	}
	assetIndex := slices.IndexFunc(resources, func(item any) bool {
		destination, _ := field(t, item, "resource", "destination").(string)
		return strings.Contains(destination, "/reminder-assets/")
	})
	if assetIndex < 0 {
		t.Fatal("no reminder asset resource")
	}
	reminderPath := field(t, resources[assetIndex], "resource", "destination").(string)
	if installedReminder, err := os.ReadFile(filepath.Join(reminderPath, "reminder.txt")); err != nil || !bytes.Equal(installedReminder, reminderText) {
		t.Errorf("installed reminder.txt = %q, %v", installedReminder, err)
	}

	if got := field(t, runReport(t, 0, command...), "status"); got != "unchanged" {
		t.Errorf("repeat update = %v", got)
	}
	realRoute := slices.Clone(command)
	realRoute[3] = project
	aliased, direct := runReport(t, 0, command...), runReport(t, 0, realRoute...)
	for _, path := range [][]any{{"project"}, {"observation", "target", "routing_context"}, {"observation", "render_digest"}, {"configured_current"}} {
		if diff := cmp.Diff(field(t, direct, path...), field(t, aliased, path...)); diff != "" {
			t.Errorf("%v differs between alias and real path (-real +alias):\n%s", path, diff)
		}
	}

	command[1] = "status"
	drift := runReport(t, 1, append(command, "--guidance", "Changed guidance")...)
	if field(t, drift, "recorded_current") != true || field(t, drift, "configured_current") != false {
		t.Errorf("drift: recorded %v configured %v", drift["recorded_current"], drift["configured_current"])
	}

	for _, agent := range flyrail.Agents() {
		supported := agent == flyrail.AgentClaude || agent == flyrail.AgentCodex
		candidate := filepath.Join(root, "agent-"+string(agent))
		args := []string{"config", "status", "--project", candidate, "--agent", string(agent), "--node", node,
			"--mcp-command", "checksum-mcp", "--mcp-arg", "one", "--mcp-arg", "two", "--mcp-url", mcpURL}
		status := runReport(t, 1, args...)
		if _, err := os.Lstat(candidate); !errors.Is(err, fs.ErrNotExist) {
			t.Errorf("status wrote for %s: %v", agent, err)
		}
		args[1] = "update"
		if !supported {
			notices, _ := field(t, status, "notices").([]any)
			if !slices.ContainsFunc(notices, func(notice any) bool { return field(t, notice, "kind") == "unsupported" }) {
				t.Errorf("%s: no unsupported notice in %v", agent, notices)
			}
			if got := field(t, runReport(t, 1, args...), "error", "code"); got != "unsupported" {
				t.Errorf("%s: update error code = %v", agent, got)
			}
			if _, err := os.Lstat(candidate); !errors.Is(err, fs.ErrNotExist) {
				t.Errorf("unsupported update wrote for %s: %v", agent, err)
			}
			continue
		}
		if got := field(t, runReport(t, 0, args...), "status"); got != "applied" {
			t.Errorf("%s: update = %v", agent, got)
		}
		removed := runReport(t, 0, "config", "uninstall", "--project", candidate, "--agent", string(agent))
		if got := field(t, removed, "status"); got != "applied" {
			t.Errorf("%s: uninstall = %v", agent, got)
		}
	}

	removed := runReport(t, 0, "config", "uninstall", "--project", project, "--agent", "claude")
	if field(t, removed, "status") != "applied" || field(t, removed, "bundle_version") != nil {
		t.Errorf("uninstall: status %v bundle_version %v", removed["status"], removed["bundle_version"])
	}
	if guidanceFile, err := os.ReadFile(filepath.Join(project, "CLAUDE.md")); err != nil || string(guidanceFile) != "# Existing\n" {
		t.Errorf("CLAUDE.md after uninstall = %q, %v", guidanceFile, err)
	}
}

func readJSON(t *testing.T, path string, value any) {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(data, value); err != nil {
		t.Fatalf("%s: %v", path, err)
	}
}

func TestOptionFormsOutsideTheSpacedGrammarAreUnknownOptions(t *testing.T) {
	ai := []string{"ai", "update", "--target", "/t", "--state", "/st"}
	config := []string{"config", "status", "--project", "/p", "--agent", "claude", "--mcp-command", "x", "--mcp-url", mcpURL}
	for _, base := range [][]string{ai, config} {
		for _, bad := range [][]string{{"-target", "/t2"}, {"--state=/s"}, {"--replace-modified=false"}, {"--"}, {"--bogus"}, {"-h"}, {"--help"}, {"extra"}} {
			code, stdout, stderr := runCLI(append(slices.Clone(base), bad...)...)
			want := "flyrail-checksum: unknown option '" + bad[0] + "'\nUsage:\n"
			if code != 2 || stdout != "" || !strings.HasPrefix(stderr, want) {
				t.Errorf("%v %v: exit %d, stdout %q, stderr %q, want prefix %q", base[:2], bad, code, stdout, stderr, want)
			}
		}
	}
}

func TestInvalidOptionValueUsesFlyrailWording(t *testing.T) {
	code, _, stderr := runCLI("config", "status", "--project", "/p", "--agent", "robot")
	if code != 2 || !strings.HasPrefix(stderr, "flyrail-checksum: 'robot' is not a valid Agent\nUsage:\n") {
		t.Errorf("exit %d, stderr %q", code, stderr)
	}
}

func TestOptionValueMayFollowSeparator(t *testing.T) {
	request, err := parseAI([]string{"update", "--target", "--", "--odd", "--state", "/s", "--replace-modified"})
	if err != nil {
		t.Fatal(err)
	}
	want := aiRequest{action: "update", targets: []string{"--odd"}, state: "/s", replaceModified: true}
	if diff := cmp.Diff(want, request, cmp.AllowUnexported(aiRequest{})); diff != "" {
		t.Errorf("request (-want +got):\n%s", diff)
	}
}

const mainArgsVariable = "FLYRAIL_EXAMPLE_MAIN_ARGS"

func TestMain(m *testing.M) {
	if args, ok := os.LookupEnv(mainArgsVariable); ok {
		os.Args = append([]string{os.Args[0]}, strings.Fields(args)...)
		main()
	}
	os.Exit(m.Run())
}

func TestFirstInterruptCancelsAndSecondExits(t *testing.T) {
	cmd := exec.Command(os.Args[0])
	cmd.Env = append(os.Environ(), mainArgsVariable+"=digest /dev/stdin")
	stdin, err := cmd.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan *os.ProcessState, 1)
	go func() {
		_ = cmd.Wait()
		done <- cmd.ProcessState
	}()
	t.Cleanup(func() { _ = cmd.Process.Kill() })
	// The write outgrows the pipe buffer, so it returns only once run reads stdin, after main installed its handler.
	if _, err := stdin.Write(make([]byte, 4<<20)); err != nil {
		t.Fatal(err)
	}
	if err := cmd.Process.Signal(os.Interrupt); err != nil {
		t.Fatal(err)
	}
	select {
	case state := <-done:
		t.Fatalf("first interrupt ended the process: %v", state)
	case <-time.After(300 * time.Millisecond):
	}
	retry := time.NewTicker(100 * time.Millisecond)
	defer retry.Stop()
	deadline := time.After(10 * time.Second)
	for {
		select {
		case state := <-done:
			status, ok := state.Sys().(syscall.WaitStatus)
			if !ok || !status.Signaled() || status.Signal() != syscall.SIGINT {
				t.Fatalf("process ended with %v, want death by SIGINT", state)
			}
			return
		case <-retry.C:
			if err := cmd.Process.Signal(os.Interrupt); err != nil && !errors.Is(err, os.ErrProcessDone) {
				t.Fatal(err)
			}
		case <-deadline:
			t.Fatal("second interrupt did not end the process")
		}
	}
}
