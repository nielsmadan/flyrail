//go:build integration && (darwin || linux)

package flyrail

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/seams"
)

const transactionWorkerEnv = "FLYRAIL_TRANSACTION_WORKER"

// Signals readiness on the inherited descriptor 3.
func parkAt(label string) *seams.Hooks {
	return &seams.Hooks{Observe: func(event seams.Event) {
		if eventLabel(event) != label {
			return
		}
		ready := os.NewFile(3, "ready")
		_, _ = ready.WriteString("ready\n")
		_ = ready.Close()
		// A bare select{} trips the runtime's deadlock detector, which exits and releases the locks.
		for {
			time.Sleep(time.Hour)
		}
	}}
}

func workerPlan(ctx context.Context, st store.Store, reference ResourceRef, text string) (ResourcePlan, *TargetError) {
	before, failed := observe(reference.Destination)
	if failed != nil {
		return ResourcePlan{}, failed
	}
	previous, failed := readReceipt(ctx, st, reference)
	if failed != nil {
		return ResourcePlan{}, failed
	}
	next, err := newReceipt(reference, newTransactionID(), nil, DocumentProvenance{}, nil, stateSchemaVersion)
	if err != nil {
		return ResourcePlan{}, invalidRecord(err, reference.Destination)
	}
	directory, failed := chooseTransactionDirectory(st, reference.Destination, next.transactionID)
	if failed != nil {
		return ResourcePlan{}, failed
	}
	captured, failed := resourceAncestors(reference.Destination, pathx.Join(directory, "staging"))
	if failed != nil {
		return ResourcePlan{}, failed
	}
	planned, err := newPlannedNode("", []byte(text), true, 0o600)
	if err != nil {
		return ResourcePlan{}, invalidRecord(err, reference.Destination)
	}
	after, err := newRevision([]node{planned})
	if err != nil {
		return ResourcePlan{}, invalidRecord(err, reference.Destination)
	}
	plan, err := newResourcePlan(ResourcePlan{resource: reference, before: before, after: after, previous: previous, receipt: next, ancestors: captured, transactionDirectory: directory})
	if err != nil {
		return ResourcePlan{}, invalidRecord(err, reference.Destination)
	}
	return plan, nil
}

func runTransactionWorker(arguments []string) int {
	if len(arguments) != 5 || arguments[3] != "transact" && arguments[3] != "recover" {
		fmt.Fprintln(os.Stderr, "worker requires state, destination, label, mode and text")
		return 64
	}
	state, destination, label, mode, text := arguments[0], arguments[1], arguments[2], arguments[3], arguments[4]
	ctx := context.Background()
	st, err := store.NewDirectoryStore(ctx, state)
	if err != nil {
		fmt.Fprintln(os.Stderr, "worker store:", err)
		return 2
	}
	authority, err := NewResourceAuthority(ctx, destination)
	if err != nil {
		fmt.Fprintln(os.Stderr, "worker authority:", err)
		return 2
	}
	reference := ResourceRef{Destination: authority.Destination(), Kind: ResourceDocument}
	locks, failed := acquireTransactionLocks(ctx, st, testIndexKey, []ResourceRef{reference}, 10*time.Second, nil)
	if failed != nil {
		fmt.Fprintln(os.Stderr, "worker locks:", failed)
		return 2
	}
	defer func() { _ = locks.Unlock() }()
	hooked := seams.With(ctx, parkAt(label))
	if mode == "recover" {
		failed = recoverResource(hooked, st, locks.resources[0])
	} else {
		failed = workerTransaction(hooked, st, locks, reference, text)
	}
	fmt.Fprintln(os.Stderr, "worker finished without reaching", label, failed)
	return 3
}

func workerTransaction(ctx context.Context, st store.Store, locks *transactionLocks, reference ResourceRef, text string) *TargetError {
	plan, failed := workerPlan(ctx, st, reference, text)
	if failed != nil {
		return failed
	}
	previous, ok, failed := readRecord(ctx, st, testIndexKey, "Index", indexFromState)
	if failed != nil {
		return failed
	}
	var stored *index
	if ok {
		stored = &previous
	}
	pending, err := pendingIndex("team", testContextDigest, stored, nil, []ResourceRef{reference})
	if err != nil {
		return invalidRecord(err, "")
	}
	completed, err := completedIndex("team", testContextDigest, nil)
	if err != nil {
		return invalidRecord(err, "")
	}
	outcome, failed := executeTransaction(ctx, locks, []ResourcePlan{plan}, stored, pending, completed)
	if failed != nil {
		return failed
	}
	return outcome.err
}

