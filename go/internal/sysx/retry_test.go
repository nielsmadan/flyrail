package sysx

import (
	"errors"
	"syscall"
	"testing"
)

func TestRetryRepeatsInterruptedCalls(t *testing.T) {
	calls := 0
	err := Retry(func() error {
		calls++
		if calls < 3 {
			return syscall.EINTR
		}
		return syscall.ENOENT
	})
	if !errors.Is(err, syscall.ENOENT) || calls != 3 {
		t.Fatalf("Retry = %v after %d calls, want ENOENT after 3", err, calls)
	}
}

func TestRetryValueReturnsFirstUninterruptedResult(t *testing.T) {
	calls := 0
	value, err := RetryValue(func() (int, error) {
		calls++
		if calls == 1 {
			return 0, syscall.EINTR
		}
		return 7, nil
	})
	if err != nil || value != 7 || calls != 2 {
		t.Fatalf("RetryValue = %d, %v after %d calls, want 7, nil after 2", value, err, calls)
	}
}
