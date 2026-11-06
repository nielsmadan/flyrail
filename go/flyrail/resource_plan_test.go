//go:build darwin || linux

package flyrail

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

func planBundle(t *testing.T) *Bundle {
	t.Helper()
	return must(NewBundleFromEntries(
		[]byte(`{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review"}]}`),
		[]BundleEntry{{Path: "review/SKILL.md", Data: []byte("# Review\n")}},
	))
}

func renderedOne(t *testing.T, params RenderedArtifactParams) (*RenderedBundle, RenderedArtifact) {
	t.Helper()
	if params.ID == "" {
		params.ID = "guide"
	}
	if params.Family == "" {
		params.Family = FamilyInstructions
	}
	artifact := must(NewRenderedArtifact(params))
	return must(NewRenderedBundle([]RenderedArtifact{artifact}, nil, nil, nil)), artifact
}

func renderedFile(t *testing.T, destination, data string) (*RenderedBundle, RenderedArtifact) {
	t.Helper()
	return renderedOne(t, RenderedArtifactParams{Destination: destination, Content: must(NewFileContent([]byte(data), ModePrivate))})
}

func noRequirements(artifacts ...RenderedArtifact) map[string][]requirement {
	result := map[string][]requirement{}
	for _, artifact := range artifacts {
		result[artifact.ID()] = nil
	}
	return result
}

type planCase struct {
	bundle          *Bundle
	rendered        *RenderedBundle
	artifacts       []RenderedArtifact
	acquisition     Acquisition
	replaceModified bool
}

func admittedResource(t *testing.T, st store.Store, artifact RenderedArtifact) ResourceRef {
	t.Helper()
	reference, failed := resourceOf(t.Context(), artifact)
	mustSucceed(t, failed)
	mustSucceed(t, admit(t.Context(), st, reference))
	return reference
}

func planFor(t *testing.T, st store.Store, reference ResourceRef, c planCase) ResourcePlan {
	t.Helper()
	if c.rendered == nil {
		c.rendered = EmptyRenderedBundle()
	}
	if c.acquisition == "" {
		c.acquisition = AcquireConflict
	}
	planned, failed := planResource(t.Context(), st, planInputs{
		reference:          reference,
		identifier:         "team",
		bundle:             c.bundle,
		rendered:           c.rendered,
		artifacts:          c.artifacts,
		acquisition:        c.acquisition,
		replaceModified:    c.replaceModified,
		requirements:       noRequirements(c.artifacts...),
		stableRequirements: noRequirements(c.artifacts...),
	})
	mustSucceed(t, failed)
	return planned
}

func commit(t *testing.T, st store.Store, planned ResourcePlan) {
	t.Helper()
	if planned.err != nil {
		t.Fatalf("committing a failed plan: %v", planned.err)
	}
	destination := planned.resource.Destination
	recorded := planned.receipt
	if !planned.after.equal(planned.before) {
		if err := os.RemoveAll(destination); err != nil {
			t.Fatal(err)
		}
		if len(planned.after.nodes) > 0 {
			written, failed := writeRevision(destination, planned.after)
			mustSucceed(t, failed)
			var markers []node
			for _, marker := range recorded.createdDirectories {
				index := slices.IndexFunc(written.nodes, func(item node) bool { return item.path == marker.path })
				markers = append(markers, written.nodes[index])
			}
			recorded = must(newReceipt(recorded.resource, recorded.transactionID, recorded.claims, recorded.provenance, markers, stateSchemaVersion))
		}
	}
	mustSucceed(t, writeRecord(t.Context(), st, receiptKey(planned.resource), recorded, planned.previous))
}

