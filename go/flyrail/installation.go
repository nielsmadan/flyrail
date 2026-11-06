package flyrail

import (
	"crypto/sha256"
	"encoding/hex"
	"reflect"
	"slices"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

// InstallationTarget is a state store together with the identity and routing contexts of one installation.
type InstallationTarget struct {
	store   store.Store
	context []contextEntry
	routing []contextEntry
}

// NewInstallationTarget returns a target in st identified by context; st must be a non-nil pointer.
func NewInstallationTarget(st store.Store, context, routing map[string]string) (*InstallationTarget, error) {
	return newInstallationTarget(st, contextEntries(context), contextEntries(routing))
}

func newInstallationTarget(st store.Store, context, routing []contextEntry) (*InstallationTarget, error) {
	if err := checkStore(st); err != nil {
		return nil, err
	}
	if !validContextEntries(context) || !validContextEntries(routing) {
		return nil, newTypeError("target context must contain string pairs")
	}
	return &InstallationTarget{store: st, context: slices.Clone(context), routing: slices.Clone(routing)}, nil
}

func checkStore(st store.Store) error {
	if st == nil {
		return newTypeError("store must be a StateStore")
	}
	if value := reflect.ValueOf(st); value.Kind() != reflect.Pointer || value.IsNil() {
		return newTypeError("store must be a StateStore")
	}
	if directory, ok := st.(*store.DirectoryStore); ok {
		if _, constructed := directory.Location(); !constructed {
			return newTypeError("directory state store must come from NewDirectoryStore")
		}
	}
	return nil
}

// Store returns the state store.
func (t *InstallationTarget) Store() store.Store {
	if t == nil {
		return nil
	}
	return t.store
}

// Context returns a copy of the identity context.
func (t *InstallationTarget) Context() map[string]string {
	if t == nil {
		return map[string]string{}
	}
	return contextMap(t.context)
}

// RoutingContext returns a copy of the routing context.
func (t *InstallationTarget) RoutingContext() map[string]string {
	if t == nil {
		return map[string]string{}
	}
	return contextMap(t.routing)
}

// ContextDigest returns the SHA-256 hex digest of the canonical identity context.
func (t *InstallationTarget) ContextDigest() string {
	var entries []contextEntry
	if t != nil {
		entries = t.context
	}
	pairs := make([]jsonx.Value, 0, len(entries))
	for _, entry := range entries {
		pairs = append(pairs, jsonx.Array(jsonx.String(entry.key), jsonx.String(entry.value)))
	}
	sum := sha256.Sum256(append(jsonx.Encode(jsonx.Array(pairs...)), '\n'))
	return hex.EncodeToString(sum[:])
}

func (t *InstallationTarget) valid() bool {
	return t != nil && t.store != nil
}

// Equal reports whether t and o share the same store, by pointer, and equal contexts.
func (t *InstallationTarget) Equal(o *InstallationTarget) bool {
	if t == nil || o == nil {
		return t == o
	}
	return t.store == o.store && slices.Equal(t.context, o.context) && slices.Equal(t.routing, o.routing)
}
