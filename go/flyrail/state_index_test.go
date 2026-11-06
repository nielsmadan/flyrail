//go:build darwin || linux

package flyrail

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

func skillRendered(t *testing.T, container string) *RenderedBundle {
	t.Helper()
	return renderedOf(t, must(NewRenderedArtifact(RenderedArtifactParams{
		ID:          "skill",
		Family:      FamilySkills,
		Destination: container,
		Content:     must(NewTreeContent([]BundleEntry{{Path: "SKILL.md", Data: []byte("skill")}})),
		Subtree:     "review",
	})))
}

func skillsBundle(t *testing.T) *Bundle {
	t.Helper()
	content := must(NewTreeContent([]BundleEntry{{Path: "SKILL.md", Data: []byte("skill")}}))
	return must(NewBundle(must(NewBundleIdentity("team", "1")), []Artifact{must(NewSkillArtifact("skill", "review", content))}, nil, nil))
}

func instructionText(t *testing.T, text string) *Bundle {
	t.Helper()
	return must(NewBundle(must(NewBundleIdentity("team", "1")), []Artifact{must(NewInstructionArtifact("guide", text))}, nil, nil))
}

func TestATargetWithoutContextAcceptsAnyStoreAndItsDigestIgnoresTheStore(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	memory := store.NewMemoryStore()
	first := targetOf(t, memory)
	second := targetOf(t, openStore(t, filepath.Join(root, "state")))
	if first.Store() != store.Store(memory) {
		t.Error("the target does not keep its store")
	}
	if got := first.ContextDigest(); got != "37517e5f3dc66819f61f5a7bb8ace1921282415f10551d2defa5c3eb0985b570" {
		t.Errorf("empty context digest = %s", got)
	}
	if second.ContextDigest() != first.ContextDigest() {
		t.Error("the store changed the context digest")
	}
	located := must(NewInstallationTarget(memory, map[string]string{"destination": "/x"}, nil))
	if got := located.ContextDigest(); got != "5b134fc5c9711eb90652d0fecdbb9c2dc5c84098880e0ba529376e1a7a734c0c" {
		t.Errorf("located context digest = %s", got)
	}
	if names := directoryNames(t, root); len(names) != 0 {
		t.Errorf("constructing targets created %v", names)
	}
}

func TestOneStoreKeepsIndependentIndexesPerContext(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	for _, name := range []string{"one", "two"} {
		container := filepath.Join(root, name)
		target := must(NewInstallationTarget(st, map[string]string{"root": container}, nil))
		appliedSync(t, skillsBundle(t), skillRendered(t, container), target)
	}
	if keys := must(st.Keys(t.Context(), "index/team/")); len(keys) != 2 {
		t.Errorf("index keys = %v", keys)
	}
	if got := readText(t, filepath.Join(root, "two", "review", "SKILL.md")); got != "skill" {
		t.Errorf("installed skill = %q", got)
	}
}

func TestAStoreInsideAManagedDestinationIsRefused(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	container := filepath.Join(root, "skills")
	st := openStore(t, filepath.Join(container, "state"))
	_, err := Sync(t.Context(), skillsBundle(t), skillRendered(t, container), targetOf(t, st), SyncOptions{})
	if message := valueMessage(t, err); !strings.Contains(message, "overlap") {
		t.Errorf("message = %q", message)
	}
	if keys := must(st.Keys(t.Context(), "")); len(keys) != 0 {
		t.Errorf("store keys = %v", keys)
	}
	if exists(t, container) {
		t.Error("the container was created")
	}
}

func TestAnAliasedStoreLocationStillOverlapsADestination(t *testing.T) {
	t.Parallel()
	for _, spelling := range []string{"traversal", "symlink"} {
		root := tempRoot(t)
		mkdir(t, filepath.Join(root, "other"))
		if err := os.Symlink(root, filepath.Join(root, "link")); err != nil {
			t.Fatal(err)
		}
		parent := filepath.Join(root, "link")
		if spelling == "traversal" {
			parent = root + "/other/.."
		}
		st := openStore(t, parent+"/AGENTS.md/state")
		_, err := Preview(t.Context(), instructionText(t, "text"), sectionRendered(t, filepath.Join(root, "AGENTS.md"), "generated"), targetOf(t, st), PreviewOptions{})
		if message := valueMessage(t, err); !strings.Contains(message, "state store overlaps a resource") {
			t.Errorf("%s: message = %q", spelling, message)
		}
		if keys := must(st.Keys(t.Context(), "")); len(keys) != 0 {
			t.Errorf("%s: store keys = %v", spelling, keys)
		}
	}
}

