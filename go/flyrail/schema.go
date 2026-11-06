package flyrail

import (
	"bytes"
	"slices"
)

func isASCIISpace(b byte) bool {
	return b == ' ' || b == '\t' || b == '\n' || b == '\f' || b == '\r'
}

func blankDocument(data []byte) bool {
	return !slices.ContainsFunc(data, func(b byte) bool { return !isASCIISpace(b) })
}

func schemaBackend(data []byte, comments bool) (*jsonDocument, error) {
	text, _, err := decodeDocument(data)
	if err != nil {
		return nil, err
	}
	return parseJSONDocument(text, comments)
}

func schemaSelector(schema DocumentSchema) (Selector, error) {
	content, err := schema.Content()
	if err != nil {
		return Selector{}, err
	}
	return content.selector, nil
}

func matchesSchema(data []byte, exists bool, requirement DocumentSchema) (bool, error) {
	if !exists || blankDocument(data) {
		return false, nil
	}
	backend, err := schemaBackend(data, requirement.format == FormatJSONC)
	if err != nil {
		return false, err
	}
	selector, err := schemaSelector(requirement)
	if err != nil {
		return false, err
	}
	snapshot, err := structuredSnapshot(backend, selector)
	if err != nil || snapshot == nil {
		return false, err
	}
	typed, ok := snapshot.value.(SnapshotTyped)
	return ok && ValuesEqual(typed.value, requirement.value), nil
}

func sameSchema(a, b DocumentSchema) bool {
	return a.format == b.format && a.key == b.key && ValuesEqual(a.value, b.value)
}

func provenanceWithSchema(provenance DocumentProvenance, fields []OwnedSelection, empty document) (DocumentProvenance, error) {
	params := DocumentProvenanceParams{
		Containers:             provenance.Containers(),
		TrailingSeparators:     provenance.TrailingSeparators(),
		SchemaFields:           fields,
		SchemaEmptyDocument:    empty.data,
		HasSchemaEmptyDocument: empty.exists,
	}
	params.EmptyDocument, params.HasEmptyDocument = provenance.EmptyDocument()
	return NewDocumentProvenance(params)
}

func isEmptyJSONObject(data []byte) bool {
	var compact []byte
	for _, b := range data {
		if !isASCIISpace(b) {
			compact = append(compact, b)
		}
	}
	return string(compact) == "{}"
}

func claimIn(claims []Claim, claim Claim) bool {
	return slices.ContainsFunc(claims, claim.Equal)
}

