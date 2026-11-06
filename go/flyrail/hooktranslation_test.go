package flyrail

import (
	"slices"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

func hookManifest(id, event string, outcomes ...string) map[string]any {
	tools := []string{}
	if event == "before-tool" || event == "after-tool" {
		tools = []string{"Read"}
	}
	return map[string]any{
		"kind": "hook", "id": id, "protocol_version": 1, "event": event,
		"command":    map[string]any{"argv": []string{"/bin/echo", "space ü#%😀"}, "env": map[string]any{"UNICODE": "ü😀"}, "cwd": "work ü😀"},
		"timeout_ms": 1000, "outcomes": outcomes, "tools": tools,
	}
}

func hookBundle(t *testing.T, hooks ...map[string]any) *Bundle {
	t.Helper()
	artifacts := make([]any, len(hooks))
	for index, hook := range hooks {
		artifacts[index] = hook
	}
	return manifestBundle(t, schemaTwoManifest("hook-matrix", artifacts))
}

func hookContext(t *testing.T, agent Agent, root string, user bool, platform Platform, runtime *HookRuntime) RenderContext {
	t.Helper()
	target := must(ProjectTarget(agent, root))
	if user {
		target = must(UserTarget(agent, root, nil))
	}
	return must(NewRenderContext(target, platform, RenderOptions{HookRuntime: runtime}))
}

func posixRuntime(t *testing.T) *HookRuntime {
	t.Helper()
	return must(NewHookRuntime("/opt/node/bin/node", ShellPOSIX))
}

func TestHookRenderMatrixEmitsRuntimeHostContentDependenciesAndNotices(t *testing.T) {
	bundle := hookBundle(t, hookManifest("start", "session-start", "continue"))
	root := "/fixture/project"
	for _, agent := range TranslatableAgents() {
		for _, user := range []bool{false, true} {
			rendered := must(Render(bundle, hookContext(t, agent, root, user, PlatformLinux, posixRuntime(t))))
			wantNotices := []noticeRecord{
				{"start", "prerequisite", "hook-runtime-prerequisite", "Requires Node 24+ for native/Pi bridges, Bun 1.4.2+ for OpenCode; host APIs documented in the hook protocol."},
				{"start", "activation", "hook-activation", "Installation only writes assets/configuration. Host trust, reload and actual activation remain host-controlled."},
			}
			if diff := cmp.Diff(wantNotices, noticeRecords(rendered.Notices())); diff != "" {
				t.Errorf("%s user=%v notices (-want +got):\n%s", agent, user, diff)
			}
			embedded := agent == AgentOpencode || agent == AgentPi
			artifacts, dependencies := 3, 2
			if embedded {
				artifacts, dependencies = 2, 1
			}
			if len(rendered.Artifacts()) != artifacts || len(rendered.Dependencies()) != dependencies {
				t.Errorf("%s user=%v: %d artifacts, %d dependencies", agent, user, len(rendered.Artifacts()), len(rendered.Dependencies()))
				continue
			}
			for _, artifact := range rendered.Artifacts() {
				tree, ok := artifact.Content().(TreeContent)
				if !ok {
					continue
				}
				var paths []string
				for _, entry := range tree.Entries() {
					paths = append(paths, entry.Path)
				}
				if !cmp.Equal([]string{"bridge.mts", "config.mts", "process.mts", "runner.mts"}, paths) ||
					!strings.HasPrefix(artifact.Destination(), root+"/.flyrail-assets/hook-matrix/.hooks/"+string(agent)+"/revisions/") {
					t.Errorf("%s runtime %q holds %v", agent, artifact.Destination(), paths)
				}
			}
			host := rendered.Artifacts()[slices.IndexFunc(rendered.Artifacts(), func(a RenderedArtifact) bool { return a.ID() == "start" })]
			hostEdge := slices.ContainsFunc(rendered.Dependencies(), func(edge Dependency) bool {
				return edge.Dependent() == "start" && (embedded || edge.Mode() == DependencyStableReference)
			})
			if !hostEdge {
				t.Errorf("%s user=%v dependencies %v", agent, user, rendered.Dependencies())
			}
			_, file := host.Content().(FileContent)
			_, structured := host.Content().(StructuredContent)
			switch {
			case agent == AgentOpencode:
				ok := file && strings.Contains(host.Destination(), "/plugins/flyrail-")
				if !ok {
					t.Errorf("OpenCode host %q", host.Destination())
				}
			case agent == AgentPi:
				if !file || !strings.Contains(host.Destination(), "/extensions/flyrail-") {
					t.Errorf("Pi host %q", host.Destination())
				}
			case agent == AgentCopilot:
				if !file || !strings.HasSuffix(host.Destination(), ".json") {
					t.Errorf("Copilot host %q", host.Destination())
				}
			case !structured:
				t.Errorf("%s host content %T", agent, host.Content())
			}
		}
	}
}

func TestNativeHookDocumentsMatchEachHost(t *testing.T) {
	bundle := hookBundle(t, hookManifest("start", "session-start", "continue"))
	root := "/fixture/project"
	host := func(agent Agent) RenderedArtifact {
		rendered := must(Render(bundle, hookContext(t, agent, root, false, PlatformLinux, posixRuntime(t))))
		return rendered.Artifacts()[slices.IndexFunc(rendered.Artifacts(), func(a RenderedArtifact) bool { return a.ID() == "start" })]
	}
	launcher := root + "/.flyrail-assets/hook-matrix/.hooks/"
	claude := host(AgentClaude)
	if claude.Destination() != root+"/.claude/settings.json" {
		t.Errorf("Claude destination %q", claude.Destination())
	}
	want := `{"hooks":[{"args":["` + launcher + `claude/entry.mjs","session-start"],"command":"/opt/node/bin/node","timeout":310,"type":"command"}]}`
	if got := jsonText(t, claude.Content().(StructuredContent).Value()); got != want {
		t.Errorf("Claude entry %s", got)
	}
	codex := host(AgentCodex)
	want = `{"hooks":[{"command":"/opt/node/bin/node ` + launcher + `codex/entry.mjs session-start","timeout":310,"type":"command"}]}`
	if got := jsonText(t, codex.Content().(StructuredContent).Value()); got != want || codex.Destination() != root+"/.codex/hooks.json" {
		t.Errorf("Codex entry %s at %q", got, codex.Destination())
	}
	cursor := host(AgentCursor)
	content := cursor.Content().(StructuredContent)
	member := content.Selector().Parts()[2].(Member)
	if content.Format() != FormatJSON || !cmp.Equal([]string{"command"}, member.Key()) || len(cursor.SchemaRequirements()) != 1 {
		t.Errorf("Cursor content %s %v %v", content.Format(), member.Key(), cursor.SchemaRequirements())
	}
}

func jsonText(t *testing.T, value Value) string {
	t.Helper()
	return string(jsonx.Encode(must(valueToJSON(value))))
}

func TestCopilotHookEventsFollowEventOrderIndependentOfArtifactOrder(t *testing.T) {
	bundle := hookBundle(t,
		hookManifest("stop", "stop", "continue"),
		hookManifest("start", "session-start", "continue"),
		hookManifest("prompt", "prompt", "continue"),
		hookManifest("before", "before-tool", "continue"),
		hookManifest("after", "after-tool", "continue"),
	)
	rendered := must(Render(bundle, hookContext(t, AgentCopilot, "/fixture/project", false, PlatformLinux, posixRuntime(t))))
	host := rendered.Artifacts()[slices.IndexFunc(rendered.Artifacts(), func(a RenderedArtifact) bool { return a.ID() == "after" })]
	text := string(host.Content().(FileContent).Data())
	last := -1
	for _, name := range []string{`"postToolUse"`, `"preToolUse"`, `"userPromptSubmitted"`, `"sessionStart"`, `"agentStop"`} {
		position := strings.Index(text, name)
		if position <= last {
			t.Errorf("%s at %d after %d in %s", name, position, last, text)
		}
		last = position
	}
	if !strings.HasSuffix(text, "\n") {
		t.Errorf("Copilot hook file lacks a final newline: %q", text)
	}
}

func TestUnicodeRootsUseFileURLsAndASCIIConfigJSON(t *testing.T) {
	root := "/fixture/space ü#%😀"
	for _, agent := range []Agent{AgentClaude, AgentOpencode} {
		rendered := must(Render(hookBundle(t, hookManifest("start", "session-start", "continue")), hookContext(t, agent, root, false, PlatformLinux, posixRuntime(t))))
		var launcher, config string
		for _, artifact := range rendered.Artifacts() {
			switch content := artifact.Content().(type) {
			case FileContent:
				launcher = string(content.Data())
			case TreeContent:
				for _, entry := range content.Entries() {
					if entry.Path == "config.mts" {
						config = string(entry.Data)
					}
				}
			}
		}
		if !strings.Contains(launcher, "file:///fixture/space%20%C3%BC%23%25%F0%9F%98%80/") {
			t.Errorf("%s launcher %q", agent, launcher)
		}
		if !strings.Contains(config, `\\u00fc\\ud83d\\ude00`) || strings.Contains(config, "ü😀") {
			t.Errorf("%s config %q", agent, config)
		}
	}
	url := must(fileURL(`C:\Tools\node.exe`, PlatformWindows))
	if url != "file:///C:/Tools/node.exe" {
		t.Errorf("Windows drive URL %q", url)
	}
	if url := must(fileURL(`\\server\share\a b`, PlatformWindows)); url != "file://server/share/a%20b" {
		t.Errorf("Windows UNC URL %q", url)
	}
}

func TestUnsupportedHookBoundariesBlockOutputsWithTypedNotices(t *testing.T) {
	root := "/fixture/project"
	start := hookManifest("start", "session-start", "continue")
	runtime := posixRuntime(t)
	cmdRuntime := must(NewHookRuntime("/opt/node/node.CMD", ShellCmd))
	noNode := must(NewHookRuntime("", ShellPOSIX))
	vscode := must(NewRenderContext(must(ProjectTarget(AgentCopilot, root)), PlatformLinux, RenderOptions{Surface: SurfaceVSCode, HookRuntime: runtime}))
	nodeModules := must(NewRenderContext(must(ProjectTarget(AgentClaude, root)), PlatformLinux,
		RenderOptions{HookRuntime: runtime, AssetRoot: root + "/Node_Modules/assets"}))
	cases := []struct {
		name    string
		bundle  *Bundle
		rc      RenderContext
		code    string
		message string
	}{
		{"no runtime", hookBundle(t, start), hookContext(t, AgentClaude, root, false, PlatformLinux, nil), "hook-runtime", "Native hooks require an explicit Node 24+ executable."},
		{"no node", hookBundle(t, start), hookContext(t, AgentCodex, root, false, PlatformLinux, noNode), "hook-runtime", "Native hooks require an explicit Node 24+ executable."},
		{"modification", hookBundle(t, hookManifest("before", "before-tool", "modify-input")), hookContext(t, AgentCodex, root, false, PlatformLinux, runtime),
			"hook-modification-approval", "This host has no verified argument-replacement channel preserving normal approval flow."},
		{"outcome", hookBundle(t, hookManifest("prompt", "prompt", "continue", "block")), hookContext(t, AgentOpencode, root, false, PlatformLinux, runtime),
			"hook-outcome", "opencode cannot preserve the declared prompt outcomes: block, continue"},
		{"surface", hookBundle(t, start), vscode, "hook-surface", "Portable hooks target Copilot CLI, not VS Code."},
		{"node modules", hookBundle(t, start), nodeModules, "hook-node-modules", "Node cannot load stripped TypeScript from a node_modules asset root."},
		{"interpolation", hookBundle(t, start), hookContext(t, AgentClaude, "/fixture/${HOME}", false, PlatformLinux, runtime),
			"hook-literal-interpolation", "Claude launcher paths cannot contain host placeholder syntax."},
		{"posix shell", hookBundle(t, start), hookContext(t, AgentCodex, root, false, PlatformLinux, must(NewHookRuntime("/opt/node/bin/node", ShellCmd))),
			"hook-shell", "This native hook requires a caller-confirmed POSIX-compatible hook shell."},
		{"command shim", hookBundle(t, start), hookContext(t, AgentCodex, root, false, PlatformWindows, cmdRuntime),
			"hook-executable", "Native hooks require a real executable, not a Windows command shim."},
		{"cursor windows", hookBundle(t, start), hookContext(t, AgentCursor, root, false, PlatformWindows, runtime),
			"cursor-windows-shell", "Cursor Windows hook shell selection is unverified; use native content."},
		{"droid", hookBundle(t, start), hookContext(t, AgentDroid, root, false, PlatformLinux, runtime),
			"agent-unsupported", "Flyrail detects droid but has no verified configuration translation."},
	}
	for _, c := range cases {
		rendered := must(Render(c.bundle, c.rc))
		notices := rendered.Notices()
		if rendered.Supported() || len(rendered.Artifacts()) != 0 || len(rendered.Dependencies()) != 0 || len(notices) != 1 {
			t.Errorf("%s: supported %v, %d artifacts, notices %v", c.name, rendered.Supported(), len(rendered.Artifacts()), notices)
			continue
		}
		if notices[0].Code != c.code || notices[0].Message != c.message {
			t.Errorf("%s: notice %s %q", c.name, notices[0].Code, notices[0].Message)
		}
	}
}

func TestHookShellQuotingMatchesTheReference(t *testing.T) {
	rc := hookContext(t, AgentCodex, "/fixture/project", false, PlatformLinux, posixRuntime(t))
	got, failure := hookShellCommand([]string{"/opt/node", "it's", "", "a=b,c"}, rc)
	if failure != nil || got != `/opt/node 'it'"'"'s' '' a=b,c` {
		t.Errorf("POSIX command %q %v", got, failure)
	}
	windows := hookContext(t, AgentCodex, "/fixture/project", false, PlatformWindows, must(NewHookRuntime("/opt/node/node.exe", ShellCmd)))
	if got, failure := hookShellCommand([]string{"/opt/node/node.exe", "entry"}, windows); failure != nil || got != `"/opt/node/node.exe" "entry"` {
		t.Errorf("cmd command %q %v", got, failure)
	}
	if _, failure := hookShellCommand([]string{"a&b"}, windows); failure == nil || failure.Code != "hook-shell-literal" {
		t.Errorf("cmd literal failure %v", failure)
	}
}

func TestRenderManyRejectsEmptyAndDeduplicatesRoutes(t *testing.T) {
	bundle := hookBundle(t, hookManifest("start", "session-start", "continue"))
	_, err := RenderMany(bundle)
	if got := valueErrorText(t, err); got != "render_many requires at least one explicit context" {
		t.Errorf("empty contexts: %q", got)
	}
	root := "/fixture/space ü😀"
	claude := hookContext(t, AgentClaude, root, false, PlatformLinux, posixRuntime(t))
	codex := hookContext(t, AgentCodex, root, false, PlatformLinux, posixRuntime(t))
	forward := must(RenderMany(bundle, claude, codex, claude))
	reverse := must(RenderMany(bundle, codex, claude))
	if forward.ContentDigest() != reverse.ContentDigest() || !cmp.Equal(forward.RoutingContext(), reverse.RoutingContext()) {
		t.Error("render_many depends on context order or repetition")
	}
	routes := forward.RoutingContext()
	if len(routes) != 2 {
		t.Fatalf("routes %v", routes)
	}
	for key, route := range routes {
		if key != identifier(route) || !strings.Contains(route, `\u00fc\ud83d\ude00`) {
			t.Errorf("route %s = %s", key, route)
		}
	}
}
