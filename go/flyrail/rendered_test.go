package flyrail

import (
	"encoding/hex"
	"encoding/json"
	"testing"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

func sectionArtifact(t *testing.T, id, destination string) RenderedArtifact {
	t.Helper()
	return must(NewRenderedArtifact(RenderedArtifactParams{
		ID: id, Family: FamilyInstructions, Destination: destination, Content: must(NewSectionContent(id, "Hello", nil)),
	}))
}

func fileArtifact(t *testing.T, id, destination string) RenderedArtifact {
	t.Helper()
	return must(NewRenderedArtifact(RenderedArtifactParams{
		ID: id, Family: FamilyHooks, Destination: destination, Content: must(NewFileContent([]byte("run\n"), ModePrivate)),
	}))
}

func TestRenderedInstructionDigestMatchesTheSharedFixture(t *testing.T) {
	artifact := sectionArtifact(t, "guide", "/fixture/project/AGENTS.md")
	rendered := must(NewRenderedBundle([]RenderedArtifact{artifact}, nil, nil, nil))
	if got := rendered.ContentDigest(); got != "249a091ac4a021ebbd33997ba8e3b49d34765516a1d2fdfab2f28f195329838b" {
		t.Errorf("digest = %s", got)
	}
	want := `["object",[["fields",["object",[["dependent",["string","guide"]],["mode",["string","revision"]],["required",["string","helper"]]]]],["type",["string","Dependency"]]]]`
	if got := encoded(t, must(NewDependency("guide", "helper", "")).modelValue()); got != want {
		t.Errorf("dependency model = %s", got)
	}
}

func renderedArtifactFromRecord(t *testing.T, raw map[string]any) RenderedArtifact {
	t.Helper()
	record := raw["content"].(map[string]any)
	var content Content
	switch record["kind"] {
	case "section":
		content = must(NewSectionContent(record["marker"].(string), record["text"].(string), nil))
	case "file":
		data := must(hex.DecodeString(record["data_hex"].(string)))
		content = must(NewFileContent(data, ModePrivate))
	case "structured":
		var parts []SelectorPart
		for _, part := range record["selector"].([]any) {
			if name, ok := part.(string); ok {
				parts = append(parts, must(NewKey(name)))
				continue
			}
			member := part.(map[string]any)
			var key []string
			for _, name := range member["key"].([]any) {
				key = append(key, name.(string))
			}
			parts = append(parts, must(NewMember(jsonFixtureValue(t, member["identity"]), key)))
		}
		content = must(NewStructuredContent(DocumentFormat(record["format"].(string)), must(NewSelector(parts...)),
			jsonFixtureValue(t, record["value"]), nil))
	}
	var schemas []DocumentSchema
	if requirements, ok := raw["schema_requirements"].([]any); ok {
		for _, item := range requirements {
			requirement := item.(map[string]any)
			value := jsonFixtureValue(t, requirement["value"]).(Scalar)
			schemas = append(schemas, must(NewDocumentSchema(DocumentFormat(requirement["format"].(string)), requirement["key"].(string), value)))
		}
	}
	return must(NewRenderedArtifact(RenderedArtifactParams{
		ID:                 raw["id"].(string),
		Family:             Family(raw["family"].(string)),
		Destination:        raw["destination"].(string),
		Content:            content,
		SchemaRequirements: schemas,
	}))
}

func jsonFixtureValue(t *testing.T, raw any) Value {
	t.Helper()
	return must(valueFromJSON(must(jsonx.Parse(must(json.Marshal(raw))))))
}

func TestSharedRenderingDigests(t *testing.T) {
	var document struct {
		Renderings []struct {
			Name          string           `json:"name"`
			Artifacts     []map[string]any `json:"artifacts"`
			Dependencies  []map[string]any `json:"dependencies"`
			ContentDigest string           `json:"content_digest"`
		} `json:"renderings"`
	}
	if err := json.Unmarshal(fixture(t, "configurations.json"), &document); err != nil {
		t.Fatal(err)
	}
	if len(document.Renderings) != 3 {
		t.Fatalf("fixture has %d renderings", len(document.Renderings))
	}
	for _, c := range document.Renderings {
		artifacts := make([]RenderedArtifact, len(c.Artifacts))
		for index, raw := range c.Artifacts {
			artifacts[index] = renderedArtifactFromRecord(t, raw)
		}
		deps := make([]Dependency, len(c.Dependencies))
		for index, raw := range c.Dependencies {
			mode, _ := raw["mode"].(string)
			deps[index] = must(NewDependency(raw["dependent"].(string), raw["required"].(string), DependencyMode(mode)))
		}
		rendered, err := NewRenderedBundle(artifacts, deps, nil, nil)
		if err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		if got := rendered.ContentDigest(); got != c.ContentDigest {
			t.Errorf("%s: digest %s, want %s", c.Name, got, c.ContentDigest)
		}
	}
}

func TestRenderedArtifactsValidateLikeTheReference(t *testing.T) {
	section := must(NewSectionContent("guide", "Hello", nil))
	structured := must(NewStructuredContent(FormatJSON, must(NewSelector(must(NewKey("hooks")))), Int(1), nil))
	schema := must(NewDocumentSchema(FormatJSON, "version", Int(1)))
	jsoncSchema := must(NewDocumentSchema(FormatJSONC, "version", Int(1)))
	cases := []struct {
		name    string
		params  RenderedArtifactParams
		message string
	}{
		{"bad id", RenderedArtifactParams{ID: "Guide", Family: FamilyInstructions, Destination: "/a/b", Content: section},
			"rendered artifact id must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
		{"bad family", RenderedArtifactParams{ID: "guide", Family: "rules", Destination: "/a/b", Content: section}, "'rules' is not a valid Family"},
		{"relative", RenderedArtifactParams{ID: "guide", Family: FamilyInstructions, Destination: "a/b", Content: section},
			"rendered destination must be an absolute Path"},
		{"root", RenderedArtifactParams{ID: "guide", Family: FamilyInstructions, Destination: "/", Content: section},
			"rendered destination must be an absolute Path"},
		{"parent", RenderedArtifactParams{ID: "guide", Family: FamilyInstructions, Destination: "/a/../b", Content: section},
			"rendered destination must be an absolute Path"},
		{"duplicate schema", RenderedArtifactParams{ID: "guide", Family: FamilyHooks, Destination: "/a/b", Content: structured,
			SchemaRequirements: []DocumentSchema{schema, schema}}, "duplicate document schema requirement"},
		{"schema format", RenderedArtifactParams{ID: "guide", Family: FamilyHooks, Destination: "/a/b", Content: structured,
			SchemaRequirements: []DocumentSchema{jsoncSchema}}, "document schema requirements need matching structured content"},
		{"schema content", RenderedArtifactParams{ID: "guide", Family: FamilyHooks, Destination: "/a/b", Content: section,
			SchemaRequirements: []DocumentSchema{schema}}, "document schema requirements need matching structured content"},
		{"subtree content", RenderedArtifactParams{ID: "guide", Family: FamilySkills, Destination: "/a/b", Content: section, Subtree: "guide"},
			"subtree claims require tree content"},
		{"subtree path", RenderedArtifactParams{ID: "guide", Family: FamilySkills, Destination: "/a/b", Content: TreeContent{}, Subtree: "../guide"},
			"rendered subtree must be an unambiguous relative path"},
	}
	for _, c := range cases {
		_, err := NewRenderedArtifact(c.params)
		if got := valueErrorText(t, err); got != c.message {
			t.Errorf("%s: %q, want %q", c.name, got, c.message)
		}
	}
}

func TestRenderedBundlesValidateIdsDependenciesNoticesAndRoutes(t *testing.T) {
	guide := sectionArtifact(t, "guide", "/fixture/AGENTS.md")
	launcher := fileArtifact(t, "launcher", "/fixture/launcher")
	stable := must(NewDependency("guide", "launcher", DependencyStableReference))
	cases := []struct {
		name      string
		artifacts []RenderedArtifact
		deps      []Dependency
		notices   []Notice
		routing   map[string]string
		message   string
	}{
		{"duplicate ids", []RenderedArtifact{guide, guide}, nil, nil, nil, "rendered artifact ids must be unique"},
		{"zero artifact", []RenderedArtifact{{}}, nil, nil, nil, "rendered artifacts must contain RenderedArtifact values"},
		{"unknown dependency", []RenderedArtifact{guide}, []Dependency{must(NewDependency("guide", "missing", ""))}, nil, nil,
			"dependency references an unknown artifact or asset"},
		{"stable on section", []RenderedArtifact{guide, launcher}, []Dependency{must(NewDependency("launcher", "guide", DependencyStableReference))}, nil, nil,
			"stable references require a whole-file routing entry"},
		{"stable chain", []RenderedArtifact{guide, launcher, sectionArtifact(t, "other", "/fixture/OTHER.md")},
			[]Dependency{stable, must(NewDependency("launcher", "other", DependencyStableReference))}, nil, nil,
			"stable routing entries require revision-bound dependencies"},
		{"blank notice", []RenderedArtifact{guide}, nil, []Notice{{ArtifactID: "guide", Kind: NoticeActivation, Message: " "}}, nil,
			"notice message must be nonblank"},
		{"notice kind", []RenderedArtifact{guide}, nil, []Notice{{ArtifactID: "guide", Kind: "info", Message: "x"}}, nil,
			"'info' is not a valid NoticeKind"},
		{"notice code", []RenderedArtifact{guide}, nil, []Notice{{ArtifactID: "guide", Kind: NoticeActivation, Message: "x", Code: "Bad"}}, nil,
			"notice code must be 1-64 lowercase ASCII letters/digits with internal hyphens"},
		{"routing", []RenderedArtifact{guide}, nil, nil, map[string]string{"key": "\xff"}, "render routing context must contain string pairs"},
	}
	for _, c := range cases {
		_, err := NewRenderedBundle(c.artifacts, c.deps, c.notices, c.routing)
		if got := valueErrorText(t, err); got != c.message {
			t.Errorf("%s: %q, want %q", c.name, got, c.message)
		}
	}

	rendered := must(NewRenderedBundle([]RenderedArtifact{launcher, guide}, []Dependency{stable},
		[]Notice{{ArtifactID: "guide", Kind: NoticeActivation, Message: "Reload."}}, map[string]string{"zeta": "1", "alpha": "2"}))
	if got := []string{rendered.Artifacts()[0].ID(), rendered.Artifacts()[1].ID()}; !cmp.Equal(got, []string{"guide", "launcher"}) {
		t.Errorf("artifact order = %v", got)
	}
	if !rendered.Supported() {
		t.Error("an activation notice blocked the bundle")
	}
	rendered.Artifacts()[0] = launcher
	if rendered.Artifacts()[0].ID() != "guide" {
		t.Error("Artifacts exposes internal storage")
	}
	if diff := cmp.Diff(map[string]string{"alpha": "2", "zeta": "1"}, rendered.RoutingContext()); diff != "" {
		t.Errorf("routing (-want +got):\n%s", diff)
	}
	empty := EmptyRenderedBundle()
	if empty.ContentDigest() != must(NewRenderedBundle(nil, nil, nil, nil)).ContentDigest() || !empty.Supported() {
		t.Errorf("empty bundle digest %s", empty.ContentDigest())
	}
}
