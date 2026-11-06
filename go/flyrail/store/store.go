package store

import (
	"context"
	"time"
)

// Expected is the value a conditional write requires a key to hold: absent, or present with exact bytes.
type Expected struct {
	value   []byte
	present bool
}

// Absent returns the expectation that the key does not exist; it equals the zero Expected.
func Absent() Expected {
	return Expected{}
}

// Equal returns the expectation that the key holds exactly v, which is copied.
func Equal(v []byte) Expected {
	return Expected{value: append([]byte{}, v...), present: true}
}

// Present reports whether the key is expected to exist.
func (e Expected) Present() bool {
	return e.present
}

// Bytes returns a copy of the expected value, or nil when the key is expected to be absent.
func (e Expected) Bytes() []byte {
	if !e.present {
		return nil
	}
	return append([]byte{}, e.value...)
}

// Unlocker releases a held store lock; Unlock is idempotent.
type Unlocker interface {
	Unlock() error
}

var (
	_ Store = (*MemoryStore)(nil)
	_ Store = (*DirectoryStore)(nil)
)

// Store holds Flyrail's state records and named locks; implementations are safe for concurrent use and copy values in and out.
type Store interface {
	Location() (string, bool)
	StagingDirectory() (string, bool)
	Read(ctx context.Context, key string) (value []byte, ok bool, err error)
	Write(ctx context.Context, key string, value []byte, expected Expected) (bool, error)
	Delete(ctx context.Context, key string, expected []byte) (bool, error)
	Keys(ctx context.Context, prefix string) ([]string, error)
	Lock(ctx context.Context, name string, timeout time.Duration) (Unlocker, error)
}
