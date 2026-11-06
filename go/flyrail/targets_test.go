package flyrail

import (
	"errors"
	"testing"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

func valueErrorText(t *testing.T, err error) string {
	t.Helper()
	failure, ok := errors.AsType[*ValueError](err)
	if !ok {
		t.Fatalf("error %v (%T) is not a *ValueError", err, err)
	}
	return failure.Message
}

func TestTargetPresetsMatchTheReferenceContainers(t *testing.T) {
	home := "/fixture/home"
	cases := []struct {
		agent         Agent
		user, project string
	}{
		{AgentClaude, ".claude/skills", ".claude/skills"},
		{AgentCodex, ".agents/skills", ".agents/skills"},
		{AgentOpencode, ".config/opencode/skills", ".opencode/skills"},
		{AgentPi, ".pi/agent/skills", ".pi/skills"},
		{AgentCursor, ".cursor/skills", ".cursor/skills"},
		{AgentCopilot, ".copilot/skills", ".github/skills"},
		{AgentDroid, ".factory/skills", ".factory/skills"},
	}
	for _, c := range cases {
		user := must(UserTarget(c.agent, home, nil))
		if got, want := user.Root(), home+"/"+c.user; got != want {
			t.Errorf("%s user root = %q, want %q", c.agent, got, want)
		}
		if user.Home() != home || user.Scope() != ScopeUser || user.Agent() != c.agent {
			t.Errorf("%s user target = %q %q %q", c.agent, user.Home(), user.Scope(), user.Agent())
		}
		project := must(ProjectTarget(c.agent, home))
		if got, want := project.Root(), home+"/"+c.project; got != want {
			t.Errorf("%s project root = %q, want %q", c.agent, got, want)
		}
		if project.Home() != "" || project.Scope() != ScopeProject {
			t.Errorf("%s project target = %q %q", c.agent, project.Home(), project.Scope())
		}
	}
	directory := must(DirectoryTarget("/fixture/skills"))
	if directory.Root() != "/fixture/skills" || directory.Agent() != "" || directory.Scope() != ScopeDirectory {
		t.Errorf("directory target = %q %q %q", directory.Root(), directory.Agent(), directory.Scope())
	}
}

func TestEveryAgentHasAnExplicitContainerAndMCPRoute(t *testing.T) {
	for _, agent := range Agents() {
		if _, err := projectSuffix(agent); err != nil {
			t.Errorf("projectSuffix(%s) = %v", agent, err)
		}
		if _, err := userSuffix(agent); err != nil {
			t.Errorf("userSuffix(%s) = %v", agent, err)
		}
		if _, _, err := relocation(agent); err != nil {
			t.Errorf("relocation(%s) = %v", agent, err)
		}
		project := must(NewRenderContext(must(ProjectTarget(agent, "/fixture/project")), PlatformLinux, RenderOptions{}))
		user := must(NewRenderContext(must(UserTarget(agent, "/fixture/home", nil)), PlatformLinux, RenderOptions{}))
		for _, rc := range []RenderContext{project, user} {
			if _, err := MCPDestination(rc); err != nil {
				t.Errorf("MCPDestination(%s %s) = %v", agent, rc.target.scope, err)
			}
			if _, err := InstructionDestination(rc); err != nil {
				t.Errorf("InstructionDestination(%s %s) = %v", agent, rc.target.scope, err)
			}
		}
	}
	const unknown = Agent("zed")
	_, projectErr := projectSuffix(unknown)
	_, userErr := userSuffix(unknown)
	_, _, relocationErr := relocation(unknown)
	for _, err := range []error{projectErr, userErr, relocationErr} {
		if got := valueErrorText(t, err); got != `unhandled Agent: "zed"` {
			t.Errorf("unknown agent error = %q", got)
		}
	}
}

func TestUserTargetsApplyRelocationsAndRetainKnownVariables(t *testing.T) {
	home := "/fixture/home"
	env := lookupMap(map[string]string{"CLAUDE_CONFIG_DIR": "~/chosen", "UNRELATED": "ignored", "CODEX_HOME": "/codex"})
	relocated := must(UserTarget(AgentClaude, home, env))
	if relocated.Root() != home+"/chosen/skills" {
		t.Errorf("relocated root = %q", relocated.Root())
	}
	if diff := cmp.Diff(map[string]string{"CLAUDE_CONFIG_DIR": "~/chosen", "CODEX_HOME": "/codex"}, relocated.Environment()); diff != "" {
		t.Errorf("environment (-want +got):\n%s", diff)
	}
	equivalent := must(UserTarget(AgentClaude, home, lookupMap(map[string]string{"CLAUDE_CONFIG_DIR": home + "//chosen/"})))
	if !relocated.Equal(equivalent) {
		t.Errorf("targets naming the same container differ: %q and %q", relocated.Root(), equivalent.Root())
	}
	if relocated.Equal(must(UserTarget(AgentClaude, home, nil))) {
		t.Error("a relocated target equals the default container")
	}

	cases := []struct {
		agent Agent
		env   map[string]string
		root  string
	}{
		{AgentOpencode, map[string]string{"XDG_CONFIG_HOME": "/xdg"}, "/xdg/opencode/skills"},
		{AgentPi, map[string]string{"PI_CODING_AGENT_DIR": "~"}, home + "/skills"},
		{AgentCopilot, map[string]string{"COPILOT_HOME": "/copilot"}, "/copilot/skills"},
		{AgentCodex, map[string]string{"CODEX_HOME": "/codex"}, home + "/.agents/skills"},
		{AgentClaude, map[string]string{"CLAUDE_CONFIG_DIR": ""}, home + "/.claude/skills"},
	}
	for _, c := range cases {
		if got := must(UserTarget(c.agent, home, lookupMap(c.env))).Root(); got != c.root {
			t.Errorf("%s %v root = %q, want %q", c.agent, c.env, got, c.root)
		}
	}

	fromEnv := must(UserTarget(AgentCodex, "", lookupMap(map[string]string{"USERPROFILE": "/profile"})))
	if fromEnv.Home() != "/profile" || fromEnv.Root() != "/profile/.agents/skills" {
		t.Errorf("home from USERPROFILE = %q %q", fromEnv.Home(), fromEnv.Root())
	}
	preferred := must(UserTarget(AgentCodex, "", lookupMap(map[string]string{"HOME": "/home", "USERPROFILE": "/profile"})))
	if preferred.Home() != "/home" {
		t.Errorf("home = %q, want HOME", preferred.Home())
	}
}

func TestTargetBoundariesAreCheckedWithoutAmbientState(t *testing.T) {
	cases := []struct {
		name    string
		build   func() (Target, error)
		message string
	}{
		{"empty root", func() (Target, error) { return DirectoryTarget("") }, "target roots must be nonempty paths without NUL"},
		{"NUL root", func() (Target, error) { return DirectoryTarget("/a\x00b") }, "target roots must be nonempty paths without NUL"},
		{"non-UTF-8 root", func() (Target, error) { return DirectoryTarget("/a\xffb") }, "target roots must be string paths"},
		{"filesystem root", func() (Target, error) { return DirectoryTarget("//./") }, "a filesystem root cannot be a skill container"},
		{"unknown agent", func() (Target, error) { return ProjectTarget("emacs", "/project") }, "'emacs' is not a valid Agent"},
		{"no home", func() (Target, error) { return UserTarget(AgentCodex, "", lookupMap(nil)) }, "an explicit env requires home, HOME, or USERPROFILE"},
		{"no lookup", func() (Target, error) { return UserTarget(AgentCodex, "", nil) }, "an explicit env requires home, HOME, or USERPROFILE"},
		{"relative HOME", func() (Target, error) {
			return UserTarget(AgentCodex, "", lookupMap(map[string]string{"HOME": "relative"}))
		}, "environment home must be absolute"},
		{"empty HOME", func() (Target, error) {
			return UserTarget(AgentCodex, "", lookupMap(map[string]string{"HOME": "", "USERPROFILE": "/profile"}))
		}, "environment home must be absolute"},
		{"relative relocation", func() (Target, error) {
			return UserTarget(AgentClaude, "/home", lookupMap(map[string]string{"CLAUDE_CONFIG_DIR": "relative"}))
		}, "environment paths must be absolute or start with ~/"},
		{"non-UTF-8 variable", func() (Target, error) {
			return UserTarget(AgentClaude, "/home", lookupMap(map[string]string{"COPILOT_HOME": "/\xff"}))
		}, "env keys and values must be strings"},
	}
	for _, c := range cases {
		_, err := c.build()
		if got := valueErrorText(t, err); got != c.message {
			t.Errorf("%s: %q, want %q", c.name, got, c.message)
		}
	}
}

func TestRelativeTargetRootsJoinTheWorkingDirectory(t *testing.T) {
	root := tempRoot(t)
	t.Chdir(root)
	target := must(ProjectTarget(AgentCodex, "project/../other"))
	if want := root + "/project/../other/.agents/skills"; target.Root() != want {
		t.Errorf("root = %q, want %q", target.Root(), want)
	}
}

func TestRouteValidationMatchesReferenceMessages(t *testing.T) {
	cases := map[string]string{
		"relative":    "render routes must be absolute Paths",
		"/tmp/a/../b": "render routes must be unambiguous paths without NUL",
		"/tmp/a\x00b": "render routes must be unambiguous paths without NUL",
	}
	for route, message := range cases {
		_, err := validateRoute(route)
		if got := valueErrorText(t, err); got != message {
			t.Errorf("%q: %q, want %q", route, got, message)
		}
	}
}

func TestPathHelpersFollowRustPathSemantics(t *testing.T) {
	if got := pathx.Join("/a/", "b", "/c", "d"); got != "/c/d" {
		t.Errorf("pathJoin = %q", got)
	}
	if !pathsEqual("/a//b/./c/", "/a/b/c") {
		t.Error("component-equal paths differ")
	}
	if comparePaths("/a/b", "/a-b") >= 0 {
		t.Error("paths do not order by components")
	}
	if got := posixPath("//a/./b//c"); got != "//a/b/c" {
		t.Errorf("posixPath = %q", got)
	}
	if got := posixPath("///a/b"); got != "/a/b" {
		t.Errorf("posixPath = %q", got)
	}
	extensions := []struct {
		path, extension string
		ok              bool
	}{{"/a/b.cmd", "cmd", true}, {"/a/.cmd", "", false}, {"/a/b.", "", true}, {"/a/b", "", false}}
	for _, c := range extensions {
		if got, ok := pathExtension(c.path); got != c.extension || ok != c.ok {
			t.Errorf("pathExtension(%q) = %q %v, want %q %v", c.path, got, ok, c.extension, c.ok)
		}
	}
	if name, ok := pathx.FileName("/a/mcp.json/"); !ok || name != "mcp.json" {
		t.Errorf("pathFileName = %q %v", name, ok)
	}
}
