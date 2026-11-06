package sysx

import (
	"errors"
	"syscall"
)

// Retry calls call again for as long as it fails with EINTR.
func Retry(call func() error) error {
	for {
		err := call()
		if !errors.Is(err, syscall.EINTR) {
			return err
		}
	}
}

// RetryValue calls call again for as long as it fails with EINTR and returns its last result.
func RetryValue[T any](call func() (T, error)) (T, error) {
	for {
		value, err := call()
		if !errors.Is(err, syscall.EINTR) {
			return value, err
		}
	}
}
