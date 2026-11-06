//go:build darwin || linux

package flyrail

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/seams"
)

func instructionsBundle(t *testing.T, identifier, version string, artifactIDs ...string) *Bundle {
	t.Helper()
	artifacts := make([]Artifact, 0, len(artifactIDs))
	for _, artifactID := range artifactIDs {
		artifacts = append(artifacts, must(NewInstructionArtifact(artifactID, "text")))
	}
	return must(NewBundle(must(NewBundleIdentity(identifier, version)), artifacts, nil, nil))
}

func teamBundle(t *testing.T, version string) *Bundle {
	t.Helper()
	return instructionsBundle(t, "team", version, "guide")
}

func sectionAt(t *testing.T, artifactID, path, text string) RenderedArtifact {
	t.Helper()
	return must(NewRenderedArtifact(RenderedArtifactParams{
		ID:          artifactID,
		Family:      FamilyInstructions,
		Destination: path,
		Content:     must(NewSectionContent(artifactID, text, nil)),
	}))
}

func fileAt(t *testing.T, artifactID, path, data string) RenderedArtifact {
	t.Helper()
	return must(NewRenderedArtifact(RenderedArtifactParams{
		ID:          artifactID,
		Family:      FamilyInstructions,
		Destination: path,
		Content:     must(NewFileContent([]byte(data), ModeReadable)),
	}))
}

func renderedOf(t *testing.T, artifacts ...RenderedArtifact) *RenderedBundle {
	t.Helper()
	return must(NewRenderedBundle(artifacts, nil, nil, nil))
}

func sectionRendered(t *testing.T, path, text string) *RenderedBundle {
	t.Helper()
	return renderedOf(t, sectionAt(t, "guide", path, text))
}

func openStore(t *testing.T, path string) store.Store {
	t.Helper()
	return must(store.NewDirectoryStore(t.Context(), path))
}

func targetOf(t *testing.T, st store.Store) *InstallationTarget {
	t.Helper()
	return must(NewInstallationTarget(st, nil, nil))
}

func stateTarget(t *testing.T, root string) *InstallationTarget {
	t.Helper()
	return targetOf(t, openStore(t, filepath.Join(root, "state")))
}

const testLockTimeout = 5 * time.Second

func syncWith(ctx context.Context, t *testing.T, b *Bundle, r *RenderedBundle, target *InstallationTarget, opts SyncOptions) InstallationResult {
	t.Helper()
	return must(Sync(ctx, b, r, target, opts))
}

func syncOnce(t *testing.T, b *Bundle, r *RenderedBundle, target *InstallationTarget) InstallationResult {
	t.Helper()
	return syncWith(t.Context(), t, b, r, target, SyncOptions{LockTimeout: testLockTimeout})
}

func wantStatus(t *testing.T, label string, result InstallationResult, want OperationStatus) {
	t.Helper()
	if result.Status != want {
		t.Fatalf("%s: status = %s, want %s (error %v)", label, result.Status, want, result.Err)
	}
}

func appliedSync(t *testing.T, b *Bundle, r *RenderedBundle, target *InstallationTarget) {
	t.Helper()
	wantStatus(t, "sync", syncOnce(t, b, r, target), StatusApplied)
}

func wantCode(t *testing.T, label string, failed *TargetError, want ErrorCode) {
	t.Helper()
	if failed == nil || failed.Code != want {
		t.Fatalf("%s: error = %+v, want code %s", label, failed, want)
	}
}

func removeAll(t *testing.T, target *InstallationTarget, replaceModified bool) InstallationResult {
	t.Helper()
	return must(Remove(t.Context(), "team", target, RemoveOptions{ReplaceModified: replaceModified, LockTimeout: testLockTimeout}))
}

func recoverAll(t *testing.T, target *InstallationTarget) InstallationResult {
	t.Helper()
	return must(RecoverInstallation(t.Context(), "team", target, LockOptions{LockTimeout: testLockTimeout}))
}

func inspectTeam(t *testing.T, target *InstallationTarget) InstallationObservation {
	t.Helper()
	return must(InspectInstallation(t.Context(), "team", target))
}

func previewOf(t *testing.T, b *Bundle, r *RenderedBundle, target *InstallationTarget) *LifecyclePreview {
	t.Helper()
	return must(Preview(t.Context(), b, r, target, PreviewOptions{}))
}

func putIndex(t *testing.T, target *InstallationTarget, data []byte) {
	t.Helper()
	key := installationKey("team", target)
	current, ok, err := target.Store().Read(t.Context(), key)
	if err != nil {
		t.Fatal(err)
	}
	expected := store.Absent()
	if ok {
		expected = store.Equal(current)
	}
	if written, err := target.Store().Write(t.Context(), key, data, expected); err != nil || !written {
		t.Fatalf("write index: %v %v", written, err)
	}
}

func replaceRecord(t *testing.T, st store.Store, key string, data []byte) []byte {
	t.Helper()
	original, ok, err := st.Read(t.Context(), key)
	if err != nil || !ok {
		t.Fatalf("read %s: %v %v", key, ok, err)
	}
	if written, err := st.Write(t.Context(), key, data, store.Equal(original)); err != nil || !written {
		t.Fatalf("replace %s: %v %v", key, written, err)
	}
	return original
}

func versionText(version string) string {
	if version == "1" {
		return "one"
	}
	return "two"
}

func runVersion(ctx context.Context, t *testing.T, target *InstallationTarget, destination, version string) OperationStatus {
	t.Helper()
	return syncWith(ctx, t, teamBundle(t, version), sectionRendered(t, destination, versionText(version)), target, SyncOptions{LockTimeout: testLockTimeout}).Status
}

