//go:build darwin || linux

package flyrail

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/seams"
)

const foreignEdit = "foreign edit\n"

type shape struct {
	name    string
	tree    bool
	replace bool
	adopt   bool
}

var transactionShapes = []shape{
	{name: "replace-document", replace: true},
	{name: "create-document"},
	{name: "replace-tree", tree: true, replace: true},
	{name: "create-tree", tree: true},
	{name: "adopt-document", adopt: true},
}

func shapeRevision(t *testing.T, s shape, text string) revision {
	t.Helper()
	if s.tree {
		return plannedTree(t, BundleEntry{Path: "SKILL.md", Data: []byte(text)})
	}
	return fileRevision(t, text)
}

func shapeReference(t *testing.T, s shape, root string) ResourceRef {
	t.Helper()
	if s.tree {
		return treeRef(t, filepath.Join(root, "skill"))
	}
	return documentRef(t, filepath.Join(root, "AGENTS.md"))
}

func shapedPlan(t *testing.T, st store.Store, root string, s shape) (ResourceRef, ResourcePlan) {
	t.Helper()
	reference := admitted(t, st, shapeReference(t, s, root))
	var previous *receipt
	switch {
	case s.replace:
		_, failed := writeRevision(reference.Destination, shapeRevision(t, s, "one\n"))
		mustSucceed(t, failed)
		old := testReceipt(t, reference, '0')
		putReceipt(t, st, reference, old)
		previous = &old
	case s.adopt:
		exclusiveWrite(t, reference.Destination, "adopted\n", 0o600)
	}
	before := mustObserve(t, reference.Destination)
	after := shapeRevision(t, s, "two\n")
	if s.adopt {
		after = before
	}
	return reference, testPlan(t, st, reference, before, after, previous, testReceipt(t, reference, '1'))
}

func eventLabel(event seams.Event) string {
	switch event.Kind {
	case seams.KindBoundary:
		return event.Boundary
	case seams.KindRename:
		area := "out of " + filepath.Base(filepath.Dir(event.Source))
		if base := filepath.Base(filepath.Dir(event.Destination)); base == "staging" || base == "backup" {
			area = "into " + base
		}
		return fmt.Sprintf("rename %s done=%v", area, event.Done)
	}
	return "unlink in " + filepath.Base(filepath.Dir(event.Path))
}

func cleanRun(t *testing.T, s shape) []string {
	t.Helper()
	st := store.NewMemoryStore()
	_, plan := shapedPlan(t, st, tempRoot(t), s)
	var labels []string
	hooks := &seams.Hooks{Observe: func(event seams.Event) {
		if label := eventLabel(event); !slices.Contains(labels, label) {
			labels = append(labels, label)
		}
	}}
	outcome := newTransaction(t, st, plan).execute(seams.With(t.Context(), hooks), t)
	if outcome.err != nil {
		t.Fatalf("%s clean run: %v", s.name, outcome.err)
	}
	return labels
}

func committedAfter(labels []string, label string) bool {
	return slices.Index(labels, label) > slices.Index(labels, seams.Receipt)
}

func assertConverged(t *testing.T, st store.Store, plan ResourcePlan, committed bool) {
	t.Helper()
	current := mustObserve(t, plan.resource.Destination)
	switch {
	case committed && !slices.Equal(intendedNodes(current), intendedNodes(plan.after)):
		t.Errorf("destination %+v is not the published revision", intendedNodes(current))
	case !committed && !current.equal(plan.before):
		t.Errorf("destination %+v is not the original revision", intendedNodes(current))
	}
	want := plan.previous
	if committed {
		want = &plan.receipt
	}
	if got := receiptOf(t, st, plan.resource); !sameOptionalReceipt(stripCreated(got), stripCreated(want)) {
		t.Errorf("receipt = %+v, want %+v", got, want)
	}
	assertSettled(t, st, plan.resource)
}

func stripCreated(recorded *receipt) *receipt {
	if recorded == nil {
		return nil
	}
	copied := *recorded
	copied.createdDirectories = nil
	return &copied
}

func faultOnce(boundary string) *seams.Hooks {
	fired := false
	return &seams.Hooks{Fault: func(selected string) bool {
		if fired || selected != boundary {
			return false
		}
		fired = true
		return true
	}}
}

