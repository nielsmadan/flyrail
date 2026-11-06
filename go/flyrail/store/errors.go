package store

import (
	"errors"
	"io/fs"
	"syscall"
)

var (
	// ErrBusy matches every error reporting a store lock that could not be acquired.
	ErrBusy = errors.New("state store lock is busy")
	// ErrInvalidKey matches every error reporting a malformed state key or lock name.
	ErrInvalidKey = errors.New("invalid state store key or lock name")
)

// BusyError reports that the lock Name could not be acquired in time.
type BusyError struct {
	Name string

	cause error
}

// NewBusyError returns a BusyError for name that wraps cause, such as the ended context's error.
func NewBusyError(name string, cause error) *BusyError {
	return &BusyError{Name: name, cause: cause}
}

// Error returns Rust's text: "state store lock is busy: <name>".
func (e *BusyError) Error() string {
	if e == nil {
		return ""
	}
	return "state store lock is busy: " + e.Name
}

// Is reports whether target is ErrBusy.
func (e *BusyError) Is(target error) bool {
	return target == ErrBusy
}

// Unwrap returns the cause, or nil when there is none.
func (e *BusyError) Unwrap() error {
	if e == nil {
		return nil
	}
	return e.cause
}

// KeyError reports a Value that is not a valid state key or lock name; Kind is "state key" or "lock name".
type KeyError struct {
	Kind  string
	Value string
}

// Error returns Rust's text: "<Kind> must be slash-separated lowercase segments".
func (e *KeyError) Error() string {
	if e == nil {
		return ""
	}
	return e.Kind + " must be slash-separated lowercase segments"
}

// Is reports whether target is ErrInvalidKey.
func (e *KeyError) Is(target error) bool {
	return target == ErrInvalidKey
}

// IOError is a store I/O failure whose Message is the complete Rust and Python text; Path is "" and Errno 0 when absent.
type IOError struct {
	Message string
	Path    string
	Errno   syscall.Errno
}

// Error returns the message.
func (e *IOError) Error() string {
	if e == nil {
		return ""
	}
	return e.Message
}

// Unwrap returns a *fs.PathError carrying Path and Errno when Errno is set, otherwise nil.
func (e *IOError) Unwrap() error {
	if e == nil || e.Errno == 0 {
		return nil
	}
	return &fs.PathError{Op: "state store", Path: e.Path, Err: e.Errno}
}
