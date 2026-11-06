package native

import (
	"errors"
	"syscall"

	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

// Kind classifies a native failure.
type Kind uint8

// The native failure kinds.
const (
	KindIO Kind = iota + 1
	KindUnsafe
	KindInvalid
	KindUnsupported
)

// Error is a native failure carrying Rust's exact message; Path is "" and Errno 0 when absent.
type Error struct {
	Kind    Kind
	Message string
	Path    string
	Errno   syscall.Errno
}

// Error returns the message.
func (e *Error) Error() string {
	if e == nil {
		return ""
	}
	return e.Message
}

// Unwrap returns the errno when set, or errors.ErrUnsupported for an unsupported platform.
func (e *Error) Unwrap() error {
	switch {
	case e == nil:
		return nil
	case e.Errno != 0:
		return e.Errno
	case e.Kind == KindUnsupported:
		return errors.ErrUnsupported
	}
	return nil
}

func unsafePath(message, path string) *Error {
	return &Error{Kind: KindUnsafe, Message: message, Path: path}
}

func invalid(message string) *Error {
	return &Error{Kind: KindInvalid, Message: message}
}

func ioFailure(err error, path string) *Error {
	message, errno := sysx.OSErrorMessage(err, path)
	return &Error{Kind: KindIO, Message: message, Path: path, Errno: errno}
}

func refused(errno syscall.Errno, message, path string) *Error {
	return &Error{Kind: KindIO, Message: sysx.PosixPathRefusalMessage(errno, message, path), Path: path, Errno: errno}
}

func ioFailureWithoutPath(err error) *Error {
	message, errno := sysx.OSErrorMessageWithoutPath(err)
	return &Error{Kind: KindIO, Message: message, Errno: errno}
}