func filesystemSnapshot(t *testing.T, root string) map[string]string {
	t.Helper()
	snapshot := map[string]string{}
	err := filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			snapshot[path] = "<dir>"
			return nil
		}
		data, err := os.ReadFile(path)
		snapshot[path] = string(data)
		return err
	})
	if err != nil {
		t.Fatal(err)
	}
	return snapshot
}

func TestACrashAfterCreatingTheTransactionDirectoryIsCleanedUpByTheNextRun(t *testing.T) {
	t.Parallel()
	for _, directoryBacked := range []bool{true, false} {
		root := tempRoot(t)
		target := targetOf(t, store.NewMemoryStore())
		if directoryBacked {
			target = stateTarget(t, root)
		}
		destination := filepath.Join(root, "AGENTS.md")
		writeFile(t, destination, "# Foreign\n")
		if status := runVersion(t.Context(), t, target, destination, "1"); status != StatusApplied {
			t.Fatalf("install = %s", status)
		}
		interrupted(t, func() {
			runVersion(seams.With(t.Context(), crashWhen(atBoundary(seams.Directory))), t, target, destination, "2")
		})
		reference := documentRef(t, destination)
		prepared, ok, failed := readRecord(t.Context(), target.Store(), preparationKey(reference), "Preparation", preparationFromState)
		mustSucceed(t, failed)
		if !ok {
			t.Fatal("no preparation record")
		}
		if hasRecord(t, target.Store(), journalKey(reference)) {
			t.Fatal("a journal was recorded")
		}
		if diff := cmp.Diff([]string{"backup", "staging"}, directoryNames(t, prepared.directory)); diff != "" {
			t.Errorf("transaction directory (-want +got):\n%s", diff)
		}
		observed := inspectTeam(t, target)
		if diff := cmp.Diff([]string{prepared.directory}, observed.Resources[0].RecoveryPaths); diff != "" {
			t.Errorf("recovery paths (-want +got):\n%s", diff)
		}
		if status := runVersion(t.Context(), t, target, destination, "2"); status != StatusApplied {
			t.Fatalf("rerun = %s", status)
		}
		if hasRecord(t, target.Store(), preparationKey(reference)) || exists(t, prepared.directory) {
			t.Error("the interrupted transaction was not cleaned up")
		}
		want := []string{"AGENTS.md"}
		if directoryBacked {
			want = append(want, "state")
		}
		if diff := cmp.Diff(want, directoryNames(t, root)); diff != "" {
			t.Errorf("root (-want +got):\n%s", diff)
		}
	}
}

func TestASameFilesystemStoreLeavesNothingBesideTheDestinationAtFilesystemCrashPoints(t *testing.T) {
	t.Parallel()
	for _, boundary := range []string{seams.Directory, seams.Backup, seams.Publish} {
		root := tempRoot(t)
		project := filepath.Join(root, "project")
		mkdir(t, project)
		destination := filepath.Join(project, "AGENTS.md")
		writeFile(t, destination, "# Foreign\n")
		target := stateTarget(t, root)
		if status := runVersion(t.Context(), t, target, destination, "1"); status != StatusApplied {
			t.Fatalf("install = %s", status)
		}
		interrupted(t, func() {
			runVersion(seams.With(t.Context(), crashWhen(atBoundary(boundary))), t, target, destination, "2")
		})
		if names := directoryNames(t, project); slices.ContainsFunc(names, func(name string) bool { return name != "AGENTS.md" }) {
			t.Errorf("%s: project holds %v", boundary, names)
		}
		if status := runVersion(t.Context(), t, target, destination, "2"); status != StatusApplied {
			t.Fatalf("%s: rerun = %s", boundary, status)
		}
		if diff := cmp.Diff([]string{"AGENTS.md"}, directoryNames(t, project)); diff != "" {
			t.Errorf("%s: project (-want +got):\n%s", boundary, diff)
		}
		if names := directoryNames(t, filepath.Join(root, "state", "staging")); len(names) != 0 {
			t.Errorf("%s: staging holds %v", boundary, names)
		}
		if !strings.Contains(readText(t, destination), "two") {
			t.Errorf("%s: the update was not published", boundary)
		}
	}
}

func TestLifecycleCreatesUpdatesAndRemovesSourceFree(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	writeFile(t, destination, "# foreign\n")
	target := stateTarget(t, root)
	first := previewOf(t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target)
	if !first.Applicable() {
		t.Fatalf("preview is not applicable: %v", first.Err())
	}
	wantStatus(t, "apply", ApplyPreview(t.Context(), first, LockOptions{LockTimeout: testLockTimeout}), StatusApplied)
	if !inspectTeam(t, target).Current {
		t.Error("the installation is not current")
	}
	wantStatus(t, "unchanged", syncOnce(t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target), StatusUnchanged)
	wantStatus(t, "update", syncOnce(t, teamBundle(t, "2"), sectionRendered(t, destination, "second"), target), StatusApplied)
	wantStatus(t, "remove", removeAll(t, target, false), StatusApplied)
	if got := readText(t, destination); got != "# foreign\n" {
		t.Errorf("destination = %q, want the foreign content", got)
	}
}

