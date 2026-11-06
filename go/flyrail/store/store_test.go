package store_test

import (
	"context"
	"errors"
	"io/fs"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/flyrail/store/storetest"
)

func TestMemoryStoreMeetsTheContract(t *testing.T) {
	t.Parallel()
	shared := store.NewMemoryStore()
	storetest.Run(t, func(*testing.T) store.Store { return shared })
}

func TestZeroMemoryStoreMeetsTheContract(t *testing.T) {
	t.Parallel()
	var shared store.MemoryStore
	storetest.Run(t, func(*testing.T) store.Store { return &shared })
}

func TestValidateKeyAndLockName(t *testing.T) {
	t.Parallel()
	valid := []string{"a", "index/app-1/x.y_z", "0", strings.Repeat("a", 200)}
	for _, key := range valid {
		if err := store.ValidateKey(key); err != nil {
			t.Errorf("ValidateKey(%q) = %v", key, err)
		}
	}
	invalid := []string{"", "/a", "a/", "a//b", "A", "_a", ".a", "a b", "a/../b", "é", strings.Repeat("a", 201)}
	for _, key := range invalid {
		err := store.ValidateKey(key)
		failure, ok := errors.AsType[*store.KeyError](err)
		if !ok || failure.Kind != "state key" || failure.Value != key || !errors.Is(err, store.ErrInvalidKey) ||
			err.Error() != "state key must be slash-separated lowercase segments" {
			t.Errorf("ValidateKey(%q) = %v", key, err)
		}
	}
	err := store.ValidateLockName("Upper")
	if err == nil || err.Error() != "lock name must be slash-separated lowercase segments" || !errors.Is(err, store.ErrInvalidKey) {
		t.Errorf("ValidateLockName(Upper) = %v", err)
	}
}

func TestErrorsFollowTheCrosswalk(t *testing.T) {
	t.Parallel()
	busy := store.NewBusyError("name", context.Canceled)
	if busy.Error() != "state store lock is busy: name" || !errors.Is(busy, store.ErrBusy) || !errors.Is(busy, context.Canceled) {
		t.Errorf("BusyError = %v", busy)
	}
	if errors.Unwrap(store.NewBusyError("name", nil)) != nil {
		t.Errorf("BusyError without a cause unwraps to a cause")
	}
	failure := &store.IOError{Message: "failed", Path: "/store", Errno: syscall.EIO}
	if pathErr, ok := errors.AsType[*fs.PathError](failure); !ok || pathErr.Path != "/store" || !errors.Is(failure, syscall.EIO) {
		t.Errorf("IOError unwraps to %v", errors.Unwrap(failure))
	}
	if errors.Unwrap(&store.IOError{Message: "custom"}) != nil {
		t.Errorf("IOError without errno unwraps to a cause")
	}
}

func TestNewUnlockerRunsReleaseOnce(t *testing.T) {
	t.Parallel()
	calls := 0
	unlocker := store.NewUnlocker(func() error {
		calls++
		return syscall.EBADF
	})
	first, second := unlocker.Unlock(), unlocker.Unlock()
	if calls != 1 || !errors.Is(first, syscall.EBADF) || !errors.Is(second, syscall.EBADF) {
		t.Errorf("calls = %d, results = %v, %v", calls, first, second)
	}
}

func TestMemoryStoreWakesAWaiterWhenTheLockIsReleased(t *testing.T) {
	t.Parallel()
	s := store.NewMemoryStore()
	held, err := s.Lock(t.Context(), "wake", 0)
	if err != nil {
		t.Fatal(err)
	}
	waiting := make(chan struct{})
	released := make(chan error, 1)
	go func() {
		close(waiting)
		next, err := s.Lock(t.Context(), "wake", time.Minute)
		if err == nil {
			err = next.Unlock()
		}
		released <- err
	}()
	<-waiting
	time.Sleep(100 * time.Millisecond)
	if err := held.Unlock(); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-released:
		if err != nil {
			t.Errorf("waiter = %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("waiter was not woken by the release")
	}
}

func TestRecordLimitIsTwiceAJournalAtTheResourceLimits(t *testing.T) {
	const resourceBytes, revisionsPerJournal, hexDigitsPerByte = 256 << 20, 4, 2
	if want := 2 * revisionsPerJournal * hexDigitsPerByte * int64(resourceBytes); store.MaxRecordBytes != want {
		t.Errorf("record limit = %d, want %d", store.MaxRecordBytes, want)
	}
}

func TestMemoryStoreRefusesValuesOverTheReadCap(t *testing.T) {
	t.Parallel()
	s := store.NewMemoryStoreWithRecordLimit(8)
	_, err := s.Write(t.Context(), "cap", []byte("123456789"), store.Absent())
	want := "[Errno 27] File too large: 'cap'"
	if ioErr, ok := errors.AsType[*store.IOError](err); !ok || ioErr.Error() != want {
		t.Fatalf("Write over the cap = %v, want %q", err, want)
	}
	if _, ok, err := s.Read(t.Context(), "cap"); err != nil || ok {
		t.Errorf("Read after the refused write = %v, %v", ok, err)
	}
	if won, err := s.Write(t.Context(), "cap", []byte("12345678"), store.Absent()); err != nil || !won {
		t.Fatalf("Write at the cap = %v, %v", won, err)
	}
}
