package flyrail

import (
	"io/fs"
	"path/filepath"
	"testing"
	"testing/fstest"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/zipx/ziptest"
)

func loadConfig(manifest string, entries ...BundleEntry) (configManifest, error) {
	raw, err := jsonx.Parse([]byte(manifest))
	if err != nil {
		return configManifest{}, err
	}
	source, err := newMemorySource([]byte(manifest), entries)
	if err != nil {
		return configManifest{}, err
	}
	parsed, err := readConfigManifest(raw, source)
	if err != nil {
		return configManifest{}, asBundleError(err)
	}
	return parsed, nil
}

func configMessage(t *testing.T, manifest string, entries ...BundleEntry) string {
	t.Helper()
	_, err := loadConfig(manifest, entries...)
	return invalidMessage(t, err)
}

func artifactsManifest(artifacts string) string {
	return `{"schema_version":2,"id":"team","version":"1","artifacts":` + artifacts + `}`
}

func modelBytesOf(t *testing.T, value Value) string {
	t.Helper()
	return encoded(t, value)
}

func TestParsesInstructionsFromTextOrSourcePaths(t *testing.T) {
	parsed := must(loadConfig(artifactsManifest(`[{"kind":"instruction","id":"guide","text":"Hello"}]`)))
	if parsed.identity.ID() != "team" || len(parsed.artifacts) != 1 || len(parsed.assets) != 0 || len(parsed.dependencies) != 0 {
		t.Fatalf("parsed = %+v", parsed)
	}
	sourced := must(loadConfig(artifactsManifest(`[{"kind":"instruction","id":"guide","path":"first.md"}]`), file("first.md", "Hello")))
	if diff := cmp.Diff(parsed.artifacts, sourced.artifacts, cmp.AllowUnexported(InstructionArtifact{})); diff != "" {
		t.Errorf("artifacts (-text +path):\n%s", diff)
	}
	cases := []struct {
		manifest string
		entries  []BundleEntry
		message  string
	}{
		{`[{"kind":"instruction","id":"guide"}]`, nil, "provide exactly one of text or path"},
		{`[{"kind":"instruction","id":"guide","text":"a","path":"b"}]`, nil, "provide exactly one of text or path"},
		{`[{"kind":"instruction","id":"guide","path":"missing.md"}]`, nil, "expected a source file: missing.md"},
		{`[{"kind":"instruction","id":"guide","path":"../escape"}]`, nil, "source path must be an unambiguous relative path"},
		{`[{"kind":"instruction","id":"guide","path":"bad.md"}]`, []BundleEntry{file("bad.md", "\xff")}, "text is not valid UTF-8"},
		{`[{"kind":"instruction","id":"guide","text":"a","unknown":true}]`, nil,
			"expected fields ['id', 'kind']; optional fields ['audience', 'command', 'content', 'destination', 'event', 'executables', 'family', 'name', 'outcomes', 'path', 'protocol_version', 'text', 'timeout_ms', 'tools', 'transport']"},
		{`[{"kind":"instruction","id":"guide","text":"a","name":"x"}]`, nil, "expected fields ['id', 'kind']; optional fields ['path', 'text']"},
		{`[{"kind":"instruction","id":7,"text":"a"}]`, nil, "artifact id must be a string"},
		{`["guide"]`, nil, "manifest and skill records must be JSON objects"},
	}
	for _, tc := range cases {
		if got := configMessage(t, artifactsManifest(tc.manifest), tc.entries...); got != tc.message {
			t.Errorf("%s: %q, want %q", tc.manifest, got, tc.message)
		}
	}
}