func TestTakeoverRestoresAndAdoptionRemoves(t *testing.T) {
	t.Parallel()
	for _, c := range []struct {
		acquisition Acquisition
		remains     bool
	}{{AcquireTakeover, true}, {AcquireAdopt, false}} {
		root := tempRoot(t)
		destination := filepath.Join(root, "owned")
		original := "original"
		if c.acquisition == AcquireAdopt {
			original = "desired"
		}
		writeFile(t, destination, original)
		if err := os.Chmod(destination, 0o644); err != nil {
			t.Fatal(err)
		}
		target := stateTarget(t, root)
		result := syncWith(t.Context(), t, teamBundle(t, "1"), renderedOf(t, fileAt(t, "guide", destination, "desired")), target, SyncOptions{Acquisition: c.acquisition, LockTimeout: testLockTimeout})
		wantStatus(t, string(c.acquisition), result, StatusApplied)
		wantStatus(t, "remove", removeAll(t, target, false), StatusApplied)
		if got := exists(t, destination); got != c.remains {
			t.Fatalf("%s: destination exists = %v", c.acquisition, got)
		}
		if c.remains && readText(t, destination) != "original" {
			t.Errorf("%s: the original was not restored", c.acquisition)
		}
	}
}

func TestPreviewRejectsChangedContentIndexAndReceiptState(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	writeFile(t, destination, "# foreign\n")
	target := stateTarget(t, root)
	proposal := previewOf(t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target)
	writeFile(t, destination, "# changed\n")
	stale := ApplyPreview(t.Context(), proposal, LockOptions{LockTimeout: testLockTimeout})
	wantStatus(t, "changed content", stale, StatusFailed)
	wantCode(t, "changed content", stale.Err, CodeConcurrentChange)

	current := previewOf(t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target)
	concurrent := must(completedIndex("team", target.ContextDigest(), nil))
	putIndex(t, target, encodeRecord(concurrent))
	wantCode(t, "changed index", ApplyPreview(t.Context(), current, LockOptions{LockTimeout: testLockTimeout}).Err, CodeConcurrentChange)
}

func TestUnsupportedNoticesBlockAllPublication(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	target := stateTarget(t, root)
	notices := []Notice{{ArtifactID: "guide", Kind: NoticeUnsupported, Message: "unsupported event"}}
	rendered := must(NewRenderedBundle([]RenderedArtifact{sectionAt(t, "guide", destination, "generated")}, nil, notices, nil))
	result := syncOnce(t, teamBundle(t, "1"), rendered, target)
	wantStatus(t, "sync", result, StatusFailed)
	wantCode(t, "sync", result.Err, CodeUnsupported)
	if diff := cmp.Diff(notices, result.Notices); diff != "" {
		t.Errorf("notices (-want +got):\n%s", diff)
	}
	proposal := previewOf(t, teamBundle(t, "1"), rendered, target)
	if proposal.Applicable() {
		t.Error("an unsupported preview is applicable")
	}
	applied := ApplyPreview(t.Context(), proposal, LockOptions{LockTimeout: testLockTimeout})
	wantCode(t, "apply", applied.Err, CodeUnsupported)
	if diff := cmp.Diff(notices, applied.Notices); diff != "" {
		t.Errorf("apply notices (-want +got):\n%s", diff)
	}
	if exists(t, destination) {
		t.Error("an unsupported rendering was published")
	}
}

func TestDesiredGenerationComparisonChecksEveryIdentity(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	target := stateTarget(t, root)
	desired, rendered := teamBundle(t, "1"), sectionRendered(t, destination, "generated")
	observed := syncOnce(t, desired, rendered, target).Observation
	if !observed.Matches(desired, rendered) {
		t.Fatal("the installed observation does not match its request")
	}
	if observed.Matches(teamBundle(t, "2"), rendered) {
		t.Error("a different version matches")
	}
	if observed.Matches(desired, sectionRendered(t, destination, "changed")) {
		t.Error("a different rendering matches")
	}
	if observed.Matches(nil, rendered) || observed.Matches(desired, nil) {
		t.Error("a nil request matches")
	}
	writeFile(t, destination, strings.ReplaceAll(readText(t, destination), "g", "x"))
	modified := inspectTeam(t, target)
	if modified.Current || modified.Matches(desired, rendered) {
		t.Error("a modified installation is current")
	}
}

func TestMissingCommittedReceiptBlocksInspectionRecoveryAndRemoval(t *testing.T) {
	t.Parallel()
	for _, membership := range []string{"current", "previous"} {
		root := tempRoot(t)
		destination := filepath.Join(root, "AGENTS.md")
		target := stateTarget(t, root)
		syncOnce(t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target)
		proposal := must(PreviewRemoval(t.Context(), "team", target, false))
		reference := proposal.Resources()[0].Resource()
		if membership == "previous" {
			installed := proposal.index
			previous := must(newIndex(installed.bundleID, installed.contextDigest, nil, nil, installed.current, nil, stateSchemaVersion))
			putIndex(t, target, encodeRecord(previous))
		}
		recorded, _, err := target.Store().Read(t.Context(), receiptKey(reference))
		if err != nil {
			t.Fatal(err)
		}
		if deleted, err := target.Store().Delete(t.Context(), receiptKey(reference), recorded); err != nil || !deleted {
			t.Fatalf("delete receipt: %v %v", deleted, err)
		}
		before := readText(t, destination)
		observed := inspectTeam(t, target)
		wantCode(t, membership+" inspection", observed.Resources[0].Err, CodeInvalidState)
		for _, result := range []InstallationResult{recoverAll(t, target), removeAll(t, target, true)} {
			wantStatus(t, membership, result, StatusFailed)
			wantCode(t, membership, result.Err, CodeInvalidState)
		}
		if readText(t, destination) != before {
			t.Errorf("%s: the payload changed", membership)
		}
	}
}

