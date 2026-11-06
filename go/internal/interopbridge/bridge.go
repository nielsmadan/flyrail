package interopbridge

import (
	"context"
	"time"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

// BundleModel returns the semantic bytes of a *flyrail.Bundle's canonical artifact, asset and dependency model.
var BundleModel func(bundle any) ([]byte, error)

// RenderedModel returns the semantic bytes of a *flyrail.RenderedBundle's canonical artifact and dependency model.
var RenderedModel func(rendered any) ([]byte, error)

// EditSchemaDocument runs flyrail's schema-scaffolding editor; requests, ownership, provenance and schemas are the flyrail slice and struct types, and the result is a flyrail.DocumentEdit.
var EditSchemaDocument func(source []byte, exists bool, requests, ownership, provenance, schemas any, validate bool) (any, error)

// EncodeEditRecord returns the canonical state encoding of a flyrail.DocumentProvenance or flyrail.OwnedSelection.
var EncodeEditRecord func(record any) ([]byte, error)

// CanonicalStateRecord decodes an Index, Journal, Preparation, Receipt or ResourceRef state record and returns its canonical encoding.
var CanonicalStateRecord func(raw []byte) ([]byte, error)

// TranscodeStateRecord decodes raw as the state record named stateType ("" = the record's own type tag) and returns its canonical encoding and type tag.
var TranscodeStateRecord func(raw []byte, stateType string) (canonical []byte, tag string, err error)

// ResourceLock acquires the store lock of the resource at the canonical destination with the given kind; st is a store.Store and failures are *flyrail.TargetError.
var ResourceLock func(ctx context.Context, st any, destination, kind string, timeout time.Duration) (unlock func() error, err error)

// Admit reserves the resource at the canonical destination in st, a store.Store, while the caller holds its lock; failures are *flyrail.TargetError.
var Admit func(ctx context.Context, st any, destination, kind string) error

// ObserveResource captures the canonical resource reference, complete revision and existing ancestors of destination as state records; failures are *flyrail.TargetError.
var ObserveResource func(ctx context.Context, destination, kind string) (resource, revision jsonx.Value, ancestors []jsonx.Value, err error)
