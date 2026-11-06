package flyrail

import (
	"errors"
	"syscall"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

// ValueError reports invalid input to a constructor, parser or validator.
type ValueError struct {
	Message string

	pythonTypeError bool
	nesting         bool
}

// Error returns the message.
func (e *ValueError) Error() string {
	if e == nil {
		return ""
	}
	return e.Message
}

func newValueError(message string) *ValueError {
	return &ValueError{Message: message}
}

func newTypeError(message string) *ValueError {
	return &ValueError{Message: message, pythonTypeError: true}
}

func newNestingError(message string) *ValueError {
	return &ValueError{Message: message, nesting: true}
}

func valueErrorFromJSON(err error) *ValueError {
	if failure, ok := errors.AsType[*jsonx.Error](err); ok {
		return newValueError(failure.Message)
	}
	return newValueError(err.Error())
}

// ErrorCode classifies a target failure.
type ErrorCode string

// The target failure codes, spelled as on the wire.
const (
	CodeBusy             ErrorCode = "busy"
	CodeConflict         ErrorCode = "conflict"
	CodeModified         ErrorCode = "modified"
	CodeUpdateRequired   ErrorCode = "update_required"
	CodeUnsupported      ErrorCode = "unsupported"
	CodeIOError          ErrorCode = "io_error"
	CodeUnsafePath       ErrorCode = "unsafe_path"
	CodeInvalidState     ErrorCode = "invalid_state"
	CodeConcurrentChange ErrorCode = "concurrent_change"
	CodeRecoveryNeeded   ErrorCode = "recovery_needed"
)

// ParseErrorCode returns the ErrorCode spelled s.
func ParseErrorCode(s string) (ErrorCode, error) {
	switch code := ErrorCode(s); code {
	case CodeBusy, CodeConflict, CodeModified, CodeUpdateRequired, CodeUnsupported, CodeIOError,
		CodeUnsafePath, CodeInvalidState, CodeConcurrentChange, CodeRecoveryNeeded:
		return code, nil
	}
	return "", newValueError("'" + s + "' is not a valid ErrorCode")
}

// TargetError is an expected failure of an operation on one target, reported inside results.
type TargetError struct {
	Code    ErrorCode
	Message string
	Path    string
	Errno   syscall.Errno

	cause error
}

func newTargetError(code ErrorCode, message, path string, errno syscall.Errno, cause error) *TargetError {
	return &TargetError{Code: code, Message: message, Path: path, Errno: errno, cause: cause}
}

// Error returns the message.
func (e *TargetError) Error() string {
	if e == nil {
		return ""
	}
	return e.Message
}

// Unwrap returns the errno when set and the underlying cause when there is one.
func (e *TargetError) Unwrap() []error {
	if e == nil {
		return nil
	}
	var wrapped []error
	if e.Errno != 0 {
		wrapped = append(wrapped, e.Errno)
	}
	if e.cause != nil {
		wrapped = append(wrapped, e.cause)
	}
	return wrapped
}

// BundleError reports a bundle that could not be loaded: Err is a *ValueError or the I/O failure.
type BundleError struct {
	Message string
	Err     error
}

// Error returns the message.
func (e *BundleError) Error() string {
	if e == nil {
		return ""
	}
	return e.Message
}

// Unwrap returns the cause.
func (e *BundleError) Unwrap() error {
	if e == nil {
		return nil
	}
	return e.Err
}

// ErrUnsafePath is the cause of an AuthorityError for a destination Flyrail refuses to manage.
var ErrUnsafePath = errors.New("unsafe path")

// AuthorityError reports a destination whose authority could not be established.
type AuthorityError struct {
	Message string
	Path    string
	Err     error
}

// Error returns the message.
func (e *AuthorityError) Error() string {
	if e == nil {
		return ""
	}
	return e.Message
}

// Unwrap returns the cause: a *ValueError, ErrUnsafePath or the I/O failure.
func (e *AuthorityError) Unwrap() error {
	if e == nil {
		return nil
	}
	return e.Err
}