func TestACleanReplacementFiresEveryPublicationBoundary(t *testing.T) {
	t.Parallel()
	labels := cleanRun(t, transactionShapes[0])
	all := []string{
		seams.ResourcePrecondition, seams.IndexRequire, seams.GuardLookup, seams.Preparation, seams.Directory, seams.Staged,
		seams.Journal, seams.Prepare, seams.BackupMove, seams.Backup, seams.Rename, seams.Staging, seams.Publish,
		seams.PublishReceipt, seams.Receipt, seams.ReceiptReread, seams.ReceiptCommitted, seams.Cleanup, seams.IndexCompletion,
	}
	var boundaries []string
	for _, label := range labels {
		if !strings.Contains(label, " ") {
			boundaries = append(boundaries, label)
		}
	}
	if !slices.Equal(boundaries, all) {
		t.Errorf("boundaries = %v, want %v", boundaries, all)
	}
}

func TestEveryBoundaryFaultConverges(t *testing.T) {
	t.Parallel()
	for _, s := range transactionShapes {
		labels := cleanRun(t, s)
		for _, label := range labels {
			if strings.Contains(label, " ") {
				continue
			}
			t.Run(s.name+"/"+label, func(t *testing.T) {
				t.Parallel()
				st := store.NewMemoryStore()
				reference, plan := shapedPlan(t, st, tempRoot(t), s)
				setup := newTransaction(t, st, plan)
				outcome := setup.execute(seams.With(t.Context(), faultOnce(label)), t)
				wantFailure(t, outcome.err, CodeIOError, "injected "+label+" fault", "")
				committed := committedAfter(labels, label)
				if got := slices.Contains(outcome.completed, reference); got != committed {
					t.Errorf("completed %v, want committed %v", outcome.completed, committed)
				}
				if got := setup.storedIndex(t); got == nil || !sameRecord(*got, setup.pending) {
					t.Errorf("index = %+v, want pending", got)
				}
				mustSucceed(t, recoverResource(t.Context(), st, setup.locks.resources[0]))
				assertConverged(t, st, plan, committed)
			})
		}
	}
}

func TestACrashAtEverySeamConverges(t *testing.T) {
	t.Parallel()
	for _, s := range transactionShapes {
		labels := cleanRun(t, s)
		for _, label := range labels {
			t.Run(s.name+"/"+label, func(t *testing.T) {
				t.Parallel()
				st := store.NewMemoryStore()
				_, plan := shapedPlan(t, st, tempRoot(t), s)
				setup := newTransaction(t, st, plan)
				hooks := crashWhen(func(event seams.Event) bool { return eventLabel(event) == label })
				interrupted(t, func() { setup.execute(seams.With(t.Context(), hooks), t) })
				mustSucceed(t, recoverResource(t.Context(), st, setup.locks.resources[0]))
				mustSucceed(t, recoverResource(t.Context(), st, setup.locks.resources[0]))
				assertConverged(t, st, plan, committedAfter(labels, label))
			})
		}
	}
}

func TestRepeatedInterruptionOfRecoveryConverges(t *testing.T) {
	t.Parallel()
	starts := []struct {
		label     string
		committed bool
	}{
		{seams.Staged, false},
		{seams.Backup, false},
		{"rename out of staging done=true", false},
		{seams.PublishReceipt, false},
		{seams.ReceiptCommitted, true},
		{"unlink in backup", true},
		{"unlink in resource", true},
	}
	for _, s := range transactionShapes[:4] {
		labels := cleanRun(t, s)
		for _, start := range starts {
			if !slices.Contains(labels, start.label) {
				continue
			}
			t.Run(s.name+"/"+start.label, func(t *testing.T) {
				t.Parallel()
				st := store.NewMemoryStore()
				_, plan := shapedPlan(t, st, tempRoot(t), s)
				setup := newTransaction(t, st, plan)
				hooks := crashWhen(func(event seams.Event) bool { return eventLabel(event) == start.label })
				interrupted(t, func() { setup.execute(seams.With(t.Context(), hooks), t) })
				guard := setup.locks.resources[0]
				for round := 0; ; round++ {
					if round > 64 {
						t.Fatal("recovery never ran to completion")
					}
					seen := 0
					hooks := crashWhen(func(seams.Event) bool {
						seen++
						return seen > round
					})
					finished := func() (finished bool) {
						defer func() {
							if value := recover(); value != nil {
								if _, ok := value.(interruption); !ok {
									panic(value)
								}
							}
						}()
						mustSucceed(t, recoverResource(seams.With(t.Context(), hooks), st, guard))
						return true
					}()
					if finished {
						break
					}
				}
				assertConverged(t, st, plan, start.committed)
			})
		}
	}
}