func TestPlansCreationObservationAndSourceFreeRemoval(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	rendered, artifact := renderedFile(t, filepath.Join(root, "guide"), "generated\n")
	st := store.NewMemoryStore()
	reference := admittedResource(t, st, artifact)
	_, failed := planResource(t.Context(), st, planInputs{
		reference: reference, identifier: "team", bundle: planBundle(t), rendered: rendered,
		artifacts: []RenderedArtifact{artifact}, acquisition: AcquireConflict,
	})
	wantFailure(t, failed, CodeInvalidState, "planned artifact requirements are missing", reference.Destination)
	created := planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}})
	if created.Err() != nil {
		t.Fatalf("plan error = %v", created.Err())
	}
	if data, ok := created.after.data(); !ok || string(data) != "generated\n" {
		t.Fatalf("planned data = %q, %v", data, ok)
	}
	if got := created.receipt.claims[0].disposition; got != DispositionCreated {
		t.Errorf("disposition = %s", got)
	}
	if created.TransactionDirectory() != must(TransactionPath(reference.Destination, created.receipt.transactionID)) {
		t.Errorf("transaction directory = %q", created.TransactionDirectory())
	}
	commit(t, st, created)
	summary, failed := summarizeResource(t.Context(), st, reference)
	mustSucceed(t, failed)
	if summary.claims[0].Status != EditCurrent || len(summary.recoveryPaths) != 0 {
		t.Errorf("summary = %+v", summary)
	}
	unchanged := planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}})
	if unchanged.receipt.transactionID != created.receipt.transactionID || !unchanged.after.equal(unchanged.before) {
		t.Errorf("an unchanged resource got a new transaction %q", unchanged.receipt.transactionID)
	}
	removal := planFor(t, st, reference, planCase{})
	if !isSourceFreeRemoval(*removal.previous, "team") || isSourceFreeRemoval(*removal.previous, "other") {
		t.Error("source-free removal is not recognized")
	}
	if len(removal.after.nodes) != 0 || len(removal.receipt.claims) != 0 {
		t.Errorf("removal plan = %d nodes, %d claims", len(removal.after.nodes), len(removal.receipt.claims))
	}
	commit(t, st, removal)
	if _, err := os.Lstat(reference.Destination); !os.IsNotExist(err) {
		t.Errorf("removed destination still exists: %v", err)
	}
}

func TestPlansRetargetingAsRetirementAndCreationInOneStore(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	first, second := filepath.Join(root, "first"), filepath.Join(root, "second")
	writeFile(t, first, "first baseline\n")
	writeFile(t, second, "second baseline\n")
	st := store.NewMemoryStore()
	renderedFirst, artifactFirst := renderedFile(t, first, "generated\n")
	firstRef := admittedResource(t, st, artifactFirst)
	commit(t, st, planFor(t, st, firstRef, planCase{bundle: planBundle(t), rendered: renderedFirst, artifacts: []RenderedArtifact{artifactFirst}, acquisition: AcquireTakeover}))

	renderedSecond, artifactSecond := renderedFile(t, second, "generated\n")
	secondRef := admittedResource(t, st, artifactSecond)
	retired := planFor(t, st, firstRef, planCase{bundle: planBundle(t), rendered: renderedSecond})
	created := planFor(t, st, secondRef, planCase{bundle: planBundle(t), rendered: renderedSecond, artifacts: []RenderedArtifact{artifactSecond}, acquisition: AcquireTakeover})
	if retired.Err() != nil || created.Err() != nil {
		t.Fatalf("retarget plan errors = %v, %v", retired.Err(), created.Err())
	}
	if retired.previous == nil || !isSourceFreeRemoval(*retired.previous, "team") || len(retired.receipt.claims) != 0 {
		t.Fatalf("retired plan = previous %v, %d claims", retired.previous, len(retired.receipt.claims))
	}
	if data, _ := retired.after.data(); string(data) != "first baseline\n" {
		t.Errorf("retirement restores %q", data)
	}
	if created.previous != nil || created.receipt.claims[0].disposition != DispositionTakenOver {
		t.Fatalf("created plan = previous %v, disposition %s", created.previous, created.receipt.claims[0].disposition)
	}
	if data, _ := created.after.data(); string(data) != "generated\n" {
		t.Errorf("creation writes %q", data)
	}
	if created.receipt.transactionID == retired.receipt.transactionID {
		t.Error("the two destinations share a transaction id")
	}
	commit(t, st, retired)
	commit(t, st, created)

	contents := map[string]string{}
	for _, path := range []string{first, second} {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		contents[path] = string(data)
	}
	if diff := cmp.Diff(map[string]string{first: "first baseline\n", second: "generated\n"}, contents); diff != "" {
		t.Errorf("retargeted destinations (-want +got):\n%s", diff)
	}
	firstSummary, failed := summarizeResource(t.Context(), st, firstRef)
	mustSucceed(t, failed)
	secondSummary, failed := summarizeResource(t.Context(), st, secondRef)
	mustSucceed(t, failed)
	if len(firstSummary.claims) != 0 || len(secondSummary.claims) != 1 || secondSummary.claims[0].Status != EditCurrent {
		t.Errorf("summaries = %+v, %+v", firstSummary, secondSummary)
	}
	removal := planFor(t, st, secondRef, planCase{})
	if data, _ := removal.after.data(); string(data) != "second baseline\n" {
		t.Errorf("removing the new destination restores %q", data)
	}
}

