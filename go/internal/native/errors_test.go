package native

import (
	"syscall"
	"testing"
)

func TestIOFailureKeepsAnEmptyPathAsAQuotedFilename(t *testing.T) {
	failure := ioFailure(syscall.ENOENT, "")
	if failure.Message != "[Errno 2] No such file or directory: ''" || failure.Path != "" || failure.Errno != syscall.ENOENT {
		t.Fatalf("ioFailure = %+v", failure)
	}
}

func TestIOFailureWithoutPathOmitsTheFilename(t *testing.T) {
	failure := ioFailureWithoutPath(syscall.EBADF)
	if failure.Message != "[Errno 9] Bad file descriptor" || failure.Errno != syscall.EBADF || failure.Kind != KindIO {
		t.Fatalf("ioFailureWithoutPath = %+v", failure)
	}
}
