//go:build integration && (darwin || linux)

package flyrail

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

const admissionWorkerEnv = "FLYRAIL_ADMISSION_WORKER"

func TestMain(m *testing.M) {
	if encoded, ok := os.LookupEnv(admissionWorkerEnv); ok {
		var arguments []string
		if err := json.Unmarshal([]byte(encoded), &arguments); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(64)
		}
		os.Exit(runAdmissionWorker(arguments))
	}
	if encoded, ok := os.LookupEnv(transactionWorkerEnv); ok {
		var arguments []string
		if err := json.Unmarshal([]byte(encoded), &arguments); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(64)
		}
		os.Exit(runTransactionWorker(arguments))
	}
	if encoded, ok := os.LookupEnv(configurationWorkerEnv); ok {
		var arguments []string
		if err := json.Unmarshal([]byte(encoded), &arguments); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(64)
		}
		os.Exit(runConfigurationWorker(arguments))
	}
	os.Exit(m.Run())
}

type pausingStore struct {
	store.Store
	key   string
	pause func() error
}

func (s pausingStore) Write(ctx context.Context, key string, value []byte, expected store.Expected) (bool, error) {
	written, err := s.Store.Write(ctx, key, value, expected)
	if err == nil && written && key == s.key {
		if err := s.pause(); err != nil {
			return false, err
		}
	}
	return written, err
}

// Mode "reserve" writes the authority record itself before waiting; "admit" lets admit write it.
func runAdmissionWorker(arguments []string) int {
	if len(arguments) != 5 || arguments[4] != "reserve" && arguments[4] != "admit" {
		fmt.Fprintln(os.Stderr, "worker requires state, destination, kind, barrier and mode")
		return 64
	}
	state, destination, barrier, mode := arguments[0], arguments[1], arguments[3], arguments[4]
	ctx := context.Background()
	kind, err := ParseResourceKind(arguments[2])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 64
	}
	authority, err := NewResourceAuthority(ctx, destination)
	if err != nil {
		fmt.Fprintln(os.Stderr, "worker authority:", err)
		return 1
	}
	reference := ResourceRef{Destination: authority.Destination(), Kind: kind}
	directory, err := store.NewDirectoryStore(ctx, state)
	if err != nil {
		fmt.Fprintln(os.Stderr, "worker store:", err)
		return 2
	}
	held, err := directory.Lock(ctx, "resource/"+destinationKey(reference.Destination), 10*time.Second)
	if err != nil {
		fmt.Fprintln(os.Stderr, "worker lock:", err)
		return 2
	}
	defer func() { _ = held.Unlock() }()
	key := resourceKey(reference.Destination, "authority")
	pause := func() error {
		ready := os.NewFile(3, "ready")
		if _, err := ready.WriteString("reserved\n"); err != nil {
			return err
		}
		_ = ready.Close()
		return awaitBarrier(barrier)
	}
	var st store.Store = directory
	if mode == "admit" {
		st = pausingStore{Store: directory, key: key, pause: pause}
	} else {
		if _, err := directory.Write(ctx, key, encodeRecord(reference), store.Absent()); err != nil {
			fmt.Fprintln(os.Stderr, "worker reserve:", err)
			return 2
		}
		if err := pause(); err != nil {
			fmt.Fprintln(os.Stderr, "worker pause:", err)
			return 3
		}
	}
	if failed := admit(ctx, st, reference); failed != nil {
		fmt.Println(failed.Code)
	} else {
		fmt.Println("admitted")
	}
	return 0
}

func awaitBarrier(barrier string) error {
	deadline := time.Now().Add(time.Minute)
	for {
		if _, err := os.Lstat(barrier); err == nil {
			return nil
		}
		if time.Now().After(deadline) {
			return errors.New("worker barrier timed out")
		}
		time.Sleep(5 * time.Millisecond)
	}
}

type admissionWorker struct {
	command *exec.Cmd
	stdout  *strings.Builder
	stderr  *strings.Builder
	barrier string
}

func startAdmissionWorker(t *testing.T, root, label, destination string, kind ResourceKind) *admissionWorker {
	t.Helper()
	return startWorker(t, root, label, destination, kind, "reserve")
}

