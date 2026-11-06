//go:build integration && (darwin || linux)

package store_test

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

const lockHelperEnv = "FLYRAIL_STORE_LOCK_HELPER"

func TestMain(m *testing.M) {
	if root, ok := os.LookupEnv(lockHelperEnv); ok {
		os.Exit(holdLockUntilStdinCloses(root))
	}
	os.Exit(m.Run())
}

func holdLockUntilStdinCloses(root string) int {
	s, err := store.NewDirectoryStore(context.Background(), root)
	if err == nil {
		var held store.Unlocker
		if held, err = s.Lock(context.Background(), "conformance/process", 0); err == nil {
			defer func() { _ = held.Unlock() }()
		}
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	ready := os.NewFile(3, "ready")
	if _, err := ready.WriteString("ready\n"); err != nil {
		return 1
	}
	_ = ready.Close()
	_, _ = io.Copy(io.Discard, os.Stdin)
	return 0
}

func startLockHolder(t *testing.T, root string) *exec.Cmd {
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
	command := exec.CommandContext(ctx, executable, "-test.run=^$")
	command.Env = append(os.Environ(), lockHelperEnv+"="+root)
	command.ExtraFiles = []*os.File{writer}
	var stderr strings.Builder
	command.Stderr = &stderr
	stdin, err := command.StdinPipe()
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	if err := command.Start(); err != nil {
		cancel()
		t.Fatal(err)
	}
	_ = writer.Close()
	t.Cleanup(func() {
		cancel()
		_ = stdin.Close()
		_ = command.Wait()
	})
	line := make(chan string, 1)
	go func() {
		text, _ := bufio.NewReader(reader).ReadString('\n')
		line <- text
	}()
	select {
	case text := <-line:
		if text != "ready\n" {
			cancel()
			_ = stdin.Close()
			_ = command.Wait()
			t.Fatalf("lock holder did not become ready: %q %s", text, stderr.String())
		}
	case <-time.After(time.Minute):
		cancel()
		_ = stdin.Close()
		_ = command.Wait()
		t.Fatalf("lock holder did not become ready within a minute: %s", stderr.String())
	}
	return command
}

func TestDirectoryStoreLocksExcludeAnotherProcess(t *testing.T) {
	t.Parallel()
	root := filepath.Join(tempRoot(t), "state")
	holder := startLockHolder(t, root)
	s := directoryStore(t, root)
	_, err := s.Lock(t.Context(), "conformance/process", 0)
	if busy, ok := errors.AsType[*store.BusyError](err); !ok || busy.Name != "conformance/process" {
		t.Fatalf("Lock while another process holds it = %v, want *store.BusyError", err)
	}
	started := time.Now()
	if _, err := s.Lock(t.Context(), "conformance/process", 200*time.Millisecond); !errors.Is(err, store.ErrBusy) {
		t.Fatalf("bounded Lock while another process holds it = %v, want busy", err)
	}
	if elapsed := time.Since(started); elapsed < 150*time.Millisecond {
		t.Errorf("bounded Lock gave up after %v", elapsed)
	}
	other, err := s.Lock(t.Context(), "conformance/other", 0)
	if err != nil {
		t.Fatalf("Lock of another name = %v", err)
	}
	if err := other.Unlock(); err != nil {
		t.Fatal(err)
	}
	if err := holder.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	_ = holder.Wait()
	held, err := s.Lock(t.Context(), "conformance/process", 0)
	if err != nil {
		t.Fatalf("Lock after the holder exited = %v", err)
	}
	if err := held.Unlock(); err != nil {
		t.Fatal(err)
	}
}