func TestRecoveryIsExplicitAndPreservesPendingMembership(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	target := stateTarget(t, root)
	interruptedSync := syncWith(seams.With(t.Context(), faultAt(seams.Cleanup)), t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target, SyncOptions{LockTimeout: testLockTimeout})
	wantStatus(t, "interrupted", interruptedSync, StatusPartial)
	if len(interruptedSync.Resources) != 1 || interruptedSync.Resources[0].Status != StatusApplied {
		t.Fatalf("resources = %+v", interruptedSync.Resources)
	}
	if !interruptedSync.Observation.Pending {
		t.Error("the interrupted installation is not pending")
	}
	recovered := recoverAll(t, target)
	wantStatus(t, "recover", recovered, StatusApplied)
	if !recovered.Observation.Pending {
		t.Error("recovery completed the pending index")
	}
	removal := must(PreviewRemoval(t.Context(), "team", target, false))
	if !removal.Applicable() {
		t.Fatalf("removal is not applicable: %v", removal.Err())
	}
	wantStatus(t, "removal", ApplyPreview(t.Context(), removal, LockOptions{LockTimeout: testLockTimeout}), StatusApplied)
	if exists(t, destination) {
		t.Error("the destination remains")
	}
}

func TestLifecycleOperationsRejectIndexDestinationOverlapWithoutMutation(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "payload")
	target := targetOf(t, openStore(t, destination))
	reference := ResourceRef{Destination: authority(t, destination).Destination(), Kind: ResourceDocument}
	recorded := must(newIndex("team", target.ContextDigest(), nil, nil, nil, []ResourceRef{reference}, stateSchemaVersion))
	putIndex(t, target, encodeRecord(recorded))
	before := filesystemSnapshot(t, root)
	results := []InstallationResult{
		recoverAll(t, target),
		removeAll(t, target, false),
		syncOnce(t, teamBundle(t, "1"), sectionRendered(t, filepath.Join(root, "other"), "generated"), target),
	}
	for index, result := range results {
		wantStatus(t, "overlap", result, StatusFailed)
		if len(result.Resources) != 0 {
			t.Errorf("%d: resources = %+v", index, result.Resources)
		}
		want := &TargetError{Code: CodeInvalidState, Message: "state store overlaps a resource or authority"}
		if diff := cmp.Diff(want, result.Err, cmp.Comparer(func(a, b *TargetError) bool {
			return a.Code == b.Code && a.Message == b.Message && a.Path == b.Path
		})); diff != "" {
			t.Errorf("%d: error (-want +got):\n%s", index, diff)
		}
		if diff := cmp.Diff(before, filesystemSnapshot(t, root)); diff != "" {
			t.Errorf("%d: filesystem changed (-before +after):\n%s", index, diff)
		}
		if hasRecord(t, target.Store(), authorityKey(reference)) {
			t.Errorf("%d: the resource was admitted", index)
		}
	}
}

func TestManagementOutputsCannotOverlapBundleSources(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name   string
		source func(root string) string
		target func(t *testing.T, root, source string) *InstallationTarget
		render func(t *testing.T, root string) *RenderedBundle
		want   string
	}{
		{
			name:   "store inside the source",
			source: func(root string) string { return filepath.Join(root, "source") },
			target: func(t *testing.T, _, source string) *InstallationTarget {
				return targetOf(t, openStore(t, filepath.Join(source, "index")))
			},
			want: "state store overlaps bundle source",
		},
		{
			name:   "staging directory inside the source",
			source: func(root string) string { return filepath.Join(root, "source") },
			target: func(t *testing.T, _, source string) *InstallationTarget {
				return targetOf(t, &pathStore{MemoryStore: store.NewMemoryStore(), staging: filepath.Join(source, "staging")})
			},
			want: "state store overlaps bundle source",
		},
		{
			name: "source is the destination's transaction directory",
			source: func(root string) string {
				return must(TransactionPath(filepath.Join(root, "AGENTS.md"), strings.Repeat("a", 32)))
			},
			target: func(t *testing.T, root, _ string) *InstallationTarget {
				return targetOf(t, openStore(t, filepath.Join(root, "index")))
			},
			want: "resource authority overlaps bundle source",
		},
		{
			name:   "skill output inside the source",
			source: func(root string) string { return filepath.Join(root, "skills", "review") },
			target: func(t *testing.T, root, _ string) *InstallationTarget {
				return targetOf(t, openStore(t, filepath.Join(root, "index")))
			},
			render: func(t *testing.T, root string) *RenderedBundle {
				return skillRendered(t, filepath.Join(root, "skills"))
			},
			want: "resource output overlaps bundle source",
		},
	}
	for _, c := range cases {
		root := tempRoot(t)
		source := c.source(root)
		writeBundle(t, source, defaultManifest)
		snapshot := openDir(t, source)
		target := c.target(t, root, source)
		destination := filepath.Join(root, "AGENTS.md")
		rendered := sectionRendered(t, destination, "generated")
		if c.render != nil {
			rendered = c.render(t, root)
		}
		before := filesystemSnapshot(t, source)
		_, syncErr := Sync(t.Context(), snapshot, rendered, target, SyncOptions{LockTimeout: testLockTimeout})
		_, previewErr := Preview(t.Context(), snapshot, rendered, target, PreviewOptions{})
		for label, err := range map[string]error{"sync": syncErr, "preview": previewErr} {
			if message := valueMessage(t, err); message != c.want {
				t.Errorf("%s: %s: message = %q, want %q", c.name, label, message, c.want)
			}
		}
		if diff := cmp.Diff(before, filesystemSnapshot(t, source)); diff != "" {
			t.Errorf("%s: the source changed (-before +after):\n%s", c.name, diff)
		}
		if exists(t, destination) {
			t.Errorf("%s: the destination was written", c.name)
		}
	}
}