func TestParsesSkillTreesAndValidatesExecutables(t *testing.T) {
	entries := []BundleEntry{file("skills/review/SKILL.md", "markdown"), file("skills/review/run.py", "print(1)"), dir("skills/review/empty")}
	parsed := must(loadConfig(artifactsManifest(`[{"kind":"skill","id":"review-skill","name":"review","path":"skills/review","executables":["run.py"]}]`), entries...))
	skill, ok := parsed.artifacts[0].(SkillArtifact)
	if !ok || skill.Name() != "review" {
		t.Fatalf("artifact = %#v", parsed.artifacts[0])
	}
	want := []BundleEntry{file("SKILL.md", "markdown"), dir("empty"), {Path: "run.py", Data: []byte("print(1)"), Executable: true}}
	if diff := cmp.Diff(want, skill.Content().Entries()); diff != "" {
		t.Errorf("entries (-want +got):\n%s", diff)
	}
	cases := []struct {
		manifest string
		entries  []BundleEntry
		message  string
	}{
		{`[{"kind":"skill","id":"s","name":"review","path":"skills/review","executables":["missing.py"]}]`, entries[:1], "every executable must name an existing file"},
		{`[{"kind":"skill","id":"s","name":"review","path":"skills/review","executables":["empty"]}]`, entries, "a directory cannot have executable file intent"},
		{`[{"kind":"skill","id":"s","name":"review","path":"skills/review","executables":["run.py","run.py"]}]`, nil, "duplicate executable path"},
		{`[{"kind":"skill","id":"s","name":"review","path":"missing"}]`, nil, "skill source must be an existing directory: missing"},
		{`[{"kind":"skill","id":"s","name":"review","path":"skills/review","executables":"run.py"}]`, entries, "expected a JSON array"},
	}
	for _, tc := range cases {
		if got := configMessage(t, artifactsManifest(tc.manifest), tc.entries...); got != tc.message {
			t.Errorf("%s: %q, want %q", tc.manifest, got, tc.message)
		}
	}
}

