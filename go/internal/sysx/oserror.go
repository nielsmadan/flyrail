package sysx

import (
	"errors"
	"strconv"
	"strings"
	"syscall"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

const nulPathMessage = "file name contained an unexpected NUL byte"

// Errno returns the syscall.Errno in err's chain, or 0 when there is none.
func Errno(err error) syscall.Errno {
	errno, _ := errors.AsType[syscall.Errno](err)
	return errno
}

// ErrnoMessage returns Python's OSError text without a filename: "[Errno N] text".
func ErrnoMessage(errno syscall.Errno) string {
	return "[Errno " + strconv.Itoa(int(errno)) + "] " + ErrnoText(errno)
}

// PathRefusalMessage returns Python's OSError text for reason at path: "[Errno N] reason: 'path'", with non-UTF-8 bytes decoded lossily.
func PathRefusalMessage(errno syscall.Errno, reason, path string) string {
	return "[Errno " + strconv.Itoa(int(errno)) + "] " + reason + ": " + jsonx.PythonStrRepr(unicodex.Lossy(path))
}

// PosixPathRefusalMessage returns Python's OSError text for reason at a pathlib path: "[Errno N] reason: PosixPath('path')".
func PosixPathRefusalMessage(errno syscall.Errno, reason, path string) string {
	return "[Errno " + strconv.Itoa(int(errno)) + "] " + reason + ": PosixPath(" + jsonx.PythonStrRepr(unicodex.Lossy(path)) + ")"
}

// PathErrorMessage returns Python's OSError text for path: "[Errno N] text: 'path'", with non-UTF-8 bytes decoded lossily.
func PathErrorMessage(errno syscall.Errno, path string) string {
	return PathRefusalMessage(errno, ErrnoText(errno), path)
}

// PosixPathErrorMessage returns Python's OSError text for a pathlib path: "[Errno N] text: PosixPath('path')".
func PosixPathErrorMessage(errno syscall.Errno, path string) string {
	return PosixPathRefusalMessage(errno, ErrnoText(errno), path)
}

// OSErrorMessage returns Python's OSError text for err at path and the errno it reports; a path containing NUL has Python's NUL-byte text and no errno.
func OSErrorMessage(err error, path string) (string, syscall.Errno) {
	if strings.IndexByte(path, 0) >= 0 {
		return nulPathMessage, 0
	}
	errno := Errno(err)
	if errno == 0 {
		return err.Error(), 0
	}
	return PathErrorMessage(errno, path), errno
}

// OSErrorMessageWithoutPath returns Python's OSError text for err without a filename and the errno it reports.
func OSErrorMessageWithoutPath(err error) (string, syscall.Errno) {
	errno := Errno(err)
	if errno == 0 {
		return err.Error(), 0
	}
	return ErrnoMessage(errno), errno
}

// RustOSErrorText returns Rust's io::Error text for err: "text (os error N)" when it carries an errno, otherwise err.Error().
func RustOSErrorText(err error) string {
	if errno, ok := errors.AsType[syscall.Errno](err); ok {
		return ErrnoText(errno) + " (os error " + strconv.Itoa(int(errno)) + ")"
	}
	return err.Error()
}
