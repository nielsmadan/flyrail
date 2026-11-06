package flyrail

import (
	"errors"
	"syscall"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/native"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

func ioCode(errno syscall.Errno) ErrorCode {
	switch errno {
	case syscall.ENOTSUP, syscall.ENOSYS, syscall.EXDEV, syscall.EINVAL:
		return CodeUnsupported
	}
	return CodeIOError
}

func nativeCode(failure *native.Error) ErrorCode {
	switch failure.Kind {
	case native.KindInvalid:
		return CodeInvalidState
	case native.KindUnsafe:
		return CodeUnsafePath
	case native.KindUnsupported:
		return CodeUnsupported
	}
	return ioCode(failure.Errno)
}

// Uses Python's OSError text, as Rust's io_failure_at does.
func ioFailure(err error, path string) *TargetError {
	message, errno := sysx.OSErrorMessageWithoutPath(err)
	if path != "" {
		message, errno = sysx.OSErrorMessage(err, path)
	}
	return newTargetError(ioCode(errno), message, path, errno, err)
}

func nativeFailure(err error) *TargetError {
	failure, ok := errors.AsType[*native.Error](err)
	if !ok {
		return ioFailure(err, "")
	}
	var cause error
	if failure.Errno == 0 && failure.Kind == native.KindUnsupported {
		cause = errors.ErrUnsupported
	}
	return newTargetError(nativeCode(failure), failure.Message, failure.Path, failure.Errno, cause)
}

func storeFailure(err error) *TargetError {
	switch {
	case err == nil:
		return nil
	case errors.Is(err, store.ErrBusy):
		return newTargetError(CodeBusy, err.Error(), "", 0, err)
	case errors.Is(err, store.ErrInvalidKey):
		return newTargetError(CodeInvalidState, err.Error(), "", 0, err)
	}
	if failure, ok := errors.AsType[*store.IOError](err); ok {
		return newTargetError(ioCode(failure.Errno), err.Error(), failure.Path, failure.Errno, err)
	}
	errno := sysx.Errno(err)
	return newTargetError(ioCode(errno), err.Error(), "", errno, err)
}
