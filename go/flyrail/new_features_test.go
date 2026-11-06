//go:build darwin || linux

package flyrail

import (
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
	"github.com/google/go-cmp/cmp/cmpopts"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

func guidanceBundle(t *testing.T) *Bundle {
	t.Helper()
	return must(NewBundle(must(NewBundleIdentity("guidance", "one")), []Artifact{must(NewInstructionArtifact("guide", "Be careful.\n"))}, nil, nil))
}

func TestDroidRefusesSelectedArtifactsAndBlocksMixedRendering(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	bundle := guidanceBundle(t)
	supported := must(NewRenderContext(must(ProjectTarget(AgentClaude, root)), PlatformLinux, RenderOptions{}))
	droid := must(NewRenderContext(must(ProjectTarget(AgentDroid, root)), PlatformLinux, RenderOptions{}))

	refused := must(Render(bundle, droid))
	if len(refused.Artifacts()) != 0 {
		t.Errorf("droid artifacts = %v", refused.Artifacts())
	}
	notices := refused.Notices()
	if len(notices) != 1 || notices[0].ArtifactID != "guide" || notices[0].Code != "agent-unsupported" {
		t.Fatalf("notices = %+v", notices)
	}
	proposal := previewOf(t, bundle, refused, must(droid.Installation(openStore(t, filepath.Join(root, ".droid-state")), nil)))
	if proposal.Applicable() {
		t.Error("an unsupported droid preview is applicable")
	}
	if diff := cmp.Diff([]string{}, directoryNames(t, root)); diff != "" {
		t.Errorf("preview touched the target (-want +got):\n%s", diff)
	}

	if artifacts := must(RenderMany(bundle, supported)).Artifacts(); len(artifacts) != 1 {
		t.Errorf("supported artifacts = %d", len(artifacts))
	}
	mixed := must(RenderMany(bundle, supported, droid))
	if len(mixed.Artifacts()) != 0 || len(mixed.Dependencies()) != 0 {
		t.Errorf("mixed rendering kept %v and %v", mixed.Artifacts(), mixed.Dependencies())
	}
	if !slices.ContainsFunc(mixed.Notices(), func(notice Notice) bool { return notice.Code == "agent-unsupported" }) {
		t.Errorf("mixed notices = %+v", mixed.Notices())
	}
}

func TestDroidRefusalCannotBeBypassedThroughPublicTranslationHelpers(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	context := must(NewRenderContext(must(ProjectTarget(AgentDroid, root)), PlatformLinux, RenderOptions{
		InstructionPath: filepath.Join(root, "instructions.md"),
		MCPPath:         filepath.Join(root, "mcp.json"),
	}))
	instruction := must(NewInstructionArtifact("guide", "Be careful.\n"))
	mcp := must(NewMCPArtifact("tools", "tools", command(t, ArgText("server"))))
	translated := []ArtifactTranslation{
		must(TranslateInstructionArtifact("guidance", instruction, context)),
		must(TranslateMCPArtifact(mcp, context, nil)),
	}
	for _, item := range translated {
		if _, ok := item.Artifact(); ok {
			t.Error("droid translation produced an artifact")
		}
		notices := item.Notices()
		if len(notices) != 1 || notices[0].Kind != NoticeUnsupported || notices[0].Code != "agent-unsupported" {
			t.Errorf("notices = %+v", notices)
		}
	}
}

func TestDroidRefusalCoversHooksAndOnlySelectedNativeArtifacts(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	droid := must(NewRenderContext(must(ProjectTarget(AgentDroid, root)), PlatformLinux, RenderOptions{}))
	native := func(id string, agent Agent, text string) Artifact {
		return must(NewNativeArtifact(id, FamilyInstructions, must(NewAudience(agent, ScopeProject, SurfaceCLI, "")), "notes.md",
			must(NewFileContent([]byte(text), ModePrivate))))
	}
	bundle := must(NewBundle(must(NewBundleIdentity("mixed", "one")), []Artifact{
		must(NewHookArtifact(HookArtifactParams{
			ID:            "guard",
			Event:         EventSessionStart,
			Command:       command(t, ArgText("check")),
			TimeoutMillis: 1_000,
			Outcomes:      []HookOutcome{OutcomeContinue},
		})),
		native("claude-only", AgentClaude, "Claude.\n"),
		native("droid-native", AgentDroid, "Droid.\n"),
	}, nil, nil))
	rendered := must(Render(bundle, droid))
	if len(rendered.Artifacts()) != 0 {
		t.Errorf("droid artifacts = %v", rendered.Artifacts())
	}
	var ids []string
	for _, notice := range rendered.Notices() {
		ids = append(ids, notice.ArtifactID)
		if notice.Code != "agent-unsupported" {
			t.Errorf("notice %+v is not agent-unsupported", notice)
		}
	}
	if diff := cmp.Diff([]string{"guard", "droid-native"}, ids); diff != "" {
		t.Errorf("notice artifacts (-want +got):\n%s", diff)
	}
}

func newFeaturesSkill(t *testing.T) *Bundle {
	t.Helper()
	content := must(NewTreeContent([]BundleEntry{{Path: "SKILL.md", Data: []byte("---\nname: review\n---\nCheck it.\n")}}))
	return must(NewBundle(must(NewBundleIdentity("skills", "one")), []Artifact{must(NewSkillArtifact("review", "review", content))}, nil, nil))
}

func guidance(t *testing.T, version string, artifacts ...[2]string) *Bundle {
	t.Helper()
	items := make([]Artifact, 0, len(artifacts))
	for _, artifact := range artifacts {
		items = append(items, must(NewInstructionArtifact(artifact[0], artifact[1])))
	}
	return must(NewBundle(must(NewBundleIdentity("guidance", version)), items, nil, nil))
}

func TestDroidSkillLifecycleRefusesBeforeTouchingTheTarget(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, ".state"))
	bundle := newFeaturesSkill(t)
	target := must(ProjectTarget(AgentDroid, root))

	rendered := must(RenderSkills(t.Context(), bundle, target))
	if len(rendered.Artifacts()) != 0 || rendered.Notices()[0].Code != "agent-unsupported" || len(rendered.RoutingContext()) != 0 {
		t.Errorf("rendered = %v %+v %v", rendered.Artifacts(), rendered.Notices(), rendered.RoutingContext())
	}
	targets := []Target{target}
	options := SkillOptions{LockTimeout: testLockTimeout}
	operations := map[string]func() ([]TargetResult, error){
		"install": func() ([]TargetResult, error) {
			return Install(t.Context(), bundle, targets, st, LockOptions{LockTimeout: testLockTimeout})
		},
		"update":    func() ([]TargetResult, error) { return Update(t.Context(), bundle, targets, st, options) },
		"uninstall": func() ([]TargetResult, error) { return Uninstall(t.Context(), "skills", targets, st, options) },
	}
	for label, operation := range operations {
		result := must(operation())[0]
		if result.Status != StatusFailed {
			t.Errorf("%s: status = %s", label, result.Status)
		}
		wantCode(t, label, result.Err, CodeUnsupported)
		if diff := cmp.Diff([]string{}, directoryNames(t, root)); diff != "" {
			t.Errorf("%s touched the target (-want +got):\n%s", label, diff)
		}
	}
	inspected := must(Inspect(t.Context(), bundle, targets, st))[0]
	if inspected.Observation.State != StateUnknown {
		t.Errorf("inspected state = %s", inspected.Observation.State)
	}
	wantCode(t, "inspect", inspected.Observation.Err, CodeUnsupported)
	if diff := cmp.Diff([]string{}, directoryNames(t, root)); diff != "" {
		t.Errorf("inspect touched the target (-want +got):\n%s", diff)
	}
}

