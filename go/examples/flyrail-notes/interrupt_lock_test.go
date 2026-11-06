//go:build darwin || linux

package main

import (
	"bytes"
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestFirstInterruptCancelsALockWait(t *testing.T) {
	root := tempRoot(t)
	source := filepath.Join(root, "source")
	copyTree(t, "bundle", source)
	state := filepath.Join(root, "state")
	args := []string{"ai", "install", "--source", source, "--target", filepath.Join(root, "target"), "--state", state}
	if code, _, stderr := runCLI(args...); code != 0 {
		t.Fatalf("first install: exit %d, stderr %q", code, stderr)
	}
	held, err := filepath.Glob(filepath.Join(state, "locks", "*"))
	if err != nil || len(held) == 0 {
		t.Fatalf("lock files %v, err %v", held, err)
	}
	for _, path := range held {
		file, err := os.Open(path)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { _ = file.Close() })
		if err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
			t.Fatal(err)
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, os.Args[0])
	cmd.Env = append(os.Environ(), mainArgsVariable+"="+strings.Join(args, " "))
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	time.Sleep(time.Second)
	started := time.Now()
	if err := cmd.Process.Signal(os.Interrupt); err != nil {
		t.Fatal(err)
	}
	waitErr := cmd.Wait()
	elapsed := time.Since(started)
	var exit *exec.ExitError
	if !errors.As(waitErr, &exit) || exit.ExitCode() != 1 {
		t.Fatalf("wait = %v, want exit code 1; stderr %q", waitErr, stderr.String())
	}
	if elapsed > 3*time.Second {
		t.Errorf("exited %v after the interrupt, want well before the 5s lock timeout", elapsed)
	}
	if !strings.Contains(stdout.String(), `"code":"busy"`) {
		t.Errorf("stdout %q lacks a busy result", stdout.String())
	}
}