func killAtSeam(t *testing.T, st store.Store, root, label, mode, text string) {
	t.Helper()
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	reader, writer, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = reader.Close() }()
	destination := filepath.Join(root, "AGENTS.md")
	arguments, err := json.Marshal([]string{filepath.Join(root, "state"), destination, label, mode, text})
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	command := exec.CommandContext(ctx, executable, "-test.run=^$")
	command.Env = append(os.Environ(), transactionWorkerEnv+"="+string(arguments))
	command.ExtraFiles = []*os.File{writer}
	var stderr strings.Builder
	command.Stderr = &stderr
	if err := command.Start(); err != nil {
		cancel()
		t.Fatal(err)
	}
	_ = writer.Close()
	var exited error
	done := make(chan struct{})
	go func() {
		exited = command.Wait()
		close(done)
	}()
	t.Cleanup(func() {
		cancel()
		<-done
	})
	line := make(chan string, 1)
	go func() {
		text, _ := bufio.NewReader(reader).ReadString('\n')
		line <- text
	}()
	select {
	case text := <-line:
		if text != "ready\n" {
			<-done
			t.Fatalf("worker did not park at %s: %q %s", label, text, stderr.String())
		}
	case <-time.After(time.Minute):
		t.Fatalf("worker did not park at %s within a minute", label)
	}
	reference := documentRef(t, destination)
	if _, failed := acquireResourceLock(t.Context(), st, reference, 0); failed == nil || failed.Code != CodeBusy {
		t.Fatalf("the parked worker does not hold the resource lock: %v", failed)
	}
	if err := command.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	<-done
	if exited == nil {
		t.Fatal("the worker exited cleanly instead of being killed")
	}
}

func killedRoot(t *testing.T) (string, store.Store, ResourceRef) {
	t.Helper()
	root := tempRoot(t)
	st := directoryStore(t, filepath.Join(root, "state"))
	reference := admitted(t, st, documentRef(t, filepath.Join(root, "AGENTS.md")))
	setup := newTransaction(t, st, testPlan(t, st, reference, revision{}, fileRevision(t, "one\n"), nil, testReceipt(t, reference, '0')))
	if outcome := setup.execute(t.Context(), t); outcome.err != nil {
		t.Fatal(outcome.err)
	}
	setup.finish(t)
	return root, st, reference
}

func recoverAfterKill(t *testing.T, st store.Store, reference ResourceRef) {
	t.Helper()
	locks, failed := acquireTransactionLocks(t.Context(), st, testIndexKey, []ResourceRef{reference}, 10*time.Second, nil)
	mustSucceed(t, failed)
	mustSucceed(t, recoverResource(t.Context(), st, locks.resources[0]))
	if err := locks.Unlock(); err != nil {
		t.Fatal(err)
	}
}

func assertKilledRecords(t *testing.T, st store.Store, reference ResourceRef, setupIndex *index, committed bool) {
	t.Helper()
	previous := testReceipt(t, reference, '0')
	got := receiptOf(t, st, reference)
	if got == nil {
		t.Fatal("the store holds no receipt")
	}
	if committed {
		if got.transactionID == previous.transactionID {
			t.Errorf("receipt still names the previous transaction %s", got.transactionID)
		}
		if got.resource != reference {
			t.Errorf("receipt resource = %+v, want %+v", got.resource, reference)
		}
	} else if !sameOptionalReceipt(stripCreated(got), stripCreated(&previous)) {
		t.Errorf("receipt = %+v, want the previous receipt %+v", got, previous)
	}
	wantIndex := must(pendingIndex("team", testContextDigest, setupIndex, nil, []ResourceRef{reference}))
	gotIndex := storedIndex(t, st)
	if gotIndex == nil || !sameRecord(*gotIndex, wantIndex) {
		t.Errorf("index = %+v, want the pending index %+v", gotIndex, wantIndex)
	}
}

func TestATransactionKilledAtEverySeamConvergesAfterRecovery(t *testing.T) {
	t.Parallel()
	labels := cleanRun(t, transactionShapes[0])
	for _, label := range labels {
		t.Run(label, func(t *testing.T) {
			t.Parallel()
			root, st, reference := killedRoot(t)
			setupIndex := storedIndex(t, st)
			killAtSeam(t, st, root, label, "transact", "two\n")
			recoverAfterKill(t, st, reference)
			want := "one\n"
			committed := committedAfter(labels, label)
			if committed {
				want = "two\n"
			}
			if got := readText(t, reference.Destination); got != want {
				t.Errorf("destination = %q, want %q", got, want)
			}
			assertKilledRecords(t, st, reference, setupIndex, committed)
			assertSettled(t, st, reference)
		})
	}
}

func TestARecoveryKilledDuringRollbackConvergesOnTheNextRecovery(t *testing.T) {
	t.Parallel()
	for _, label := range []string{"rename into staging done=true", "rename out of backup done=false", seams.Rollback, "unlink in staging"} {
		t.Run(label, func(t *testing.T) {
			t.Parallel()
			root, st, reference := killedRoot(t)
			setupIndex := storedIndex(t, st)
			killAtSeam(t, st, root, seams.PublishReceipt, "transact", "two\n")
			killAtSeam(t, st, root, label, "recover", "")
			recoverAfterKill(t, st, reference)
			if got := readText(t, reference.Destination); got != "one\n" {
				t.Errorf("destination = %q, want the original restored", got)
			}
			assertKilledRecords(t, st, reference, setupIndex, false)
			assertSettled(t, st, reference)
		})
	}
}

func TestTransactionWorkerArgumentsAreChecked(t *testing.T) {
	t.Parallel()
	if code := runTransactionWorker(nil); code != 64 {
		t.Errorf("runTransactionWorker(nil) = %d", code)
	}
	if code := runTransactionWorker([]string{"s", "d", "l", "other", ""}); code != 64 {
		t.Errorf("runTransactionWorker with a bad mode = %d", code)
	}
}
