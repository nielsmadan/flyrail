//go:build darwin || linux

package flyrail

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"testing"
	"unicode/utf8"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

func reviewSkill(t *testing.T, identifier, version, data string) *Bundle {
	t.Helper()
	content := must(NewTreeContent([]BundleEntry{
		{Path: "SKILL.md", Data: []byte(data)},
		{Path: "empty", Dir: true},
		{Path: "run", Data: []byte("run"), Executable: true},
	}))
	return must(NewBundle(must(NewBundleIdentity(identifier, version)), []Artifact{must(NewSkillArtifact("skill", "review", content))}, nil, nil))
}

func directoryTargets(t *testing.T, roots ...string) []Target {
	t.Helper()
	targets := make([]Target, 0, len(roots))
	for _, root := range roots {
		targets = append(targets, must(DirectoryTarget(root)))
	}
	return targets
}

var skillLock = LockOptions{LockTimeout: testLockTimeout}

func installSkills(t *testing.T, b *Bundle, st store.Store, roots ...string) []TargetResult {
	t.Helper()
	return must(Install(t.Context(), b, directoryTargets(t, roots...), st, skillLock))
}

func updateSkills(t *testing.T, b *Bundle, st store.Store, replaceModified bool, roots ...string) []TargetResult {
	t.Helper()
	return must(Update(t.Context(), b, directoryTargets(t, roots...), st, SkillOptions{ReplaceModified: replaceModified, LockTimeout: testLockTimeout}))
}

func uninstallSkills(t *testing.T, bundleID string, st store.Store, roots ...string) []TargetResult {
	t.Helper()
	return must(Uninstall(t.Context(), bundleID, directoryTargets(t, roots...), st, SkillOptions{LockTimeout: testLockTimeout}))
}

func inspectSkills(t *testing.T, b *Bundle, st store.Store, roots ...string) []TargetInspection {
	t.Helper()
	return must(Inspect(t.Context(), b, directoryTargets(t, roots...), st))
}

func wantTargetStatus(t *testing.T, label string, result TargetResult, want OperationStatus) {
	t.Helper()
	if result.Status != want {
		t.Fatalf("%s: status = %s, want %s (error %v)", label, result.Status, want, result.Err)
	}
}

func TestSkillLifecycleTracksInventoryAliasesUpdatesAndSourceFreeRemoval(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, "state"))
	target := filepath.Join(root, "skills")
	first := reviewSkill(t, "team", "1", "original")

	if state := inspectSkills(t, first, st, target)[0].Observation.State; state != StateAbsent {
		t.Fatalf("initial state = %s", state)
	}
	installed := installSkills(t, first, st, target, target)
	wantTargetStatus(t, "install", installed[0], StatusApplied)
	wantTargetStatus(t, "alias", installed[1], StatusApplied)
	if !installed[1].IsAlias || installed[1].AliasOf != 0 {
		t.Errorf("alias = %v %d", installed[1].IsAlias, installed[1].AliasOf)
	}
	if !installed[0].Succeeded() || installed[0].Failure() != nil {
		t.Errorf("applied result: succeeded %v, failure %v", installed[0].Succeeded(), installed[0].Failure())
	}
	if _, ok, err := st.Read(t.Context(), resourceKey(target, "authority")); err != nil || !ok {
		t.Errorf("authority record: %v %v", ok, err)
	}
	if keys := must(st.Keys(t.Context(), "index/team/")); len(keys) != 1 {
		t.Errorf("index keys = %v", keys)
	}
	if got := readText(t, filepath.Join(target, "review", "SKILL.md")); got != "original" {
		t.Errorf("installed skill = %q", got)
	}

	observed := inspectSkills(t, first, st, target)[0].Observation
	if !observed.IsCurrent() {
		t.Fatalf("installed observation is not current: %+v", observed)
	}
	if observed.Installed == nil {
		t.Fatal("no installation recorded")
	}
	entries := observed.Installed.Entries
	if !slices.ContainsFunc(entries, func(entry InventoryEntry) bool { return entry.Dir }) {
		t.Errorf("no directory in inventory %+v", entries)
	}
	script := slices.IndexFunc(entries, func(entry InventoryEntry) bool { return entry.Path == "review/run" })
	if script < 0 {
		t.Fatalf("no script in inventory %+v", entries)
	}
	want := InventoryEntry{Path: "review/run", Size: 3, SHA256: "acba25512100f80b56fc3ccd14c65be55d94800cda77585c5f41a887e398f9be", Executable: true}
	if diff := cmp.Diff(want, entries[script]); diff != "" {
		t.Errorf("script inventory (-want +got):\n%s", diff)
	}

	second := reviewSkill(t, "team", "2", "changed")
	refused := installSkills(t, second, st, target)[0]
	wantTargetStatus(t, "install over another version", refused, StatusFailed)
	wantCode(t, "install over another version", refused.Err, CodeUpdateRequired)
	if failure, ok := errors.AsType[*TargetError](refused.Failure()); !ok || failure.Code != CodeUpdateRequired {
		t.Errorf("Failure() = %v", refused.Failure())
	}
	wantTargetStatus(t, "update", updateSkills(t, second, st, false, target)[0], StatusApplied)
	writeFile(t, filepath.Join(target, "review", "SKILL.md"), "user")
	wantTargetStatus(t, "safe removal", uninstallSkills(t, "team", st, target)[0], StatusFailed)
	wantTargetStatus(t, "replace modified", updateSkills(t, second, st, true, target)[0], StatusApplied)
	wantTargetStatus(t, "source-free removal", uninstallSkills(t, "team", st, target)[0], StatusApplied)
	if exists(t, filepath.Join(target, "review")) {
		t.Error("the skill survived uninstall")
	}
}

