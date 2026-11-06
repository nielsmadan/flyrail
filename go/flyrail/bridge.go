package flyrail

import (
	"context"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/interopbridge"
	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

func init() {
	interopbridge.BundleModel = bridgeBundleModel
	interopbridge.RenderedModel = bridgeRenderedModel
	interopbridge.EditSchemaDocument = bridgeEditSchemaDocument
	interopbridge.EncodeEditRecord = bridgeEncodeEditRecord
	interopbridge.CanonicalStateRecord = bridgeCanonicalStateRecord
	interopbridge.TranscodeStateRecord = bridgeTranscodeStateRecord
	interopbridge.ResourceLock = bridgeResourceLock
	interopbridge.Admit = bridgeAdmit
	interopbridge.ObserveResource = bridgeObserveResource
}

func bridgeBundleModel(bundle any) ([]byte, error) {
	b, ok := bundle.(*Bundle)
	if !ok || b == nil {
		return nil, newValueError("bundle model requires a non-nil *Bundle")
	}
	return EncodeValue(bundleModel(b.artifacts, b.assets, b.dependencies))
}

func bridgeRenderedModel(rendered any) ([]byte, error) {
	r, ok := rendered.(*RenderedBundle)
	if !ok || r == nil {
		return nil, newValueError("rendered model requires a non-nil *RenderedBundle")
	}
	return EncodeValue(renderedModel(r.artifacts, r.dependencies))
}

func bridgeEditSchemaDocument(source []byte, exists bool, requests, ownership, provenance, schemas any, validate bool) (any, error) {
	typedRequests, okRequests := requests.([]DocumentRequest)
	typedOwnership, okOwnership := ownership.([]OwnedSelection)
	typedProvenance, okProvenance := provenance.(DocumentProvenance)
	typedSchemas, okSchemas := schemas.([]DocumentSchema)
	if !okRequests || !okOwnership || !okProvenance || !okSchemas {
		return nil, newValueError("schema editing requires flyrail requests, ownership, provenance and schemas")
	}
	return editSchemaDocument(source, exists, typedRequests, typedOwnership, typedProvenance, typedSchemas, validate)
}

func bridgeEncodeEditRecord(record any) ([]byte, error) {
	switch value := record.(type) {
	case DocumentProvenance:
		return encodeRecord(value), nil
	case OwnedSelection:
		return encodeRecord(value), nil
	}
	return nil, newValueError("edit records are DocumentProvenance or OwnedSelection values")
}

func canonicalStateRecord[T stateRecord](raw []byte, from func(jsonx.Value) (T, error)) ([]byte, error) {
	value, err := decodeState(raw, from)
	if err != nil {
		return nil, err
	}
	return encodeRecord(value), nil
}

func bridgeCanonicalStateRecord(raw []byte) ([]byte, error) {
	record, err := parseState(raw)
	if err != nil {
		return nil, err
	}
	switch tag, _ := record.Get("type"); stringOf(tag) {
	case "Index":
		return canonicalStateRecord(raw, indexFromState)
	case "Journal":
		return canonicalStateRecord(raw, journalFromState)
	case "Preparation":
		return canonicalStateRecord(raw, preparationFromState)
	case "Receipt":
		return canonicalStateRecord(raw, receiptFromState)
	case "ResourceRef":
		return canonicalStateRecord(raw, resourceRefFromState)
	}
	return nil, unknownStateRecord()
}

func transcodeAs[T stateRecord](from func(jsonx.Value) (T, error)) func([]byte, string) ([]byte, error) {
	return func(raw []byte, tag string) ([]byte, error) {
		value, err := decodeMetadata(raw, tag, from)
		if err != nil {
			return nil, err
		}
		return encodeRecord(value), nil
	}
}

var stateTranscoders = map[string]func([]byte, string) ([]byte, error){
	"Ancestor":           transcodeAs(ancestorFromState),
	"Claim":              transcodeAs(claimFromState),
	"CreatedContainer":   transcodeAs(createdContainerFromState),
	"DocumentProvenance": transcodeAs(documentProvenanceFromState),
	"Generation":         transcodeAs(generationFromState),
	"Index":              transcodeAs(indexFromState),
	"Journal":            transcodeAs(journalFromState),
	"Node":               transcodeAs(nodeFromState),
	"OwnedClaim":         transcodeAs(ownedClaimFromState),
	"OwnedSelection":     transcodeAs(ownedSelectionFromState),
	"Preparation":        transcodeAs(preparationFromState),
	"Receipt":            transcodeAs(receiptFromState),
	"ResourceRef":        transcodeAs(resourceRefFromState),
	"Revision":           transcodeAs(revisionFromState),
}

func bridgeTranscodeStateRecord(raw []byte, stateType string) ([]byte, string, error) {
	tag := stateType
	if tag == "" {
		record, err := parseState(raw)
		if err != nil {
			return nil, "", err
		}
		recorded, _ := record.Get("type")
		tag = stringOf(recorded)
		if stateTranscoders[tag] == nil {
			return nil, "", unknownStateRecord()
		}
	}
	transcode := stateTranscoders[tag]
	if transcode == nil {
		return nil, "", newValueError("unsupported state type: " + stateType)
	}
	canonical, err := transcode(raw, tag)
	if err != nil {
		return nil, "", err
	}
	return canonical, tag, nil
}

func bridgeReference(st any, destination, kind string) (store.Store, ResourceRef, error) {
	selected, ok := st.(store.Store)
	if st != nil && !ok {
		return nil, ResourceRef{}, newValueError("interop resource operations require a store.Store")
	}
	parsed, err := ParseResourceKind(kind)
	if err != nil {
		return nil, ResourceRef{}, err
	}
	reference, err := newResourceRef(destination, parsed, stateSchemaVersion)
	if err != nil {
		return nil, ResourceRef{}, err
	}
	return selected, reference, nil
}

func bridgeResourceLock(ctx context.Context, st any, destination, kind string, timeout time.Duration) (func() error, error) {
	selected, reference, err := bridgeReference(st, destination, kind)
	if err != nil {
		return nil, err
	}
	if selected == nil {
		return nil, newValueError("interop resource operations require a store.Store")
	}
	held, failed := acquireResourceLock(ctx, selected, reference, timeout)
	if failed != nil {
		return nil, failed
	}
	return held.Unlock, nil
}

func bridgeAdmit(ctx context.Context, st any, destination, kind string) error {
	selected, reference, err := bridgeReference(st, destination, kind)
	if err != nil {
		return err
	}
	if selected == nil {
		return newValueError("interop resource operations require a store.Store")
	}
	if failed := admit(ctx, selected, reference); failed != nil {
		return failed
	}
	return nil
}

func bridgeObserveResource(ctx context.Context, destination, kind string) (jsonx.Value, jsonx.Value, []jsonx.Value, error) {
	authority, err := NewResourceAuthority(ctx, destination)
	if err != nil {
		return jsonx.Value{}, jsonx.Value{}, nil, errorDetail(err)
	}
	_, reference, err := bridgeReference(nil, authority.destination, kind)
	if err != nil {
		return jsonx.Value{}, jsonx.Value{}, nil, err
	}
	observed, failed := observe(reference.Destination)
	if failed != nil {
		return jsonx.Value{}, jsonx.Value{}, nil, failed
	}
	parents, failed := captureAncestors(reference.Destination)
	if failed != nil {
		return jsonx.Value{}, jsonx.Value{}, nil, failed
	}
	ancestors := make([]jsonx.Value, 0, len(parents))
	for _, parent := range parents {
		ancestors = append(ancestors, parent.toState())
	}
	return reference.toState(), observed.toState(), ancestors, nil
}
