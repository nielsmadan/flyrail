package flyrail

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

type discoveryVector struct {
	Name            string
	Files           map[string]string
	Env             map[string]string
	Status          LoadoutStatus
	Code            string
	Profile         string
	Chain           []string
	Sources         []string
	SourcePaths     []string `json:"source_paths"`
	Harnesses       []Agent
	InstructionPath string   `json:"instruction_path"`
	InstructionKey  []string `json:"instruction_key"`
}

func loadoutFixtureFile(t *testing.T, root, relative, data string) {
	t.Helper()
	path := filepath.Join(root, relative)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(strings.ReplaceAll(data, "{root}", root)), 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestLoadoutDiscoveryVectors(t *testing.T) {
	var values struct{ Discovery []discoveryVector }
	if err := json.Unmarshal(fixture(t, "loadout.json"), &values); err != nil {
		t.Fatal(err)
	}
	for _, example := range values.Discovery {
		t.Run(example.Name, func(t *testing.T) {
			root, err := filepath.EvalSymlinks(t.TempDir())
			if err != nil {
				t.Fatal(err)
			}
			for name, text := range example.Files {
				loadoutFixtureFile(t, root, name, text)
			}
			for key, value := range example.Env {
				example.Env[key] = strings.ReplaceAll(value, "{root}", root)
			}
			found, err := DiscoverLoadout(filepath.Join(root, "home"), "", example.Env)
			if err != nil || found.Status != example.Status {
				t.Fatalf("discover got status %s, diagnostic %v, error %v", found.Status, found.Diagnostic, err)
			}
			if example.Status == LoadoutInvalid {
				if found.Diagnostic == nil || found.Diagnostic.Code != example.Code {
					t.Fatalf("expected %s, got %+v", example.Code, found.Diagnostic)
				}
			}
			if example.Status != LoadoutAvailable {
				return
			}
			setup := found.Setup
			chain := setup.ProfileChain()
			for i, path := range chain {
				chain[i], err = filepath.Rel(root, path)
				if err != nil {
					t.Fatal(err)
				}
			}
			var sources []string
			for _, source := range setup.Sources() {
				sources = append(sources, source.Name())
			}
			if example.SourcePaths != nil {
				paths := make([]string, 0, len(sources))
				for _, item := range setup.Sources() {
					path, err := filepath.Rel(root, item.Path())
					if err != nil {
						t.Fatal(err)
					}
					paths = append(paths, filepath.ToSlash(path))
				}
				if !slices.Equal(paths, example.SourcePaths) {
					t.Fatalf("source paths: %v", paths)
				}
			}
			var agents []Agent
			for _, h := range setup.Harnesses() {
				agents = append(agents, h.Agent())
			}
			if setup.Profile() != example.Profile || !slices.Equal(chain, example.Chain) ||
				!slices.Equal(sources, example.Sources) || !slices.Equal(agents, example.Harnesses) {
				t.Fatalf("wrong setup profile=%q chain=%v sources=%v agents=%v", setup.Profile(), chain, sources, agents)
			}
			if example.InstructionPath != "" {
				selectors := setup.Selectors()
				if len(selectors) != 1 || selectors[0].Manifest() != filepath.Join(root, example.InstructionPath) ||
					!slices.Equal(selectors[0].Key(), example.InstructionKey) {
					t.Fatalf("wrong selector: %+v", selectors)
				}
			}
		})
	}
}

func TestLoadoutSharedMCPPreviews(t *testing.T) {
	var vectors struct {
		Previews []struct {
			Name         string
			Files        map[string]string
			Harnesses    []Agent
			Code         *string
			EnvReference bool `json:"env_reference"`
		} `json:"mcp_previews"`
	}
	if err := json.Unmarshal(fixture(t, "loadout.json"), &vectors); err != nil {
		t.Fatal(err)
	}
	for _, vector := range vectors.Previews {
		t.Run(vector.Name, func(t *testing.T) {
			root, err := filepath.EvalSymlinks(t.TempDir())
			if err != nil {
				t.Fatal(err)
			}
			for path, data := range vector.Files {
				loadoutFixtureFile(t, root, path, data)
			}
			found, err := DiscoverLoadout(filepath.Join(root, "home"), "", nil)
			if err != nil || found.Setup == nil {
				t.Fatalf("discover: %+v %v", found, err)
			}
			selected, err := found.Setup.Select("local", vector.Harnesses, false)
			if err != nil {
				t.Fatal(err)
			}
			state, err := store.NewDirectoryStore(t.Context(), filepath.Join(root, "state"))
			if err != nil {
				t.Fatal(err)
			}
			var environment map[string]EnvironmentValue
			if vector.EnvReference {
				token, err := NewEnvRef("REVIEW_TOKEN")
				if err != nil {
					t.Fatal(err)
				}
				environment = map[string]EnvironmentValue{"TOKEN": token}
			}
			command, err := NewCommand([]CommandArgument{ArgText("review")}, environment, nil)
			if err != nil {
				t.Fatal(err)
			}
			mcp, err := NewMCPArtifact("server", "review", command)
			if err != nil {
				t.Fatal(err)
			}
			identity, err := NewBundleIdentity("toolbox", "1")
			if err != nil {
				t.Fatal(err)
			}
			bundle, err := NewBundle(identity, []Artifact{mcp}, nil, nil)
			if err != nil {
				t.Fatal(err)
			}
			plan, err := PreviewLoadout(t.Context(), bundle, selected, state, LoadoutOptions{Platform: PlatformMacOS})
			if err != nil {
				t.Fatal(err)
			}
			if plan.Applicable() != (vector.Code == nil) {
				t.Fatalf("preview: %+v %v", plan.Diagnostic(), plan.Lifecycle().Err())
			}
			if vector.Code != nil && (plan.Diagnostic() == nil || plan.Diagnostic().Code != *vector.Code) {
				t.Fatalf("want %q got %+v", *vector.Code, plan.Diagnostic())
			}
		})
	}
}

func loadoutTestBundle(t *testing.T, version, guide string) *Bundle {
	t.Helper()
	bundleID, err := NewBundleIdentity("loadout-probe", version)
	if err != nil {
		t.Fatal(err)
	}
	instruction, err := NewInstructionArtifact("guide", guide)
	if err != nil {
		t.Fatal(err)
	}
	token, err := NewEnvRef("REVIEW_TOKEN")
	if err != nil {
		t.Fatal(err)
	}
	transport, err := NewHTTPTransport("https://example.test/mcp", &token)
	if err != nil {
		t.Fatal(err)
	}
	mcp, err := NewMCPArtifact("server", "review-http", transport)
	if err != nil {
		t.Fatal(err)
	}
	tree, err := NewTreeContent([]BundleEntry{
		{Path: "SKILL.md", Data: []byte("---\nname: review\ndescription: Review\n---\n")},
		{Path: "assets/input.bin", Data: []byte{0, 255}},
	})
	if err != nil {
		t.Fatal(err)
	}
	skill, err := NewSkillArtifact("skill", "review", tree)
	if err != nil {
		t.Fatal(err)
	}
	bundle, err := NewBundle(bundleID, []Artifact{instruction, mcp, skill}, nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	return bundle
}

func TestLoadoutSourceLifecycle(t *testing.T) {
	ctx := t.Context()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	home := filepath.Join(root, "home")
	source := filepath.Join(root, "source")
	const registration = "home/.config/loadout/config.toml"
	const manifest = "# keep me\n[[source]]\nname='local'\npath='.'\n[claude]\ninstructions=[]\n[codex]\ninstructions=[]\n"
	const valid = "source='{root}/source'\n"
	loadoutFixtureFile(t, root, registration, valid)
	loadoutFixtureFile(t, root, "source/loadout.toml", manifest)
	state, err := store.NewDirectoryStore(ctx, filepath.Join(root, "state"))
	if err != nil {
		t.Fatal(err)
	}
	options := LoadoutOptions{Platform: PlatformMacOS}
	plan := func(bundle *Bundle, agents []Agent, accept bool) *LoadoutPreview {
		t.Helper()
		found, err := DiscoverLoadout(home, "", nil)
		if err != nil || found.Setup == nil {
			t.Fatalf("discover: %+v %v", found, err)
		}
		selected, err := found.Setup.Select("local", agents, accept)
		if err != nil {
			t.Fatal(err)
		}
		preview, err := PreviewLoadout(ctx, bundle, selected, state, options)
		if err != nil {
			t.Fatal(err)
		}
		return preview
	}
	bundle := loadoutTestBundle(t, "1", "Read the diff.")
	denied := plan(bundle, []Agent{AgentClaude}, false)
	if issue := denied.Diagnostic(); issue == nil || issue.Code != "shared-scope" {
		t.Fatalf("expected shared scope: %+v", issue)
	}
	agents := []Agent{AgentClaude, AgentCodex}
	stale := plan(bundle, agents, false)
	loadoutFixtureFile(t, root, registration, valid+"# changed\n")
	if result := ApplyLoadout(ctx, stale, LockOptions{}); result.Lifecycle.Err == nil || result.Lifecycle.Err.Code != CodeConcurrentChange {
		t.Fatalf("stale result: %+v", result)
	}
	first := plan(bundle, agents, false)
	if result := ApplyLoadout(ctx, first, LockOptions{}); !result.SyncRequired() || result.Lifecycle.Status != StatusApplied {
		t.Fatalf("install result: %+v", result.Lifecycle)
	}
	raw, err := os.ReadFile(filepath.Join(source, "skills", "review", "assets", "input.bin"))
	if err != nil || string(raw) != "\x00\xff" {
		t.Fatalf("skill asset: %v %v", raw, err)
	}
	metadata, err := os.ReadFile(filepath.Join(source, "mcp.toml"))
	if err != nil || !strings.Contains(string(metadata), `auth_env_var = "REVIEW_TOKEN"`) {
		t.Fatalf("invalid stored definition: %s %v", metadata, err)
	}
	if repeated := ApplyLoadout(ctx, plan(bundle, agents, false), LockOptions{}); repeated.Lifecycle.Status != StatusUnchanged {
		t.Fatalf("repeat: %+v", repeated.Lifecycle)
	}
	upgrade := plan(loadoutTestBundle(t, "2", "Review the patch."), agents, false)
	if result := ApplyLoadout(ctx, upgrade, LockOptions{}); result.Lifecycle.Status != StatusApplied {
		t.Fatalf("upgrade: %+v", result.Lifecycle)
	}
	loadoutFixtureFile(t, root, registration, "source='~/missing'\n")
	observed, err := InspectLoadout(ctx, bundle.ID(), first.Location(), state)
	if err != nil || !observed.Current {
		t.Fatalf("source-free inspection: %+v %v", observed, err)
	}
	retirement, err := PreviewLoadoutRemoval(ctx, bundle.ID(), first.Location(), state, false)
	if err != nil {
		t.Fatal(err)
	}
	if result := ApplyLoadout(ctx, retirement, LockOptions{}); !result.SyncRequired() || result.Lifecycle.Status != StatusApplied {
		t.Fatalf("remove: %+v", result.Lifecycle)
	}
	current, err := os.ReadFile(filepath.Join(source, "loadout.toml"))
	if err != nil || string(current) != manifest {
		t.Fatalf("restored manifest: %s %v", current, err)
	}
}
