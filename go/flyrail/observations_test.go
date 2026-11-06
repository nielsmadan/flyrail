package flyrail

import (
	"context"
	"errors"
	"io/fs"
	"syscall"
	"testing"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/native"
)

func TestParseObservationEnums(t *testing.T) {
	t.Parallel()
	for _, state := range []ObservationState{StateAbsent, StateInstalled, StateRecoveryNeeded, StateUnknown} {
		if got := must(ParseObservationState(string(state))); got != state {
			t.Errorf("ParseObservationState(%q) = %q", state, got)
		}
	}
	for _, kind := range []ModificationKind{ModificationMissing, ModificationAdded, ModificationTypeChanged, ModificationContentChanged, ModificationExecutableChanged} {
		if got := must(ParseModificationKind(string(kind))); got != kind {
			t.Errorf("ParseModificationKind(%q) = %q", kind, got)
		}
	}
	if _, err := ParseObservationState("gone"); err == nil || err.Error() != "'gone' is not a valid ObservationState" {
		t.Errorf("ParseObservationState(gone) = %v", err)
	}
	if _, err := ParseModificationKind("gone"); err == nil || err.Error() != "'gone' is not a valid ModificationKind" {
		t.Errorf("ParseModificationKind(gone) = %v", err)
	}
}

func TestObservationIsCurrentOnlyForAMatchingUnmodifiedInstallation(t *testing.T) {
	t.Parallel()
	yes, no := true, false
	current := func() Observation {
		return Observation{
			State:                  StateInstalled,
			Installed:              &Installation{BundleID: "app", Version: "1"},
			VersionMatches:         &yes,
			RecordedContentMatches: &yes,
			ContentMatches:         &yes,
			Modifications:          []Modification{{BundleID: "other", Path: "x", Kind: ModificationAdded}},
		}
	}
	if !current().IsCurrent() {
		t.Errorf("a matching installation is not current")
	}
	for name, change := range map[string]func(*Observation){
		"absent":        func(o *Observation) { o.State = StateAbsent },
		"no installed":  func(o *Observation) { o.Installed = nil },
		"undetermined":  func(o *Observation) { o.ContentMatches = nil },
		"stale version": func(o *Observation) { o.VersionMatches = &no },
		"stale record":  func(o *Observation) { o.RecordedContentMatches = &no },
		"conflict":      func(o *Observation) { o.Conflicts = []Conflict{{Path: "x"}} },
		"error":         func(o *Observation) { o.Err = &TargetError{Code: CodeIOError} },
		"own change": func(o *Observation) {
			o.Modifications = append(o.Modifications, Modification{BundleID: "app", Path: "y"})
		},
		"recovery state": func(o *Observation) { o.State = StateRecoveryNeeded },
	} {
		observation := current()
		change(&observation)
		if observation.IsCurrent() {
			t.Errorf("%s: observation is current", name)
		}
	}
}

func TestStoreErrorsMapToTargetErrors(t *testing.T) {
	t.Parallel()
	busy := storeFailure(store.NewBusyError("resource/x", context.Canceled))
	if busy.Code != CodeBusy || busy.Message != "state store lock is busy: resource/x" ||
		!errors.Is(busy, store.ErrBusy) || !errors.Is(busy, context.Canceled) {
		t.Errorf("busy = %#v", busy)
	}
	invalid := storeFailure(store.ValidateKey("Bad"))
	if invalid.Code != CodeInvalidState || invalid.Message != "state key must be slash-separated lowercase segments" ||
		!errors.Is(invalid, store.ErrInvalidKey) {
		t.Errorf("invalid = %#v", invalid)
	}
	for _, errno := range []syscall.Errno{syscall.ENOTSUP, syscall.ENOSYS, syscall.EXDEV, syscall.EINVAL} {
		failure := storeFailure(&store.IOError{Message: "failed", Path: "/store", Errno: errno})
		if failure.Code != CodeUnsupported || failure.Errno != errno || failure.Path != "/store" || failure.Message != "failed" {
			t.Errorf("errno %d = %#v", errno, failure)
		}
	}
	failure := storeFailure(&store.IOError{Message: "failed", Path: "/store", Errno: syscall.EIO})
	if failure.Code != CodeIOError || failure.Errno != syscall.EIO || failure.Path != "/store" || !errors.Is(failure, syscall.EIO) {
		t.Errorf("EIO = %#v", failure)
	}
	custom := storeFailure(errors.New("remote store unavailable"))
	if custom.Code != CodeIOError || custom.Message != "remote store unavailable" || custom.Errno != 0 || custom.Path != "" {
		t.Errorf("custom = %#v", custom)
	}
	wrapped := storeFailure(&fs.PathError{Op: "read", Path: "/x", Err: syscall.ENOSYS})
	if wrapped.Code != CodeUnsupported || wrapped.Errno != syscall.ENOSYS {
		t.Errorf("wrapped errno = %#v", wrapped)
	}
	if storeFailure(nil) != nil {
		t.Errorf("storeFailure(nil) is not nil")
	}
}

func TestIOFailuresUsePythonOSErrorText(t *testing.T) {
	t.Parallel()
	failure := ioFailure(&fs.PathError{Op: "open", Path: "/x", Err: syscall.ENOENT}, "/x")
	if failure.Code != CodeIOError || failure.Message != "[Errno 2] No such file or directory: '/x'" || failure.Errno != syscall.ENOENT {
		t.Errorf("ioFailure = %#v", failure)
	}
	unsupported := ioFailure(syscall.EXDEV, "")
	if unsupported.Code != CodeUnsupported || unsupported.Message != "[Errno 18] Invalid cross-device link" && unsupported.Message != "[Errno 18] Cross-device link" {
		t.Errorf("EXDEV = %#v", unsupported)
	}
	mapped := nativeFailure(&native.Error{Kind: native.KindUnsafe, Message: "portable filename collision", Path: "/x"})
	if mapped.Code != CodeUnsafePath || mapped.Message != "portable filename collision" || mapped.Path != "/x" {
		t.Errorf("native = %#v", mapped)
	}
}
