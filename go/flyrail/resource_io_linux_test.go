//go:build linux

package flyrail

import (
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

func sharedDeviceMounts(t *testing.T) (string, string) {
	t.Helper()
	table, err := os.ReadFile("/proc/self/mountinfo")
	if err != nil {
		t.Skipf("no mount table: %v", err)
	}
	seen := map[uint64]string{}
	for line := range strings.SplitSeq(strings.TrimSpace(string(table)), "\n") {
		fields := strings.Fields(line)
		if len(fields) < 5 || strings.Contains(fields[4], `\`) {
			continue
		}
		info, err := os.Stat(fields[4])
		if err != nil || !info.IsDir() {
			continue
		}
		device := info.Sys().(*syscall.Stat_t).Dev
		if first, ok := seen[device]; ok && first != fields[4] {
			return first, fields[4]
		}
		seen[device] = fields[4]
	}
	t.Skip("no two mounts share a device")
	return "", ""
}

func TestTransactionDirectoryRefusesStagingOnAnotherMountOfTheSameDevice(t *testing.T) {
	t.Parallel()
	first, second := sharedDeviceMounts(t)
	destination := filepath.Join(second, "flyrail-project", "AGENTS.md")
	fallback := must(TransactionPath(destination, testTransaction))
	shared := pathStore{MemoryStore: store.NewMemoryStore(), staging: filepath.Join(first, "flyrail-staging")}
	directory, failed := chooseTransactionDirectory(shared, destination, testTransaction)
	mustSucceed(t, failed)
	if directory != fallback {
		t.Errorf("transaction directory across mounts %s and %s = %q, want %q", first, second, directory, fallback)
	}
	same := pathStore{MemoryStore: store.NewMemoryStore(), staging: filepath.Join(second, "flyrail-staging")}
	directory, failed = chooseTransactionDirectory(same, destination, testTransaction)
	mustSucceed(t, failed)
	if want := filepath.Join(second, "flyrail-staging", destinationKey(destination)+"-"+testTransaction); directory != want {
		t.Errorf("transaction directory on mount %s = %q, want %q", second, directory, want)
	}
}
