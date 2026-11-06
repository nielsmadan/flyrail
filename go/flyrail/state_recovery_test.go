//go:build darwin || linux

package flyrail

import (
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

const repaired = "# mine\n\n<!-- flyrail:guide:start -->\none\n<!-- flyrail:guide:end -->\n"

func inode(t *testing.T, path string) uint64 {
	t.Helper()
	info, err := os.Lstat(path)
	if err != nil {
		t.Fatal(err)
	}
	return info.Sys().(*syscall.Stat_t).Ino
}

func TestADriftRepairCommitsAFreshTransactionAndANoopKeepsTheReceipt(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	destination := filepath.Join(root, "AGENTS.md")
	writeFile(t, destination, "# mine\n")
	target := targetOf(t, openStore(t, filepath.Join(root, "index")))
	appliedSync(t, teamBundle(t, "1"), sectionRendered(t, destination, "one"), target)
	writeFile(t, destination, strings.ReplaceAll(readText(t, destination), "one", "EDITED"))
	reference := documentRef(t, destination)
	drifted := receiptOf(t, target.Store(), reference)

	repairedResult := syncWith(t.Context(), t, teamBundle(t, "1"), sectionRendered(t, destination, "one"), target, SyncOptions{ReplaceModified: true, LockTimeout: testLockTimeout})

	wantStatus(t, "repair", repairedResult, StatusApplied)
	if repairedResult.Resources[0].Status != StatusApplied {
		t.Errorf("resource status = %s", repairedResult.Resources[0].Status)
	}
	if got := readText(t, destination); got != repaired {
		t.Errorf("destination = %q", got)
	}
	current := receiptOf(t, target.Store(), reference)
	if current.transactionID == drifted.transactionID {
		t.Error("the repair reused the drifted transaction id")
	}
	if !current.equivalentExceptTransaction(*drifted) {
		t.Error("the repair changed the receipt beyond its transaction id")
	}
	wantStatus(t, "noop", syncOnce(t, teamBundle(t, "1"), sectionRendered(t, destination, "one"), target), StatusUnchanged)
	if !sameOptionalReceipt(receiptOf(t, target.Store(), reference), current) {
		t.Error("a no-op changed the receipt")
	}
}

func TestRecoveryRefusesThePreviousRevisionRewrittenInPlaceAfterPreparation(t *testing.T) {
	t.Parallel()
	for _, location := range []string{"store", "beside"} {
		root := tempRoot(t)
		project := filepath.Join(root, "project")
		mkdir(t, project)
		destination := filepath.Join(project, "AGENTS.md")
		writeFile(t, destination, "# Foreign\n")
		inner := store.Store(store.NewMemoryStore())
		if location == "store" {
			inner = openStore(t, filepath.Join(root, "state"))
		}
		st := hooked(inner)
		target := targetOf(t, st)
		appliedSync(t, teamBundle(t, "1"), sectionRendered(t, destination, "one"), target)
		original := readText(t, destination)
		before := inode(t, destination)
		crashed(t, st, crashPoint{"after-journal", opWrite, true, "/journal"}, func() {
			syncOnce(t, teamBundle(t, "2"), sectionRendered(t, destination, "two"), target)
		})
		directory := journalDirectory(t, st, destination)
		stream, err := os.OpenFile(destination, os.O_WRONLY|os.O_TRUNC, 0)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := stream.WriteString(original); err != nil {
			t.Fatal(err)
		}
		if err := stream.Close(); err != nil {
			t.Fatal(err)
		}
		info, err := os.Lstat(directory)
		if err != nil {
			t.Fatal(err)
		}
		later := info.ModTime().Add(time.Minute)
		if err := os.Chtimes(destination, later, later); err != nil {
			t.Fatal(err)
		}
		if inode(t, destination) != before {
			t.Fatal("the rewrite replaced the file")
		}

		result := recoverAll(t, target)

		wantStatus(t, location, result, StatusIncomplete)
		wantFailure(t, result.Err, CodeRecoveryNeeded, "destination revision changed after preparation", destination)
		if readText(t, destination) != original {
			t.Errorf("%s: the destination changed", location)
		}
		if !exists(t, directory) || !hasRecord(t, st, journalKey(documentRef(t, destination))) {
			t.Errorf("%s: the transaction evidence was discarded", location)
		}
	}
}