func TestFilesystemBundleInspectionSurvivesSourceRemoval(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, "state"))
	source := filepath.Join(root, "source")
	mkdir(t, filepath.Join(source, "review", "empty"))
	writeFile(t, filepath.Join(source, "review", "SKILL.md"), "snapshot")
	writeFile(t, filepath.Join(source, "review", "run"), "run")
	writeFile(t, filepath.Join(source, "flyrail.json"), `{
		"schema_version": 1,
		"id": "team",
		"version": "1",
		"skills": [{"name": "review", "path": "review", "executables": ["run"]}]
	}`)
	bundle := must(OpenBundle(t.Context(), source))
	target := filepath.Join(root, "skills")
	wantTargetStatus(t, "install", installSkills(t, bundle, st, target)[0], StatusApplied)
	if err := os.RemoveAll(source); err != nil {
		t.Fatal(err)
	}
	if observed := inspectSkills(t, bundle, st, target)[0].Observation; !observed.IsCurrent() {
		t.Errorf("source-free inspection is not current: %+v", observed)
	}
	wantTargetStatus(t, "uninstall", uninstallSkills(t, "team", st, target)[0], StatusApplied)
}

func TestIndependentTargetFailureDoesNotBlockHealthyTarget(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, "state"))
	failed := filepath.Join(root, "failed")
	healthy := filepath.Join(root, "healthy")
	if err := os.Symlink(filepath.Join(root, "missing"), failed); err != nil {
		t.Fatal(err)
	}
	results := installSkills(t, reviewSkill(t, "team", "1", "content"), st, failed, healthy, healthy)
	if len(results) != 3 {
		t.Fatalf("results = %d", len(results))
	}
	wantTargetStatus(t, "unsafe target", results[0], StatusFailed)
	wantCode(t, "unsafe target", results[0].Err, CodeUnsafePath)
	if results[0].Succeeded() || !errors.Is(results[0].Failure(), ErrUnsafePath) {
		t.Errorf("unsafe result: succeeded %v, failure %v", results[0].Succeeded(), results[0].Failure())
	}
	if results[0].Observation.State != StateUnknown {
		t.Errorf("unsafe observation = %s", results[0].Observation.State)
	}
	wantTargetStatus(t, "healthy target", results[1], StatusApplied)
	if !results[2].IsAlias || results[2].AliasOf != 1 {
		t.Errorf("alias = %v %d", results[2].IsAlias, results[2].AliasOf)
	}
	if got := readText(t, filepath.Join(healthy, "review", "SKILL.md")); got != "content" {
		t.Errorf("healthy skill = %q", got)
	}
	if link := must(os.Readlink(failed)); link != filepath.Join(root, "missing") {
		t.Errorf("link = %s", link)
	}
}