func TestAJournalRepeatingThePreviousReceiptIsRefusedWithEvidenceRetained(t *testing.T) {
	t.Parallel()
	st := store.NewMemoryStore()
	destination := filepath.Join(tempRoot(t), "AGENTS.md")
	exclusiveWrite(t, destination, "drifted\n", 0o600)
	reference := admitted(t, st, documentRef(t, destination))
	old := testReceipt(t, reference, '0')
	putReceipt(t, st, reference, old)
	plan := testPlan(t, st, reference, mustObserve(t, destination), fileRevision(t, "repaired\n"), &old, old)
	setup := newTransaction(t, st, plan)
	interrupted(t, func() { setup.execute(seams.With(t.Context(), crashWhen(atBoundary(seams.Backup))), t) })
	journalRecord := storeValue(t, st, journalKey(reference))
	staged, backup := transactionPaths(plan.transactionDirectory)
	stagedBefore, backupBefore := mustObserve(t, staged), mustObserve(t, backup)
	failed := recoverResource(t.Context(), st, setup.locks.resources[0])
	wantFailure(t, failed, CodeRecoveryNeeded, "transaction record repeats the previous receipt", plan.transactionDirectory)
	if exists(t, destination) {
		t.Error("recovery restored the destination")
	}
	if !mustObserve(t, staged).equal(stagedBefore) || !mustObserve(t, backup).equal(backupBefore) {
		t.Error("recovery changed the retained evidence")
	}
	if got := storeValue(t, st, journalKey(reference)); !bytes.Equal(got, journalRecord) {
		t.Error("recovery changed the journal")
	}
}

func directoryStore(t *testing.T, path string) store.Store {
	t.Helper()
	st := must(store.NewDirectoryStore(t.Context(), path))
	_, failed := createStoreParents(st, nil)
	mustSucceed(t, failed)
	return st
}

func installed(t *testing.T, root, location string) (store.Store, ResourceRef) {
	t.Helper()
	project := filepath.Join(root, "project")
	mkdir(t, project)
	destination := filepath.Join(project, "AGENTS.md")
	exclusiveWrite(t, destination, "# Foreign\n", 0o600)
	var st store.Store = store.NewMemoryStore()
	if location == "store" {
		st = directoryStore(t, filepath.Join(root, "state"))
	}
	reference := admitted(t, st, documentRef(t, destination))
	plan := testPlan(t, st, reference, mustObserve(t, destination), fileRevision(t, "one\n"), nil, testReceipt(t, reference, '0'))
	setup := newTransaction(t, st, plan)
	if outcome := setup.execute(t.Context(), t); outcome.err != nil {
		t.Fatal(outcome.err)
	}
	setup.finish(t)
	return st, reference
}

func nextPlan(t *testing.T, st store.Store, reference ResourceRef, text string) ResourcePlan {
	t.Helper()
	return testPlan(t, st, reference, mustObserve(t, reference.Destination), fileRevision(t, text), receiptOf(t, st, reference), must(newReceipt(reference, newTransactionID(), nil, DocumentProvenance{}, nil, stateSchemaVersion)))
}

func TestRecoveryRefusesADestinationReplacedAfterPreparation(t *testing.T) {
	t.Parallel()
	for _, replaced := range []string{"published", "previous"} {
		for _, location := range []string{"store", "beside"} {
			t.Run(replaced+"/"+location, func(t *testing.T) {
				t.Parallel()
				root := tempRoot(t)
				st, reference := installed(t, root, location)
				destination := reference.Destination
				original := readText(t, destination)
				plan := nextPlan(t, st, reference, "two\n")
				if inStore := strings.HasPrefix(plan.transactionDirectory, filepath.Join(root, "state")); inStore != (location == "store") {
					t.Fatalf("transaction directory %s for location %s", plan.transactionDirectory, location)
				}
				setup := newTransaction(t, st, plan)
				when := renameOutOf("staging", true)
				if replaced == "previous" {
					when = atBoundary(seams.Backup)
				}
				interrupted(t, func() { setup.execute(seams.With(t.Context(), crashWhen(when)), t) })
				if replaced == "previous" {
					_, backup := transactionPaths(plan.transactionDirectory)
					exclusiveWrite(t, destination, readText(t, backup), 0o600)
				}
				laterThanBoundary(t, destination, plan.transactionDirectory)
				current := readText(t, destination)
				failed := recoverResource(t.Context(), st, setup.locks.resources[0])
				message := "destination revision changed after preparation"
				if replaced == "previous" {
					message = "unrecognized published revision"
				}
				wantFailure(t, failed, CodeRecoveryNeeded, message, destination)
				if got := readText(t, destination); got != current {
					t.Errorf("destination = %q, want %q retained", got, current)
				}
				if replaced == "previous" && current != original {
					t.Errorf("replacement = %q, want the original %q", current, original)
				}
				if !exists(t, plan.transactionDirectory) || !hasRecord(t, st, journalKey(reference)) {
					t.Error("evidence was not retained")
				}
			})
		}
	}
}

