//go:build darwin || linux

package flyrail

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func executableFile(t *testing.T, path string, mode os.FileMode) {
	t.Helper()
	writeFile(t, path, "")
	if err := os.Chmod(path, mode); err != nil {
		t.Fatal(err)
	}
}

func presentAgents(found []AgentPresence) []Agent {
	agents := make([]Agent, 0, len(found))
	for _, presence := range found {
		agents = append(agents, presence.Agent)
	}
	return agents
}

func TestDetectAgentsFindsExecutablesInAgentOrderWithAbsoluteEvidence(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	bin := filepath.Join(root, "bin")
	for _, name := range []string{"droid", "claude", "cursor"} {
		executableFile(t, filepath.Join(bin, name), 0o700)
	}
	executableFile(t, filepath.Join(bin, "codex"), 0o600)
	mkdir(t, filepath.Join(bin, "pi"))
	found := must(DetectAgents(t.Context(), "", DiscoveryOptions{SearchPath: []string{bin}, NoApplications: true}))
	want := []AgentPresence{
		{Agent: AgentClaude, Executable: filepath.Join(bin, "claude")},
		{Agent: AgentCursor, Executable: filepath.Join(bin, "cursor")},
		{Agent: AgentDroid, Executable: filepath.Join(bin, "droid")},
	}
	if diff := cmp.Diff(want, found); diff != "" {
		t.Errorf("found (-want +got):\n%s", diff)
	}
}

func TestDetectAgentsUsesTheFirstMatchingSearchDirectory(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	first, second := filepath.Join(root, "first"), filepath.Join(root, "second")
	mkdir(t, first)
	executableFile(t, filepath.Join(second, "claude"), 0o700)
	executableFile(t, filepath.Join(first, "codex"), 0o700)
	executableFile(t, filepath.Join(second, "codex"), 0o700)
	found := must(DetectAgents(t.Context(), "", DiscoveryOptions{SearchPath: []string{filepath.Join(root, "missing"), first, second}, NoApplications: true}))
	want := []AgentPresence{
		{Agent: AgentClaude, Executable: filepath.Join(second, "claude")},
		{Agent: AgentCodex, Executable: filepath.Join(first, "codex")},
	}
	if diff := cmp.Diff(want, found); diff != "" {
		t.Errorf("found (-want +got):\n%s", diff)
	}
}

func TestDetectAgentsNormalizesParentComponentsInExecutableEvidence(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	mkdir(t, filepath.Join(root, "nested"))
	executable := filepath.Join(root, "bin", "claude")
	executableFile(t, executable, 0o700)
	if got := findExecutable("claude", []string{root + "/nested/../bin"}); got != executable {
		t.Errorf("evidence = %q, want %q", got, executable)
	}
	if got := findExecutable("claude", []string{root + "/./bin/"}); got != executable {
		t.Errorf("evidence = %q, want %q", got, executable)
	}
}

func TestLexicalAbsoluteFoldsComponentsLikeRust(t *testing.T) {
	t.Parallel()
	for _, test := range []struct{ path, want string }{
		{"/a/./b/../c", "/a/c"},
		{"/..", "/"},
		{"//a//b/", "/a/b"},
		{"/a/b/../../..", "/"},
	} {
		if got := lexicalAbsolute(test.path); got != test.want {
			t.Errorf("lexicalAbsolute(%q) = %q, want %q", test.path, got, test.want)
		}
	}
}

func TestDetectAgentsFindsSupportedMacOSApplicationsAfterExecutables(t *testing.T) {
	t.Parallel()
	if runtime.GOOS != "darwin" {
		t.Skip("application evidence is macOS-only")
	}
	root := tempRoot(t)
	mkdir(t, filepath.Join(root, "Cursor.app"))
	mkdir(t, filepath.Join(root, "Visual Studio Code.app"))
	found := must(DetectAgents(t.Context(), "", DiscoveryOptions{SearchPath: []string{}, Applications: []string{root}}))
	want := []AgentPresence{
		{Agent: AgentCursor, Application: filepath.Join(root, "Cursor.app")},
		{Agent: AgentCopilot, Application: filepath.Join(root, "Visual Studio Code.app")},
	}
	if diff := cmp.Diff(want, found); diff != "" {
		t.Errorf("found (-want +got):\n%s", diff)
	}
}

func TestDetectAgentsDefaultsApplicationDirectoriesToHome(t *testing.T) {
	t.Parallel()
	if runtime.GOOS != "darwin" {
		t.Skip("application evidence is macOS-only")
	}
	home := tempRoot(t)
	mkdir(t, filepath.Join(home, "Applications", "Cursor.app"))
	found := must(DetectAgents(t.Context(), home, DiscoveryOptions{}))
	cursor := AgentPresence{Agent: AgentCursor, Application: filepath.Join(home, "Applications", "Cursor.app")}
	if _, err := os.Stat("/Applications/Cursor.app"); err == nil {
		cursor.Application = "/Applications/Cursor.app"
	}
	if diff := cmp.Diff([]AgentPresence{cursor}, filterAgent(found, AgentCursor)); diff != "" {
		t.Errorf("cursor (-want +got):\n%s", diff)
	}
}

func filterAgent(found []AgentPresence, agent Agent) []AgentPresence {
	var selected []AgentPresence
	for _, presence := range found {
		if presence.Agent == agent {
			selected = append(selected, presence)
		}
	}
	return selected
}

func TestDetectAgentsRefusesRelativeApplicationDirectoriesWhateverIsInstalled(t *testing.T) {
	t.Parallel()
	for _, test := range []struct {
		label string
		home  string
		opts  DiscoveryOptions
	}{
		{"explicit", "", DiscoveryOptions{SearchPath: []string{}, Applications: []string{"apps"}}},
		{"derived from home", "home", DiscoveryOptions{SearchPath: []string{}}},
	} {
		_, err := DetectAgents(t.Context(), test.home, test.opts)
		if message := valueMessage(t, err); message != "agent presence evidence must be an absolute Path" {
			t.Errorf("%s: message = %q", test.label, message)
		}
	}
}

func TestDetectAgentsTreatsAnEmptySearchPathEntryAsTheCurrentDirectory(t *testing.T) {
	root := tempRoot(t)
	executableFile(t, filepath.Join(root, "claude"), 0o700)
	t.Chdir(root)
	found := must(DetectAgents(t.Context(), "", DiscoveryOptions{SearchPath: []string{""}, NoApplications: true}))
	want := []AgentPresence{{Agent: AgentClaude, Executable: must(filepath.EvalSymlinks(filepath.Join(root, "claude")))}}
	for index := range found {
		found[index].Executable = must(filepath.EvalSymlinks(found[index].Executable))
	}
	if diff := cmp.Diff(want, found); diff != "" {
		t.Errorf("found (-want +got):\n%s", diff)
	}
}

func TestDetectAgentsReturnsPresenceInAgentsOrder(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	for _, agent := range Agents() {
		executableFile(t, filepath.Join(root, string(agent)), 0o700)
	}
	found := must(DetectAgents(t.Context(), "", DiscoveryOptions{SearchPath: []string{root}, NoApplications: true}))
	if diff := cmp.Diff(Agents(), presentAgents(found)); diff != "" {
		t.Errorf("agents (-want +got):\n%s", diff)
	}
}