func startWorker(t *testing.T, root, label, destination string, kind ResourceKind, mode string) *admissionWorker {
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
	ctx, cancel := context.WithCancel(context.Background())
	barrier := filepath.Join(root, "barrier-"+label)
	worker := &admissionWorker{
		command: exec.CommandContext(ctx, executable, "-test.run=^$"),
		stdout:  &strings.Builder{},
		stderr:  &strings.Builder{},
		barrier: barrier,
	}
	arguments, err := json.Marshal([]string{filepath.Join(root, "state"), destination, string(kind), barrier, mode})
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	worker.command.Env = append(os.Environ(), admissionWorkerEnv+"="+string(arguments))
	worker.command.ExtraFiles = []*os.File{writer}
	worker.command.Stdout = worker.stdout
	worker.command.Stderr = worker.stderr
	if err := worker.command.Start(); err != nil {
		cancel()
		t.Fatal(err)
	}
	_ = writer.Close()
	t.Cleanup(func() {
		cancel()
		_ = worker.command.Wait()
	})
	line := make(chan string, 1)
	go func() {
		text, _ := bufio.NewReader(reader).ReadString('\n')
		line <- text
	}()
	select {
	case text := <-line:
		if text != "reserved\n" {
			t.Fatalf("worker %s did not reserve: %q %s", label, text, worker.stderr.String())
		}
	case <-time.After(time.Minute):
		t.Fatalf("worker %s did not reserve within a minute", label)
	}
	return worker
}