func TestParsesMCPTransportsAndHooks(t *testing.T) {
	parsed := must(loadConfig(artifactsManifest(`[{"kind":"mcp","id":"local","name":"local","transport":{"kind":"stdio",` +
		`"command":{"argv":["python","server.py"],"env":{"API_TOKEN":{"env":"TOOLS_TOKEN"},"MODE":"local"},"cwd":"tools"}}},` +
		`{"kind":"mcp","id":"remote","name":"remote","transport":{"kind":"http",` +
		`"url":"https://tools.example.test/mcp","bearer_token_env":"REMOTE_TOKEN"}}]`)))
	local := parsed.artifacts[0].(MCPArtifact).Transport().(Command)
	if diff := cmp.Diff([]CommandArgument{ArgText("python"), ArgText("server.py")}, local.Argv()); diff != "" {
		t.Errorf("argv (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff(map[string]EnvironmentValue{"API_TOKEN": must(NewEnvRef("TOOLS_TOKEN")), "MODE": EnvText("local")}, local.Env(), cmp.AllowUnexported(EnvRef{})); diff != "" {
		t.Errorf("env (-want +got):\n%s", diff)
	}
	if local.Cwd() != CommandArgument(ArgText("tools")) {
		t.Errorf("cwd = %v", local.Cwd())
	}
	remote := parsed.artifacts[1].(MCPArtifact).Transport().(HTTPTransport)
	if token, ok := remote.BearerToken(); remote.URL() != "https://tools.example.test/mcp" || !ok || token.Name() != "REMOTE_TOKEN" {
		t.Errorf("remote = %+v", remote)
	}

	hooks := must(loadConfig(artifactsManifest(`[{"kind":"hook","id":"check","event":"before-tool","protocol_version":1,` +
		`"command":{"argv":["python","check.py"]},"outcomes":["continue","block"],"tools":["shell"]}]`)))
	hook := hooks.artifacts[0].(HookArtifact)
	if hook.TimeoutMillis() != 10_000 || hook.ProtocolVersion() != 1 || len(hook.Outcomes()) != 2 {
		t.Errorf("hook = %+v", hook)
	}
	defaulted := must(loadConfig(artifactsManifest(`[{"kind":"hook","id":"check","event":"stop","command":{"argv":["test"]},"protocol_version":1}]`)))
	if hook := defaulted.artifacts[0].(HookArtifact); !cmp.Equal([]HookOutcome{OutcomeContinue}, hook.Outcomes()) || len(hook.Tools()) != 0 {
		t.Errorf("defaulted hook = %+v", hook)
	}

	hookCase := func(extra string) string {
		return `[{"kind":"hook","id":"h","event":"stop","command":{"argv":["t"]}` + extra + `}]`
	}
	cases := []struct {
		manifest string
		message  string
	}{
		{hookCase(`,"protocol_version":true`), "hook protocol_version must be integer 1"},
		{hookCase(`,"protocol_version":2`), "hook protocol_version must be integer 1"},
		{hookCase(`,"protocol_version":1,"timeout_ms":true`), "hook timeout must be an integer"},
		{hookCase(`,"protocol_version":1,"timeout_ms":1.0`), "hook timeout must be an integer"},
		{`[{"kind":"hook","id":"h","event":"never","command":{"argv":["t"]},"protocol_version":1,"timeout_ms":4294967296}]`, "hook timeout must be 1-300000 integer milliseconds"},
		{`[{"kind":"hook","id":"h","event":"never","command":{"argv":["t"]},"protocol_version":1,"timeout_ms":0}]`, "'never' is not a valid HookEvent"},
		{hookCase(`,"protocol_version":1,"timeout_ms":0`), "hook timeout must be 1-300000 integer milliseconds"},
		{hookCase(`,"protocol_version":1,"outcomes":["continue","maybe"]`), "'maybe' is not a valid HookOutcome"},
		{hookCase(`,"protocol_version":1,"tools":["shell"]`), "tool selection requires a tool event"},
		{`[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"other"}}]`, "unknown MCP transport"},
		{`[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"stdio","command":{"argv":[],"env":[]}}}]`, "command env must be a JSON object"},
		{`[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"stdio","command":{"argv":[]}}}]`, "argv requires a command"},
		{`[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"stdio","command":{"argv":["x"],"env":{"A":{"env":"9"}}}}}]`, "invalid environment reference"},
		{`[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"stdio","command":{"argv":["x"],"env":{"B":"a\u0000","1x":"y"}}}}]`, "environment values cannot contain NUL"},
		{`[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"stdio","command":{"argv":["x"],"env":{"1x":"y","B":"a\u0000"}}}}]`, "invalid environment reference"},
		{`[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"stdio","command":{"argv":[{"asset":"tools","path":"../x"}]}}}]`, "source path must be an unambiguous relative path"},
		{`[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"http","url":"ftp://x/"}}]`, "MCP URL must be an absolute HTTP or HTTPS URL"},
		{`[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"http","url":"https://x/","command":{}}}]`, "expected fields ['kind', 'url']; optional fields ['bearer_token_env']"},
	}
	for _, tc := range cases {
		if got := configMessage(t, artifactsManifest(tc.manifest)); got != tc.message {
			t.Errorf("%s: %q, want %q", tc.manifest, got, tc.message)
		}
	}
}

func TestCommandEnvIsStoredInNameByteOrderWhateverTheManifestOrder(t *testing.T) {
	const wantDigest = "9b19f387eefe69f7bf747e2b16f1d330dac5d4cd11ba31f2626c05a46625040c"
	const wantModel = `["array",[["array",[["object",[["fields",["object",[["id",["string","s"]],["name",["string","s"]],` +
		`["transport",["object",[["fields",["object",[["argv",["array",[["string","x"]]]],["cwd",["null"]],["env",["array",[` +
		`["array",[["string","API_TOKEN"],["object",[["fields",["object",[["name",["string","TOOLS_TOKEN"]]]]],["type",["string","EnvRef"]]]]]],` +
		`["array",[["string","B"],["string","one"]]],["array",[["string","B_2"],["string","two"]]],["array",[["string","Z"],["string","z"]]],` +
		`["array",[["string","_x"],["string","u"]]],["array",[["string","mode"],["string","local"]]]]]]]]],["type",["string","Command"]]]]]]]],` +
		`["type",["string","McpArtifact"]]]]]],["array",[]],["array",[]]]]`
	manifest := func(env string) string {
		return artifactsManifest(`[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"stdio","command":{"argv":["x"],"env":` + env + `}}}]`)
	}
	for _, env := range []string{
		`{"mode":"local","_x":"u","API_TOKEN":{"env":"TOOLS_TOKEN"},"Z":"z","B_2":"two","B":"one"}`,
		`{"API_TOKEN":{"env":"TOOLS_TOKEN"},"B":"one","B_2":"two","Z":"z","_x":"u","mode":"local"}`,
	} {
		bundle := must(NewBundleFromEntries([]byte(manifest(env)), nil))
		if got := bundleModelOf(t, bundle); got != wantModel {
			t.Errorf("%s: model\n%s\nwant\n%s", env, got, wantModel)
		}
		if bundle.ContentDigest() != wantDigest {
			t.Errorf("%s: digest %s, want %s", env, bundle.ContentDigest(), wantDigest)
		}
	}
}

func TestParsesNativeContentOfEveryKind(t *testing.T) {
	entries := []BundleEntry{file("native.md", "# Native\n"), file("skills/review/SKILL.md", "markdown")}
	parsed := must(loadConfig(artifactsManifest(`[{"kind":"native","id":"native-guide","family":"instructions",`+
		`"audience":{"agent":"claude","scope":"project"},"destination":"guidance/custom.md",`+
		`"content":{"kind":"file","path":"native.md","mode":"readable"}},`+
		`{"kind":"native","id":"native-section","family":"instructions",`+
		`"audience":{"agent":"codex","scope":"project"},"destination":"AGENTS.md",`+
		`"content":{"kind":"section","marker":"guide","text":"Use the project checks.\n"}},`+
		`{"kind":"native","id":"native-array","family":"hooks",`+
		`"audience":{"agent":"claude","scope":"user"},"destination":".claude/settings.json",`+
		`"content":{"kind":"structured","format":"json","selector":["hooks","PreToolUse",`+
		`{"identity":{"command":"check"}}],"value":{"command":"check"},`+
		`"placement":{"position":"after","anchor":{"identity":{"command":"existing"}}}}},`+
		`{"kind":"native","id":"native-key","family":"mcp",`+
		`"audience":{"agent":"codex","scope":"user","platform":"linux"},"destination":".codex/config.toml",`+
		`"content":{"kind":"structured","format":"toml","selector":["mcp_servers","review"],`+
		`"value":{"command":"review"}}},`+
		`{"kind":"native","id":"native-tree","family":"skills",`+
		`"audience":{"agent":"cursor","scope":"project","surface":"ide"},"destination":".cursor/skills/review",`+
		`"content":{"kind":"tree","path":"skills/review"}}]`), entries...))

	guide := parsed.artifacts[0].(NativeArtifact)
	fileContent := guide.Content().(FileContent)
	if guide.Audience().Agent() != AgentClaude || string(fileContent.Data()) != "# Native\n" || fileContent.Mode() != ModeReadable {
		t.Errorf("guide = %+v", guide)
	}
	section := parsed.artifacts[1].(NativeArtifact).Content().(SectionContent)
	if section.Text() != "Use the project checks.\n" || section.Selector().Start() != "<!-- flyrail:guide:start -->" {
		t.Errorf("section = %+v", section)
	}
	structured := parsed.artifacts[2].(NativeArtifact).Content().(StructuredContent)
	parts := structured.Selector().Parts()
	member, isMember := parts[2].(Member)
	placement, placed := structured.Placement()
	if len(parts) != 3 || parts[0].(Key).Name() != "hooks" || parts[1].(Key).Name() != "PreToolUse" ||
		!isMember || len(member.Key()) != 0 || !placed || placement.Position() != PositionAfter {
		t.Errorf("structured = %+v", structured)
	}
	keyed := parsed.artifacts[3].(NativeArtifact)
	if keyed.Content().(StructuredContent).Format() != FormatTOML || keyed.Audience().Platform() != PlatformLinux {
		t.Errorf("keyed = %+v", keyed)
	}
	tree := parsed.artifacts[4].(NativeArtifact)
	if tree.Content().(TreeContent).Entries()[0].Path != "SKILL.md" || tree.Audience().Surface() != SurfaceIDE || tree.Audience().Scope() != ScopeProject {
		t.Errorf("tree = %+v", tree)
	}
}

func TestStructuredValuesArePlainOrSemanticButNeverBoth(t *testing.T) {
	native := `[{"kind":"native","id":"native-key","family":"mcp",` +
		`"audience":{"agent":"codex","scope":"user"},"destination":".codex/config.toml",` +
		`"content":{"kind":"structured","format":"toml","selector":["mcp_servers","review"],`
	semantic := must(loadConfig(artifactsManifest(native + `"semantic_value":["date","2026-01-02"]}}]`)))
	if value := semantic.artifacts[0].(NativeArtifact).Content().(StructuredContent).Value(); !ValuesEqual(value, mustDate(t, 2026, 1, 2)) {
		t.Errorf("semantic value = %v", value)
	}
	plain := must(loadConfig(artifactsManifest(native + `"value":1}}]`)))
	if value := plain.artifacts[0].(NativeArtifact).Content().(StructuredContent).Value(); !ValuesEqual(value, Int(1)) {
		t.Errorf("plain value = %v", value)
	}
	if got := configMessage(t, artifactsManifest(native+`"value":1,"semantic_value":["integer","1"]}}]`)); got != "provide exactly one of value or semantic_value" {
		t.Errorf("both values: %q", got)
	}
	if got := configMessage(t, artifactsManifest(native+`"value":18446744073709551616}}]`)); got != "integers must fit signed 64 bits" {
		t.Errorf("big integer: %q", got)
	}
	membered := must(loadConfig(artifactsManifest(`[{"kind":"native","id":"m","family":"mcp",` +
		`"audience":{"agent":"codex","scope":"user"},"destination":"c.toml",` +
		`"content":{"kind":"structured","format":"toml","selector":["mcp_servers",` +
		`{"identity":"review","key":["name"]}],` +
		`"semantic_value":["object",[["command",["string","run"]],["name",["string","review"]]]]}}]`)))
	parts := membered.artifacts[0].(NativeArtifact).Content().(StructuredContent).Selector().Parts()
	member := parts[1].(Member)
	if !ValuesEqual(member.Identity(), must(NewString("review"))) || !cmp.Equal([]string{"name"}, member.Key()) {
		t.Errorf("member = %+v", member)
	}
}

func TestParsesAssetsDependenciesAndEnforcesScope(t *testing.T) {
	parsed := must(loadConfig(`{"schema_version":2,"id":"team","version":"1",`+
		`"artifacts":[{"kind":"instruction","id":"guide","text":"Hello"}],`+
		`"assets":[{"id":"hook-support","family":"hooks","path":"support"}],`+
		`"dependencies":[{"dependent":"guide","required":"hook-support"}]}`, file("support/check.py", "pass\n")))
	if len(parsed.assets) != 1 || parsed.assets[0].ID() != "hook-support" || parsed.dependencies[0].Dependent() != "guide" ||
		parsed.dependencies[0].Mode() != DependencyRevision {
		t.Errorf("parsed = %+v", parsed)
	}
	native := func(audience, content string) string {
		return `{"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":"native","id":"n","family":"instructions",` +
			`"audience":` + audience + `,"destination":"AGENTS.md","content":` + content + `}]}`
	}
	cases := []struct {
		manifest string
		message  string
	}{
		{`{"schema_version":2.0,"id":"team","version":"1","artifacts":[{"kind":"instruction","id":"guide","text":"x"}]}`, "configuration schema_version must be integer 2"},
		{`{"schema_version":2,"id":"team","version":"1","artifacts":[],"permissions":{}}`, "expected fields ['artifacts', 'id', 'schema_version', 'version']; optional fields ['assets', 'dependencies']"},
		{`{"schema_version":2,"id":"team","version":"1","artifacts":{}}`, "expected a JSON array"},
		{`{"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":"settings","id":"settings"}]}`, "unknown artifact kind"},
		{`{"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":7,"id":"settings"}]}`, "unknown artifact kind"},
		{`{"schema_version":2,"id":"Team","version":"1","artifacts":[]}`, "bundle id must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
		{`{"schema_version":2,"id":"team","version":"1","artifacts":[],"dependencies":[{"dependent":"a","required":"b","mode":"later"}]}`, "'later' is not a valid DependencyMode"},
		{`{"schema_version":2,"id":"team","version":"1","artifacts":[],"assets":[{"id":"a","family":"rules","path":"x"}]}`, "'rules' is not a valid Family"},
		{native(`{"agent":"codex","scope":"project"}`, `{"kind":"yaml"}`), "unknown native content kind"},
		{native(`{"agent":"codex","scope":"directory"}`, `{"kind":"section","marker":"guide","text":"hi"}`), "native audiences require project or user scope"},
		{native(`{"agent":"codex","scope":"project","os":"linux"}`, `{"kind":"section","marker":"guide","text":"hi"}`), "expected fields ['agent', 'scope']; optional fields ['platform', 'surface']"},
		{native(`{"agent":"codex","scope":"project"}`, `{"kind":"file","path":"x","marker":"m"}`), "expected fields ['kind', 'path']; optional fields ['mode']"},
		{native(`{"agent":"codex","scope":"project"}`, `{"kind":"structured","format":"json","selector":[],"value":1}`), "a selector must select a key or member"},
		{native(`{"agent":"codex","scope":"project"}`, `{"kind":"structured","format":"ini","selector":["a"],"value":1}`), "'ini' is not a valid DocumentFormat"},
		{native(`{"agent":"codex","scope":"project"}`, `{"kind":"structured","format":"json","selector":["a"],"value":1,"placement":{"position":"first"}}`), "placement is only valid for an array member"},
		{native(`{"agent":"codex","scope":"project"}`, `{"kind":"section","marker":"guide","text":"start\n","boundaries":{"start":"start","end":"end"}}`), "section text cannot contain its boundary lines"},
	}
	for _, tc := range cases {
		if got := configMessage(t, tc.manifest); got != tc.message {
			t.Errorf("%s: %q, want %q", tc.manifest, got, tc.message)
		}
	}
}

func TestRejectsDuplicateConfigurationFields(t *testing.T) {
	manifest := artifactsManifest(`[{"kind":"instruction","id":"guide","text":"a","text":"b"}]`)
	_, err := NewBundleFromEntries([]byte(manifest), nil)
	if got := invalidMessage(t, err); got != "duplicate JSON field: text" {
		t.Errorf("duplicate field: %q", got)
	}
}

type configurationCase struct {
	name     string
	manifest string
	entries  []BundleEntry
	digest   string
}

func configurationCases(t *testing.T) []configurationCase {
	t.Helper()
	raw, err := jsonx.Parse(fixture(t, "configurations.json"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []configurationCase
	for _, item := range fixtureItems(t, raw, "cases") {
		manifest, _ := item.Get("manifest")
		c := configurationCase{
			name:     fixtureText(t, item, "name"),
			manifest: string(jsonx.Encode(manifest)),
			digest:   fixtureText(t, item, "content_digest"),
		}
		for _, entry := range fixtureItems(t, item, "entries") {
			data, _ := entry.Get("data_hex")
			if text, ok := data.AsString(); ok {
				c.entries = append(c.entries, BundleEntry{Path: fixtureText(t, entry, "path"), Data: fromHex(t, text)})
			} else {
				c.entries = append(c.entries, dir(fixtureText(t, entry, "path")))
			}
		}
		cases = append(cases, c)
	}
	if len(cases) == 0 {
		t.Fatal("no configuration fixtures")
	}
	return cases
}

func configurationArchive(c configurationCase) []byte {
	specs := []ziptest.Spec{ziptest.File("flyrail/flyrail.json", []byte(c.manifest))}
	for _, entry := range c.entries {
		if entry.Dir {
			specs = append(specs, ziptest.Directory("flyrail/"+entry.Path))
		} else {
			specs = append(specs, ziptest.File("flyrail/"+entry.Path, entry.Data))
		}
	}
	return ziptest.Write(specs...)
}

func bundleModelOf(t *testing.T, b *Bundle) string {
	t.Helper()
	return modelBytesOf(t, bundleModel(b.Artifacts(), b.Assets(), b.Dependencies()))
}

func TestMatchesTheSharedConfigurationFixtures(t *testing.T) {
	for _, c := range configurationCases(t) {
		t.Run(c.name, func(t *testing.T) {
			remembered := must(NewBundleFromEntries([]byte(c.manifest), c.entries))
			parent := tempRoot(t)
			root := filepath.Join(parent, "flyrail")
			writeFile(t, filepath.Join(root, "flyrail.json"), c.manifest)
			fsys := fstest.MapFS{"flyrail.json": {Data: []byte(c.manifest)}}
			for _, entry := range c.entries {
				if entry.Dir {
					mkdir(t, filepath.Join(root, entry.Path))
					fsys[entry.Path] = &fstest.MapFile{Mode: fs.ModeDir | 0o755}
				} else {
					writeFile(t, filepath.Join(root, entry.Path), string(entry.Data))
					fsys[entry.Path] = &fstest.MapFile{Data: entry.Data, Mode: 0o755}
				}
			}
			directory := openDir(t, root)
			archive := filepath.Join(parent, "package.zip")
			writeFile(t, archive, string(configurationArchive(c)))
			zipped := must(OpenBundleZip(t.Context(), archive, "flyrail"))
			loaded := must(LoadBundleFS(fsys))

			model := bundleModelOf(t, remembered)
			for name, bundle := range map[string]*Bundle{"memory": remembered, "directory": directory, "archive": zipped, "fs": loaded} {
				if bundle.SchemaVersion() != 2 || bundle.ContentDigest() != c.digest {
					t.Errorf("%s: schema %d digest %s, want %s", name, bundle.SchemaVersion(), bundle.ContentDigest(), c.digest)
				}
				if got := bundleModelOf(t, bundle); got != model {
					t.Errorf("%s model differs from memory:\n%s\n%s", name, got, model)
				}
			}
			if len(remembered.SourceRoots()) != 0 || !cmp.Equal([]string{root}, directory.SourceRoots()) || !cmp.Equal([]string{archive}, zipped.SourceRoots()) {
				t.Errorf("roots %v %v %v", remembered.SourceRoots(), directory.SourceRoots(), zipped.SourceRoots())
			}

			rebuilt := must(NewBundle(remembered.Identity(), remembered.Artifacts(), remembered.Assets(), remembered.Dependencies()))
			if rebuilt.ContentDigest() != c.digest || bundleModelOf(t, rebuilt) != model {
				t.Errorf("rebuilt digest %s", rebuilt.ContentDigest())
			}
		})
	}
}

func TestConfigurationBundlesDeriveSkillEntriesAndOrderByID(t *testing.T) {
	manifest := `{"schema_version":2,"id":"team","version":"1","artifacts":[` +
		`{"kind":"skill","id":"zeta","name":"review","path":"skills/review","executables":["run.py"]},` +
		`{"kind":"instruction","id":"alpha","text":"hi"}]}`
	bundle := must(NewBundleFromEntries([]byte(manifest), []BundleEntry{file("skills/review/SKILL.md", "m"), file("skills/review/run.py", "r")}))
	ids := make([]string, 0, 2)
	for _, artifact := range bundle.Artifacts() {
		ids = append(ids, artifact.ID())
	}
	if !cmp.Equal([]string{"alpha", "zeta"}, ids) {
		t.Errorf("artifact order = %v", ids)
	}
	wantEntries := []BundleEntry{dir("review"), file("review/SKILL.md", "m"), {Path: "review/run.py", Data: []byte("r"), Executable: true}}
	if diff := cmp.Diff(wantEntries, bundle.Entries()); diff != "" {
		t.Errorf("entries (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff([]SkillSpec{must(NewSkillSpec("review", "review", []string{"run.py"}))}, bundle.Skills(), cmp.AllowUnexported(SkillSpec{})); diff != "" {
		t.Errorf("skills (-want +got):\n%s", diff)
	}
}

func TestSchemaOneBundlesExposeOneSkillArtifactPerSkill(t *testing.T) {
	manifest := `{"schema_version":1,"id":"team","version":"1","skills":[{"name":"zeta","path":"zeta"},{"name":"alpha","path":"src/alpha","executables":["run.sh"]}]}`
	bundle := must(NewBundleFromEntries([]byte(manifest), []BundleEntry{
		file("zeta/SKILL.md", "z"), file("src/alpha/SKILL.md", "a"), file("src/alpha/run.sh", "r"),
	}))
	artifacts := bundle.Artifacts()
	if len(artifacts) != 2 || artifacts[0].ID() != "zeta" || artifacts[1].ID() != "alpha" {
		t.Fatalf("artifacts = %v", artifacts)
	}
	alpha := artifacts[1].(SkillArtifact)
	want := []BundleEntry{file("SKILL.md", "a"), {Path: "run.sh", Data: []byte("r"), Executable: true}}
	if diff := cmp.Diff(want, alpha.Content().Entries()); diff != "" {
		t.Errorf("alpha entries (-want +got):\n%s", diff)
	}
	if len(bundle.Assets()) != 0 || len(bundle.Dependencies()) != 0 {
		t.Errorf("assets %v dependencies %v", bundle.Assets(), bundle.Dependencies())
	}
}

func TestNewBundleValidatesAssetReferencesAgainstDependenciesAndSnapshots(t *testing.T) {
	identity := must(NewBundleIdentity("team", "1"))
	asset := must(NewSupportAsset("support", FamilyHooks, must(NewTreeContent([]BundleEntry{file("run.sh", "run"), dir("tools")}))))
	server := func(argument AssetRef, cwd CommandArgument) Artifact {
		return must(NewMCPArtifact("server", "server", must(NewCommand([]CommandArgument{argument}, nil, cwd))))
	}
	runScript := must(NewAssetRef("support", "run.sh"))
	dependency := must(NewDependency("server", "support", ""))
	bundle := must(NewBundle(identity, []Artifact{server(runScript, nil)}, []SupportAsset{asset}, []Dependency{dependency}))
	if bundle.SchemaVersion() != 2 || len(bundle.Assets()) != 1 {
		t.Errorf("bundle = %+v", bundle)
	}
	instruction := must(NewInstructionArtifact("server", "x"))
	cases := []struct {
		artifacts []Artifact
		assets    []SupportAsset
		deps      []Dependency
		message   string
	}{
		{[]Artifact{server(must(NewAssetRef("unknown", "")), nil)}, []SupportAsset{asset}, []Dependency{dependency}, "asset reference names an unknown support asset"},
		{[]Artifact{server(must(NewAssetRef("support", "missing.sh")), nil)}, []SupportAsset{asset}, []Dependency{dependency}, "asset reference path does not exist in snapshot"},
		{[]Artifact{server(runScript, nil)}, []SupportAsset{asset}, []Dependency{must(NewDependency("server", "support", DependencyStableReference))}, "asset reference requires a direct dependency"},
		{[]Artifact{server(runScript, runScript)}, []SupportAsset{asset}, []Dependency{dependency}, "asset cwd must reference a directory"},
		{[]Artifact{server(must(NewAssetRef("support", "tools")), nil)}, []SupportAsset{asset}, []Dependency{dependency}, "asset command must reference a file"},
		{[]Artifact{server(must(NewAssetRef("support", "")), nil)}, []SupportAsset{asset}, []Dependency{dependency}, "asset command must reference a file"},
		{nil, nil, nil, "a configuration bundle requires at least one artifact"},
		{[]Artifact{instruction, instruction}, nil, nil, "artifact and asset ids must be unique"},
		{[]Artifact{must(NewInstructionArtifact("support", "x"))}, []SupportAsset{asset}, []Dependency{}, "artifact and asset ids must be unique"},
		{[]Artifact{instruction}, []SupportAsset{asset}, nil, "support assets must have an explicit dependent"},
		{[]Artifact{instruction}, nil, []Dependency{dependency}, "dependency references an unknown artifact or asset"},
		{[]Artifact{nil}, nil, nil, "artifacts must be SkillArtifact, InstructionArtifact, MCPArtifact, HookArtifact or NativeArtifact values"},
		{[]Artifact{InstructionArtifact{}}, nil, nil, "artifact id must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
		{[]Artifact{instruction}, []SupportAsset{{}}, nil, "asset id must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
	}
	for index, tc := range cases {
		_, err := NewBundle(identity, tc.artifacts, tc.assets, tc.deps)
		if got := invalidMessage(t, err); got != tc.message {
			t.Errorf("case %d: %q, want %q", index, got, tc.message)
		}
	}
	skill := func(id string) Artifact {
		return must(NewSkillArtifact(id, "review", must(NewTreeContent([]BundleEntry{file("SKILL.md", "m")}))))
	}
	if _, err := NewBundle(identity, []Artifact{skill("a"), skill("b")}, nil, nil); invalidMessage(t, err) != "duplicate skill name" {
		t.Errorf("duplicate skill name: %v", err)
	}
	if _, err := NewBundle(BundleIdentity{}, []Artifact{instruction}, nil, nil); invalidMessage(t, err) != "bundle id must be 1-64 lowercase ASCII letters/digits with internal hyphens" {
		t.Errorf("zero identity: %v", err)
	}
}

func TestBundleArtifactAccessorsReturnCopies(t *testing.T) {
	bundle := must(NewBundleFromEntries([]byte(artifactsManifest(`[{"kind":"instruction","id":"guide","text":"Hello"}]`)), nil))
	artifacts := bundle.Artifacts()
	artifacts[0] = nil
	if bundle.Artifacts()[0] == nil {
		t.Error("Artifacts exposed the internal slice")
	}
	var empty *Bundle
	if empty.Artifacts() != nil || empty.Assets() != nil || empty.Dependencies() != nil {
		t.Error("nil bundle accessors returned values")
	}
}