func TestPreviewBindsReceiptsSecurityAncestorsAndIndexState(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	project := filepath.Join(root, "project")
	destination := filepath.Join(project, "AGENTS.md")
	if err := os.Mkdir(project, 0o755); err != nil {
		t.Fatal(err)
	}
	writeFile(t, destination, "# foreign\n")
	target := stateTarget(t, root)
	syncOnce(t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target)

	receiptPreview := previewOf(t, teamBundle(t, "2"), sectionRendered(t, destination, "second"), target)
	key := receiptKey(receiptPreview.Resources()[0].Resource())
	original, _, err := target.Store().Read(t.Context(), key)
	if err != nil {
		t.Fatal(err)
	}
	changed := append([]byte{original[0], ' '}, original[1:]...)
	replaceRecord(t, target.Store(), key, changed)
	wantCode(t, "receipt", ApplyPreview(t.Context(), receiptPreview, LockOptions{LockTimeout: testLockTimeout}).Err, CodeConcurrentChange)
	replaceRecord(t, target.Store(), key, original)

	ancestorPreview := previewOf(t, teamBundle(t, "2"), sectionRendered(t, destination, "second"), target)
	if err := os.Chmod(project, 0o700); err != nil {
		t.Fatal(err)
	}
	wantCode(t, "ancestor", ApplyPreview(t.Context(), ancestorPreview, LockOptions{LockTimeout: testLockTimeout}).Err, CodeConcurrentChange)
}

func TestRoutingContextIsFrozenButNotPartOfIndexIdentity(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	routing := map[string]string{"agent": "claude"}
	first := must(NewInstallationTarget(st, map[string]string{"surface": "application"}, routing))
	routing["agent"] = "codex"
	second := must(NewInstallationTarget(st, map[string]string{"surface": "application"}, routing))
	if first.ContextDigest() != second.ContextDigest() {
		t.Error("routing changed the context digest")
	}
	if diff := cmp.Diff(map[string]string{"agent": "claude"}, first.RoutingContext()); diff != "" {
		t.Errorf("routing (-want +got):\n%s", diff)
	}
}

func TestPreflightConflictsPreventAllResourcePublication(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	first, second := filepath.Join(root, "first"), filepath.Join(root, "second")
	writeFile(t, second, "foreign")
	target := stateTarget(t, root)
	rendered := renderedOf(t, fileAt(t, "first", first, "new"), fileAt(t, "second", second, "new"))
	wantStatus(t, "sync", syncOnce(t, teamBundle(t, "1"), rendered, target), StatusFailed)
	if exists(t, first) || readText(t, second) != "foreign" {
		t.Error("a resource was published despite a preflight conflict")
	}
}

func TestResultFailureIsANilErrorAfterSuccessAndTheTargetErrorAfterFailure(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	target := stateTarget(t, root)
	destination := filepath.Join(root, "AGENTS.md")
	applied := syncOnce(t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target)
	unchanged := syncOnce(t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target)
	for label, result := range map[string]InstallationResult{"applied": applied, "unchanged": unchanged} {
		if !result.Succeeded() || result.Failure() != nil {
			t.Errorf("%s: Succeeded() = %t, Failure() = %v", label, result.Succeeded(), result.Failure())
		}
	}
	if !applied.Resources[0].Succeeded() || applied.Resources[0].Failure() != nil {
		t.Errorf("resource: Succeeded() = %t, Failure() = %v", applied.Resources[0].Succeeded(), applied.Resources[0].Failure())
	}
	foreign := filepath.Join(root, "foreign")
	writeFile(t, foreign, "foreign")
	failed := syncOnce(t, teamBundle(t, "1"), renderedOf(t, fileAt(t, "guide", foreign, "new")), target)
	failure, ok := errors.AsType[*TargetError](failed.Failure())
	if failed.Succeeded() || !ok || failure != failed.Err {
		t.Errorf("failed: Succeeded() = %t, Failure() = %v, Err = %v", failed.Succeeded(), failed.Failure(), failed.Err)
	}
}

func TestEveryTargetErrorCarrierReportsFailureAsAnError(t *testing.T) {
	t.Parallel()
	failure := &TargetError{Code: CodeConflict, Message: "conflict", Path: "/fixture"}
	carriers := func(err *TargetError) map[string]func() error {
		return map[string]func() error{
			"ResourceObservation":     ResourceObservation{Err: err}.Failure,
			"InstallationObservation": InstallationObservation{Err: err}.Failure,
			"Observation":             Observation{Err: err}.Failure,
			"PlannedChange":           PlannedChange{Err: err}.Failure,
			"PlanSummary":             PlanSummary{Err: err}.Failure,
			"ResourcePlan":            ResourcePlan{err: err}.Failure,
			"LifecyclePreview":        (&LifecyclePreview{err: err}).Failure,
		}
	}
	for name, failed := range carriers(nil) {
		if err := failed(); err != nil {
			t.Errorf("%s without a failure: Failure() = %#v", name, err)
		}
	}
	for name, failed := range carriers(failure) {
		got, ok := errors.AsType[*TargetError](failed())
		if !ok || *got != *failure {
			t.Errorf("%s: Failure() = %v, want %v", name, got, failure)
		}
	}
	if err := (*LifecyclePreview)(nil).Failure(); err != nil {
		t.Errorf("nil preview: Failure() = %#v", err)
	}
}