func (w *admissionWorker) release(t *testing.T) {
	t.Helper()
	file, err := os.OpenFile(w.barrier, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
}

func (w *admissionWorker) outcome(t *testing.T) string {
	t.Helper()
	if err := w.command.Wait(); err != nil {
		t.Fatalf("worker failed: %v: %s", err, w.stderr.String())
	}
	lines := strings.Split(strings.TrimSpace(w.stdout.String()), "\n")
	return lines[len(lines)-1]
}

func recordedAuthority(t *testing.T, root, destination string) ResourceRef {
	t.Helper()
	st, err := store.NewDirectoryStore(t.Context(), filepath.Join(root, "state"))
	if err != nil {
		t.Fatal(err)
	}
	canonical := authority(t, destination).Destination()
	data, ok, err := st.Read(t.Context(), resourceKey(canonical, "authority"))
	if err != nil || !ok {
		t.Fatalf("authority record for %s = %v, %v", destination, ok, err)
	}
	recorded, err := decodeMetadata(data, "ResourceRef", resourceRefFromState)
	if err != nil {
		t.Fatal(err)
	}
	return recorded
}

func assertFencesRetained(t *testing.T, root, tree, child string) {
	t.Helper()
	if got, want := recordedAuthority(t, root, tree), treeRef(t, tree); got != want {
		t.Errorf("tree authority = %+v, want %+v", got, want)
	}
	if got, want := recordedAuthority(t, root, child), documentRef(t, child); got != want {
		t.Errorf("child authority = %+v, want %+v", got, want)
	}
}

func raceRoot(t *testing.T) (root, tree, child string) {
	t.Helper()
	root = tempRoot(t)
	tree = filepath.Join(root, "config")
	if err := os.Mkdir(tree, 0o700); err != nil {
		t.Fatal(err)
	}
	return root, tree, filepath.Join(tree, "child")
}

func TestAdmissionRaceTreeFirst(t *testing.T) {
	t.Parallel()
	root, tree, child := raceRoot(t)
	treeWorker := startAdmissionWorker(t, root, "tree", tree, ResourceTree)
	treeWorker.release(t)
	if got := treeWorker.outcome(t); got != "admitted" {
		t.Fatalf("tree outcome = %q", got)
	}
	childWorker := startAdmissionWorker(t, root, "child", child, ResourceDocument)
	childWorker.release(t)
	if got := childWorker.outcome(t); got != string(CodeConflict) {
		t.Fatalf("child outcome = %q", got)
	}
	assertFencesRetained(t, root, tree, child)
}

func TestAdmissionRaceChildFirst(t *testing.T) {
	t.Parallel()
	root, tree, child := raceRoot(t)
	childWorker := startAdmissionWorker(t, root, "child", child, ResourceDocument)
	childWorker.release(t)
	if got := childWorker.outcome(t); got != "admitted" {
		t.Fatalf("child outcome = %q", got)
	}
	treeWorker := startAdmissionWorker(t, root, "tree", tree, ResourceTree)
	treeWorker.release(t)
	if got := treeWorker.outcome(t); got != string(CodeConflict) {
		t.Fatalf("tree outcome = %q", got)
	}
	assertFencesRetained(t, root, tree, child)
}

func TestAdmissionRaceSimultaneous(t *testing.T) {
	t.Parallel()
	root, tree, child := raceRoot(t)
	treeWorker := startAdmissionWorker(t, root, "tree", tree, ResourceTree)
	childWorker := startAdmissionWorker(t, root, "child", child, ResourceDocument)
	treeWorker.release(t)
	childWorker.release(t)
	if got := treeWorker.outcome(t); got != string(CodeConflict) {
		t.Errorf("tree outcome = %q", got)
	}
	if got := childWorker.outcome(t); got != string(CodeConflict) {
		t.Errorf("child outcome = %q", got)
	}
	assertFencesRetained(t, root, tree, child)
}

func TestAdmissionRaceCaseAlias(t *testing.T) {
	t.Parallel()
	root, tree, child := raceRoot(t)
	if !caseInsensitive(t, root) {
		t.Skip("host is case sensitive")
	}
	childWorker := startAdmissionWorker(t, root, "child", child, ResourceDocument)
	childWorker.release(t)
	if got := childWorker.outcome(t); got != "admitted" {
		t.Fatalf("child outcome = %q", got)
	}
	treeWorker := startAdmissionWorker(t, root, "tree", filepath.Join(root, "CONFIG"), ResourceTree)
	treeWorker.release(t)
	if got := treeWorker.outcome(t); got != string(CodeConflict) {
		t.Fatalf("tree outcome = %q", got)
	}
	if got, want := recordedAuthority(t, root, tree), treeRef(t, tree); got != want {
		t.Errorf("aliased tree authority = %+v, want the canonical %+v", got, want)
	}
	if got, want := recordedAuthority(t, root, child), documentRef(t, child); got != want {
		t.Errorf("child authority = %+v, want %+v", got, want)
	}
}

func TestConcurrentAdmitCallsOfAnAncestorAndADescendantCannotBothSucceed(t *testing.T) {
	t.Parallel()
	for _, treeFirst := range []bool{true, false} {
		t.Run(fmt.Sprintf("tree first %v", treeFirst), func(t *testing.T) {
			t.Parallel()
			root, tree, child := raceRoot(t)
			var workers []*admissionWorker
			if treeFirst {
				workers = append(workers, startWorker(t, root, "tree", tree, ResourceTree, "admit"), startWorker(t, root, "child", child, ResourceDocument, "admit"))
			} else {
				workers = append(workers, startWorker(t, root, "child", child, ResourceDocument, "admit"), startWorker(t, root, "tree", tree, ResourceTree, "admit"))
			}
			for _, worker := range workers {
				worker.release(t)
			}
			outcomes := []string{workers[0].outcome(t), workers[1].outcome(t)}
			if diff := cmp.Diff([]string{string(CodeConflict), string(CodeConflict)}, outcomes); diff != "" {
				t.Errorf("admit outcomes (-want +got):\n%s", diff)
			}
			assertFencesRetained(t, root, tree, child)
		})
	}
}

func TestAdmissionWorkerArgumentsAreChecked(t *testing.T) {
	t.Parallel()
	if code := runAdmissionWorker(nil); code != 64 {
		t.Errorf("runAdmissionWorker(nil) = %d", code)
	}
	if code := runAdmissionWorker([]string{"s", "d", "folder", "b", "admit"}); code != 64 {
		t.Errorf("runAdmissionWorker with a bad kind = %d", code)
	}
	if code := runAdmissionWorker([]string{"s", "d", "document", "b", "other"}); code != 64 {
		t.Errorf("runAdmissionWorker with a bad mode = %d", code)
	}
}