func TestSkillConflictsAreReportedWithoutInspectionWrites(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, "state"))
	target := filepath.Join(root, "skills")
	writeFile(t, filepath.Join(target, "review", "SKILL.md"), "foreign")

	observation := inspectSkills(t, reviewSkill(t, "team", "1", "desired"), st, target)[0].Observation
	if observation.State != StateAbsent {
		t.Errorf("state = %s (error %v)", observation.State, observation.Err)
	}
	if diff := cmp.Diff([]Conflict{{Path: target}}, observation.Conflicts); diff != "" {
		t.Errorf("conflicts (-want +got):\n%s", diff)
	}
	if got := readText(t, filepath.Join(target, "review", "SKILL.md")); got != "foreign" {
		t.Errorf("foreign skill = %q", got)
	}
	if exists(t, filepath.Join(root, "state")) {
		t.Error("inspection created the state store")
	}
}

func TestCorruptReceiptsProduceUnknownObservationsAndBlockInstallation(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, "state"))
	target := filepath.Join(root, "skills")
	desired := reviewSkill(t, "team", "1", "content")
	wantTargetStatus(t, "install", installSkills(t, desired, st, target)[0], StatusApplied)
	key := resourceKey(target, "receipt")
	recorded, ok, err := st.Read(t.Context(), key)
	if err != nil || !ok {
		t.Fatalf("receipt: %v %v", ok, err)
	}
	if !must(st.Write(t.Context(), key, []byte("{}"), store.Equal(recorded))) {
		t.Fatal("the receipt was not replaced")
	}

	observed := inspectSkills(t, desired, st, target)[0].Observation
	if observed.State != StateUnknown {
		t.Errorf("state = %s", observed.State)
	}
	wantCode(t, "corrupt receipt", observed.Err, CodeInvalidState)
	wantTargetStatus(t, "blocked install", installSkills(t, desired, st, target)[0], StatusFailed)
}

func TestRenderSkillsRejectsNonSkillBundlesBeforeWrites(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	_, err := RenderSkills(t.Context(), instructionText(t, "text"), must(DirectoryTarget(filepath.Join(root, "skills"))))
	failure, ok := errors.AsType[*AuthorityError](err)
	if !ok || failure.Error() != "use explicit rendering for non-skill artifacts" {
		t.Fatalf("error = %T %v", err, err)
	}
	if message := valueMessage(t, err); message != "use explicit rendering for non-skill artifacts" {
		t.Errorf("value error = %q", message)
	}
	if names := directoryNames(t, root); len(names) != 0 {
		t.Errorf("root holds %v", names)
	}
}

func TestSkillOperationsRejectInvalidArgumentsBeforeIO(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, "state"))
	skill := reviewSkill(t, "team", "1", "content")
	targets := directoryTargets(t, filepath.Join(root, "skills"))
	withAsset := must(NewBundle(must(NewBundleIdentity("team", "1")), skill.Artifacts(),
		[]SupportAsset{must(NewSupportAsset("helper", FamilySkills, must(NewTreeContent([]BundleEntry{{Path: "x", Data: []byte("x")}}))))},
		[]Dependency{must(NewDependency("skill", "helper", DependencyRevision))}))
	for _, test := range []struct {
		label string
		err   error
		want  string
	}{
		{"no targets", errorOf(Install(t.Context(), skill, nil, st, skillLock)), "at least one target is required"},
		{"non-skill bundle", errorOf(Update(t.Context(), instructionText(t, "text"), targets, st, SkillOptions{})), "use explicit rendering for non-skill artifacts"},
		{"assets", errorOf(Inspect(t.Context(), withAsset, targets, st)), "use explicit rendering for skill bundles with assets or dependencies"},
		{"bundle id", errorOf(Uninstall(t.Context(), "Not Valid", targets, st, SkillOptions{})), validateBundleID("Not Valid").Error()},
		{"zero target", errorOf(Install(t.Context(), skill, []Target{{}}, st, skillLock)), "targets must be Target values"},
		{"nil store", errorOf(Install(t.Context(), skill, targets, nil, skillLock)), "store must be a StateStore"},
		{"zero directory store install", errorOf(Install(t.Context(), skill, targets, &store.DirectoryStore{}, skillLock)), "directory state store must come from NewDirectoryStore"},
		{"zero directory store uninstall", errorOf(Uninstall(t.Context(), "team", targets, &store.DirectoryStore{}, SkillOptions{})), "directory state store must come from NewDirectoryStore"},
		{"zero directory store inspect", errorOf(Inspect(t.Context(), skill, targets, &store.DirectoryStore{})), "directory state store must come from NewDirectoryStore"},
		{"negative timeout", errorOf(Uninstall(t.Context(), "team", targets, st, SkillOptions{LockTimeout: -1})), "lock timeout must be finite and nonnegative"},
	} {
		if message := valueMessage(t, test.err); message != test.want {
			t.Errorf("%s: message = %q, want %q", test.label, message, test.want)
		}
	}
	if names := directoryNames(t, root); len(names) != 0 {
		t.Errorf("root holds %v", names)
	}
}