func TestPlansSharedDocumentEditsAndDetectsLocalModification(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	writeFile(t, destination, "foreign\n")
	content := must(NewSectionContent("guide", "generated", nil))
	rendered, artifact := renderedOne(t, RenderedArtifactParams{Destination: destination, Content: content})
	st := store.NewMemoryStore()
	reference := admittedResource(t, st, artifact)
	first := planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}})
	if first.Err() != nil {
		t.Fatalf("plan error = %v", first.Err())
	}
	want := "foreign\n\n<!-- flyrail:guide:start -->\ngenerated\n<!-- flyrail:guide:end -->\n"
	if data, _ := first.after.data(); string(data) != want {
		t.Fatalf("planned document = %q", data)
	}
	if first.after.nodes[0].inode != first.before.nodes[0].inode {
		t.Error("a document edit lost the destination's identity")
	}
	commit(t, st, first)
	if err := os.WriteFile(destination, []byte("foreign\n\n<!-- flyrail:guide:start -->\nchanged\n<!-- flyrail:guide:end -->\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	modified := planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}})
	if modified.Err() == nil || modified.Err().Code != CodeModified {
		t.Fatalf("modified plan error = %v", modified.Err())
	}
	if !modified.after.equal(modified.before) || modified.receipt.transactionID != first.receipt.transactionID {
		t.Error("a failed plan changes the resource or receipt")
	}
	replaced := planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}, replaceModified: true})
	if replaced.Err() != nil {
		t.Fatalf("replacing plan error = %v", replaced.Err())
	}
	if data, _ := replaced.after.data(); string(data) != want {
		t.Errorf("replacing plan document = %q", data)
	}
	summary, failed := summarizeResource(t.Context(), st, reference)
	mustSucceed(t, failed)
	if summary.claims[0].Status != EditModified {
		t.Errorf("summary status = %s", summary.claims[0].Status)
	}
}

func TestWholeResourceAcquisitionModes(t *testing.T) {
	t.Parallel()
	for _, test := range []struct {
		acquisition Acquisition
		existing    string
		code        ErrorCode
		message     string
		disposition Disposition
	}{
		{AcquireConflict, "foreign\n", CodeConflict, "unowned content already exists", ""},
		{AcquireAdopt, "foreign\n", CodeConflict, "adoption requires matching content", ""},
		{AcquireAdopt, "generated\n", "", "", DispositionAdopted},
		{AcquireTakeover, "foreign\n", "", "", DispositionTakenOver},
	} {
		t.Run(string(test.acquisition)+"-"+strings.TrimSpace(test.existing), func(t *testing.T) {
			t.Parallel()
			root := tempRoot(t)
			destination := filepath.Join(root, "guide")
			writeFile(t, destination, test.existing)
			rendered, artifact := renderedFile(t, destination, "generated\n")
			st := store.NewMemoryStore()
			reference := admittedResource(t, st, artifact)
			planned := planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}, acquisition: test.acquisition})
			if test.code != "" {
				wantFailure(t, planned.Err(), test.code, test.message, reference.Destination)
				return
			}
			if planned.Err() != nil {
				t.Fatalf("plan error = %v", planned.Err())
			}
			owned := planned.receipt.claims[0]
			if owned.disposition != test.disposition {
				t.Fatalf("disposition = %s", owned.disposition)
			}
			commit(t, st, planned)
			removal := planFor(t, st, reference, planCase{})
			want := ""
			if test.disposition == DispositionTakenOver {
				want = test.existing
				if owned.baseline == nil {
					t.Fatal("takeover recorded no baseline")
				}
			}
			if data, _ := removal.after.data(); string(data) != want {
				t.Errorf("removal restores %q, want %q", data, want)
			}
		})
	}
}

