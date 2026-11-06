package storetest

import (
	"context"
	"testing"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

type ignoresExpected struct {
	*store.MemoryStore
}

func (s ignoresExpected) Write(ctx context.Context, key string, value []byte, _ store.Expected) (bool, error) {
	current, ok, err := s.Read(ctx, key)
	if err != nil {
		return false, err
	}
	expected := store.Absent()
	if ok {
		expected = store.Equal(current)
	}
	_, err = s.MemoryStore.Write(ctx, key, value, expected)
	return true, err
}

type ignoresCancellation struct {
	*store.MemoryStore
}

func (s ignoresCancellation) Lock(_ context.Context, name string, timeout time.Duration) (store.Unlocker, error) {
	return s.MemoryStore.Lock(context.Background(), name, timeout)
}

func TestCheckNamesTheGuaranteeABrokenStoreMisses(t *testing.T) {
	t.Parallel()
	for _, test := range []struct {
		name  string
		store store.Store
		want  string
	}{
		{"ignores expected", ignoresExpected{store.NewMemoryStore()}, "creating an existing key must fail"},
		{"ignores cancellation", ignoresCancellation{store.NewMemoryStore()}, "a busy lock with an ended context must wrap the context error"},
	} {
		err := check(t.Context(), test.store, test.store)
		if err == nil || err.Error() != test.want {
			t.Errorf("%s: check = %v, want %q", test.name, err, test.want)
		}
	}
}
