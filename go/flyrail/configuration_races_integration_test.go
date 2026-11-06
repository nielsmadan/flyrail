//go:build integration && (darwin || linux)

package flyrail

import (
	"bufio"
	"bytes"
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
)

const configurationWorkerEnv = "FLYRAIL_CONFIGURATION_WORKER"

func workerSection(path, text string) (*Bundle, *RenderedBundle, error) {
	identity, err := NewBundleIdentity("team", "2")
	if err != nil {
		return nil, nil, err
	}
	instruction, err := NewInstructionArtifact("artifact", "text")
	if err != nil {
		return nil, nil, err
	}
	bundle, err := NewBundle(identity, []Artifact{instruction}, nil, nil)
	if err != nil {
		return nil, nil, err
	}
	content, err := NewSectionContent("artifact", text, nil)
	if err != nil {
		return nil, nil, err
	}
	artifact, err := NewRenderedArtifact(RenderedArtifactParams{ID: "artifact", Family: FamilyInstructions, Destination: path, Content: content})
	if err != nil {
		return nil, nil, err
	}
	rendered, err := NewRenderedBundle([]RenderedArtifact{artifact}, nil, nil, nil)
	return bundle, rendered, err
}

// "hold" keeps the installation index lock until a barrier file appears; readiness is signalled on descriptor 3.
func runConfigurationWorker(arguments []string) int {
	if len(arguments) != 4 || arguments[0] != "sync" && arguments[0] != "hold" {
		fmt.Fprintln(os.Stderr, "worker requires a mode, state, destination and text or barrier")
		return 64
	}
	mode, state, destination, extra := arguments[0], arguments[1], arguments[2], arguments[3]
	ctx := context.Background()
	st, err := store.NewDirectoryStore(ctx, state)
	if err != nil {
		fmt.Fprintln(os.Stderr, "worker store:", err)
		return 2
	}
	target, err := NewInstallationTarget(st, nil, nil)
	if err != nil {
		fmt.Fprintln(os.Stderr, "worker target:", err)
		return 2
	}
	ready := os.NewFile(3, "ready")
	defer func() { _ = ready.Close() }()
	if mode == "hold" {
		held, err := st.Lock(ctx, installationKey("team", target), 0)
		if err != nil {
			fmt.Fprintln(os.Stderr, "worker lock:", err)
			return 2
		}
		_, _ = ready.WriteString("ready\n")
		if err := awaitBarrier(extra); err != nil {
			fmt.Fprintln(os.Stderr, err)
			return 2
		}
		if err := held.Unlock(); err != nil {
			fmt.Fprintln(os.Stderr, "worker unlock:", err)
			return 2
		}
		return 0
	}
	bundle, rendered, err := workerSection(destination, extra)
	if err != nil {
		fmt.Fprintln(os.Stderr, "worker request:", err)
		return 2
	}
	result, err := Sync(ctx, bundle, rendered, target, SyncOptions{LockTimeout: 10 * time.Second})
	if err != nil {
		fmt.Fprintln(os.Stderr, "worker sync:", err)
		return 2
	}
	_, _ = ready.WriteString(string(result.Status) + "\n")
	return 0
}

type configurationWorker struct {
	command *exec.Cmd
	stderr  *strings.Builder
	line    string
}

func startConfigurationWorker(t *testing.T, arguments ...string) *configurationWorker {
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
	encoded, err := json.Marshal(arguments)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	worker := &configurationWorker{command: exec.CommandContext(ctx, executable, "-test.run=^$"), stderr: &strings.Builder{}}
	worker.command.Env = append(os.Environ(), configurationWorkerEnv+"="+string(encoded))
	worker.command.ExtraFiles = []*os.File{writer}
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
	case worker.line = <-line:
	case <-time.After(time.Minute):
		t.Fatalf("worker %v did not report within a minute: %s", arguments, worker.stderr.String())
	}
	if worker.line == "" {
		t.Fatalf("worker %v failed: %s", arguments, worker.stderr.String())
	}
	return worker
}

func (w *configurationWorker) wait(t *testing.T) {
	t.Helper()
	if err := w.command.Wait(); err != nil {
		t.Fatalf("worker failed: %v: %s", err, w.stderr.String())
	}
}

func raceRootAt(t *testing.T) (root, parent, path string, target *InstallationTarget) {
	t.Helper()
	root = tempRoot(t)
	parent = filepath.Join(root, "project")
	mkdir(t, parent)
	path = filepath.Join(parent, "AGENTS.md")
	writeFile(t, path, "# foreign\n")
	target = stateTarget(t, root)
	sync := syncOnce(t, instructionsBundle(t, "team", "1", "artifact"), renderedOf(t, sectionAt(t, "artifact", path, "generated")), target)
	wantStatus(t, "install", sync, StatusApplied)
	return root, parent, path, target
}