func TestWholeResourcePlansRefuseLocalModificationsUnlessReplacing(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	rendered, artifact := renderedFile(t, filepath.Join(root, "guide"), "generated\n")
	st := store.NewMemoryStore()
	reference := admittedResource(t, st, artifact)
	commit(t, st, planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}}))
	if err := os.WriteFile(reference.Destination, []byte("edited\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	refused := planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}})
	wantFailure(t, refused.Err(), CodeModified, "owned resource has local modifications", reference.Destination)
	replaced := planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}, replaceModified: true})
	if data, _ := replaced.after.data(); replaced.Err() != nil || string(data) != "generated\n" {
		t.Errorf("replacing plan = %v, %q", replaced.Err(), data)
	}
	summary, failed := summarizeResource(t.Context(), st, reference)
	mustSucceed(t, failed)
	if summary.claims[0].Status != EditModified {
		t.Errorf("summary status = %s", summary.claims[0].Status)
	}
}

func TestPlansRefuseOverlapsMissingResourcesAndDuplicateClaims(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "guide")
	rendered, artifact := renderedFile(t, destination, "generated\n")
	st := store.NewMemoryStore()
	reference := admittedResource(t, st, artifact)
	commit(t, st, planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}}))
	other, failed := planResource(t.Context(), st, planInputs{
		reference: reference, identifier: "other", bundle: planBundle(t), rendered: rendered,
		artifacts: []RenderedArtifact{artifact}, acquisition: AcquireConflict,
		requirements: noRequirements(artifact), stableRequirements: noRequirements(artifact),
	})
	mustSucceed(t, failed)
	wantFailure(t, other.Err(), CodeConflict, "claim overlaps recorded ownership", reference.Destination)

	_, required := renderedOne(t, RenderedArtifactParams{
		Destination: filepath.Join(root, "missing.md"), Content: must(NewFileContent([]byte("x"), ModePrivate)), RequireExisting: true,
	})
	requiredRef := admittedResource(t, st, required)
	missing := planFor(t, st, requiredRef, planCase{bundle: planBundle(t), artifacts: []RenderedArtifact{required}})
	wantFailure(t, missing.Err(), CodeConflict, "resource must already exist", requiredRef.Destination)
	if !missing.RequireExisting() {
		t.Error("RequireExisting = false")
	}

	_, failed = planResource(t.Context(), st, planInputs{
		reference: reference, identifier: "team", bundle: planBundle(t), rendered: rendered,
		artifacts: []RenderedArtifact{artifact, artifact}, acquisition: AcquireConflict,
		requirements: noRequirements(artifact), stableRequirements: noRequirements(artifact),
	})
	wantFailure(t, failed, CodeInvalidState, "rendered artifacts duplicate a physical claim", reference.Destination)

	directory := filepath.Join(root, "directory")
	if err := os.Mkdir(directory, 0o700); err != nil {
		t.Fatal(err)
	}
	kind := planFor(t, st, ResourceRef{Destination: directory, Kind: ResourceDocument}, planCase{})
	wantFailure(t, kind.Err(), CodeUnsafePath, "resource kind disagrees with existing destination", directory)
}

func TestPreparedDirectoryMarkersAreNotLogicalPlaceholders(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	content := must(NewTreeContent([]BundleEntry{{Path: "SKILL.md", Data: []byte("# Review\n")}}))
	rendered, artifact := renderedOne(t, RenderedArtifactParams{
		ID: "skill", Family: FamilySkills, Destination: filepath.Join(root, "skills"), Content: content, Subtree: "team/review",
	})
	st := store.NewMemoryStore()
	reference := admittedResource(t, st, artifact)
	if reference.Kind != ResourceSkillContainer {
		t.Fatalf("kind = %s", reference.Kind)
	}
	planned := planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}})
	if planned.Err() != nil {
		t.Fatalf("plan error = %v", planned.Err())
	}
	var markers, paths []string
	for _, item := range planned.receipt.createdDirectories {
		markers = append(markers, item.path)
		if !countMatches(item.inode, 0) {
			t.Errorf("marker %q inode = %+v", item.path, item.inode)
		}
	}
	for _, item := range planned.after.nodes {
		paths = append(paths, item.path)
	}
	if diff := cmp.Diff([]string{"", "team"}, markers); diff != "" {
		t.Errorf("created directories (-want +got):\n%s", diff)
	}
	if diff := cmp.Diff([]string{"", "team", "team/review", "team/review/SKILL.md"}, paths); diff != "" {
		t.Errorf("planned nodes (-want +got):\n%s", diff)
	}
	commit(t, st, planned)
	removal := planFor(t, st, reference, planCase{})
	if len(removal.after.nodes) != 0 || len(removal.receipt.createdDirectories) != 0 {
		t.Errorf("removal keeps %d nodes and %d markers", len(removal.after.nodes), len(removal.receipt.createdDirectories))
	}
}