func TestRecoveryAcceptsItsOwnUntouchedRevisions(t *testing.T) {
	t.Parallel()
	points := map[string]func(seams.Event) bool{
		"before-publication": atBoundary(seams.Prepare),
		"after-backup-move":  atBoundary(seams.Backup),
		"after-publish":      renameOutOf("staging", true),
	}
	for point, when := range points {
		for _, location := range []string{"store", "beside"} {
			t.Run(point+"/"+location, func(t *testing.T) {
				t.Parallel()
				st, reference := installed(t, tempRoot(t), location)
				original := readText(t, reference.Destination)
				setup := newTransaction(t, st, nextPlan(t, st, reference, "two\n"))
				interrupted(t, func() { setup.execute(seams.With(t.Context(), crashWhen(when)), t) })
				mustSucceed(t, recoverResource(t.Context(), st, setup.locks.resources[0]))
				setup.finish(t)
				if got := readText(t, reference.Destination); got != original {
					t.Errorf("restored %q, want %q", got, original)
				}
				retry := newTransaction(t, st, nextPlan(t, st, reference, "two\n"))
				if outcome := retry.execute(t.Context(), t); outcome.err != nil {
					t.Fatal(outcome.err)
				}
				if got := readText(t, reference.Destination); got != "two\n" {
					t.Errorf("retried %q", got)
				}
				assertSettled(t, st, reference)
			})
		}
	}
}

func appendForeign(t *testing.T, destination string, tree bool) {
	t.Helper()
	path := destination
	if tree {
		if !exists(t, destination) {
			mkdir(t, destination)
		}
		path = filepath.Join(destination, "foreign")
	}
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_APPEND|os.O_CREATE, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := file.WriteString(foreignEdit); err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
}

func filesBelow(t *testing.T, path string) []string {
	t.Helper()
	info, err := os.Lstat(path)
	switch {
	case err != nil:
		return nil
	case info.Mode().IsRegular():
		return []string{path}
	case !info.IsDir():
		return nil
	}
	entries, err := os.ReadDir(path)
	if err != nil {
		t.Fatal(err)
	}
	var files []string
	for _, entry := range entries {
		files = append(files, filesBelow(t, filepath.Join(path, entry.Name()))...)
	}
	return files
}

func foreignRetained(t *testing.T, st store.Store, destination string) bool {
	t.Helper()
	candidates := filesBelow(t, destination)
	for _, directory := range managementEntries(t, st, destination) {
		for _, area := range []string{"staging", "backup"} {
			candidates = append(candidates, filesBelow(t, filepath.Join(directory, area, "resource"))...)
		}
	}
	return slices.ContainsFunc(candidates, func(path string) bool {
		data, err := os.ReadFile(path)
		return err == nil && bytes.Contains(data, []byte(foreignEdit))
	})
}

