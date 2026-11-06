package flyrail

import (
	"encoding/json"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

const translationRoot = "/fixture/translation"

type noticeRecord struct {
	ID, Kind, Code, Message string
}

func noticeRecords(notices []Notice) []noticeRecord {
	records := make([]noticeRecord, len(notices))
	for index, notice := range notices {
		records[index] = noticeRecord{notice.ArtifactID, string(notice.Kind), notice.Code, notice.Message}
	}
	return records
}

func assertUnsupported(t *testing.T, result ArtifactTranslation, id, code, message string) {
	t.Helper()
	if result.Supported() {
		t.Errorf("%s: translation is supported", code)
	}
	if _, ok := result.Artifact(); ok {
		t.Errorf("%s: translation produced an artifact", code)
	}
	if diff := cmp.Diff([]noticeRecord{{id, "unsupported", code, message}}, noticeRecords(result.Notices())); diff != "" {
		t.Errorf("notices (-want +got):\n%s", diff)
	}
}

func fixtureCommand(t *testing.T, raw map[string]any) Command {
	t.Helper()
	argument := func(value any) CommandArgument {
		if text, ok := value.(string); ok {
			return ArgText(text)
		}
		reference := value.(map[string]any)
		return must(NewAssetRef(reference["asset"].(string), reference["path"].(string)))
	}
	var argv []CommandArgument
	for _, value := range raw["argv"].([]any) {
		argv = append(argv, argument(value))
	}
	env := map[string]EnvironmentValue{}
	if values, ok := raw["env"].(map[string]any); ok {
		for name, value := range values {
			if text, ok := value.(string); ok {
				env[name] = EnvText(text)
			} else {
				env[name] = must(NewEnvRef(value.(map[string]any)["env"].(string)))
			}
		}
	}
	var cwd CommandArgument
	if value, ok := raw["cwd"]; ok {
		cwd = argument(value)
	}
	return must(NewCommand(argv, env, cwd))
}

func fixtureMCP(t *testing.T, source map[string]any) MCPArtifact {
	t.Helper()
	transport := source["transport"].(map[string]any)
	var selected MCPTransport
	if transport["kind"] == "stdio" {
		selected = fixtureCommand(t, transport["command"].(map[string]any))
	} else {
		var token *EnvRef
		if name, ok := transport["bearer_token_env"].(string); ok {
			reference := must(NewEnvRef(name))
			token = &reference
		}
		selected = must(NewHTTPTransport(transport["url"].(string), token))
	}
	return must(NewMCPArtifact(source["id"].(string), source["name"].(string), selected))
}

func plainMCP(t *testing.T, id, name string, argv ...string) MCPArtifact {
	t.Helper()
	arguments := make([]CommandArgument, len(argv))
	for index, value := range argv {
		arguments[index] = ArgText(value)
	}
	return must(NewMCPArtifact(id, name, must(NewCommand(arguments, nil, nil))))
}

func TestAllSharedInstructionAndMCPVectorsTranslateRealContent(t *testing.T) {
	var document struct {
		Cases []struct {
			Name          string         `json:"name"`
			Agent         Agent          `json:"agent"`
			Surface       Surface        `json:"surface"`
			Scope         TargetScope    `json:"scope"`
			ExecutionRoot string         `json:"execution_root"`
			MCPPath       string         `json:"mcp_path"`
			Artifact      map[string]any `json:"artifact"`
			Expected      map[string]any `json:"expected"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(fixture(t, "translations.json"), &document); err != nil {
		t.Fatal(err)
	}
	unsupportedMessages := map[string]string{
		"instruction-destination":      "Cursor user rules have no verified filesystem preset.",
		"portable-auth-env-unverified": "Copilot CLI bearer environment syntax is unverified.",
		"literal-interpolation":        "env contains host interpolation syntax; use native content.",
	}
	instructions, mcp := 0, 0
	for _, platform := range []Platform{PlatformMacOS, PlatformLinux, PlatformWindows} {
		for _, c := range document.Cases {
			target := must(ProjectTarget(c.Agent, translationRoot))
			if c.Scope == ScopeUser {
				target = must(UserTarget(c.Agent, translationRoot, nil))
			}
			opts := RenderOptions{Surface: c.Surface}
			if c.ExecutionRoot != "" {
				opts.ExecutionRoot = translationRoot + "/" + c.ExecutionRoot
			}
			if c.MCPPath != "" {
				opts.MCPPath = translationRoot + "/" + c.MCPPath
			}
			rc := must(NewRenderContext(target, platform, opts))
			id := c.Artifact["id"].(string)
			var result ArtifactTranslation
			if c.Artifact["kind"] == "instruction" {
				instructions++
				result = must(TranslateInstructionArtifact("translation-demo", must(NewInstructionArtifact(id, c.Artifact["text"].(string))), rc))
			} else {
				mcp++
				result = must(TranslateMCPArtifact(fixtureMCP(t, c.Artifact), rc, nil))
			}
			if code, ok := c.Expected["unsupported"].(string); ok {
				assertUnsupported(t, result, id, code, unsupportedMessages[code])
				continue
			}
			artifact, ok := result.Artifact()
			if !result.Supported() || !ok || len(artifact.SchemaRequirements()) != 0 {
				t.Errorf("%s: unsupported translation %+v", c.Name, result.Notices())
				continue
			}
			if destination, ok := c.Expected["destination"].(string); ok && artifact.Destination() != translationRoot+"/"+destination {
				t.Errorf("%s: destination %q", c.Name, artifact.Destination())
			}
			if directory, ok := c.Expected["directory"].(string); ok {
				name, _ := pathx.FileName(artifact.Destination())
				if suffix := c.Expected["suffix"].(string); artifact.Destination() != translationRoot+"/"+directory+"/"+name || len(name) <= len(suffix) || name[len(name)-len(suffix):] != suffix {
					t.Errorf("%s: destination %q", c.Name, artifact.Destination())
				}
			}
			switch content := artifact.Content().(type) {
			case StructuredContent:
				expected := c.Expected["value"].(map[string]any)
				if relative, ok := c.Expected["cwd_from_root"].(string); ok {
					expected["cwd"] = translationRoot + "/" + relative
				}
				if content.Format() != DocumentFormat(c.Expected["format"].(string)) || !ValuesEqual(content.Value(), jsonFixtureValue(t, expected)) {
					t.Errorf("%s: content %s %s", c.Name, content.Format(), encoded(t, content.Value()))
				}
				parts := content.Selector().Parts()
				if len(parts) != 2 || parts[0].(Key).Name() != c.Expected["key"] || parts[1].(Key).Name() != "tools" {
					t.Errorf("%s: selector %v", c.Name, parts)
				}
			case SectionContent:
				if content.Text() != c.Expected["text"] {
					t.Errorf("%s: section text %q", c.Name, content.Text())
				}
			case FileContent:
				if string(content.Data()) != c.Expected["text"] {
					t.Errorf("%s: file text %q", c.Name, content.Data())
				}
			default:
				t.Errorf("%s: unexpected content %T", c.Name, content)
			}
		}
	}
	if instructions != 48 || mcp != 57 {
		t.Errorf("translated %d instructions and %d MCP registrations", instructions, mcp)
	}
}

func TestUnsupportedBoundariesPreserveExactCodesAndMessages(t *testing.T) {
	project := func(agent Agent, platform Platform, opts RenderOptions) RenderContext {
		return must(NewRenderContext(must(ProjectTarget(agent, translationRoot)), platform, opts))
	}
	alias := must(NewMCPArtifact("tools", "tools", must(NewCommand([]CommandArgument{ArgText("server")},
		map[string]EnvironmentValue{"ALIAS": must(NewEnvRef("TOKEN"))}, nil))))
	collision := must(NewMCPArtifact("windows-environment", "tools", must(NewCommand([]CommandArgument{ArgText("server")},
		map[string]EnvironmentValue{"Mode": EnvText("one"), "MODE": EnvText("two")}, nil))))
	reference := must(NewMCPArtifact("portable-reference", "tools", must(NewCommand([]CommandArgument{ArgText("server")},
		map[string]EnvironmentValue{"TOKEN": must(NewEnvRef("TOKEN"))}, nil))))
	cwd := must(NewMCPArtifact("tools", "tools", must(NewCommand([]CommandArgument{ArgText("server")}, nil, ArgText("work")))))
	relocated := must(NewRenderContext(must(UserTarget(AgentClaude, translationRoot,
		lookupMap(map[string]string{"CLAUDE_CONFIG_DIR": "~/selected"}))), PlatformLinux, RenderOptions{}))
	vscodeUser := func(opts RenderOptions) RenderContext {
		opts.Surface = SurfaceVSCode
		return must(NewRenderContext(must(UserTarget(AgentCopilot, translationRoot, nil)), PlatformLinux, opts))
	}
	cases := []struct {
		artifact      MCPArtifact
		rc            RenderContext
		code, message string
	}{
		{plainMCP(t, "tools", "workspace", "server"), project(AgentClaude, PlatformLinux, RenderOptions{}), "reserved-server-name", "Claude reserves this MCP server name."},
		{alias, project(AgentCodex, PlatformLinux, RenderOptions{}), "environment-alias", "Codex env_vars forwards a variable under its own name."},
		{plainMCP(t, "explicit-path", "tools", "server"), relocated, "mcp-explicit-path", "Relocated Claude user MCP requires an explicit mcp_path."},
		{collision, project(AgentClaude, PlatformWindows, RenderOptions{}), "windows-env-collision", "Environment names collide under Windows case-insensitive lookup."},
		{reference, project(AgentPi, PlatformLinux, RenderOptions{}), "portable-env-reference-unverified", "This host cannot represent this environment reference."},
		{cwd, project(AgentClaude, PlatformLinux, RenderOptions{}), "command-cwd", "The selected MCP schema cannot honor cwd."},
		{cwd, project(AgentCursor, PlatformLinux, RenderOptions{}), "command-cwd", "The selected MCP schema cannot honor cwd."},
		{plainMCP(t, "tools", "tools", "safe"), vscodeUser(RenderOptions{}), "mcp-profile-path", "VS Code user MCP requires an explicit profile mcp.json path."},
		{plainMCP(t, "tools", "tools", "safe"), vscodeUser(RenderOptions{MCPPath: translationRoot + "/profile/wrong.json"}), "mcp-profile-schema",
			"VS Code routes require mcp.json with the servers schema; use the CLI surface for Agent Host native mcpServers files."},
		{plainMCP(t, "tools", "tools", "server", "${TOKEN}"), project(AgentClaude, PlatformLinux, RenderOptions{}), "literal-interpolation", "argv contains host interpolation syntax; use native content."},
		{plainMCP(t, "tools", "tools", "server", "${workspaceFolder}"), project(AgentCursor, PlatformLinux, RenderOptions{Surface: SurfaceIDE}), "literal-interpolation", "argv contains host interpolation syntax; use native content."},
		{plainMCP(t, "tools", "tools", "server", "{file:secret}"), project(AgentOpencode, PlatformLinux, RenderOptions{}), "literal-interpolation", "argv contains host interpolation syntax; use native content."},
		{plainMCP(t, "tools", "tools", "server", "${TOKEN}"), project(AgentCopilot, PlatformLinux, RenderOptions{}), "literal-interpolation-unverified", "Literal argv substitution-like syntax is unverified for Copilot CLI."},
		{plainMCP(t, "tools", "tools", "server", "${input:secret}"), project(AgentCopilot, PlatformLinux, RenderOptions{Surface: SurfaceVSCode}), "literal-interpolation", "argv contains host interpolation syntax; use native content."},
		{plainMCP(t, "tools", "tools", "server", "$env:TOKEN"), project(AgentPi, PlatformLinux, RenderOptions{}), "literal-interpolation", "argv contains host interpolation syntax; use native content."},
		{plainMCP(t, "tools", "tools", "server"), project(AgentDroid, PlatformLinux, RenderOptions{}), "agent-unsupported", "Flyrail detects droid but has no verified configuration translation."},
	}
	for _, c := range cases {
		assertUnsupported(t, must(TranslateMCPArtifact(c.artifact, c.rc, nil)), c.artifact.ID(), c.code, c.message)
	}

	instruction := must(NewInstructionArtifact("guide", "Always."))
	assertUnsupported(t, must(TranslateInstructionArtifact("translation-demo", instruction,
		project(AgentCursor, PlatformLinux, RenderOptions{Surface: SurfaceIDE, InstructionPath: translationRoot + "/custom.mdc"}))),
		"guide", "native-instruction-required", "Use native content for authored modular rule metadata.")
	assertUnsupported(t, must(TranslateInstructionArtifact("translation-demo", instruction,
		vscodeUser(RenderOptions{InstructionPath: translationRoot + "/bad.md"}))),
		"guide", "instruction-extension", "VS Code user instructions require an .instructions.md file.")

	asset := must(NewMCPArtifact("tools", "tools", must(NewCommand([]CommandArgument{ArgText("node"), must(NewAssetRef("runtime", "server.js"))}, nil, nil))))
	_, err := TranslateMCPArtifact(asset, project(AgentCodex, PlatformLinux, RenderOptions{}), nil)
	if got := valueErrorText(t, err); got != "unknown translated asset reference" {
		t.Errorf("unknown asset: %q", got)
	}
}

func TestInstructionMarkerAndRenderDigestRemainWireCompatible(t *testing.T) {
	rc := must(NewRenderContext(must(ProjectTarget(AgentCodex, "/fixture/project")), PlatformLinux, RenderOptions{}))
	translation := must(TranslateInstructionArtifact("translation-demo", must(NewInstructionArtifact("guide", "Run the checks.\n")), rc))
	artifact, _ := translation.Artifact()
	section := artifact.Content().(SectionContent)
	if section.Marker() != "81a6fb0b74cac727508df8661502ff555a19792d232631d282cc3bfcd196dc93" {
		t.Errorf("marker = %s", section.Marker())
	}
	rendered := must(AssembleTranslations([]ArtifactTranslation{translation}, nil, []RenderContext{rc}, rc.RoutingContext()))
	if got := rendered.ContentDigest(); got != "51ff0633093b74dd5486198a0145530c3883c3f68c30f8140805a66f536f6f8b" {
		t.Errorf("digest = %s", got)
	}
}

func TestNoticesMatchEachSurface(t *testing.T) {
	project := func(agent Agent, surface Surface) RenderContext {
		return must(NewRenderContext(must(ProjectTarget(agent, translationRoot)), PlatformLinux, RenderOptions{Surface: surface}))
	}
	vscodeUser := must(NewRenderContext(must(UserTarget(AgentCopilot, translationRoot, nil)), PlatformLinux,
		RenderOptions{Surface: SurfaceVSCode, MCPPath: translationRoot + "/profile/mcp.json"}))
	instruction := must(NewInstructionArtifact("guide", "Always."))
	mcp := plainMCP(t, "tools", "tools", "server")
	loading := noticeRecord{"guide", "activation", "instruction-loading", "Instruction discovery and loading are controlled by the host and current session."}
	activation := noticeRecord{"tools", "activation", "mcp-activation", "Registration does not grant trust, approve tools, supply credentials or prove server activation."}
	cases := []struct {
		result ArtifactTranslation
		want   []noticeRecord
	}{
		{must(TranslateInstructionArtifact("translation-demo", instruction, project(AgentCodex, SurfaceCLI))), []noticeRecord{loading, {"guide", "activation", "codex-instruction-discovery",
			"AGENTS.override.md takes precedence; instructions default to a 32 KiB limit. Start a new session after changes and check host discovery."}}},
		{must(TranslateInstructionArtifact("translation-demo", instruction, vscodeUser)), []noticeRecord{loading, {"guide", "activation", "vscode-agent-host-instructions",
			"User instructions target VS Code Agent Host; local extension-host sessions may use different discovery settings."}}},
		{must(TranslateMCPArtifact(mcp, project(AgentClaude, SurfaceCLI), nil)), []noticeRecord{activation}},
		{must(TranslateMCPArtifact(mcp, project(AgentPi, SurfaceCLI), nil)), []noticeRecord{activation, {"tools", "prerequisite", "pi-mcp-adapter-v2-32-1",
			"Requires externally installed pi-mcp-adapter; emitted schema targets v2.32.1."}}},
		{must(TranslateMCPArtifact(mcp, vscodeUser, nil)), []noticeRecord{activation, {"tools", "activation", "vscode-mcp-forwarding",
			"VS Code forwards these servers to Agent Host, which does not read this servers document directly."}}},
	}
	for _, c := range cases {
		if diff := cmp.Diff(c.want, noticeRecords(c.result.Notices())); diff != "" {
			t.Errorf("notices (-want +got):\n%s", diff)
		}
	}
	file, _ := must(TranslateInstructionArtifact("translation-demo", instruction, vscodeUser)).Artifact()
	if got := string(file.Content().(FileContent).Data()); got != "---\napplyTo: \"**\"\n---\n\nAlways." {
		t.Errorf("VS Code instruction file = %q", got)
	}
}

func TestSharedAssetManifestSubstitutesThePinnedRevision(t *testing.T) {
	var document translationFixture
	if err := json.Unmarshal(fixture(t, "translations.json"), &document); err != nil {
		t.Fatal(err)
	}
	source := document.AssetManifest["artifacts"].([]any)[0].(map[string]any)
	rc := must(NewRenderContext(must(ProjectTarget(AgentCodex, "/fixture/project")), PlatformLinux, RenderOptions{}))
	revision := "/fixture/assets/runtime/revisions/sha256-pinned"
	assets := map[string]string{"runtime": revision}
	result := must(TranslateMCPArtifact(fixtureMCP(t, source), rc, assets))
	assets["runtime"] = "/changed"
	artifact, _ := result.Artifact()
	if artifact.Destination() != "/fixture/project/.codex/config.toml" {
		t.Errorf("destination = %q", artifact.Destination())
	}
	content := artifact.Content().(StructuredContent)
	want := jsonFixtureValue(t, map[string]any{"command": "node", "args": []string{revision + "/server.js"}, "env": map[string]any{}, "cwd": revision + "/data"})
	if content.Format() != FormatTOML || !ValuesEqual(want, content.Value()) {
		t.Errorf("content = %s %s", content.Format(), encoded(t, content.Value()))
	}

	bundle := manifestBundle(t, document.AssetManifest,
		BundleEntry{Path: "runtime/server.js", Data: []byte(`console.log("MCP");`)},
		BundleEntry{Path: "runtime/data", Dir: true},
	)
	rendered := must(Render(bundle, must(NewRenderContext(must(ProjectTarget(AgentCodex, translationRoot)), PlatformLinux,
		RenderOptions{AssetRoot: translationRoot + "/assets"}))))
	if !rendered.Supported() || len(rendered.Artifacts()) != 2 || len(rendered.Dependencies()) != 1 {
		t.Fatalf("rendered %d artifacts, %d dependencies, notices %v", len(rendered.Artifacts()), len(rendered.Dependencies()), rendered.Notices())
	}
	runtime, tools := rendered.Artifacts()[0], rendered.Artifacts()[1]
	if filepath.Dir(runtime.Destination()) != translationRoot+"/assets/support-demo/runtime" || len(filepath.Base(runtime.Destination())) != 64 {
		t.Errorf("runtime destination = %q", runtime.Destination())
	}
	want = jsonFixtureValue(t, map[string]any{
		"command": "node", "args": []string{runtime.Destination() + "/server.js"}, "env": map[string]any{}, "cwd": runtime.Destination() + "/data",
	})
	if got := tools.Content().(StructuredContent).Value(); !ValuesEqual(want, got) {
		t.Errorf("tools value = %s", encoded(t, got))
	}
}

func TestSharedCombinedVectorsAreOrderIndependent(t *testing.T) {
	var document translationFixture
	if err := json.Unmarshal(fixture(t, "translations.json"), &document); err != nil {
		t.Fatal(err)
	}
	var raw struct {
		CombinedCases []struct {
			Expected map[string]any `json:"expected"`
		} `json:"combined_cases"`
	}
	if err := json.Unmarshal(fixture(t, "translations.json"), &raw); err != nil {
		t.Fatal(err)
	}
	if len(document.CombinedCases) != 4 {
		t.Fatalf("fixture has %d combined cases", len(document.CombinedCases))
	}
	for index, c := range document.CombinedCases {
		bundle := manifestBundle(t, schemaTwoManifest("combined", c.Artifacts))
		contexts := make([]RenderContext, len(c.Contexts))
		for position, value := range c.Contexts {
			contexts[position] = must(NewRenderContext(must(ProjectTarget(value.Agent, translationRoot)), PlatformLinux, RenderOptions{}))
		}
		forward := must(RenderMany(bundle, contexts...))
		reversed := slices.Clone(contexts)
		slices.Reverse(reversed)
		reverse := must(RenderMany(bundle, reversed...))
		if forward.ContentDigest() != reverse.ContentDigest() || !cmp.Equal(forward.Notices(), reverse.Notices()) ||
			!cmp.Equal(forward.RoutingContext(), reverse.RoutingContext()) {
			t.Errorf("%s: render depends on context order", c.Name)
		}
		var unsupported []string
		for _, notice := range forward.Notices() {
			if notice.Kind == NoticeUnsupported {
				unsupported = append(unsupported, notice.Code)
			}
		}
		if code, ok := raw.CombinedCases[index].Expected["unsupported"].(string); ok {
			if forward.Supported() || len(forward.Artifacts()) != 0 || !cmp.Equal([]string{code}, unsupported) {
				t.Errorf("%s: supported %v, %d artifacts, unsupported %v", c.Name, forward.Supported(), len(forward.Artifacts()), unsupported)
			}
			continue
		}
		if !forward.Supported() || len(forward.Artifacts()) != 2 || len(forward.Dependencies()) != 0 || len(forward.RoutingContext()) != 2 {
			t.Errorf("%s: supported %v, %d artifacts, %d routes", c.Name, forward.Supported(), len(forward.Artifacts()), len(forward.RoutingContext()))
		}
	}
}

func TestAssemblyPreservesDependenciesAndPropagatesDependencyErrors(t *testing.T) {
	rc := must(NewRenderContext(must(ProjectTarget(AgentClaude, translationRoot)), PlatformLinux, RenderOptions{}))
	translated := func(id string) ArtifactTranslation {
		return must(TranslateMCPArtifact(plainMCP(t, id, id, "server"), rc, nil))
	}
	edge := must(NewDependency("tools", "runtime", ""))
	assembled := must(AssembleTranslations([]ArtifactTranslation{translated("runtime"), translated("tools")}, []Dependency{edge}, []RenderContext{rc}, rc.RoutingContext()))
	if !assembled.Supported() || !cmp.Equal([]Dependency{edge}, assembled.Dependencies(), cmp.AllowUnexported(Dependency{})) {
		t.Errorf("assembled dependencies = %v", assembled.Dependencies())
	}
	cases := []struct {
		deps    []Dependency
		message string
	}{
		{[]Dependency{must(NewDependency("tools", "missing", ""))}, "dependency references an unknown artifact or asset"},
		{[]Dependency{edge, must(NewDependency("runtime", "tools", ""))}, "cyclic artifact dependencies"},
	}
	for _, c := range cases {
		_, err := AssembleTranslations([]ArtifactTranslation{translated("runtime"), translated("tools")}, c.deps, []RenderContext{rc}, rc.RoutingContext())
		if got := valueErrorText(t, err); got != c.message {
			t.Errorf("%q, want %q", got, c.message)
		}
	}
}

func TestAssemblyBlocksDestinationFormatAndCopilotPrecedenceCollisions(t *testing.T) {
	project := func(agent Agent, mcpPath string) RenderContext {
		return must(NewRenderContext(must(ProjectTarget(agent, translationRoot)), PlatformLinux, RenderOptions{MCPPath: mcpPath}))
	}
	first := plainMCP(t, "first", "tools", "first-server")
	second := plainMCP(t, "second", "tools", "second-server")
	compatible := plainMCP(t, "compatible", "tools", "first-server")
	translate := func(artifact MCPArtifact, rc RenderContext) ArtifactTranslation {
		return must(TranslateMCPArtifact(artifact, rc, nil))
	}
	activation := func(id string) noticeRecord {
		return noticeRecord{id, "activation", "mcp-activation", "Registration does not grant trust, approve tools, supply credentials or prove server activation."}
	}
	claude := project(AgentClaude, "")
	common := must(AssembleTranslations([]ArtifactTranslation{translate(first, claude), translate(compatible, claude)}, nil, []RenderContext{claude}, nil))
	if !common.Supported() || len(common.Artifacts()) != 2 {
		t.Errorf("identical registrations: supported %v, %d artifacts", common.Supported(), len(common.Artifacts()))
	}

	shared := translationRoot + "/shared"
	codexShared, claudeShared := project(AgentCodex, shared), project(AgentClaude, shared)
	copilot := project(AgentCopilot, translationRoot+"/.github/mcp.json")
	codexHigher := project(AgentCodex, translationRoot+"/.mcp.json")
	cases := []struct {
		name         string
		translations []ArtifactTranslation
		contexts     []RenderContext
		last         noticeRecord
		supported    bool
	}{
		{"destination", []ArtifactTranslation{translate(first, claude), translate(second, claude)}, []RenderContext{claude},
			noticeRecord{"second", "unsupported", "destination-collision", "Incompatible claims from first and second at " + translationRoot + "/.mcp.json."}, false},
		{"format", []ArtifactTranslation{translate(first, codexShared), translate(second, claudeShared)}, []RenderContext{codexShared, claudeShared},
			noticeRecord{"second", "unsupported", "document-format-collision", "Incompatible document formats from first and second at " + shared + "."}, false},
		{"precedence", []ArtifactTranslation{translate(first, claude), translate(second, copilot)}, []RenderContext{claude, copilot},
			noticeRecord{"second", "unsupported", "copilot-mcp-precedence-collision", "Copilot CLI selects " + translationRoot + "/.mcp.json over " +
				translationRoot + "/.github/mcp.json for server tools; the selected records disagree."}, false},
		{"unverified", []ArtifactTranslation{translate(first, codexHigher), translate(second, copilot)}, []RenderContext{codexHigher, copilot},
			noticeRecord{"second", "activation", "copilot-mcp-precedence-unverified", "Copilot CLI gives " + translationRoot + "/.mcp.json precedence over " +
				translationRoot + "/.github/mcp.json; native content requires caller review of the effective server definitions."}, true},
	}
	for _, c := range cases {
		assembled := must(AssembleTranslations(c.translations, nil, c.contexts, nil))
		want := []noticeRecord{activation("first"), activation("second"), c.last}
		if diff := cmp.Diff(want, noticeRecords(assembled.Notices())); diff != "" {
			t.Errorf("%s notices (-want +got):\n%s", c.name, diff)
		}
		if assembled.Supported() != c.supported || (len(assembled.Artifacts()) == 0) == c.supported {
			t.Errorf("%s: supported %v with %d artifacts", c.name, assembled.Supported(), len(assembled.Artifacts()))
		}
	}
}

func TestRenderingDoesNoIO(t *testing.T) {
	root := filepath.Join(tempRoot(t), "missing", "project")
	bundle := manifestBundle(t, schemaTwoManifest("offline", []any{
		map[string]any{"kind": "instruction", "id": "guide", "text": "Offline."},
		map[string]any{"kind": "mcp", "id": "tools", "name": "tools", "transport": map[string]any{
			"kind": "stdio", "command": map[string]any{"argv": []string{"server"}, "cwd": "work"},
		}},
		map[string]any{"kind": "hook", "id": "start", "protocol_version": 1, "event": "session-start",
			"command": map[string]any{"argv": []string{"/bin/echo"}}, "timeout_ms": 1000, "outcomes": []string{"continue"}, "tools": []string{}},
	}))
	runtime := must(NewHookRuntime("/opt/node/bin/node", ShellPOSIX))
	rc := must(NewRenderContext(must(ProjectTarget(AgentCodex, root)), PlatformLinux, RenderOptions{HookRuntime: runtime}))
	rendered := must(Render(bundle, rc))
	if !rendered.Supported() || len(rendered.Artifacts()) != 5 {
		t.Fatalf("rendered %d artifacts with notices %v", len(rendered.Artifacts()), rendered.Notices())
	}
	for _, artifact := range rendered.Artifacts() {
		if len(artifact.Destination()) <= len(root) || artifact.Destination()[:len(root)+1] != root+"/" {
			t.Errorf("%s destination %q is outside the root", artifact.ID(), artifact.Destination())
		}
	}
	if _, err := os.Lstat(filepath.Dir(root)); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("rendering created %s: %v", filepath.Dir(root), err)
	}
}

func TestSkillsRenderIntoTheContainerSubtree(t *testing.T) {
	bundle := must(NewBundleFromEntries([]byte(defaultManifest), []BundleEntry{
		{Path: "skills/review/SKILL.md", Data: []byte("---\nname: review\ndescription: Review.\n---\n")},
	}))
	user := must(NewRenderContext(must(UserTarget(AgentCopilot, "/home", nil)), PlatformLinux, RenderOptions{Surface: SurfaceVSCode}))
	rendered := must(Render(bundle, user))
	if len(rendered.Artifacts()) != 1 {
		t.Fatalf("rendered %v with notices %v", rendered.Artifacts(), rendered.Notices())
	}
	skill := rendered.Artifacts()[0]
	if skill.ID() != "review" || skill.Family() != FamilySkills || skill.Destination() != "/home/.copilot/skills" || skill.Subtree() != "review" {
		t.Errorf("skill artifact %s %s %q %q", skill.ID(), skill.Family(), skill.Destination(), skill.Subtree())
	}
	entries := skill.Content().(TreeContent).Entries()
	if len(entries) != 1 || entries[0].Path != "SKILL.md" {
		t.Errorf("skill entries %v", entries)
	}
	droid := must(Render(bundle, must(NewRenderContext(must(ProjectTarget(AgentDroid, "/project")), PlatformLinux, RenderOptions{}))))
	if diff := cmp.Diff([]noticeRecord{{"review", "unsupported", "agent-unsupported", "Flyrail detects droid but has no verified configuration translation."}},
		noticeRecords(droid.Notices())); diff != "" || len(droid.Artifacts()) != 0 {
		t.Errorf("droid notices (-want +got):\n%s", diff)
	}
}