func TestResultSucceededOnlyForAppliedOrUnchanged(t *testing.T) {
	t.Parallel()
	failure := &TargetError{Code: CodeConflict, Message: "conflict"}
	for _, c := range []struct {
		status OperationStatus
		err    *TargetError
		want   bool
	}{
		{StatusApplied, nil, true},
		{StatusUnchanged, nil, true},
		{StatusFailed, failure, false},
		{StatusPartial, failure, false},
		{StatusIncomplete, failure, false},
	} {
		results := []interface {
			Succeeded() bool
			Failure() error
		}{
			InstallationResult{Status: c.status, Err: c.err},
			ResourceResult{Status: c.status, Err: c.err},
			TargetResult{Status: c.status, Err: c.err},
		}
		for _, result := range results {
			if result.Succeeded() != c.want {
				t.Errorf("%T %s: Succeeded() = %t, want %t", result, c.status, result.Succeeded(), c.want)
			}
			got, _ := errors.AsType[*TargetError](result.Failure())
			if (result.Failure() == nil) != (c.err == nil) || got != c.err {
				t.Errorf("%T %s: Failure() = %v, want %v", result, c.status, result.Failure(), c.err)
			}
		}
	}
}

func TestIndexCompletionFailureAggregatesAppliedResourcesAsPartial(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	first, second := filepath.Join(root, "first"), filepath.Join(root, "second")
	target := stateTarget(t, root)
	rendered := renderedOf(t, fileAt(t, "first", first, "one"), fileAt(t, "second", second, "two"))
	result := syncWith(seams.With(t.Context(), faultAt(seams.IndexCompletion)), t, teamBundle(t, "1"), rendered, target, SyncOptions{LockTimeout: testLockTimeout})
	wantStatus(t, "sync", result, StatusPartial)
	if len(result.Resources) != 2 || slices.ContainsFunc(result.Resources, func(item ResourceResult) bool { return item.Status != StatusApplied }) {
		t.Fatalf("resources = %+v", result.Resources)
	}
	if readText(t, first) != "one" || readText(t, second) != "two" {
		t.Error("the payloads were not published")
	}
	wantStatus(t, "recover", recoverAll(t, target), StatusUnchanged)
	wantStatus(t, "reconcile", syncOnce(t, teamBundle(t, "1"), rendered, target), StatusApplied)
	if !inspectTeam(t, target).Current {
		t.Error("the reconciled installation is not current")
	}
}

func TestLaterResourcePreconditionFailureIsPartialAndOrdered(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	target := stateTarget(t, root)
	rendered := renderedOf(t, fileAt(t, "first", filepath.Join(root, "first"), "one"), fileAt(t, "second", filepath.Join(root, "second"), "two"))
	proposal := previewOf(t, teamBundle(t, "1"), rendered, target)
	order := planResources(proposal.Resources())
	result := ApplyPreview(seams.With(t.Context(), faultAfter(seams.ResourcePrecondition, 1)), proposal, LockOptions{LockTimeout: testLockTimeout})
	wantStatus(t, "apply", result, StatusPartial)
	got := make([]ResourceResult, 0, len(result.Resources))
	for _, item := range result.Resources {
		got = append(got, ResourceResult{Resource: item.Resource, Status: item.Status})
	}
	want := []ResourceResult{{Resource: order[0], Status: StatusApplied}, {Resource: order[1], Status: StatusFailed}}
	if diff := cmp.Diff(want, got); diff != "" {
		t.Errorf("resources (-want +got):\n%s", diff)
	}
}

func TestSharedSectionsArePlannedInArtifactIDOrder(t *testing.T) {
	t.Parallel()
	for run := range 16 {
		root := tempRoot(t)
		destination := filepath.Join(root, "AGENTS.md")
		writeFile(t, destination, "# Foreign\n")
		target := stateTarget(t, root)
		desired := must(NewBundle(must(NewBundleIdentity("team", "1")), []Artifact{
			must(NewInstructionArtifact("zeta", "z")),
			must(NewInstructionArtifact("alpha", "a")),
		}, nil, nil))
		alpha := sectionAt(t, "alpha", destination, "Alpha")
		beta := must(NewRenderedArtifact(RenderedArtifactParams{ID: "beta", Family: alpha.Family(), Destination: alpha.Destination(), Content: alpha.Content()}))
		rendered := renderedOf(t, sectionAt(t, "zeta", destination, "Zeta"), beta, alpha)
		wantStatus(t, "sync", syncOnce(t, desired, rendered, target), StatusApplied)
		want := "# Foreign\n\n<!-- flyrail:alpha:start -->\nAlpha\n<!-- flyrail:alpha:end -->\n\n<!-- flyrail:zeta:start -->\nZeta\n<!-- flyrail:zeta:end -->\n"
		if got := readText(t, destination); got != want {
			t.Fatalf("run %d: document = %q", run, got)
		}
		installed := receiptOf(t, target.Store(), documentRef(t, destination))
		if installed == nil || len(installed.claims) != 2 {
			t.Fatalf("run %d: receipt = %+v", run, installed)
		}
	}
}