func TestSummariesReportCreateUnchangedUpdateDeleteAndSharedDestinations(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	first := guidance(t, "one", [2]string{"alpha", "Alpha.\n"}, [2]string{"beta", "Beta.\n"})
	rc := must(NewRenderContext(must(ProjectTarget(AgentClaude, root)), PlatformLinux, RenderOptions{}))
	installation := must(rc.Installation(openStore(t, filepath.Join(root, ".state")), nil))
	rendered := must(Render(first, rc))
	summary := must(Summarize(previewOf(t, first, rendered, installation)))
	if summary.Agent != AgentClaude || summary.Scope != ScopeProject || summary.Surface != SurfaceCLI || !summary.Applicable || summary.Err != nil {
		t.Errorf("summary = %+v", summary)
	}
	if diff := cmp.Diff(rendered.Notices(), summary.Notices); diff != "" {
		t.Errorf("notices (-want +got):\n%s", diff)
	}
	want := []PlannedChange{{
		ArtifactIDs: []string{"alpha", "beta"},
		Families:    []Family{FamilyInstructions, FamilyInstructions},
		Kind:        ResourceDocument,
		Destination: filepath.Join(root, "CLAUDE.md"),
		Action:      ActionCreate,
	}}
	if diff := cmp.Diff(want, summary.Changes, cmpopts.EquateEmpty()); diff != "" {
		t.Errorf("changes (-want +got):\n%s", diff)
	}

	appliedSync(t, first, rendered, installation)
	if action := must(Summarize(previewOf(t, first, rendered, installation))).Changes[0].Action; action != ActionUnchanged {
		t.Errorf("unchanged action = %s", action)
	}
	second := guidance(t, "two", [2]string{"alpha", "Changed.\n"}, [2]string{"beta", "Beta.\n"})
	changed := must(Render(second, rc))
	if action := must(Summarize(previewOf(t, second, changed, installation))).Changes[0].Action; action != ActionUpdate {
		t.Errorf("update action = %s", action)
	}
	removal := must(PreviewRemoval(t.Context(), first.ID(), installation, false))
	if action := must(Summarize(removal)).Changes[0].Action; action != ActionDelete {
		t.Errorf("removal action = %s", action)
	}
	writeFile(t, filepath.Join(root, "CLAUDE.md"), "Local change.\n")
	conflict := must(Summarize(previewOf(t, second, changed, installation)))
	if conflict.Applicable || conflict.Changes[0].Action != ActionConflict {
		t.Errorf("conflict summary = %+v", conflict)
	}
}

