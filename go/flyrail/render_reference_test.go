package flyrail

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

const referenceRoot = "/fixture/project"

type translationFixture struct {
	Cases []struct {
		Name          string         `json:"name"`
		Agent         Agent          `json:"agent"`
		Surface       Surface        `json:"surface"`
		Scope         TargetScope    `json:"scope"`
		ExecutionRoot string         `json:"execution_root"`
		MCPPath       string         `json:"mcp_path"`
		Artifact      map[string]any `json:"artifact"`
	} `json:"cases"`
	AssetManifest map[string]any `json:"asset_manifest"`
	CombinedCases []struct {
		Name      string           `json:"name"`
		Artifacts []map[string]any `json:"artifacts"`
		Contexts  []struct {
			Agent Agent `json:"agent"`
		} `json:"contexts"`
	} `json:"combined_cases"`
}

func manifestBundle(t *testing.T, manifest map[string]any, entries ...BundleEntry) *Bundle {
	t.Helper()
	data, err := json.Marshal(manifest)
	if err != nil {
		t.Fatal(err)
	}
	return must(NewBundleFromEntries(data, entries))
}

func schemaTwoManifest(id string, artifacts any) map[string]any {
	return map[string]any{"schema_version": 2, "id": id, "version": "same", "artifacts": artifacts}
}

func referenceContext(t *testing.T, agent Agent, opts RenderOptions) RenderContext {
	t.Helper()
	return must(NewRenderContext(must(ProjectTarget(agent, referenceRoot)), PlatformLinux, opts))
}

func modelDigest(t *testing.T, value Value) string {
	t.Helper()
	sum := sha256.Sum256(encodedBytes(t, value))
	return hex.EncodeToString(sum[:])
}

func encodedBytes(t *testing.T, value Value) []byte {
	t.Helper()
	return must(EncodeValue(value))
}

func renderRecordLine(t *testing.T, name string, rendered *RenderedBundle) string {
	t.Helper()
	artifacts := make([]string, 0, len(rendered.artifacts))
	for _, artifact := range rendered.artifacts {
		artifacts = append(artifacts, modelDigest(t, artifact.modelValue()))
	}
	dependencies := make([]string, 0, len(rendered.dependencies))
	for _, dependency := range rendered.dependencies {
		dependencies = append(dependencies, modelDigest(t, dependency.modelValue()))
	}
	notices := make([]string, 0, len(rendered.notices))
	for _, notice := range rendered.notices {
		notices = append(notices, strings.Join([]string{
			hex.EncodeToString([]byte(notice.ArtifactID)), string(notice.Kind),
			hex.EncodeToString([]byte(notice.Code)), hex.EncodeToString([]byte(notice.Message)),
		}, ":"))
	}
	routing := rendered.RoutingContext()
	routes := make([]string, 0, len(routing))
	for _, key := range slices.Sorted(maps.Keys(routing)) {
		routes = append(routes, hex.EncodeToString([]byte(key))+":"+hex.EncodeToString([]byte(routing[key])))
	}
	return fmt.Sprintf("render-record\t%s\t%d\t%s\t%s\t%s\t%s\t%s", name, boolDigit(rendered.Supported()), rendered.ContentDigest(),
		strings.Join(artifacts, ","), strings.Join(dependencies, ","), strings.Join(notices, ","), strings.Join(routes, ","))
}

func boolDigit(b bool) int {
	if b {
		return 1
	}
	return 0
}

func capabilityDescriptor(t *testing.T, rc RenderContext) string {
	t.Helper()
	capabilities := must(Capabilities(rc))
	slices.SortFunc(capabilities, func(a, b Capability) int { return strings.Compare(string(a.Family), string(b.Family)) })
	parts := make([]string, len(capabilities))
	for index, capability := range capabilities {
		parts[index] = strings.Join([]string{
			string(capability.Family), fmt.Sprint(boolDigit(capability.Portable)), fmt.Sprint(boolDigit(capability.Native)),
			strings.Join(capability.Limitations, ","), strings.Join(capability.Prerequisites, ","),
		}, "|")
	}
	return strings.Join(parts, ";")
}

func translationReferenceLines(t *testing.T, document translationFixture) []string {
	t.Helper()
	var lines []string
	for _, item := range document.Cases {
		bundle := manifestBundle(t, schemaTwoManifest("translation-demo", []any{item.Artifact}))
		var target Target
		if item.Scope == ScopeProject {
			target = must(ProjectTarget(item.Agent, referenceRoot))
		} else {
			target = must(UserTarget(item.Agent, referenceRoot, nil))
		}
		opts := RenderOptions{Surface: item.Surface}
		if item.ExecutionRoot != "" {
			opts.ExecutionRoot = pathx.Join(referenceRoot, item.ExecutionRoot)
		}
		if item.MCPPath != "" {
			opts.MCPPath = pathx.Join(referenceRoot, item.MCPPath)
		}
		rc := must(NewRenderContext(target, PlatformLinux, opts))
		rendered := must(Render(bundle, rc))
		encoded := "-"
		for _, artifact := range rendered.artifacts {
			if artifact.id == item.Artifact["id"] {
				encoded = hex.EncodeToString(encodedBytes(t, artifact.modelValue()))
			}
		}
		notices := make([]string, len(rendered.notices))
		for index, notice := range rendered.notices {
			notices[index] = string(notice.Kind) + ":" + notice.Code
		}
		lines = append(lines,
			fmt.Sprintf("translation\t%s\t%d\t%s\t%s", item.Name, boolDigit(rendered.Supported()), encoded, strings.Join(notices, ",")),
			fmt.Sprintf("rendering\t%s\t%s", item.Name, rendered.ContentDigest()),
			fmt.Sprintf("capabilities\t%s\t%s", item.Name, capabilityDescriptor(t, rc)),
		)
	}
	return lines
}

