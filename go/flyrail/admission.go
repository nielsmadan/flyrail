package flyrail

import (
	"context"
	"errors"
	"slices"
	"strings"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/native"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

func storeLocation(st store.Store) string {
	location, _ := st.Location()
	return location
}

func stateFailure(st store.Store, code ErrorCode, message string) *TargetError {
	return failure(code, message, storeLocation(st))
}

func readRecord[T stateRecord](ctx context.Context, st store.Store, key, tag string, from func(jsonx.Value) (T, error)) (value T, ok bool, failed *TargetError) {
	data, present, err := st.Read(ctx, key)
	if err != nil {
		return value, false, storeFailure(err)
	}
	if !present {
		return value, false, nil
	}
	value, err = decodeMetadata(data, tag, from)
	if err != nil {
		message := err.Error()
		if message == "metadata record has the wrong type" {
			message = "record has the wrong type"
		}
		return value, false, stateFailure(st, CodeInvalidState, key+": "+message)
	}
	if string(encodeRecord(value)) != string(data) {
		return value, false, stateFailure(st, CodeInvalidState, key+": record is not canonical")
	}
	return value, true, nil
}

// Pointer types are excluded so absence is only ever a nil *T.
type storedRecord interface {
	ResourceRef | receipt | index | journal | preparation
	stateRecord
}

// A nil expected means absent.
func writeRecord[T storedRecord](ctx context.Context, st store.Store, key string, value T, expected *T) *TargetError {
	condition := store.Absent()
	if expected != nil {
		condition = store.Equal(encodeRecord(*expected))
	}
	written, err := st.Write(ctx, key, encodeRecord(value), condition)
	if err != nil {
		return storeFailure(err)
	}
	if !written {
		return stateFailure(st, CodeConcurrentChange, key+": state changed")
	}
	return nil
}

func deleteRecord[T storedRecord](ctx context.Context, st store.Store, key string, expected T) *TargetError {
	deleted, err := st.Delete(ctx, key, encodeRecord(expected))
	if err != nil {
		return storeFailure(err)
	}
	if !deleted {
		return stateFailure(st, CodeConcurrentChange, key+": state changed")
	}
	return nil
}

func storeLock(ctx context.Context, st store.Store, name string, timeout time.Duration) (store.Unlocker, *TargetError) {
	held, err := st.Lock(ctx, name, timeout)
	if err == nil {
		return held, nil
	}
	if errors.Is(err, store.ErrBusy) {
		return nil, newTargetError(CodeBusy, err.Error(), storeLocation(st), 0, err)
	}
	return nil, storeFailure(err)
}

func authorityChecked(ctx context.Context, reference ResourceRef) (ResourceAuthority, *TargetError) {
	authority, err := NewResourceAuthority(ctx, reference.Destination)
	if err != nil {
		return ResourceAuthority{}, errorDetail(err)
	}
	if authority.destination != reference.Destination {
		changed := newValueError("resource canonical destination changed")
		return ResourceAuthority{}, newTargetError(CodeInvalidState, changed.Message, "", 0, changed)
	}
	return authority, nil
}

func hierarchyConflicts(ctx context.Context, st store.Store, authority ResourceAuthority, wholeTree bool) ([]string, *TargetError) {
	destination := authority.destination
	var conflicts []string
	for _, parent := range parentPaths(destination) {
		if !hasParent(parent) {
			continue
		}
		_, present, err := st.Read(ctx, resourceKey(parent, "authority"))
		if err != nil {
			return nil, storeFailure(err)
		}
		if present {
			conflicts = append(conflicts, parent)
		}
	}
	if wholeTree {
		keys, err := st.Keys(ctx, "resource/")
		if err != nil {
			return nil, storeFailure(err)
		}
		for _, key := range keys {
			if !strings.HasSuffix(key, "/authority") {
				continue
			}
			reference, ok, failed := readRecord(ctx, st, key, "ResourceRef", resourceRefFromState)
			if failed != nil {
				return nil, failed
			}
			if ok && slices.ContainsFunc(parentPaths(reference.Destination), func(parent string) bool { return pathsEqual(parent, destination) }) {
				conflicts = append(conflicts, reference.Destination)
			}
		}
		if _, err := sysx.StatMetadata(destination); err == nil {
			entries, err := native.TransactionEntries(destination)
			if err != nil {
				return nil, nativeFailure(err)
			}
			conflicts = append(conflicts, entries...)
		}
	}
	slices.Sort(conflicts)
	return slices.Compact(conflicts), nil
}

// The caller holds the resource lock.
func admit(ctx context.Context, st store.Store, reference ResourceRef) *TargetError {
	if _, err := newResourceRef(reference.Destination, reference.Kind, stateSchemaVersion); err != nil {
		return failure(CodeInvalidState, err.Error(), reference.Destination)
	}
	if ctx != nil && ctx.Err() != nil {
		return newTargetError(CodeIOError, ctx.Err().Error(), reference.Destination, 0, ctx.Err())
	}
	authority, failed := authorityChecked(ctx, reference)
	if failed != nil {
		return failed
	}
	key := resourceKey(reference.Destination, "authority")
	current, ok, failed := readRecord(ctx, st, key, "ResourceRef", resourceRefFromState)
	switch {
	case failed != nil:
		return failed
	case !ok:
		if failed := writeRecord(ctx, st, key, reference, nil); failed != nil {
			return failed
		}
	case !resourceRefsEqual(current, reference):
		return failure(CodeInvalidState, "authority identity disagrees", reference.Destination)
	}
	if failed := validateKind(reference); failed != nil {
		return failed
	}
	conflicts, failed := hierarchyConflicts(ctx, st, authority, reference.Kind != ResourceDocument)
	if failed != nil {
		return failed
	}
	if len(conflicts) > 0 {
		return failure(CodeConflict, "ancestor or descendant resource conflicts", conflicts[0])
	}
	return nil
}
