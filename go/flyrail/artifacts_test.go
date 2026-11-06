package flyrail

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func command(t *testing.T, argv ...CommandArgument) Command {
	t.Helper()
	return must(NewCommand(argv, nil, nil))
}

func TestCommandsSnapshotAndSortTheirEnvironment(t *testing.T) {
	argv := []CommandArgument{ArgText("python"), ArgText("space and 'quotes'"), ArgText("$(not-executed)")}
	env := map[string]EnvironmentValue{"TOKEN": must(NewEnvRef("SOURCE_TOKEN")), "MODE": EnvText("quiet")}
	cmd := must(NewCommand(argv, env, ArgText("support")))
	argv[0] = ArgText("changed")
	env["EXTRA"] = EnvText("x")

	if diff := cmp.Diff([]CommandArgument{ArgText("python"), ArgText("space and 'quotes'"), ArgText("$(not-executed)")}, cmd.Argv()); diff != "" {
		t.Errorf("argv (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff(map[string]EnvironmentValue{"MODE": EnvText("quiet"), "TOKEN": must(NewEnvRef("SOURCE_TOKEN"))}, cmd.Env(), cmp.AllowUnexported(EnvRef{})); diff != "" {
		t.Errorf("env (-want +got):\n%s", diff)
	}
	if cmd.Cwd() != CommandArgument(ArgText("support")) {
		t.Errorf("cwd = %v", cmd.Cwd())
	}
	want := `["object",[["fields",["object",[["argv",["array",[["string","python"],["string","space and 'quotes'"],["string","$(not-executed)"]]]],` +
		`["cwd",["string","support"]],["env",["array",[["array",[["string","MODE"],["string","quiet"]]],` +
		`["array",[["string","TOKEN"],["object",[["fields",["object",[["name",["string","SOURCE_TOKEN"]]]]],["type",["string","EnvRef"]]]]]]]]]]]],` +
		`["type",["string","Command"]]]]`
	if got := encoded(t, cmd.modelValue()); got != want {
		t.Errorf("model:\n got %s\nwant %s", got, want)
	}
}

func TestCommandsRejectUnsafeOrAmbiguousInput(t *testing.T) {
	cases := []struct {
		err     error
		message string
	}{
		{errorOf(NewCommand(nil, nil, nil)), "argv requires a command"},
		{errorOf(NewCommand([]CommandArgument{ArgText("")}, nil, nil)), "command cannot be empty"},
		{errorOf(NewCommand([]CommandArgument{ArgText("\x00")}, nil, nil)), "command arguments cannot contain NUL"},
		{errorOf(NewCommand([]CommandArgument{ArgText("x")}, map[string]EnvironmentValue{"A": EnvText("\x00")}, nil)), "environment values cannot contain NUL"},
		{errorOf(NewCommand([]CommandArgument{ArgText("x")}, nil, ArgText("../escape"))), "command cwd must be an unambiguous relative path"},
		{errorOf(NewCommand([]CommandArgument{ArgText("x")}, map[string]EnvironmentValue{"9BAD": EnvText("x")}, nil)), "invalid environment reference"},
		{errorOf(NewCommand([]CommandArgument{ArgText("x")}, map[string]EnvironmentValue{"A": nil}, nil)), "environment values must be EnvText or EnvRef values"},
		{errorOf(NewCommand([]CommandArgument{ArgText("x")}, map[string]EnvironmentValue{"A": EnvRef{}}, nil)), "invalid environment reference"},
		{errorOf(NewCommand([]CommandArgument{nil}, nil, nil)), "command arguments must be ArgText or AssetRef values"},
		{errorOf(NewCommand([]CommandArgument{AssetRef{}}, nil, nil)), "referenced asset id must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
		{errorOf(NewCommand([]CommandArgument{ArgText("\xff")}, nil, nil)), "command argument must contain Unicode scalar values"},
	}
	for index, tc := range cases {
		if got := valueMessage(t, tc.err); got != tc.message {
			t.Errorf("case %d: %q, want %q", index, got, tc.message)
		}
	}
	asset := must(NewAssetRef("runtime", "tools"))
	if _, err := NewCommand([]CommandArgument{asset}, nil, must(NewAssetRef("runtime", ""))); err != nil {
		t.Error(err)
	}
}

func TestHTTPTransportsRequireAbsoluteCleanHTTPURLs(t *testing.T) {
	token := must(NewEnvRef("REMOTE_TOKEN"))
	transport := must(NewHTTPTransport("https://tools.example.test:8443/mcp", &token))
	if got, ok := transport.BearerToken(); transport.URL() != "https://tools.example.test:8443/mcp" || !ok || got.Name() != "REMOTE_TOKEN" {
		t.Errorf("transport = %+v", transport)
	}
	for _, url := range []string{
		"stdio:python", "https://user:pass@example.test/x", "https://example.test/a b", "http:///x", "http:////x",
		"http://", "http://:80/", "http://user@h/", "http://h/#frag", "http://h:0/", "http://h:65536/", "http://h:a/",
		"http://h:+80/", "http://h: 80/", "http://h/ ", "http://[abc]/", "http://[]/", "http://[::1/",
		"http://[1:2:3]/", "http://[v1.]/", "1http://x", "http://h:99999999999/", "http://h/\xff",
	} {
		if _, err := NewHTTPTransport(url, nil); err == nil {
			t.Errorf("accepted %q", url)
		}
	}
	for _, url := range []string{
		"HTTP://H/", "http://h:80/", "http://h/#", "http://h:", "http://h/?q=1", "http://[::1]:8080/", "http://[v1.x]/",
		"http://[::ffff:192.0.2.1]/", "http://h/\x7f", "https://K.example/", "https://ｅxample.test/",
	} {
		if _, err := NewHTTPTransport(url, nil); err != nil {
			t.Errorf("rejected %q: %v", url, err)
		}
	}
	if got := valueMessage(t, errorOf(NewHTTPTransport("https://h/", &EnvRef{}))); got != "invalid environment reference" {
		t.Errorf("zero bearer token: %q", got)
	}
}

func TestHTTPTransportAuthorityCasesMatchPythonURLSplit(t *testing.T) {
	cases := map[string]string{
		"http://host]":               "Invalid IPv6 URL",
		"http://]":                   "Invalid IPv6 URL",
		"http://host]:80":            "Invalid IPv6 URL",
		"https://[::1]junk/":         "Invalid IPv6 URL",
		"https://prefix[::1]/":       "Invalid IPv6 URL",
		"https://example／evil/path":  "netloc 'example／evil' contains invalid characters under NFKC normalization",
		"https://example？evil/path":  "netloc 'example？evil' contains invalid characters under NFKC normalization",
		"https://example＃evil/path":  "netloc 'example＃evil' contains invalid characters under NFKC normalization",
		"https://example＠evil/path":  "netloc 'example＠evil' contains invalid characters under NFKC normalization",
		"https://example：80/path":    "netloc 'example：80' contains invalid characters under NFKC normalization",
		"https://user@[::1]:8080/":   "MCP URL cannot embed credentials or a fragment",
		"http://[1:2:3:4:5:6:7:8:9]": "'1:2:3:4:5:6:7:8:9' does not appear to be an IPv4 or IPv6 address",
		"http://[1::2::3]/":          "'1::2::3' does not appear to be an IPv4 or IPv6 address",
		"http://[1:2:3:4:5:6:7]/":    "'1:2:3:4:5:6:7' does not appear to be an IPv4 or IPv6 address",
		"http://[fe80::1%]/":         "'fe80::1%' does not appear to be an IPv4 or IPv6 address",
		"http://[::1.2.3.04]/":       "bracketed host is not a valid IPv6 address",
		"http://[vg.x]/":             "IPvFuture address is invalid",
		"ftp://h/":                   "MCP URL must be an absolute HTTP or HTTPS URL",
		"http://h:0/":                "MCP URL port must be 1-65535",
		"http://h/a\tb":              "MCP URL cannot contain whitespace or control characters",
	}
	for url, message := range cases {
		if got := valueMessage(t, errorOf(NewHTTPTransport(url, nil))); got != message {
			t.Errorf("%q: %q, want %q", url, got, message)
		}
	}
	for _, url := range []string{"https://[::1]/", "https://[::1]:8080/", "https://[v1.x]/", "http://[fe80::1%25en0]/", "http://[1:2:3:4:5:6:7:8]/"} {
		if _, err := NewHTTPTransport(url, nil); err != nil {
			t.Errorf("rejected %q: %v", url, err)
		}
	}
}

func hookParams(t *testing.T) HookArtifactParams {
	t.Helper()
	return HookArtifactParams{
		ID:            "check-tool",
		Event:         EventBeforeTool,
		Command:       command(t, ArgText("test")),
		TimeoutMillis: 10_000,
		Outcomes:      []HookOutcome{OutcomeContinue, OutcomeBlock},
		Tools:         []string{"shell", "edit"},
	}
}

func TestHooksValidateEventsOutcomesToolsAndTimeouts(t *testing.T) {
	params := hookParams(t)
	hook := must(NewHookArtifact(params))
	params.Outcomes[0] = OutcomeAsk
	if hook.ProtocolVersion() != 1 || hook.TimeoutMillis() != 10_000 || hook.Event() != EventBeforeTool {
		t.Errorf("hook = %+v", hook)
	}
	if diff := cmp.Diff([]HookOutcome{OutcomeBlock, OutcomeContinue}, hook.Outcomes()); diff != "" {
		t.Errorf("outcomes (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff([]string{"edit", "shell"}, hook.Tools()); diff != "" {
		t.Errorf("tools (-want +got):\n%s", diff)
	}
	stop := hookParams(t)
	stop.Event, stop.Tools, stop.TimeoutMillis = EventStop, nil, 1
	if tools := must(NewHookArtifact(stop)).Tools(); len(tools) != 0 {
		t.Errorf("tools = %v", tools)
	}

	cases := []struct {
		change  func(*HookArtifactParams)
		message string
	}{
		{func(p *HookArtifactParams) { p.TimeoutMillis = 0 }, "hook timeout must be 1-300000 integer milliseconds"},
		{func(p *HookArtifactParams) { p.TimeoutMillis = 300_001 }, "hook timeout must be 1-300000 integer milliseconds"},
		{func(p *HookArtifactParams) { p.Outcomes = nil }, "hook requires declared HookOutcome values"},
		{func(p *HookArtifactParams) { p.Outcomes = []HookOutcome{OutcomeContinue, OutcomeContinue} }, "hook outcomes must be unique"},
		{func(p *HookArtifactParams) { p.Outcomes = []HookOutcome{"maybe"} }, "'maybe' is not a valid HookOutcome"},
		{func(p *HookArtifactParams) { p.Tools = []string{"x", "x"} }, "tool names must be unique"},
		{func(p *HookArtifactParams) { p.Event = EventStop }, "tool selection requires a tool event"},
		{func(p *HookArtifactParams) { p.Tools = []string{""} }, "tool names must be nonempty and cannot contain NUL"},
		{func(p *HookArtifactParams) { p.Event = "" }, "'' is not a valid HookEvent"},
		{func(p *HookArtifactParams) { p.Command = Command{} }, "argv requires a command"},
		{func(p *HookArtifactParams) { p.ID = "Bad" }, "artifact id must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
	}
	for index, tc := range cases {
		params := hookParams(t)
		tc.change(&params)
		if got := valueMessage(t, errorOf(NewHookArtifact(params))); got != tc.message {
			t.Errorf("case %d: %q, want %q", index, got, tc.message)
		}
	}
}

func TestAudiencesBindSurfacesToTheirAgents(t *testing.T) {
	cursor := must(NewAudience(AgentCursor, ScopeProject, SurfaceIDE, PlatformWindows))
	if cursor.Platform() != PlatformWindows || cursor.Surface() != SurfaceIDE {
		t.Errorf("cursor = %+v", cursor)
	}
	if copilot := must(NewAudience(AgentCopilot, ScopeUser, SurfaceVSCode, "")); copilot.Platform() != "" {
		t.Errorf("copilot = %+v", copilot)
	}
	cases := []struct {
		err     error
		message string
	}{
		{errorOf(NewAudience(AgentCodex, ScopeDirectory, SurfaceCLI, "")), "native audiences require project or user scope"},
		{errorOf(NewAudience(AgentCodex, ScopeProject, SurfaceIDE, "")), "IDE surface is only defined for Cursor"},
		{errorOf(NewAudience(AgentCursor, ScopeProject, SurfaceVSCode, "")), "VSCode surface is only defined for Copilot"},
		{errorOf(NewAudience("vim", ScopeProject, SurfaceCLI, "")), "'vim' is not a valid Agent"},
		{errorOf(NewAudience(AgentCodex, ScopeProject, "", "")), "'' is not a valid Surface"},
		{errorOf(NewAudience(AgentCodex, ScopeProject, SurfaceCLI, "beos")), "'beos' is not a valid Platform"},
	}
	for index, tc := range cases {
		if got := valueMessage(t, tc.err); got != tc.message {
			t.Errorf("case %d: %q, want %q", index, got, tc.message)
		}
	}
}

func TestEnvironmentAndAssetReferencesValidateTheirNames(t *testing.T) {
	for _, name := range []string{"SOURCE_TOKEN", "_private9"} {
		if _, err := NewEnvRef(name); err != nil {
			t.Errorf("rejected %q: %v", name, err)
		}
	}
	for _, name := range []string{"BAD-NAME", "9BAD", "", "WITH SPACE", "ÄB"} {
		if got := valueMessage(t, errorOf(NewEnvRef(name))); got != "invalid environment reference" {
			t.Errorf("%q: %q", name, got)
		}
	}
	reference := must(NewAssetRef("runtime", "server.js"))
	if reference.AssetID() != "runtime" || reference.Path() != "server.js" {
		t.Errorf("reference = %+v", reference)
	}
	if got := valueMessage(t, errorOf(NewAssetRef("runtime", "../escape"))); got != "asset reference path must be an unambiguous relative path" {
		t.Errorf("escape: %q", got)
	}
	if got := valueMessage(t, errorOf(NewAssetRef("Bad", ""))); got != "referenced asset id must be 1-64 lowercase ASCII letters/digits with internal hyphens" {
		t.Errorf("bad id: %q", got)
	}
}

func TestSkillsInstructionsAndNativeArtifactsValidateTheirFields(t *testing.T) {
	content := must(NewTreeContent([]BundleEntry{file("SKILL.md", "# Review\n"), {Path: "run.py", Data: []byte("print(1)\n"), Executable: true}}))
	skill := must(NewSkillArtifact("review-skill", "review", content))
	if skill.ID() != "review-skill" || skill.Name() != "review" || len(skill.Content().Entries()) != 2 {
		t.Errorf("skill = %+v", skill)
	}
	readable := must(NewFileContent([]byte("hi"), ModeReadable))
	audience := must(NewAudience(AgentCodex, ScopeProject, SurfaceCLI, ""))
	native := must(NewNativeArtifact("guide", FamilyInstructions, audience, "rules/guide.md", readable))
	if native.Destination() != "rules/guide.md" || native.Family() != FamilyInstructions || native.Audience() != audience {
		t.Errorf("native = %+v", native)
	}
	cases := []struct {
		err     error
		message string
	}{
		{errorOf(NewSkillArtifact("x", "x", must(NewTreeContent([]BundleEntry{dir("SKILL.md")})))), "skill must contain a SKILL.md file"},
		{errorOf(NewSkillArtifact("x", "x", TreeContent{})), "skill must contain a SKILL.md file"},
		{errorOf(NewInstructionArtifact("guide", "\x00")), "instruction text cannot contain NUL"},
		{errorOf(NewNativeArtifact("guide", FamilyInstructions, audience, "../escape", readable)), "native destination must be an unambiguous relative path"},
		{errorOf(NewNativeArtifact("guide", FamilyInstructions, Audience{}, "a.md", readable)), "'' is not a valid Agent"},
		{errorOf(NewNativeArtifact("guide", "rules", audience, "a.md", readable)), "'rules' is not a valid Family"},
		{errorOf(NewNativeArtifact("guide", FamilyInstructions, audience, "a.md", nil)), "content must be FileContent, TreeContent, SectionContent or StructuredContent"},
		{errorOf(NewNativeArtifact("guide", FamilyInstructions, audience, "a.md", FileContent{})), "'' is not a valid FileMode"},
		{errorOf(NewNativeArtifact("guide", FamilyInstructions, audience, "a.md", StructuredContent{})), "'' is not a valid DocumentFormat"},
		{errorOf(NewMCPArtifact("tools", "tools", nil)), "MCP transports must be Command or HTTPTransport values"},
		{errorOf(NewMCPArtifact("tools", "tools", HTTPTransport{})), "MCP URL must be an absolute HTTP or HTTPS URL"},
		{errorOf(NewMCPArtifact("tools", "Tools", command(t, ArgText("x")))), "MCP server name must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
		{errorOf(NewSupportAsset("hook-support", "", TreeContent{})), "'' is not a valid Family"},
	}
	for index, tc := range cases {
		if got := valueMessage(t, tc.err); got != tc.message {
			t.Errorf("case %d: %q, want %q", index, got, tc.message)
		}
	}
}

func TestParseEnumsUseRustSpellingsAndText(t *testing.T) {
	if family, err := ParseFamily("mcp"); err != nil || family != FamilyMCP {
		t.Errorf("ParseFamily = %q, %v", family, err)
	}
	if event, err := ParseHookEvent("session-start"); err != nil || event != EventSessionStart {
		t.Errorf("ParseHookEvent = %q, %v", event, err)
	}
	if outcome, err := ParseHookOutcome("modify-input"); err != nil || outcome != OutcomeModifyInput {
		t.Errorf("ParseHookOutcome = %q, %v", outcome, err)
	}
	if mode, err := ParseDependencyMode("stable-reference"); err != nil || mode != DependencyStableReference {
		t.Errorf("ParseDependencyMode = %q, %v", mode, err)
	}
	if got := valueMessage(t, errorOf(ParseFamily("MCP"))); got != "'MCP' is not a valid Family" {
		t.Errorf("ParseFamily(MCP) = %q", got)
	}
	if diff := cmp.Diff([]Agent{"claude", "codex", "opencode", "pi", "cursor", "copilot", "droid"}, Agents()); diff != "" {
		t.Errorf("Agents (-want +got):\n%s", diff)
	}
}

func TestDependencyOrderPutsRequiredArtifactsFirst(t *testing.T) {
	deps := []Dependency{must(NewDependency("b", "a", "")), must(NewDependency("c", "a", DependencyStableReference))}
	if deps[0].Mode() != DependencyRevision {
		t.Errorf("default mode = %q", deps[0].Mode())
	}
	if diff := cmp.Diff([]string{"a", "d", "b", "c"}, must(DependencyOrder([]string{"d", "c", "b", "a"}, deps))); diff != "" {
		t.Errorf("order (-want +got):\n%s", diff)
	}
	cases := []struct {
		err     error
		message string
	}{
		{errorOf(DependencyOrder([]string{"a", "b"}, []Dependency{deps[0], deps[0]})), "duplicate dependency"},
		{errorOf(DependencyOrder([]string{"b"}, deps[:1])), "dependency references an unknown artifact or asset"},
		{errorOf(DependencyOrder([]string{"a", "b"}, []Dependency{deps[0], must(NewDependency("a", "b", ""))})), "cyclic artifact dependencies"},
		{errorOf(NewDependency("a", "a", "")), "an artifact cannot depend on itself"},
		{errorOf(NewDependency("a", "b", "eventual")), "'eventual' is not a valid DependencyMode"},
		{errorOf(DependencyOrder([]string{"a"}, []Dependency{{}})), "dependent artifact must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
	}
	for index, tc := range cases {
		if got := valueMessage(t, tc.err); got != tc.message {
			t.Errorf("case %d: %q, want %q", index, got, tc.message)
		}
	}
}