func expandedReferenceLines(t *testing.T, document translationFixture) []string {
	t.Helper()
	assetBundle := manifestBundle(t, document.AssetManifest,
		BundleEntry{Path: "runtime/server.js", Data: []byte(`console.log("MCP");`)},
		BundleEntry{Path: "runtime/data", Dir: true},
	)
	lines := []string{renderRecordLine(t, "asset-manifest", must(Render(assetBundle,
		referenceContext(t, AgentCodex, RenderOptions{AssetRoot: pathx.Join(referenceRoot, "assets")}))))}

	for _, item := range document.CombinedCases {
		bundle := manifestBundle(t, schemaTwoManifest("combined", item.Artifacts))
		contexts := make([]RenderContext, len(item.Contexts))
		for index, value := range item.Contexts {
			contexts[index] = referenceContext(t, value.Agent, RenderOptions{})
		}
		lines = append(lines, renderRecordLine(t, "combined-"+item.Name, must(RenderMany(bundle, contexts...))))
	}

	hookBundle := manifestBundle(t, schemaTwoManifest("hook-differential", []any{map[string]any{
		"kind": "hook", "id": "start", "protocol_version": 1, "event": "session-start",
		"command":    map[string]any{"argv": []string{"/bin/echo", "space ü😀"}},
		"timeout_ms": 1000, "outcomes": []string{"continue"}, "tools": []string{},
	}}))
	runtime := must(NewHookRuntime("/opt/node/bin/node", ShellPOSIX))
	for _, agent := range TranslatableAgents() {
		rc := referenceContext(t, agent, RenderOptions{HookRuntime: runtime})
		lines = append(lines, renderRecordLine(t, "hook-"+string(agent), must(Render(hookBundle, rc))))
	}

	relocationBundle := manifestBundle(t, schemaTwoManifest("relocation", []any{
		map[string]any{"kind": "instruction", "id": "guide", "text": "Relocated."},
		map[string]any{"kind": "mcp", "id": "tools", "name": "tools", "transport": map[string]any{
			"kind": "stdio", "command": map[string]any{"argv": []string{"server"}},
		}},
	}))
	environment := map[string]string{"XDG_CONFIG_HOME": "~/configuration", "OPENCODE_CONFIG": "~/selected/opencode.jsonc"}
	relocated := must(NewRenderContext(must(UserTarget(AgentOpencode, referenceRoot, lookupMap(environment))), PlatformLinux, RenderOptions{}))
	lines = append(lines, renderRecordLine(t, "relocation-opencode", must(Render(relocationBundle, relocated))))

	dependencyBundle := manifestBundle(t, map[string]any{
		"schema_version": 2, "id": "audience-dependency", "version": "same",
		"artifacts": []any{
			map[string]any{"kind": "instruction", "id": "guide", "text": "Use it."},
			map[string]any{
				"kind": "native", "id": "native", "family": "mcp",
				"audience": map[string]any{"agent": "claude", "scope": "project"}, "destination": "native.json",
				"content": map[string]any{"kind": "structured", "format": "json", "selector": []string{"native"}, "value": map[string]any{"enabled": true}},
			},
		},
		"dependencies": []any{map[string]any{"dependent": "guide", "required": "native"}},
	})
	lines = append(lines, renderRecordLine(t, "dependency-audience", must(Render(dependencyBundle, referenceContext(t, AgentCodex, RenderOptions{})))))

	droidBundle := manifestBundle(t, schemaTwoManifest("mixed-droid", []any{
		map[string]any{
			"kind": "native", "id": "claude-only", "family": "instructions",
			"audience": map[string]any{"agent": "claude", "scope": "project"}, "destination": "CLAUDE.local.md",
			"content": map[string]any{"kind": "structured", "format": "json", "selector": []string{"claude"}, "value": map[string]any{"enabled": true}},
		},
		map[string]any{"kind": "instruction", "id": "guide", "text": "Portable"},
	}))
	mixed := must(RenderMany(droidBundle, referenceContext(t, AgentClaude, RenderOptions{}), referenceContext(t, AgentDroid, RenderOptions{})))
	return append(lines, renderRecordLine(t, "mixed-droid", mixed))
}

func lookupMap(environment map[string]string) func(string) (string, bool) {
	return func(key string) (string, bool) {
		value, ok := environment[key]
		return value, ok
	}
}

func TestRenderingMatchesTheReferenceRecords(t *testing.T) {
	var document translationFixture
	if err := json.Unmarshal(fixture(t, "translations.json"), &document); err != nil {
		t.Fatal(err)
	}
	golden, err := os.ReadFile(filepath.Join("testdata", "render_reference.txt"))
	if err != nil {
		t.Fatal(err)
	}
	var want []string
	for line := range strings.SplitSeq(strings.TrimSuffix(string(golden), "\n"), "\n") {
		if !strings.HasPrefix(line, "#") {
			want = append(want, line)
		}
	}
	got := append(translationReferenceLines(t, document), expandedReferenceLines(t, document)...)
	if diff := cmp.Diff(want, got); diff != "" {
		t.Errorf("reference records (-want +got):\n%s", diff)
	}
}
