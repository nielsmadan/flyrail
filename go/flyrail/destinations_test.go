package flyrail

import (
	"context"
	"encoding/json"
	"errors"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

type fakeStore struct{ name string }

func (fakeStore) Location() (string, bool)         { return "", false }
func (fakeStore) StagingDirectory() (string, bool) { return "", false }
func (fakeStore) Read(context.Context, string) ([]byte, bool, error) {
	return nil, false, nil
}

func (fakeStore) Write(context.Context, string, []byte, store.Expected) (bool, error) {
	return false, nil
}
func (fakeStore) Delete(context.Context, string, []byte) (bool, error) { return false, nil }
func (fakeStore) Keys(context.Context, string) ([]string, error)       { return nil, nil }
func (fakeStore) Lock(context.Context, string, time.Duration) (store.Unlocker, error) {
	return nil, nil
}

func projectContext(t *testing.T, agent Agent, root string, opts RenderOptions) RenderContext {
	t.Helper()
	return must(NewRenderContext(must(ProjectTarget(agent, root)), PlatformLinux, opts))
}

func TestProjectDestinationsCoverTheAgentMatrix(t *testing.T) {
	root := "/fixture/project"
	cases := []struct {
		agent             Agent
		instructions, mcp string
	}{
		{AgentClaude, "CLAUDE.md", ".mcp.json"},
		{AgentCodex, "AGENTS.md", ".codex/config.toml"},
		{AgentOpencode, "AGENTS.md", "opencode.json"},
		{AgentPi, "AGENTS.md", ".pi/mcp.json"},
		{AgentCursor, "AGENTS.md", ".cursor/mcp.json"},
		{AgentCopilot, ".github/copilot-instructions.md", ".mcp.json"},
	}
	for _, c := range cases {
		rc := projectContext(t, c.agent, root, RenderOptions{})
		if got := must(InstructionDestination(rc)); got != root+"/"+c.instructions {
			t.Errorf("%s instructions = %q", c.agent, got)
		}
		if got := must(MCPDestination(rc)); got != root+"/"+c.mcp {
			t.Errorf("%s MCP = %q", c.agent, got)
		}
		if got := len(rc.RoutingContext()); got != 13 {
			t.Errorf("%s routing context has %d entries", c.agent, got)
		}
	}
}

func TestUserDestinationsCoverTheAgentMatrix(t *testing.T) {
	root := "/fixture/home"
	cases := []struct {
		agent             Agent
		instructions, mcp string
	}{
		{AgentClaude, ".claude/CLAUDE.md", ".claude.json"},
		{AgentCodex, ".codex/AGENTS.md", ".codex/config.toml"},
		{AgentOpencode, ".config/opencode/AGENTS.md", ".config/opencode/opencode.json"},
		{AgentPi, ".pi/agent/AGENTS.md", ".pi/agent/mcp.json"},
		{AgentCursor, "", ".cursor/mcp.json"},
		{AgentCopilot, ".copilot/copilot-instructions.md", ".copilot/mcp-config.json"},
	}
	for _, c := range cases {
		rc := must(NewRenderContext(must(UserTarget(c.agent, root, nil)), PlatformMacOS, RenderOptions{}))
		want := ""
		if c.instructions != "" {
			want = root + "/" + c.instructions
		}
		if got := must(InstructionDestination(rc)); got != want {
			t.Errorf("%s instructions = %q, want %q", c.agent, got, want)
		}
		if got := must(MCPDestination(rc)); got != root+"/"+c.mcp {
			t.Errorf("%s MCP = %q", c.agent, got)
		}
	}
}

func TestEveryTranslationFixtureUsesTheDestinationResolver(t *testing.T) {
	var document struct {
		Cases []struct {
			Name     string         `json:"name"`
			Agent    Agent          `json:"agent"`
			Surface  Surface        `json:"surface"`
			Scope    TargetScope    `json:"scope"`
			MCPPath  string         `json:"mcp_path"`
			Artifact map[string]any `json:"artifact"`
			Expected map[string]any `json:"expected"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(fixture(t, "translations.json"), &document); err != nil {
		t.Fatal(err)
	}
	if len(document.Cases) != 35 {
		t.Fatalf("fixture has %d cases", len(document.Cases))
	}
	root := "/fixture/destinations"
	for _, c := range document.Cases {
		var target Target
		if c.Scope == ScopeProject {
			target = must(ProjectTarget(c.Agent, root))
		} else {
			target = must(UserTarget(c.Agent, root, nil))
		}
		opts := RenderOptions{Surface: c.Surface}
		if c.MCPPath != "" {
			opts.MCPPath = root + "/" + c.MCPPath
		}
		rc := must(NewRenderContext(target, PlatformLinux, opts))
		var got string
		if c.Artifact["kind"] == "instruction" {
			got = must(InstructionDestination(rc))
		} else {
			got = must(MCPDestination(rc))
		}
		destination, hasDestination := c.Expected["destination"].(string)
		directory, hasDirectory := c.Expected["directory"].(string)
		var want string
		switch {
		case hasDestination:
			want = root + "/" + destination
		case hasDirectory:
			want = root + "/" + directory
		case c.Artifact["kind"] != "instruction":
			want = root + "/.mcp.json"
		}
		if got != want {
			t.Errorf("%s: destination %q, want %q", c.Name, got, want)
		}
	}
}

func TestRelocationsCapabilitiesAndOverridesMatchTheReference(t *testing.T) {
	root := "/fixture/home"
	target := must(UserTarget(AgentClaude, root, lookupMap(map[string]string{"CLAUDE_CONFIG_DIR": "~/selected"})))
	rc := must(NewRenderContext(target, PlatformLinux, RenderOptions{}))
	if got := must(InstructionDestination(rc)); got != root+"/selected/CLAUDE.md" {
		t.Errorf("instructions = %q", got)
	}
	if got := must(MCPDestination(rc)); got != "" {
		t.Errorf("relocated Claude MCP = %q, want none", got)
	}
	if got := len(rc.RoutingContext()); got != 14 {
		t.Errorf("routing context has %d entries", got)
	}
	mcp := must(Capabilities(rc))[2]
	if mcp.Family != FamilyMCP || mcp.Portable {
		t.Errorf("relocated MCP capability = %+v", mcp)
	}
	overridden := must(NewRenderContext(target, PlatformLinux, RenderOptions{MCPPath: root + "/chosen.json"}))
	if mcp := must(Capabilities(overridden))[2]; !mcp.Portable {
		t.Errorf("overridden MCP capability = %+v", mcp)
	}
	opencode := must(NewRenderContext(must(UserTarget(AgentOpencode, root, lookupMap(map[string]string{
		"XDG_CONFIG_HOME": "~/configuration", "OPENCODE_CONFIG": "~/selected/opencode.jsonc",
	}))), PlatformLinux, RenderOptions{}))
	if got := must(MCPDestination(opencode)); got != root+"/selected/opencode.jsonc" {
		t.Errorf("OpenCode MCP = %q", got)
	}
	if got := must(InstructionDestination(opencode)); got != root+"/configuration/opencode/AGENTS.md" {
		t.Errorf("OpenCode instructions = %q", got)
	}
	codex := must(NewRenderContext(must(UserTarget(AgentCodex, root, lookupMap(map[string]string{"CODEX_HOME": "relative"}))), PlatformLinux, RenderOptions{}))
	_, err := InstructionDestination(codex)
	if got := valueErrorText(t, err); got != "environment paths must be absolute or start with ~/" {
		t.Errorf("relative CODEX_HOME: %q", got)
	}
}

func TestCapabilitiesDescribeEachFamily(t *testing.T) {
	root := "/fixture/project"
	runtime := must(NewHookRuntime("/opt/node/bin/node", ShellPOSIX))
	hostControlled := []string{"Discovery, overrides, byte limits and session loading remain host-controlled."}
	checked := []string{"Transport, interpolation, environment references and cwd are checked per request."}
	validated := []string{"Event/outcome and native shell support are validated for each request."}
	node := []string{"Node 24+; explicit executable for native hosts. See the hook protocol matrix."}
	want := []Capability{
		{Family: FamilySkills, Portable: true, Native: true, Limitations: []string{}, Prerequisites: []string{}},
		{Family: FamilyInstructions, Portable: true, Native: true, Limitations: hostControlled, Prerequisites: []string{}},
		{Family: FamilyMCP, Portable: true, Native: true, Limitations: checked, Prerequisites: []string{"pi-mcp-adapter@2.32.1"}},
		{Family: FamilyHooks, Portable: true, Native: true, Limitations: validated, Prerequisites: node},
	}
	if diff := cmp.Diff(want, must(Capabilities(projectContext(t, AgentPi, root, RenderOptions{HookRuntime: runtime})))); diff != "" {
		t.Errorf("Pi capabilities (-want +got):\n%s", diff)
	}
	vscode := must(Capabilities(projectContext(t, AgentCopilot, root, RenderOptions{Surface: SurfaceVSCode, HookRuntime: runtime})))
	if vscode[3].Portable || !vscode[2].Portable || len(vscode[2].Prerequisites) != 0 {
		t.Errorf("VS Code capabilities = %+v", vscode)
	}
	cursorUser := must(Capabilities(must(NewRenderContext(must(UserTarget(AgentCursor, root, nil)), PlatformLinux, RenderOptions{}))))
	if cursorUser[1].Portable || cursorUser[3].Portable {
		t.Errorf("Cursor user capabilities = %+v", cursorUser)
	}
}

func TestDroidHasNoTranslationDestinationsOrCapabilities(t *testing.T) {
	root := "/fixture/droid"
	for _, target := range []Target{must(ProjectTarget(AgentDroid, root)), must(UserTarget(AgentDroid, root, nil))} {
		for _, opts := range []RenderOptions{{}, {InstructionPath: root + "/instructions.md", MCPPath: root + "/mcp.json"}} {
			rc := must(NewRenderContext(target, PlatformLinux, opts))
			if got := must(InstructionDestination(rc)); got != opts.InstructionPath {
				t.Errorf("%s instructions = %q", target.Scope(), got)
			}
			if got := must(MCPDestination(rc)); got != opts.MCPPath {
				t.Errorf("%s MCP = %q", target.Scope(), got)
			}
			want := []Capability{
				{Family: FamilySkills, Limitations: []string{}, Prerequisites: []string{}},
				{Family: FamilyInstructions, Limitations: []string{}, Prerequisites: []string{}},
				{Family: FamilyMCP, Limitations: []string{}, Prerequisites: []string{}},
				{Family: FamilyHooks, Limitations: []string{}, Prerequisites: []string{}},
			}
			if diff := cmp.Diff(want, must(Capabilities(rc))); diff != "" {
				t.Errorf("capabilities (-want +got):\n%s", diff)
			}
		}
		err := RequireTranslatable(must(NewRenderContext(target, PlatformLinux, RenderOptions{})))
		failure, ok := errors.AsType[*UnsupportedTranslationError](err)
		if !ok || failure.Code != "agent-unsupported" ||
			failure.Message != "Flyrail detects droid but has no verified configuration translation." {
			t.Errorf("RequireTranslatable = %v", err)
		}
	}
}

func TestRenderContextsRejectInvalidAudiencesAndRoutes(t *testing.T) {
	root := "/fixture/project"
	codex := must(ProjectTarget(AgentCodex, root))
	cases := []struct {
		name     string
		target   Target
		platform Platform
		opts     RenderOptions
		message  string
	}{
		{"directory target", must(DirectoryTarget(root)), PlatformLinux, RenderOptions{}, "render context requires a project or user agent Target"},
		{"zero target", Target{}, PlatformLinux, RenderOptions{}, "render context requires a project or user agent Target"},
		{"IDE surface", codex, PlatformLinux, RenderOptions{Surface: SurfaceIDE}, "IDE surface is only defined for Cursor"},
		{"missing platform", codex, "", RenderOptions{}, "'' is not a valid Platform"},
		{"relative route", codex, PlatformLinux, RenderOptions{MCPPath: "relative"}, "render routes must be absolute Paths"},
		{"ambiguous route", codex, PlatformLinux, RenderOptions{AssetRoot: root + "/a/../b"}, "render routes must be unambiguous paths without NUL"},
		{"ambiguous root", must(ProjectTarget(AgentCodex, "/a/../b")), PlatformLinux, RenderOptions{}, "render routes must be unambiguous paths without NUL"},
	}
	for _, c := range cases {
		_, err := NewRenderContext(c.target, c.platform, c.opts)
		if got := valueErrorText(t, err); got != c.message {
			t.Errorf("%s: %q, want %q", c.name, got, c.message)
		}
	}
	guide := must(NewInstructionArtifact("guide", "Hello"))
	_, err := Render(must(NewBundle(must(NewBundleIdentity("team", "1")), []Artifact{guide}, nil, nil)), RenderContext{})
	if got := valueErrorText(t, err); got != "render context requires a project or user agent Target" {
		t.Errorf("zero context: %q", got)
	}
}

func TestRoutingContextNamesEveryRoute(t *testing.T) {
	root := "/fixture/project"
	runtime := must(NewHookRuntime("/opt/node/bin/node", ShellPOSIX))
	rc := projectContext(t, AgentCodex, root, RenderOptions{HookRuntime: runtime, MCPPath: root + "/mcp.toml"})
	want := map[string]string{
		"agent": "codex", "scope": "project", "surface": "cli", "platform": "linux", "root": root,
		"skills": root + "/.agents/skills", "execution": root, "instructions": "", "mcp": root + "/mcp.toml",
		"native": root, "assets": root + "/.flyrail-assets", "hook-node": "/opt/node/bin/node", "hook-shell": "posix",
	}
	if diff := cmp.Diff(want, rc.RoutingContext()); diff != "" {
		t.Errorf("routing context (-want +got):\n%s", diff)
	}
	vscode := must(NewRenderContext(must(UserTarget(AgentCopilot, "/home", nil)), PlatformLinux, RenderOptions{Surface: SurfaceVSCode}))
	if vscode.Skills() != "/home/.copilot/skills" || vscode.Root() != "/home" {
		t.Errorf("VS Code user skills %q root %q", vscode.Skills(), vscode.Root())
	}
}

func TestInstallationsDefaultToTheStableRoutingIdentity(t *testing.T) {
	root := "/fixture/project"
	st := &fakeStore{}
	runtime := must(NewHookRuntime("", ShellPOSIX))
	rc := projectContext(t, AgentCodex, root, RenderOptions{HookRuntime: runtime})
	installation := must(rc.Installation(st, nil))
	if diff := cmp.Diff(map[string]string{"agent": "codex", "root": root, "scope": "project", "surface": "cli"}, installation.Context()); diff != "" {
		t.Errorf("context (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff(rc.RoutingContext(), installation.RoutingContext()); diff != "" {
		t.Errorf("routing (-want +got):\n%s", diff)
	}
	plain := must(projectContext(t, AgentCodex, root, RenderOptions{}).Installation(st, nil))
	if installation.ContextDigest() != plain.ContextDigest() {
		t.Error("the hook runtime changed the identity digest")
	}
	if got := installation.ContextDigest(); got != "2c75a7af56b9572ccfd3c42c7376f4bd70283d8c850d0b93e3761c28bea206c4" {
		t.Errorf("identity digest = %s", got)
	}
	if installation.Store() != store.Store(st) {
		t.Error("installation does not share the store")
	}
	explicit := must(rc.Installation(st, map[string]string{"app": "notes"}))
	if diff := cmp.Diff(map[string]string{"app": "notes"}, explicit.Context()); diff != "" {
		t.Errorf("explicit context (-want +got):\n%s", diff)
	}
	empty := must(rc.Installation(st, map[string]string{}))
	if len(empty.Context()) != 0 || empty.ContextDigest() != must(rc.InstallationWithContext(st, nil)).ContextDigest() {
		t.Errorf("empty context = %v", empty.Context())
	}
}

func TestInstallationTargetsValidateStoresAndCompareByPointer(t *testing.T) {
	st := &fakeStore{name: "one"}
	var typedNil *fakeStore
	cases := []struct {
		name  string
		store store.Store
	}{
		{"nil", nil},
		{"typed nil", typedNil},
		{"value store", fakeStore{}},
	}
	for _, c := range cases {
		_, err := NewInstallationTarget(c.store, nil, nil)
		if got := valueErrorText(t, err); got != "store must be a StateStore" {
			t.Errorf("%s: %q", c.name, got)
		}
	}
	_, err := NewInstallationTarget(&store.DirectoryStore{}, nil, nil)
	if got := valueErrorText(t, err); got != "directory state store must come from NewDirectoryStore" {
		t.Errorf("zero directory store: %q", got)
	}
	_, err = NewInstallationTarget(st, map[string]string{"key": "\xff"}, nil)
	if got := valueErrorText(t, err); got != "target context must contain string pairs" {
		t.Errorf("invalid context: %q", got)
	}
	first := must(NewInstallationTarget(st, map[string]string{"app": "notes"}, map[string]string{"agent": "codex"}))
	second := must(NewInstallationTarget(st, map[string]string{"app": "notes"}, map[string]string{"agent": "codex"}))
	other := must(NewInstallationTarget(&fakeStore{name: "one"}, map[string]string{"app": "notes"}, map[string]string{"agent": "codex"}))
	if !first.Equal(second) || first.Equal(other) {
		t.Errorf("equality: same store %v, other store %v", first.Equal(second), first.Equal(other))
	}
	if got := must(NewInstallationTarget(st, nil, nil)).ContextDigest(); got != "37517e5f3dc66819f61f5a7bb8ace1921282415f10551d2defa5c3eb0985b570" {
		t.Errorf("empty context digest = %s", got)
	}
}