func TestCorruptResourceStateNeverAuthorizesPayloadChanges(t *testing.T) {
	t.Parallel()
	for _, damage := range []string{"receipt", "receipt-resource"} {
		root := tempRoot(t)
		destination := filepath.Join(root, "AGENTS.md")
		target := stateTarget(t, root)
		syncOnce(t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target)
		proposal := must(PreviewRemoval(t.Context(), "team", target, false))
		key := receiptKey(proposal.Resources()[0].Resource())
		corrupt := []byte("{}")
		if damage == "receipt-resource" {
			other := ResourceRef{Destination: filepath.Join(root, "OTHER.md"), Kind: ResourceDocument}
			corrupt = encodeRecord(must(newReceipt(other, strings.Repeat("1", 32), nil, DocumentProvenance{}, nil, stateSchemaVersion)))
		}
		replaceRecord(t, target.Store(), key, corrupt)
		before := readText(t, destination)
		wantCode(t, damage, inspectTeam(t, target).Resources[0].Err, CodeInvalidState)
		for _, result := range []InstallationResult{
			syncWith(t.Context(), t, teamBundle(t, "2"), sectionRendered(t, destination, "changed"), target, SyncOptions{ReplaceModified: true, LockTimeout: testLockTimeout}),
			removeAll(t, target, true),
		} {
			wantStatus(t, damage, result, StatusFailed)
		}
		if readText(t, destination) != before {
			t.Errorf("%s: the payload changed", damage)
		}
	}
}

func TestMalformedIndexRecordDoesNotAdmitResources(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	target := stateTarget(t, root)
	putIndex(t, target, []byte("{}"))
	result := syncOnce(t, teamBundle(t, "1"), sectionRendered(t, destination, "generated"), target)
	wantStatus(t, "sync", result, StatusFailed)
	wantCode(t, "sync", result.Err, CodeInvalidState)
	if keys := must(target.Store().Keys(t.Context(), "resource/")); len(keys) != 0 {
		t.Errorf("resource keys = %v", keys)
	}
	if exists(t, destination) {
		t.Error("the destination was written")
	}
}

func TestOrdinaryPlanFailuresReturnEmptyNotices(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "owned")
	writeFile(t, destination, "foreign")
	target := stateTarget(t, root)
	notices := []Notice{{ArtifactID: "guide", Kind: NoticePrerequisite, Message: "install helper"}}
	rendered := must(NewRenderedBundle([]RenderedArtifact{fileAt(t, "guide", destination, "desired")}, nil, notices, nil))
	applied := ApplyPreview(t.Context(), previewOf(t, teamBundle(t, "1"), rendered, target), LockOptions{LockTimeout: testLockTimeout})
	synchronized := syncOnce(t, teamBundle(t, "1"), rendered, target)
	for _, result := range []InstallationResult{applied, synchronized} {
		wantStatus(t, "conflict", result, StatusFailed)
		if len(result.Notices) != 0 {
			t.Errorf("notices = %+v", result.Notices)
		}
	}
}

func TestPublicLifecycleEntryPointsValidateBundleIDsBeforeIO(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	target := stateTarget(t, root)
	ctx := t.Context()
	_, previewErr := PreviewRemoval(ctx, "Bad ID", target, false)
	_, inspectErr := InspectInstallation(ctx, "Bad ID", target)
	_, recoverErr := RecoverInstallation(ctx, "Bad ID", target, LockOptions{})
	_, removeErr := Remove(ctx, "Bad ID", target, RemoveOptions{})
	for _, err := range []error{previewErr, inspectErr, recoverErr, removeErr} {
		if got := valueMessage(t, err); got != "bundle id must be 1-64 lowercase ASCII letters/digits with internal hyphens" {
			t.Errorf("message = %q", got)
		}
	}
	if exists(t, filepath.Join(root, "state")) {
		t.Error("validation created the store")
	}
}

func TestInstallationTargetsValidateContextBeforeIO(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	target := must(NewInstallationTarget(openStore(t, filepath.Join(root, "state")), map[string]string{"surface": "application"}, nil))
	if diff := cmp.Diff(map[string]string{"surface": "application"}, target.Context()); diff != "" {
		t.Errorf("context (-want +got):\n%s", diff)
	}
	if exists(t, filepath.Join(root, "state")) {
		t.Error("constructing the target created the store")
	}
}

func TestEveryContextTakingFunctionRefusesANilContextAsAValueError(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	target := stateTarget(t, root)
	st := store.NewMemoryStore()
	b, r := teamBundle(t, "1"), sectionRendered(t, filepath.Join(root, "AGENTS.md"), "x")
	skill := reviewSkill(t, "team", "1", "content")
	skillTargets := directoryTargets(t, filepath.Join(root, "skills"))
	var nilContext context.Context
	for name, err := range map[string]error{
		"DetectAgents":         errorOf(DetectAgents(nilContext, root, DiscoveryOptions{})),
		"Install":              errorOf(Install(nilContext, skill, skillTargets, st, skillLock)),
		"Update":               errorOf(Update(nilContext, skill, skillTargets, st, SkillOptions{})),
		"Uninstall":            errorOf(Uninstall(nilContext, "team", skillTargets, st, SkillOptions{})),
		"Inspect":              errorOf(Inspect(nilContext, skill, skillTargets, st)),
		"RenderSkills":         errorOf(RenderSkills(nilContext, skill, skillTargets[0])),
		"NewResourceAuthority": errorOf(NewResourceAuthority(nilContext, filepath.Join(root, "AGENTS.md"))),
		"CanonicalDestination": errorOf(CanonicalDestination(nilContext, filepath.Join(root, "AGENTS.md"))),
		"OpenBundle":           errorOf(OpenBundle(nilContext, root)),
		"OpenBundleZip":        errorOf(OpenBundleZip(nilContext, filepath.Join(root, "bundle.zip"), "bundle")),
		"Preview":              errorOf(Preview(nilContext, b, r, target, PreviewOptions{})),
		"PreviewRemoval":       errorOf(PreviewRemoval(nilContext, "team", target, false)),
		"Sync":                 errorOf(Sync(nilContext, b, r, target, SyncOptions{})),
		"RecoverInstallation":  errorOf(RecoverInstallation(nilContext, "team", target, LockOptions{})),
		"InspectInstallation":  errorOf(InspectInstallation(nilContext, "team", target)),
		"Remove":               errorOf(Remove(nilContext, "team", target, RemoveOptions{})),
	} {
		failure, ok := errors.AsType[*ValueError](err)
		if !ok || failure.Message != "context must not be nil" {
			t.Errorf("%s: error = %T %v, want *ValueError \"context must not be nil\"", name, err, err)
		}
	}
	result := ApplyPreview(nilContext, previewOf(t, b, r, target), LockOptions{})
	if result.Status != StatusFailed || result.Err == nil || result.Err.Message != "context must not be nil" {
		t.Errorf("ApplyPreview: status %s, err %v", result.Status, result.Err)
	}
}