func TestAStoreBehindADanglingSymlinkIntoASkillContainerOverlapsIt(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	container := filepath.Join(root, "skills")
	if err := os.Symlink(container, filepath.Join(root, "link")); err != nil {
		t.Fatal(err)
	}
	st := openStore(t, filepath.Join(root, "link", "state"))
	_, err := Preview(t.Context(), skillsBundle(t), skillRendered(t, container), targetOf(t, st), PreviewOptions{})
	if message := valueMessage(t, err); message != "state store overlaps a resource or authority" {
		t.Errorf("message = %q", message)
	}
	if keys := must(st.Keys(t.Context(), "")); len(keys) != 0 {
		t.Errorf("store keys = %v", keys)
	}
	if exists(t, container) {
		t.Error("the container was created")
	}
}

func TestAStagingDirectoryThatOverlapsADestinationIsRefused(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	staged := &pathStore{MemoryStore: store.NewMemoryStore(), staging: filepath.Join(root, "state.staging")}
	_, err := Preview(t.Context(), instructionText(t, "text"), sectionRendered(t, filepath.Join(root, "state.staging", "AGENTS.md"), "generated"), targetOf(t, staged), PreviewOptions{})
	if message := valueMessage(t, err); !strings.Contains(message, "state store overlaps a resource") {
		t.Errorf("message = %q", message)
	}
	if names := directoryNames(t, root); len(names) != 0 {
		t.Errorf("root holds %v", names)
	}
}

func TestPreviewApprovesParentsCreatedForTheStore(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	target := targetOf(t, openStore(t, filepath.Join(root, "project", "state")))
	destination := filepath.Join(root, "project", "AGENTS.md")
	plan := previewOf(t, instructionText(t, "text"), sectionRendered(t, destination, "generated"), target)
	wantStatus(t, "apply", ApplyPreview(t.Context(), plan, LockOptions{LockTimeout: testLockTimeout}), StatusApplied)
	if written := readText(t, destination); !strings.Contains(written, "generated") {
		t.Errorf("destination = %q", written)
	}
}

func TestRenderContextsSharingAStoreKeepSeparateInstallations(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, "state"))
	project := filepath.Join(root, "project")
	shared := instructionText(t, "Shared guidance.\n")
	var contexts []RenderContext
	var installations []*InstallationTarget
	for _, agent := range []Agent{AgentClaude, AgentCodex} {
		rc := must(NewRenderContext(must(ProjectTarget(agent, project)), PlatformLinux, RenderOptions{}))
		contexts = append(contexts, rc)
		installations = append(installations, must(rc.Installation(st, nil)))
	}
	want := map[string]string{"agent": "claude", "root": project, "scope": "project", "surface": "cli"}
	if diff := cmp.Diff(want, installations[0].Context()); diff != "" {
		t.Errorf("default context (-want +got):\n%s", diff)
	}
	explicit := must(contexts[0].Installation(st, map[string]string{"app": "notes"}))
	if diff := cmp.Diff(map[string]string{"app": "notes"}, explicit.Context()); diff != "" {
		t.Errorf("explicit context (-want +got):\n%s", diff)
	}
	for index, rc := range contexts {
		appliedSync(t, shared, must(Render(shared, rc)), installations[index])
	}
	for _, name := range []string{"CLAUDE.md", "AGENTS.md"} {
		if text := readText(t, filepath.Join(project, name)); !strings.Contains(text, "Shared guidance.") {
			t.Errorf("%s = %q", name, text)
		}
	}
	for _, installation := range installations {
		if !inspectTeam(t, installation).Current {
			t.Error("an installation is not current")
		}
	}
}

func TestRemovalFromAContextWithoutAHookRuntimeRemovesTheContent(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, "state"))
	shared := instructionText(t, "Shared.\n")
	installed := must(NewRenderContext(must(ProjectTarget(AgentCodex, root)), PlatformMacOS, RenderOptions{
		HookRuntime: must(NewHookRuntime("", ShellPOSIX)),
		AssetRoot:   filepath.Join(root, "assets"),
	}))
	removing := must(NewRenderContext(must(ProjectTarget(AgentCodex, root)), PlatformLinux, RenderOptions{}))
	appliedSync(t, shared, must(Render(shared, installed)), must(installed.Installation(st, nil)))
	wantStatus(t, "remove", must(Remove(t.Context(), "team", must(removing.Installation(st, nil)), RemoveOptions{LockTimeout: testLockTimeout})), StatusApplied)
	if diff := cmp.Diff([]string{"state"}, directoryNames(t, root)); diff != "" {
		t.Errorf("root (-want +got):\n%s", diff)
	}
}
