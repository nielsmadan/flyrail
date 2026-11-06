package store

import (
	"bytes"
	"context"
	"maps"
	"slices"
	"strings"
	"sync"
	"syscall"
	"time"
)

// MemoryStore is an in-process Store; its zero value is an empty, ready store that must not be copied after first use.
type MemoryStore struct {
	mu        sync.Mutex
	values    map[string][]byte
	released  map[string]chan struct{}
	maxRecord int64
}

// NewMemoryStore returns an empty in-memory store.
func NewMemoryStore() *MemoryStore {
	return &MemoryStore{}
}

// Location reports that a memory store has no filesystem location.
func (*MemoryStore) Location() (string, bool) {
	return "", false
}

// StagingDirectory reports that a memory store has no staging directory.
func (*MemoryStore) StagingDirectory() (string, bool) {
	return "", false
}

// Read returns a copy of the value stored at key; ok is false when the key is absent.
func (s *MemoryStore) Read(_ context.Context, key string) ([]byte, bool, error) {
	if err := ValidateKey(key); err != nil {
		return nil, false, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	value, ok := s.values[key]
	if !ok {
		return nil, false, nil
	}
	return bytes.Clone(nonNil(value)), true, nil
}

// Write stores a copy of value at key when the key currently holds expected, and reports whether it did.
func (s *MemoryStore) Write(_ context.Context, key string, value []byte, expected Expected) (bool, error) {
	if err := ValidateKey(key); err != nil {
		return false, err
	}
	limit := s.maxRecord
	if limit == 0 {
		limit = maxRecordBytes
	}
	if int64(len(value)) > limit {
		return false, ioFailure(syscall.EFBIG, key)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	current, ok := s.values[key]
	if !matches(current, ok, expected) {
		return false, nil
	}
	if s.values == nil {
		s.values = map[string][]byte{}
	}
	s.values[key] = bytes.Clone(nonNil(value))
	return true, nil
}

// Delete removes key when it currently holds exactly expected, and reports whether it did.
func (s *MemoryStore) Delete(_ context.Context, key string, expected []byte) (bool, error) {
	if err := ValidateKey(key); err != nil {
		return false, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	current, ok := s.values[key]
	if !ok || !bytes.Equal(current, expected) {
		return false, nil
	}
	delete(s.values, key)
	return true, nil
}

// Keys returns the stored keys starting with prefix in ascending byte order.
func (s *MemoryStore) Keys(_ context.Context, prefix string) ([]string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	keys := slices.DeleteFunc(slices.Sorted(maps.Keys(s.values)), func(key string) bool {
		return !strings.HasPrefix(key, prefix)
	})
	return nonNilKeys(keys), nil
}

// Lock acquires the named lock, waiting up to timeout and while ctx lasts; a zero timeout makes exactly one attempt.
func (s *MemoryStore) Lock(ctx context.Context, name string, timeout time.Duration) (Unlocker, error) {
	if err := ValidateLockName(name); err != nil {
		return nil, err
	}
	var held chan struct{}
	err := acquire(ctx, name, timeout, func() (bool, <-chan struct{}, error) {
		s.mu.Lock()
		defer s.mu.Unlock()
		if wake, busy := s.released[name]; busy {
			return false, wake, nil
		}
		if s.released == nil {
			s.released = map[string]chan struct{}{}
		}
		held = make(chan struct{})
		s.released[name] = held
		return true, nil, nil
	})
	if err != nil {
		return nil, err
	}
	return NewUnlocker(func() error {
		s.mu.Lock()
		defer s.mu.Unlock()
		delete(s.released, name)
		close(held)
		return nil
	}), nil
}

func matches(current []byte, present bool, expected Expected) bool {
	if present != expected.Present() {
		return false
	}
	return !present || bytes.Equal(current, expected.value)
}

func nonNil(value []byte) []byte {
	if value == nil {
		return []byte{}
	}
	return value
}

func nonNilKeys(keys []string) []string {
	if keys == nil {
		return []string{}
	}
	return keys
}