func TestPreviewsRejectAllObservedRevisionChangesWithoutOverwritingPayloads(t *testing.T) {
	t.Parallel()
	changes := []string{"foreign", "owned", "receipt", "index", "mode", "ancestor", "recovery", "hardlink", "symlink", "process"}
	for _, change := range changes {
		root, parent, path, target := raceRootAt(t)
		proposal := previewOf(t, instructionsBundle(t, "team", "2", "artifact"), renderedOf(t, sectionAt(t, "artifact", path, "second")), target)
		destination := authority(t, path).Destination()
		st := target.Store()
		switch change {
		case "foreign":
			writeFile(t, path, strings.ReplaceAll(readText(t, path), "f", "x"))
		case "owned":
			writeFile(t, path, strings.Replace(readText(t, path), "generated", "Generated", 1))
		case "receipt":
			key := resourceKey(destination, "receipt")
			original := storeValue(t, st, key)
			marker := []byte(`"transaction_id":"`)
			changed := bytes.Clone(original)
			start := bytes.Index(changed, marker) + len(marker)
			if changed[start] == 'f' {
				changed[start] = 'e'
			} else {
				changed[start] = 'f'
			}
			replaceRecord(t, st, key, changed)
		case "index":
			key := installationKey("team", target)
			original := storeValue(t, st, key)
			replaceRecord(t, st, key, bytes.ReplaceAll(original, []byte(`"version":"1"`), []byte(`"version":"different"`)))
		case "mode":
			if err := os.Chmod(path, 0o400); err != nil {
				t.Fatal(err)
			}
		case "ancestor":
			if err := os.Rename(parent, filepath.Join(root, "old-project")); err != nil {
				t.Fatal(err)
			}
			mkdir(t, parent)
			writeFile(t, path, "replacement")
		case "recovery":
			if written, err := st.Write(t.Context(), resourceKey(destination, "journal"), []byte("{}"), store.Absent()); err != nil || !written {
				t.Fatalf("recovery marker: %v %v", written, err)
			}
		case "hardlink":
			if err := os.Link(path, filepath.Join(root, "linked")); err != nil {
				t.Fatal(err)
			}
		case "symlink":
			if err := os.Rename(path, filepath.Join(root, "actual")); err != nil {
				t.Fatal(err)
			}
			if err := os.Symlink(filepath.Join(root, "actual"), path); err != nil {
				t.Fatal(err)
			}
		case "process":
			worker := startConfigurationWorker(t, "sync", filepath.Join(root, "state"), path, "from another process")
			worker.wait(t)
			if worker.line != "applied\n" {
				t.Fatalf("the competing sync reported %q", worker.line)
			}
		}
		before := readText(t, path)
		result := ApplyPreview(t.Context(), proposal, LockOptions{LockTimeout: testLockTimeout})
		wantStatus(t, change, result, StatusFailed)
		if result.Err == nil || result.Err.Code != CodeConcurrentChange && result.Err.Code != CodeUnsafePath {
			t.Errorf("%s: error = %+v", change, result.Err)
		}
		if readText(t, path) != before {
			t.Errorf("%s: the payload was overwritten", change)
		}
	}
}

func TestAnIndexLockHeldByAnotherProcessDelaysApplication(t *testing.T) {
	t.Parallel()
	root, _, path, target := raceRootAt(t)
	proposal := previewOf(t, instructionsBundle(t, "team", "2", "artifact"), renderedOf(t, sectionAt(t, "artifact", path, "second")), target)
	barrier := filepath.Join(root, "barrier")
	worker := startConfigurationWorker(t, "hold", filepath.Join(root, "state"), path, barrier)
	before := readText(t, path)
	busy := ApplyPreview(t.Context(), proposal, LockOptions{})
	wantStatus(t, "busy", busy, StatusFailed)
	wantCode(t, "busy", busy.Err, CodeBusy)
	if readText(t, path) != before {
		t.Fatal("a busy apply changed the payload")
	}
	done := make(chan InstallationResult, 1)
	started := time.Now()
	go func() { done <- ApplyPreview(t.Context(), proposal, LockOptions{LockTimeout: 30 * time.Second}) }()
	time.Sleep(100 * time.Millisecond)
	exclusiveWrite(t, barrier, "", 0o600)
	result := <-done
	worker.wait(t)
	wantStatus(t, "apply", result, StatusApplied)
	if elapsed := time.Since(started); elapsed < 100*time.Millisecond {
		t.Errorf("apply finished after %v without waiting for the lock", elapsed)
	}
	if !strings.Contains(readText(t, path), "second") {
		t.Error("the waiting apply did not publish")
	}
}

func TestTombstonedAuthorityRecordsStillExcludeOtherBoundaries(t *testing.T) {
	t.Parallel()
	root := tempRoot(t)
	tree := filepath.Join(root, "tree")
	mkdir(t, tree)
	st := openStore(t, filepath.Join(root, "state"))
	canonical := authority(t, tree).Destination()
	tombstone := ResourceRef{Destination: canonical, Kind: ResourceTree}
	if written, err := st.Write(t.Context(), resourceKey(canonical, "authority"), encodeRecord(tombstone), store.Absent()); err != nil || !written {
		t.Fatalf("tombstone: %v %v", written, err)
	}
	child := renderedOf(t, must(NewRenderedArtifact(RenderedArtifactParams{
		ID:          "artifact",
		Family:      FamilySkills,
		Destination: filepath.Join(tree, "child"),
		Content:     must(NewFileContent([]byte("x"), ModeReadable)),
	})))
	childResult := syncOnce(t, skillsBundle(t), child, targetOf(t, st))
	wantStatus(t, "child", childResult, StatusFailed)
	wantCode(t, "child", childResult.Err, CodeConflict)
	if !hasRecord(t, st, resourceKey(pathx.Join(canonical, "child"), "authority")) {
		t.Error("the child authority was not reserved")
	}
	desiredTree := renderedOf(t, must(NewRenderedArtifact(RenderedArtifactParams{
		ID:          "artifact",
		Family:      FamilySkills,
		Destination: tree,
		Content:     must(NewTreeContent([]BundleEntry{{Path: "x", Data: []byte("x")}})),
	})))
	other := must(NewInstallationTarget(st, map[string]string{"installation": "other"}, nil))
	result := syncOnce(t, skillsBundle(t), desiredTree, other)
	wantStatus(t, "tree", result, StatusFailed)
	wantCode(t, "tree", result.Err, CodeConflict)
}