func TestLifecycleArgumentsAreValidatedAsValueErrors(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	target := stateTarget(t, root)
	b, r := teamBundle(t, "1"), sectionRendered(t, filepath.Join(root, "AGENTS.md"), "x")
	var nilContext context.Context
	_, nilTarget := Preview(t.Context(), b, r, nil, PreviewOptions{})
	_, zeroTarget := Sync(t.Context(), b, r, &InstallationTarget{}, SyncOptions{})
	_, nilBundle := Sync(t.Context(), nil, r, target, SyncOptions{})
	_, badAcquisition := Preview(t.Context(), b, r, target, PreviewOptions{Acquisition: "steal"})
	_, negative := Sync(t.Context(), b, r, target, SyncOptions{LockTimeout: -1})
	_, missingContext := InspectInstallation(nilContext, "team", target)
	_, removalTarget := PreviewRemoval(t.Context(), "team", nil, false)
	_, inspectTarget := InspectInstallation(t.Context(), "team", nil)
	cases := []struct {
		err  error
		want string
	}{
		{nilTarget, "preview requires a bundle snapshot, rendered bundle and installation target"},
		{zeroTarget, "sync requires a bundle snapshot, rendered bundle and installation target"},
		{nilBundle, "sync requires a bundle snapshot, rendered bundle and installation target"},
		{badAcquisition, "invalid acquisition or replacement policy"},
		{negative, "lock timeout must be finite and nonnegative"},
		{missingContext, "context must not be nil"},
		{removalTarget, "invalid installation target or replacement policy"},
		{inspectTarget, "target must be an InstallationTarget"},
	}
	for _, c := range cases {
		if got := valueMessage(t, c.err); got != c.want {
			t.Errorf("message = %q, want %q", got, c.want)
		}
	}
	for _, result := range []InstallationResult{
		ApplyPreview(t.Context(), nil, LockOptions{}),
		ApplyPreview(t.Context(), &LifecyclePreview{}, LockOptions{}),
		ApplyPreview(nilContext, previewOf(t, b, r, target), LockOptions{}),
	} {
		wantStatus(t, "apply", result, StatusFailed)
		wantCode(t, "apply", result.Err, CodeInvalidState)
	}
	if exists(t, filepath.Join(root, "AGENTS.md")) {
		t.Error("an invalid request wrote the destination")
	}
}

func TestAPreviewIsImmutableAndReportsItsRequest(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	target := stateTarget(t, root)
	b, r := teamBundle(t, "1"), sectionRendered(t, destination, "generated")
	proposal := must(Preview(t.Context(), b, r, target, PreviewOptions{Acquisition: AcquireTakeover, ReplaceModified: true}))
	if proposal.BundleID() != "team" || proposal.Bundle() != b || proposal.Rendered() != r || proposal.Target() != target ||
		proposal.Acquisition() != AcquireTakeover || !proposal.ReplaceModified() {
		t.Fatalf("preview does not report its request")
	}
	plans := proposal.Resources()
	plans[0] = ResourcePlan{}
	if proposal.Resources()[0].Resource().Destination != authority(t, destination).Destination() {
		t.Error("mutating the returned plans changed the preview")
	}
	if failed := proposal.Err(); failed != nil {
		t.Fatalf("preview error = %v", failed)
	}
	removal := must(PreviewRemoval(t.Context(), "team", target, false))
	if removal.Bundle() != nil || removal.Acquisition() != AcquireConflict || len(removal.Resources()) != 0 {
		t.Errorf("an empty removal preview = %+v", removal)
	}
	var missing *LifecyclePreview
	if missing.Applicable() || missing.BundleID() != "" || missing.Resources() != nil || missing.Err() != nil {
		t.Error("a nil preview reports state")
	}
}

func TestALockWaitEndsAtTheContextDeadline(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	target := stateTarget(t, root)
	holdLock(t, target.Store(), installationKey("team", target))
	ctx, cancel := context.WithTimeout(t.Context(), 100*time.Millisecond)
	defer cancel()
	started := time.Now()
	result := syncWith(ctx, t, teamBundle(t, "1"), sectionRendered(t, destination, "x"), target, SyncOptions{LockTimeout: time.Minute})
	wantStatus(t, "sync", result, StatusFailed)
	wantCode(t, "sync", result.Err, CodeBusy)
	if !errors.Is(result.Err, context.DeadlineExceeded) {
		t.Errorf("error %v does not wrap the deadline", result.Err)
	}
	if elapsed := time.Since(started); elapsed >= time.Minute {
		t.Errorf("the wait lasted %v", elapsed)
	}
	if exists(t, destination) {
		t.Error("a sync that never locked wrote the destination")
	}
}