// The scaffolded schema keys belong to no claim and leave with the last one.
func editSchemaDocument(source []byte, sourceExists bool, requests []DocumentRequest, ownership []OwnedSelection, provenance DocumentProvenance, active []DocumentSchema, validate bool) (DocumentEdit, error) {
	data := document{data: source, exists: sourceExists}.clone()
	var requirements []DocumentSchema
	for _, schema := range active {
		index := slices.IndexFunc(requirements, func(existing DocumentSchema) bool { return existing.key == schema.key })
		if index < 0 {
			requirements = append(requirements, schema)
		} else if !sameSchema(requirements[index], schema) {
			return DocumentEdit{}, newValueError("incompatible document schema requirements")
		}
	}
	for _, schema := range requirements {
		content, err := schema.Content()
		if err != nil {
			return DocumentEdit{}, err
		}
		claim, err := ContentClaim(content)
		if err != nil {
			return DocumentEdit{}, err
		}
		overlaps := slices.ContainsFunc(requests, func(request DocumentRequest) bool { return claim.Overlaps(request.claim) }) ||
			slices.ContainsFunc(ownership, func(item OwnedSelection) bool { return claim.Overlaps(item.claim) })
		if overlaps {
			return DocumentEdit{}, newValueError("document schema overlaps an app-owned field")
		}
	}
	_, hasSchemaEmpty := provenance.SchemaEmptyDocument()
	if len(active) == 0 && len(provenance.schemaFields) == 0 && !hasSchemaEmpty {
		return EditDocument(source, sourceExists, requests, ownership, provenance)
	}
	normalized := data
	if data.exists && blankDocument(data.data) {
		normalized = document{data: []byte("{}"), exists: true}
	}
	var emptySource document
	if normalized.exists != data.exists || !bytes.Equal(normalized.data, data.data) {
		emptySource = data
	} else if empty, ok := provenance.SchemaEmptyDocument(); ok {
		emptySource = document{data: empty, exists: true}
	}
	parsed := []byte("{}")
	if normalized.exists {
		parsed = normalized.data
	}
	backend, err := schemaBackend(parsed, true)
	if err != nil {
		return DocumentEdit{}, err
	}
	var additions []DocumentRequest
	if validate {
		for _, schema := range requirements {
			content, err := schema.Content()
			if err != nil {
				return DocumentEdit{}, err
			}
			snapshot, err := structuredSnapshot(backend, content.selector)
			if err != nil {
				return DocumentEdit{}, err
			}
			if snapshot == nil {
				addition, err := NewSetRequest(content, AcquireConflict)
				if err != nil {
					return DocumentEdit{}, err
				}
				additions = append(additions, addition)
				continue
			}
			if typed, ok := snapshot.value.(SnapshotTyped); !ok || !ValuesEqual(typed.value, schema.value) {
				observations := make([]EditObservation, 0, len(requests))
				for _, request := range requests {
					observations = append(observations, EditObservation{Claim: request.claim, Status: EditConflict, Message: "document schema differs"})
				}
				return newDocumentEdit(data, data, ownership, provenance, observations), nil
			}
		}
	}
	var created []OwnedSelection
	for _, item := range provenance.schemaFields {
		selector, ok := item.claim.selector.(Selector)
		if !ok {
			continue
		}
		snapshot, err := structuredSnapshot(backend, selector)
		if err != nil {
			return DocumentEdit{}, err
		}
		replaced := slices.ContainsFunc(requests, func(request DocumentRequest) bool {
			return request.content != nil && request.claim.Overlaps(item.claim)
		})
		if snapshot != nil && snapshot.equal(item.installed) && !replaced {
			created = append(created, item)
		}
	}
	var createdIDs []Claim
	for _, item := range created {
		createdIDs = append(createdIDs, item.claim)
	}
	for _, addition := range additions {
		createdIDs = append(createdIDs, addition.claim)
	}
	ordinary, err := provenanceWithSchema(provenance, nil, document{})
	if err != nil {
		return DocumentEdit{}, err
	}
	edit, err := EditDocument(normalized.data, normalized.exists, slices.Concat(requests, additions), slices.Concat(ownership, created), ordinary)
	if err != nil {
		return DocumentEdit{}, err
	}
	if !edit.Applicable() {
		return newDocumentEdit(data, data, ownership, provenance, edit.Observations), nil
	}
	var fields, owned []OwnedSelection
	for _, item := range edit.Ownership {
		if claimIn(createdIDs, item.claim) {
			fields = append(fields, item)
		} else {
			owned = append(owned, item)
		}
	}
	observations := slices.DeleteFunc(slices.Clone(edit.Observations), func(item EditObservation) bool { return claimIn(createdIDs, item.Claim) })
	after := document{data: edit.After, exists: edit.AfterExists}
	if len(active) > 0 {
		schemaProvenance, err := provenanceWithSchema(edit.Provenance, fields, emptySource)
		if err != nil {
			return DocumentEdit{}, err
		}
		return newDocumentEdit(data, after, owned, schemaProvenance, observations), nil
	}
	retirements := make([]DocumentRequest, 0, len(fields))
	for _, item := range fields {
		retirement, err := NewRetireRequest(item.claim)
		if err != nil {
			return DocumentEdit{}, err
		}
		retirements = append(retirements, retirement)
	}
	candidate, err := EditDocument(edit.After, edit.AfterExists, retirements, edit.Ownership, edit.Provenance)
	if err != nil {
		return DocumentEdit{}, err
	}
	if candidate.Applicable() && len(owned) == 0 && (!candidate.AfterExists || isEmptyJSONObject(candidate.After)) {
		final := document{data: candidate.After, exists: candidate.AfterExists}
		if emptySource.exists {
			final = emptySource
		}
		return newDocumentEdit(data, final, candidate.Ownership, candidate.Provenance, observations), nil
	}
	empty := document{}
	if len(owned) > 0 {
		empty = emptySource
	}
	schemaProvenance, err := provenanceWithSchema(edit.Provenance, nil, empty)
	if err != nil {
		return DocumentEdit{}, err
	}
	return newDocumentEdit(data, after, owned, schemaProvenance, observations), nil
}