func TestSchemaOneSkillsInstallThroughAnAlias(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, "state"))
	manifest := []byte(`{
		"schema_version": 1,
		"id": "team",
		"version": "1",
		"skills": [{"name": "review", "path": "review", "executables": ["run"]}]
	}`)
	bundle := must(NewBundleFromEntries(manifest, []BundleEntry{
		{Path: "review/SKILL.md", Data: []byte("schema one")},
		{Path: "review/run", Data: []byte("run"), Executable: true},
	}))
	target := filepath.Join(root, "skills")
	rendered := must(RenderSkills(t.Context(), bundle, must(DirectoryTarget(target))))
	artifacts := rendered.Artifacts()
	if len(artifacts) != 1 || artifacts[0].ID() != "review" || artifacts[0].Subtree() != "review" || artifacts[0].Destination() != target {
		t.Fatalf("rendered artifacts = %+v", artifacts)
	}

	results := installSkills(t, bundle, st, target, target)
	wantTargetStatus(t, "install", results[0], StatusApplied)
	if !results[1].IsAlias || results[1].AliasOf != 0 || results[1].Status != StatusApplied {
		t.Errorf("alias result = %+v", results[1])
	}
	if got := readText(t, filepath.Join(target, "review", "SKILL.md")); got != "schema one" {
		t.Errorf("installed skill = %q", got)
	}
	info := must(os.Stat(filepath.Join(target, "review", "run")))
	if info.Mode().Perm()&0o100 == 0 {
		t.Errorf("run mode = %v", info.Mode())
	}
}

func TestOneStoreKeepsIndependentSkillIndexesPerTarget(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	bundle := skillsBundle(t)
	first := installSkills(t, bundle, st, filepath.Join(root, "one"))
	second := installSkills(t, bundle, st, filepath.Join(root, "two"))
	wantTargetStatus(t, "first", first[0], StatusApplied)
	wantTargetStatus(t, "second", second[0], StatusApplied)
	if keys := must(st.Keys(t.Context(), "index/team/")); len(keys) != 2 {
		t.Errorf("index keys = %v", keys)
	}
	if got := readText(t, filepath.Join(root, "two", "review", "SKILL.md")); got != "skill" {
		t.Errorf("installed skill = %q", got)
	}
}

func TestAStoreInsideASkillTargetIsAFailedResult(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	container := filepath.Join(root, "skills")
	st := openStore(t, filepath.Join(container, "state"))
	results := installSkills(t, skillsBundle(t), st, container)
	wantTargetStatus(t, "install", results[0], StatusFailed)
	wantCode(t, "install", results[0].Err, CodeInvalidState)
	if results[0].Err.Message != "state store overlaps a resource or authority" {
		t.Errorf("message = %q", results[0].Err.Message)
	}
	if keys := must(st.Keys(t.Context(), "")); len(keys) != 0 {
		t.Errorf("store keys = %v", keys)
	}
	if exists(t, container) {
		t.Error("the container was created")
	}
}