func TestContainerProvenanceKeepsDirectoriesOthersStillUse(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	writeFile(t, filepath.Join(root, "skills", "foreign", "SKILL.md"), "foreign")
	content := must(NewTreeContent([]BundleEntry{{Path: "SKILL.md", Data: []byte("# Review\n")}}))
	rendered, artifact := renderedOne(t, RenderedArtifactParams{
		ID: "skill", Family: FamilySkills, Destination: filepath.Join(root, "skills"), Content: content, Subtree: "team/review",
	})
	st := store.NewMemoryStore()
	reference := admittedResource(t, st, artifact)
	planned := planFor(t, st, reference, planCase{bundle: planBundle(t), rendered: rendered, artifacts: []RenderedArtifact{artifact}})
	var markers []string
	for _, item := range planned.receipt.createdDirectories {
		markers = append(markers, item.path)
	}
	if diff := cmp.Diff([]string{"team"}, markers); diff != "" {
		t.Errorf("created directories (-want +got):\n%s", diff)
	}
	commit(t, st, planned)
	removal := planFor(t, st, reference, planCase{})
	var paths []string
	for _, item := range removal.after.nodes {
		paths = append(paths, item.path)
	}
	if diff := cmp.Diff([]string{"", "foreign", "foreign/SKILL.md"}, paths); diff != "" {
		t.Errorf("removal keeps (-want +got):\n%s", diff)
	}
}

func TestIndexTransitionKernelPreservesMembership(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	rendered, artifact := renderedFile(t, filepath.Join(root, "guide"), "x")
	reference, failed := resourceOf(t.Context(), artifact)
	mustSucceed(t, failed)
	identity := must(artifactClaim(artifact).identity(reference.Destination))
	current := must(newGeneration("1", strings.Repeat("a", 64), rendered.ContentDigest(),
		[]membership{{resource: reference, claimIDs: []string{identity}}}))
	completed := must(completedIndex("team", strings.Repeat("b", 64), &current))
	pending := must(pendingIndex("team", strings.Repeat("b", 64), &completed, &current, []ResourceRef{reference}))
	if diff := cmp.Diff([]ResourceRef{reference}, pending.resources()); diff != "" {
		t.Errorf("pending resources (-want +got):\n%s", diff)
	}
	if !expectsReceipt(&completed, reference) || !expectsReceipt(&pending, reference) {
		t.Error("indexes do not expect the member's receipt")
	}
	fresh := must(pendingIndex("team", strings.Repeat("b", 64), nil, &current, []ResourceRef{reference}))
	if expectsReceipt(&fresh, reference) || expectsReceipt(nil, reference) {
		t.Error("a pending-only generation expects a receipt")
	}
}

func TestPendingRecoveryReportsAndChecksTransactionRecords(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	_, artifact := renderedFile(t, filepath.Join(root, "guide"), "x")
	st := store.NewMemoryStore()
	reference := admittedResource(t, st, artifact)
	directory := must(TransactionPath(reference.Destination, testTransaction))
	prepared := must(newPreparation(reference, testTransaction, directory, stateSchemaVersion))
	mustSucceed(t, writeRecord(t.Context(), st, preparationKey(reference), prepared, nil))
	paths, failed := pendingRecovery(t.Context(), st, reference)
	mustSucceed(t, failed)
	if diff := cmp.Diff([]string{directory}, paths); diff != "" {
		t.Errorf("recovery paths (-want +got):\n%s", diff)
	}
	planned := planFor(t, st, reference, planCase{})
	if diff := cmp.Diff([]string{directory}, planned.RecoveryPaths()); diff != "" {
		t.Errorf("plan recovery paths (-want +got):\n%s", diff)
	}

	unknown := filepath.Join(root, "unknown")
	if err := os.Mkdir(unknown, 0o700); err != nil {
		t.Fatal(err)
	}
	foreign := must(newPreparation(reference, testTransaction, unknown, stateSchemaVersion))
	mustSucceed(t, writeRecord(t.Context(), st, preparationKey(reference), foreign, &prepared))
	_, failed = pendingRecovery(t.Context(), st, reference)
	wantFailure(t, failed, CodeRecoveryNeeded, "transaction record names an unrecognized directory", "")
	if err := os.Remove(unknown); err != nil {
		t.Fatal(err)
	}
	paths, failed = pendingRecovery(t.Context(), st, reference)
	mustSucceed(t, failed)
	if diff := cmp.Diff([]string{unknown}, paths); diff != "" {
		t.Errorf("absent unrecognized recovery paths (-want +got):\n%s", diff)
	}

	mustSucceed(t, deleteRecord(t.Context(), st, preparationKey(reference), foreign))
	before := mustObserve(t, reference.Destination)
	recorded := must(newReceipt(reference, testTransaction, nil, DocumentProvenance{}, nil, stateSchemaVersion))
	orphan := must(newJournal(journal{resource: reference, receipt: recorded, before: before, staged: before, published: before, backup: before}, stateSchemaVersion))
	mustSucceed(t, writeRecord(t.Context(), st, journalKey(reference), orphan, nil))
	_, failed = pendingRecovery(t.Context(), st, reference)
	wantFailure(t, failed, CodeInvalidState, "transaction record has no matching preparation", "")
	mustSucceed(t, writeRecord(t.Context(), st, preparationKey(reference), prepared, nil))
	paths, failed = pendingRecovery(t.Context(), st, reference)
	mustSucceed(t, failed)
	if diff := cmp.Diff([]string{directory}, paths); diff != "" {
		t.Errorf("journaled recovery paths (-want +got):\n%s", diff)
	}
}

