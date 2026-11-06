package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io/fs"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"
)

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

func copyTree(t *testing.T, source, destination string) {
	t.Helper()
	if err := os.CopyFS(destination, os.DirFS(source)); err != nil {
		t.Fatal(err)
	}
}

func TestValueOptionsRejectFollowingOptionsWithoutWrites(t *testing.T) {
	root := tempRoot(t)
	t.Chdir(root)
	for _, option := range []string{"--source", "--bundle-id", "--target", "--state"} {
		action := "install"
		if option == "--bundle-id" {
			action = "uninstall"
		}
		code, stdout, stderr := runCLI("ai", action, option, "--replace-modified")
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

func TestVersionWordsAndUsageContract(t *testing.T) {
	root := tempRoot(t)
	notes := filepath.Join(root, "notes.txt")
	if err := os.WriteFile(notes, []byte("one two\nthree\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	cases := []struct {
		args   []string
		code   int
		stdout string
	}{
		{[]string{"--version"}, 0, "flyrail-notes 0.4.0\n"},
		{[]string{"words", notes}, 0, "3\n"},
		{[]string{"words", filepath.Join(root, "missing")}, 2, ""},
		{[]string{"ai", "install"}, 2, ""},
		{[]string{"ai", "remove", "--state", root}, 2, ""},
		{[]string{"words"}, 2, ""},
		{nil, 2, ""},
	}
	for _, tc := range cases {
		code, stdout, stderr := runCLI(tc.args...)
		if code != tc.code || stdout != tc.stdout {
			t.Errorf("%v: exit %d stdout %q, want %d %q (stderr %q)", tc.args, code, stdout, tc.code, tc.stdout, stderr)
		}
		if tc.code == 2 && !strings.HasPrefix(stderr, "flyrail-notes: ") {
			t.Errorf("%v: stderr %q", tc.args, stderr)
		}
	}
}

func TestNonUTF8PathsWorkAndNonUTF8TextIsRejected(t *testing.T) {
	root := tempRoot(t)
	file := filepath.Join(root, "notes-\xff.txt")
	err := os.WriteFile(file, []byte("one two"), 0o600)
	switch {
	case err == nil:
		code, stdout, stderr := runCLI("words", file)
		if code != 0 || stdout != "2\n" {
			t.Errorf("words: exit %d stdout %q stderr %q", code, stdout, stderr)
		}
	case runtime.GOOS == "darwin" && errors.Is(err, syscall.EILSEQ):
		t.Logf("SKIP: filesystem rejects non-UTF-8 file names (%v); still checking non-UTF-8 text", err)
	case errors.Is(err, fs.ErrPermission) && os.Getenv("CI") == "":
		t.Skip("SKIP: active sandbox rejects non-UTF-8 filesystem names")
	default:
		t.Fatal(err)
	}

	if code, _, stderr := runCLI("words", filepath.Join(root, "missing-\xff")); code != 2 || stderr == "" {
		t.Errorf("missing non-UTF-8 path: exit %d stderr %q", code, stderr)
	}
	binary := filepath.Join(root, "binary.txt")
	if err := os.WriteFile(binary, []byte("one \xff two"), 0o600); err != nil {
		t.Fatal(err)
	}
	if code, _, stderr := runCLI("words", binary); code != 2 || !strings.Contains(stderr, "not valid UTF-8") {
		t.Errorf("non-UTF-8 text: exit %d stderr %q", code, stderr)
	}
	if code, _, _ := runCLI("ai", "uninstall", "--bundle-id", "bad-\xff", "--target", root); code != 2 {
		t.Errorf("non-UTF-8 bundle ID: exit %d", code)
	}
	state := filepath.Join(root, "state")
	code, _, stderr := runCLI("ai", "uninstall", "--bundle-id", "bad-\xff", "--target", filepath.Join(root, "skills"), "--state", state)
	if code != 2 || stderr == "" {
		t.Errorf("non-UTF-8 bundle ID with state: exit %d stderr %q", code, stderr)
	}
}

func TestLifecycleSupportsRepeatTargetsModificationsAndSourceFreeRemoval(t *testing.T) {
	root := tempRoot(t)
	source := filepath.Join(root, "source")
	copyTree(t, "bundle", source)
	first := filepath.Join(root, "first")
	second := filepath.Join(root, "second")
	state := filepath.Join(root, "state")
	targets := []string{"--target", first, "--target", second, "--state", state}
	command := func(action string, extra ...string) []string {
		args := append([]string{"ai", action, "--source", source}, targets...)
		return append(args, extra...)
	}

	status := runReport(t, 1, command("status")...)
	if got := field(t, status, "results", 0, "observation", "state"); got != "absent" {
		t.Errorf("status state = %v", got)
	}
	if got := field(t, status, "results", 0, "target", "scope"); got != "directory" {
		t.Errorf("target scope = %v", got)
	}

	installed := runReport(t, 0, command("install")...)
	for index := range 2 {
		if got := field(t, installed, "results", index, "status"); got != "applied" {
			t.Errorf("install result %d = %v", index, got)
		}
	}
	skillBytes, err := os.ReadFile(filepath.Join(source, "skills", "notes-summary", "SKILL.md"))
	if err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256(skillBytes)
	wantEntries := []any{
		map[string]any{"path": "notes-summary", "size": nil, "sha256": nil, "executable": false},
		map[string]any{"path": "notes-summary/SKILL.md", "size": float64(len(skillBytes)), "sha256": hex.EncodeToString(digest[:]), "executable": false},
	}
	if diff := cmp.Diff(wantEntries, field(t, installed, "results", 0, "observation", "installed", "entries")); diff != "" {
		t.Errorf("installed entries (-want +got):\n%s", diff)
	}
	wantInstalledKeys := []string{"bundle_id", "content_digest", "entries", "transaction_id", "version"}
	if diff := cmp.Diff(wantInstalledKeys, slices.Sorted(maps.Keys(field(t, installed, "results", 0, "observation", "installed").(map[string]any)))); diff != "" {
		t.Errorf("installed keys (-want +got):\n%s", diff)
	}
	if got := field(t, runReport(t, 0, command("install")...), "results", 0, "status"); got != "unchanged" {
		t.Errorf("repeat install = %v", got)
	}

	competing := filepath.Join(root, "competing")
	copyTree(t, "bundle", competing)
	manifest, err := os.ReadFile(filepath.Join(competing, "flyrail.json"))
	if err != nil {
		t.Fatal(err)
	}
	manifest = bytes.ReplaceAll(manifest, []byte(`"example-notes"`), []byte(`"competing-notes"`))
	if err := os.WriteFile(filepath.Join(competing, "flyrail.json"), manifest, 0o600); err != nil {
		t.Fatal(err)
	}
	skill := filepath.Join(first, "notes-summary", "SKILL.md")
	before, err := os.ReadFile(skill)
	if err != nil {
		t.Fatal(err)
	}
	conflictStatus := runReport(t, 1, "ai", "status", "--source", competing, "--target", first, "--state", state)
	want := []any{map[string]any{"path": first, "owner": nil}}
	if diff := cmp.Diff(want, field(t, conflictStatus, "results", 0, "observation", "conflicts")); diff != "" {
		t.Errorf("status conflicts (-want +got):\n%s", diff)
	}
	if after, err := os.ReadFile(skill); err != nil || !bytes.Equal(after, before) {
		t.Errorf("status changed the installed skill: %v", err)
	}
	conflict := runReport(t, 1, "ai", "install", "--source", competing, "--target", first, "--state", state)
	wantError := map[string]any{"code": "conflict", "errno": nil, "message": "claim overlaps recorded ownership", "path": first}
	if diff := cmp.Diff(wantError, field(t, conflict, "results", 0, "error")); diff != "" {
		t.Errorf("conflict error (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff([]any{}, field(t, conflict, "results", 0, "observation", "conflicts")); diff != "" {
		t.Errorf("conflict observation conflicts (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff([]any{}, field(t, conflict, "results", 0, "recovery_paths")); diff != "" {
		t.Errorf("conflict recovery paths (-want +got):\n%s", diff)
	}

	if err := os.WriteFile(skill, []byte("local edit"), 0o600); err != nil {
		t.Fatal(err)
	}
	modified := runReport(t, 1, command("update")...)
	if got := field(t, modified, "results", 0, "error", "code"); got != "modified" {
		t.Errorf("update error code = %v", got)
	}
	wantModification := map[string]any{"bundle_id": "example-notes", "kind": "content_changed", "path": "notes-summary"}
	if diff := cmp.Diff(wantModification, field(t, modified, "results", 0, "observation", "modifications", 0)); diff != "" {
		t.Errorf("modification (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff([]any{}, field(t, modified, "results", 0, "recovery_paths")); diff != "" {
		t.Errorf("modified recovery paths (-want +got):\n%s", diff)
	}
	if got := field(t, runReport(t, 0, command("update", "--replace-modified")...), "results", 0, "status"); got != "applied" {
		t.Errorf("replacing update = %v", got)
	}

	if err := os.RemoveAll(source); err != nil {
		t.Fatal(err)
	}
	removed := runReport(t, 0, append([]string{"ai", "uninstall", "--bundle-id", "example-notes"}, targets...)...)
	if got := field(t, removed, "bundle_version"); got != nil {
		t.Errorf("uninstall bundle_version = %v", got)
	}
	if got := field(t, removed, "results", 0, "status"); got != "applied" {
		t.Errorf("uninstall = %v", got)
	}
	for _, target := range []string{first, second} {
		if _, err := os.Lstat(target); !errors.Is(err, fs.ErrNotExist) {
			t.Errorf("%s still present after uninstall: %v", target, err)
		}
	}
}

func TestParseFlagsAcceptsValuesAfterSeparator(t *testing.T) {
	request, err := parseAI([]string{"update", "--source", "--", "--odd", "--target", "/t", "--state", "/s", "--replace-modified"})
	if err != nil {
		t.Fatal(err)
	}
	want := aiRequest{action: "update", source: "--odd", targets: []string{"/t"}, state: "/s", replaceModified: true}
	if diff := cmp.Diff(want, request, cmp.AllowUnexported(aiRequest{})); diff != "" {
		t.Errorf("request (-want +got):\n%s", diff)
	}
}

func TestOptionFormsOutsideTheSpacedGrammarAreUnknownOptions(t *testing.T) {
	base := []string{"ai", "update", "--source", "/s", "--target", "/t", "--state", "/st"}
	for _, bad := range [][]string{{"-target", "/t2"}, {"--state=/s"}, {"--replace-modified=false"}, {"--"}, {"--bogus"}, {"-h"}, {"--help"}, {"extra"}} {
		code, stdout, stderr := runCLI(append(slices.Clone(base), bad...)...)
		want := "flyrail-notes: unknown option '" + bad[0] + "'\nUsage:\n"
		if code != 2 || stdout != "" || !strings.HasPrefix(stderr, want) {
			t.Errorf("%v: exit %d, stdout %q, stderr %q, want prefix %q", bad, code, stdout, stderr, want)
		}
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
	cmd.Env = append(os.Environ(), mainArgsVariable+"=words /dev/stdin")
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