func TestInstallRefusesTheSameVersionWithDifferentContent(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	target := filepath.Join(root, "skills")
	wantTargetStatus(t, "first install", installSkills(t, reviewSkill(t, "team", "1", "original"), st, target)[0], StatusApplied)
	refused := installSkills(t, reviewSkill(t, "team", "1", "rebuilt"), st, target)[0]
	wantTargetStatus(t, "same version, new content", refused, StatusFailed)
	wantCode(t, "same version, new content", refused.Err, CodeUpdateRequired)
	if got := readText(t, filepath.Join(target, "review", "SKILL.md")); got != "original" {
		t.Errorf("installed skill = %q", got)
	}
}

func TestUpdateInstallsAnAbsentSkill(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	target := filepath.Join(root, "skills")
	wantTargetStatus(t, "update on a fresh target", updateSkills(t, reviewSkill(t, "team", "1", "fresh"), st, false, target)[0], StatusApplied)
	if got := readText(t, filepath.Join(target, "review", "SKILL.md")); got != "fresh" {
		t.Errorf("installed skill = %q", got)
	}
}

func TestInspectMarksDuplicateTargetsAsAliases(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	bundle := reviewSkill(t, "team", "1", "skill")
	target := filepath.Join(root, "skills")
	wantTargetStatus(t, "install", installSkills(t, bundle, st, target)[0], StatusApplied)
	inspected := inspectSkills(t, bundle, st, target, filepath.Join(root, "other"), target)
	if len(inspected) != 3 {
		t.Fatalf("inspections = %d", len(inspected))
	}
	if inspected[0].IsAlias || inspected[1].IsAlias {
		t.Errorf("originals marked as aliases: %+v %+v", inspected[0].IsAlias, inspected[1].IsAlias)
	}
	if !inspected[2].IsAlias || inspected[2].AliasOf != 0 {
		t.Errorf("alias = %v of %d", inspected[2].IsAlias, inspected[2].AliasOf)
	}
	if !inspected[2].Observation.IsCurrent() {
		t.Errorf("alias observation = %+v", inspected[2].Observation)
	}
	if inspected[2].Observation.ContentMatches == inspected[0].Observation.ContentMatches {
		t.Error("alias shares its content flag with the original")
	}
}

func TestAliasResultsDoNotShareMemoryWithTheirOriginal(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := store.NewMemoryStore()
	bundle := reviewSkill(t, "team", "1", "skill")
	target := filepath.Join(root, "skills")
	results := installSkills(t, bundle, st, target, target)
	if results[0].Observation.ContentMatches == nil || results[1].Observation.ContentMatches == nil {
		t.Fatalf("content flags: %v %v", results[0].Observation.ContentMatches, results[1].Observation.ContentMatches)
	}
	*results[1].Observation.ContentMatches = !*results[1].Observation.ContentMatches
	if *results[0].Observation.ContentMatches == *results[1].Observation.ContentMatches {
		t.Error("mutating the alias changed the original")
	}

	original := TargetResult{RecoveryPaths: []string{"a"}}
	alias := aliasResult(original, preparedSkill{aliasOf: 0})
	alias.RecoveryPaths[0] = "b"
	if original.RecoveryPaths[0] != "a" {
		t.Errorf("recovery paths shared: %v", original.RecoveryPaths)
	}
}

func TestNonUTF8StatePathIsRefusedBeforeAnyWrite(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	st := openStore(t, filepath.Join(root, "state-\xfd"))
	targets := directoryTargets(t, filepath.Join(root, "skills"))
	want := fmt.Sprintf("'utf-8' codec can't encode character '\\udcfd' in position %d: surrogates not allowed",
		utf8.RuneCountInString(`{"fields":{"directory":{"path":"`+root+"/state-"))
	installed := must(Install(t.Context(), reviewSkill(t, "team", "1", "content"), targets, st, skillLock))
	removed := must(Uninstall(t.Context(), "team", targets, st, SkillOptions{}))
	for _, result := range append(installed, removed...) {
		if result.Status != StatusFailed || result.Err == nil || result.Err.Code != CodeInvalidState || result.Err.Message != want {
			t.Errorf("result %s %+v, want failed invalid_state %q", result.Status, result.Err, want)
		}
	}
	if names := directoryNames(t, root); len(names) != 0 {
		t.Errorf("root holds %q", names)
	}
}