func TestSummariesReportRecoveryPathsAsConflicts(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	bundle := guidanceBundle(t)
	rc := must(NewRenderContext(must(ProjectTarget(AgentClaude, root)), PlatformLinux, RenderOptions{}))
	installation := must(rc.Installation(openStore(t, filepath.Join(root, ".state")), nil))
	rendered := must(Render(bundle, rc))
	appliedSync(t, bundle, rendered, installation)
	resource := previewOf(t, bundle, rendered, installation).Resources()[0].Resource()
	transaction := strings.Repeat("f", 32)
	recovery := must(TransactionPath(resource.Destination, transaction))
	prepared := must(newPreparation(resource, transaction, recovery, stateSchemaVersion))
	if !must(installation.Store().Write(t.Context(), resourceKey(resource.Destination, "preparation"), encodeRecord(prepared), store.Absent())) {
		t.Fatal("the preparation record was not written")
	}

	summary := must(Summarize(previewOf(t, bundle, rendered, installation)))
	if summary.Applicable || summary.Err != nil {
		t.Errorf("summary = %+v", summary)
	}
	change := summary.Changes[0]
	if change.Action != ActionConflict || change.Err != nil {
		t.Errorf("change = %+v", change)
	}
	if diff := cmp.Diff([]string{recovery}, change.RecoveryPaths); diff != "" {
		t.Errorf("recovery paths (-want +got):\n%s", diff)
	}
}

func TestSummarizeRejectsAMissingPreview(t *testing.T) {
	t.Parallel()
	if message := valueMessage(t, errorOf(Summarize(nil))); message != "summarize requires a LifecyclePreview" {
		t.Errorf("message = %q", message)
	}
	for _, action := range []ChangeAction{ActionCreate, ActionUpdate, ActionDelete, ActionUnchanged, ActionConflict} {
		if parsed := must(ParseChangeAction(string(action))); parsed != action {
			t.Errorf("parsed %s as %s", action, parsed)
		}
	}
	if message := valueMessage(t, errorOf(ParseChangeAction("move"))); message != "'move' is not a valid ChangeAction" {
		t.Errorf("message = %q", message)
	}
}

func TestEmptyExplicitDiscoveryInputsDoNotSelectInstalledAgents(t *testing.T) {
	t.Parallel()
	found := must(DetectAgents(t.Context(), "", DiscoveryOptions{SearchPath: []string{}, NoApplications: true}))
	if len(found) != 0 {
		t.Errorf("found = %+v", found)
	}
}
