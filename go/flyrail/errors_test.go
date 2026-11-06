package flyrail

import (
	"context"
	"errors"
	"io/fs"
	"syscall"
	"testing"

	"github.com/nielsmadan/flyrail/go/internal/native"
)

func TestParseErrorCodeAcceptsWireSpellings(t *testing.T) {
	for _, code := range []ErrorCode{CodeBusy, CodeConflict, CodeModified, CodeUpdateRequired, CodeUnsupported,
		CodeIOError, CodeUnsafePath, CodeInvalidState, CodeConcurrentChange, CodeRecoveryNeeded} {
		parsed, err := ParseErrorCode(string(code))
		if err != nil || parsed != code {
			t.Errorf("ParseErrorCode(%q) = %q, %v", code, parsed, err)
		}
	}
	_, err := ParseErrorCode("io-error")
	var invalid *ValueError
	if !errors.As(err, &invalid) || invalid.Message != "'io-error' is not a valid ErrorCode" {
		t.Fatalf("ParseErrorCode(io-error) = %v", err)
	}
}

func TestTargetErrorUnwrapsErrnoAndCause(t *testing.T) {
	failure := newTargetError(CodeBusy, "state store lock is busy: x", "/tmp/x", syscall.EAGAIN, context.Canceled)
	if failure.Error() != "state store lock is busy: x" {
		t.Errorf("Error() = %q", failure.Error())
	}
	if !errors.Is(failure, syscall.EAGAIN) || !errors.Is(failure, context.Canceled) {
		t.Errorf("errors.Is does not reach the errno and cause of %v", failure)
	}
	plain := &TargetError{Code: CodeConflict, Message: "conflict"}
	if len(plain.Unwrap()) != 0 {
		t.Errorf("Unwrap() = %v, want none", plain.Unwrap())
	}
	var absent *TargetError
	if absent.Error() != "" || absent.Unwrap() != nil {
		t.Errorf("nil *TargetError = %q, %v", absent.Error(), absent.Unwrap())
	}
}

func TestBundleAndAuthorityErrorsUnwrapTheirCause(t *testing.T) {
	cause := &fs.PathError{Op: "open", Path: "/x", Err: syscall.ENOENT}
	bundle := &BundleError{Message: "[Errno 2] No such file or directory: '/x'", Err: cause}
	if !errors.Is(bundle, syscall.ENOENT) || bundle.Error() != bundle.Message {
		t.Errorf("BundleError = %v", bundle)
	}
	authority := &AuthorityError{Message: "destination is a symbolic link", Path: "/x", Err: ErrUnsafePath}
	if !errors.Is(authority, ErrUnsafePath) || authority.Error() != authority.Message {
		t.Errorf("AuthorityError = %v", authority)
	}
	invalid := &AuthorityError{Message: "bad", Err: newValueError("bad")}
	if _, ok := errors.AsType[*ValueError](invalid); !ok {
		t.Errorf("AuthorityError does not unwrap to *ValueError")
	}
}

func TestNativeFailuresMapToOneTargetErrorCodeTable(t *testing.T) {
	cases := map[*native.Error]ErrorCode{
		{Kind: native.KindInvalid}:                    CodeInvalidState,
		{Kind: native.KindUnsafe}:                     CodeUnsafePath,
		{Kind: native.KindUnsupported}:                CodeUnsupported,
		{Kind: native.KindIO, Errno: syscall.EACCES}:  CodeIOError,
		{Kind: native.KindIO}:                         CodeIOError,
		{Kind: native.KindIO, Errno: syscall.ENOTSUP}: CodeUnsupported,
		{Kind: native.KindIO, Errno: syscall.ENOSYS}:  CodeUnsupported,
		{Kind: native.KindIO, Errno: syscall.EXDEV}:   CodeUnsupported,
		{Kind: native.KindIO, Errno: syscall.EINVAL}:  CodeUnsupported,
	}
	for failure, want := range cases {
		if got := nativeFailure(failure).Code; got != want {
			t.Errorf("nativeFailure(%+v) code = %s, want %s", failure, got, want)
		}
		if failure.Kind == native.KindIO {
			if got := ioFailure(failure.Errno, "/fixture").Code; failure.Errno != 0 && got != want {
				t.Errorf("ioFailure(%d) code = %s, want %s", failure.Errno, got, want)
			}
		}
	}
}