func TestStoredReceiptMustNameItsResource(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	_, artifact := renderedFile(t, filepath.Join(root, "guide"), "x")
	st := store.NewMemoryStore()
	reference := admittedResource(t, st, artifact)
	other := documentRef(t, filepath.Join(root, "other"))
	recorded := must(newReceipt(other, testTransaction, nil, DocumentProvenance{}, nil, stateSchemaVersion))
	mustSucceed(t, writeRecord(t.Context(), st, receiptKey(reference), recorded, nil))
	_, failed := storedReceipt(t.Context(), st, reference)
	wantFailure(t, failed, CodeInvalidState, "receipt resource disagrees", reference.Destination)
}

func TestResourcePlanAccessorsCopyAndValidate(t *testing.T) {
	t.Parallel()
	reference := ResourceRef{Destination: "/tmp/AGENTS.md", Kind: ResourceDocument}
	recorded := must(newReceipt(reference, testTransaction, nil, DocumentProvenance{}, nil, stateSchemaVersion))
	planned := must(newResourcePlan(ResourcePlan{
		resource: reference, receipt: recorded, transactionDirectory: "/tmp/txn", recoveryPaths: []string{"/tmp/old"},
	}))
	paths := planned.RecoveryPaths()
	paths[0] = "/changed"
	if planned.RecoveryPaths()[0] != "/tmp/old" || planned.Resource() != reference {
		t.Error("accessors expose internal state")
	}
	failed := planned.withError(failure(CodeConflict, "x", "/tmp"))
	failed.Err().Message = "changed"
	if failed.Err().Message != "x" || planned.Err() != nil {
		t.Error("Err exposes internal state")
	}
	other := ResourceRef{Destination: "/tmp/other", Kind: ResourceDocument}
	if _, err := newResourcePlan(ResourcePlan{resource: other, receipt: recorded, transactionDirectory: "/tmp/txn"}); err == nil || err.Error() != "planned receipts disagree with resource" {
		t.Errorf("mismatched receipt = %v", err)
	}
	if _, err := newResourcePlan(ResourcePlan{resource: reference, receipt: recorded, transactionDirectory: "txn"}); err == nil || err.Error() != "expected an absolute concrete path" {
		t.Errorf("relative directory = %v", err)
	}
	moved := planned.withAncestors([]ancestor{{path: "/tmp"}})
	if len(moved.ancestors) != 1 || len(planned.ancestors) != 0 {
		t.Error("withAncestors changed the original plan")
	}
}

func TestDestinationOrderSortsByPosixSpelling(t *testing.T) {
	t.Parallel()
	refs := []ResourceRef{
		{Destination: "/b/x", Kind: ResourceDocument},
		{Destination: "/a/z", Kind: ResourceTree},
		{Destination: "/a/y", Kind: ResourceDocument},
	}
	ordered := destinationOrder(refs)
	var got []string
	for _, reference := range ordered {
		got = append(got, reference.Destination)
	}
	if !slices.Equal(got, []string{"/a/y", "/a/z", "/b/x"}) || refs[0].Destination != "/b/x" {
		t.Errorf("destination order = %v; input %v", got, refs)
	}
}