func TestForeignEditsDuringPublicationAreNeverLost(t *testing.T) {
	t.Parallel()
	points := map[string]func(seams.Event) bool{
		"before-backup-move":  atBoundary(seams.BackupMove),
		"during-backup-move":  renameInto("backup", false),
		"before-publish-move": renameOutOf("staging", false),
		"after-publish-move":  renameOutOf("staging", true),
		"before-receipt":      atBoundary(seams.PublishReceipt),
		"after-receipt":       atBoundary(seams.ReceiptCommitted),
		"rollback-unpublish":  renameInto("staging", false),
		"rollback-restore":    renameOutOf("backup", false),
	}
	for _, s := range transactionShapes[:4] {
		for point, when := range points {
			if !s.replace && (strings.Contains(point, "backup") || point == "rollback-restore") {
				continue
			}
			t.Run(s.name+"/"+point, func(t *testing.T) {
				t.Parallel()
				root := tempRoot(t)
				st := directoryStore(t, filepath.Join(root, "state"))
				reference := admitted(t, st, shapeReference(t, s, root))
				if s.replace {
					first := newTransaction(t, st, testPlan(t, st, reference, mustObserve(t, reference.Destination), shapeRevision(t, s, "one\n"), nil, testReceipt(t, reference, '0')))
					if outcome := first.execute(t.Context(), t); outcome.err != nil {
						t.Fatal(outcome.err)
					}
					first.finish(t)
				}
				plan := testPlan(t, st, reference, mustObserve(t, reference.Destination), shapeRevision(t, s, "two\n"), receiptOf(t, st, reference), testReceipt(t, reference, '1'))
				edits := 0
				hooks := &seams.Hooks{Observe: func(event seams.Event) {
					if edits == 0 && when(event) {
						edits++
						appendForeign(t, reference.Destination, s.tree)
					}
				}}
				if strings.HasPrefix(point, "rollback") {
					hooks.Fault = func(boundary string) bool { return boundary == seams.PublishReceipt }
				}
				outcome := newTransaction(t, st, plan).execute(seams.With(t.Context(), hooks), t)
				if edits != 1 {
					t.Fatalf("edits = %d: %+v", edits, outcome)
				}
				if !foreignRetained(t, st, reference.Destination) {
					t.Errorf("the foreign edit was lost: %+v", outcome)
				}
			})
		}
	}
}

func heldDescriptorWrite(t *testing.T, write func(seams.Event) bool) (kept bool, outcome transactionOutcome) {
	t.Helper()
	root := tempRoot(t)
	st := directoryStore(t, filepath.Join(root, "state"))
	destination := filepath.Join(root, "AGENTS.md")
	reference := admitted(t, st, documentRef(t, destination))
	first := newTransaction(t, st, testPlan(t, st, reference, revision{}, fileRevision(t, "one\n"), nil, testReceipt(t, reference, '0')))
	if outcome := first.execute(t.Context(), t); outcome.err != nil {
		t.Fatal(outcome.err)
	}
	first.finish(t)
	var held *os.File
	written := false
	hooks := &seams.Hooks{Observe: func(event seams.Event) {
		switch {
		case held == nil && !written && atBoundary(seams.BackupMove)(event):
			var err error
			if held, err = os.OpenFile(destination, os.O_WRONLY|os.O_APPEND, 0); err != nil {
				t.Fatal(err)
			}
		case held != nil && write(event):
			if _, err := held.WriteString(foreignEdit); err != nil {
				t.Fatal(err)
			}
			if err := held.Close(); err != nil {
				t.Fatal(err)
			}
			held, written = nil, true
		}
	}}
	plan := testPlan(t, st, reference, mustObserve(t, destination), fileRevision(t, "two\n"), receiptOf(t, st, reference), testReceipt(t, reference, '1'))
	outcome = newTransaction(t, st, plan).execute(seams.With(t.Context(), hooks), t)
	if !written {
		t.Fatalf("the held descriptor was not written: %+v", outcome)
	}
	return foreignRetained(t, st, destination), outcome
}

func TestADescriptorHeldAcrossTheBackupMoveKeepsAWriteBeforeTheReceipt(t *testing.T) {
	t.Parallel()
	kept, outcome := heldDescriptorWrite(t, atBoundary(seams.PublishReceipt))
	if !kept {
		t.Errorf("the held write was lost: %+v", outcome)
	}
}

func TestPinsKnownWindowAHeldDescriptorWriteBeforeTheBackupUnlinkIsLost(t *testing.T) {
	t.Parallel()
	kept, _ := heldDescriptorWrite(t, func(event seams.Event) bool {
		return event.Kind == seams.KindUnlink && parentIs(event.Path, "backup")
	})
	if kept {
		t.Error("the documented descriptor window closed; update spec/transaction-protocol.md")
	}
}

func TestAFailedStagingDoesNotBlockTheNextTransaction(t *testing.T) {
	t.Parallel()
	st, reference := installed(t, tempRoot(t), "store")
	setup := newTransaction(t, st, nextPlan(t, st, reference, "two\n"))
	outcome := setup.execute(seams.With(t.Context(), faultOnce(seams.Staged)), t)
	wantFailure(t, outcome.err, CodeIOError, "injected staged fault", "")
	setup.finish(t)
	assertSettled(t, st, reference)
	retry := newTransaction(t, st, nextPlan(t, st, reference, "two\n"))
	if outcome := retry.execute(t.Context(), t); outcome.err != nil {
		t.Fatal(outcome.err)
	}
	if got := readText(t, reference.Destination); got != "two\n" {
		t.Errorf("destination = %q", got)
	}
}
