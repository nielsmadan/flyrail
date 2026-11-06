package main

import (
	"encoding/hex"
	"errors"
	"slices"
	"strconv"
	"strings"

	"github.com/nielsmadan/flyrail/go/flyrail"
	"github.com/nielsmadan/flyrail/go/internal/interopbridge"
	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

var errEditsUsage = errors.New("usage: edits --corpus FILE")

type editOperation struct {
	set         bool
	value       string
	acquisition flyrail.Acquisition
}

type editRow struct {
	name       string
	format     string
	initial    []byte
	exists     bool
	operations []editOperation
}

func parseEditRow(row []string) (editRow, error) {
	if len(row) < 5 {
		return editRow{}, errors.New("invalid differential edit row")
	}
	count, err := strconv.Atoi(row[4])
	if err != nil || count < 0 || len(row)-5 != count*3 {
		return editRow{}, errors.New(row[1] + ": invalid edit operation count")
	}
	parsed := editRow{name: row[1], format: row[2]}
	if row[3] != "-" {
		if parsed.initial, err = hex.DecodeString(row[3]); err != nil {
			return editRow{}, errors.New("invalid hexadecimal data")
		}
		parsed.exists = true
	}
	for index := 5; index < len(row); index += 3 {
		operation := editOperation{set: row[index] == "set", value: row[index+1]}
		switch {
		case operation.set:
			if operation.acquisition, err = flyrail.ParseAcquisition(row[index+2]); err != nil {
				return editRow{}, err
			}
		case row[index] != "retire" || index == 5:
			return editRow{}, errors.New(row[1] + ": invalid edit operation")
		}
		parsed.operations = append(parsed.operations, operation)
	}
	return parsed, nil
}

func editContent(format, text string) (flyrail.Content, error) {
	if format == "section" {
		return flyrail.NewSectionContent("managed", text, nil)
	}
	documentFormat, err := flyrail.ParseDocumentFormat(format)
	if err != nil {
		return nil, err
	}
	key, err := flyrail.NewKey("tool")
	if err != nil {
		return nil, err
	}
	selector, err := flyrail.NewSelector(key)
	if err != nil {
		return nil, err
	}
	value, err := flyrail.NewString(text)
	if err != nil {
		return nil, err
	}
	return flyrail.NewStructuredContent(documentFormat, selector, value, nil)
}

func editRequests(row editRow) ([]flyrail.DocumentRequest, error) {
	var requests []flyrail.DocumentRequest
	var claim flyrail.Claim
	for _, operation := range row.operations {
		if !operation.set {
			request, err := flyrail.NewRetireRequest(claim)
			if err != nil {
				return nil, err
			}
			requests = append(requests, request)
			continue
		}
		content, err := editContent(row.format, operation.value)
		if err != nil {
			return nil, err
		}
		request, err := flyrail.NewSetRequest(content, operation.acquisition)
		if err != nil {
			return nil, err
		}
		claim = request.Claim()
		requests = append(requests, request)
	}
	return requests, nil
}

func editRecords(dst []byte, rows []editRow) ([]byte, error) {
	for _, row := range rows {
		requests, err := editRequests(row)
		if err != nil {
			return nil, err
		}
		data, exists := row.initial, row.exists
		var ownership []flyrail.OwnedSelection
		var provenance flyrail.DocumentProvenance
		for step, request := range requests {
			edit, err := flyrail.EditDocument(data, exists, []flyrail.DocumentRequest{request}, ownership, provenance)
			if err != nil {
				return nil, err
			}
			data, exists, ownership, provenance = edit.After, edit.AfterExists, edit.Ownership, edit.Provenance
			prefix := strconv.Itoa(step + 1)
			bytes := []byte("-")
			if exists {
				bytes = data
			}
			statuses := make([]string, 0, len(edit.Observations))
			for _, observation := range edit.Observations {
				statuses = append(statuses, string(observation.Status))
			}
			encoded, err := interopbridge.EncodeEditRecord(provenance)
			if err != nil {
				return nil, err
			}
			dst = appendRecord(dst, "edit", row.name, prefix+"-bytes", bytes)
			dst = appendRecord(dst, "edit", row.name, prefix+"-status", []byte(strings.Join(statuses, ",")))
			dst = appendRecord(dst, "edit", row.name, prefix+"-provenance", encoded)
			dst = appendRecord(dst, "edit", row.name, prefix+"-ownership-count", []byte(strconv.Itoa(len(ownership))))
			for index, item := range ownership {
				if encoded, err = interopbridge.EncodeEditRecord(item); err != nil {
					return nil, err
				}
				dst = appendRecord(dst, "edit", row.name, prefix+"-ownership-"+strconv.Itoa(index), encoded)
			}
		}
	}
	return dst, nil
}

func editsCommand(args []string) ([]byte, error) {
	if len(args) != 2 || args[0] != "--corpus" {
		return nil, errEditsUsage
	}
	text, err := readCorpus(args[1])
	if err != nil {
		return nil, err
	}
	corpus, err := jsonx.Parse([]byte(text))
	if err != nil {
		return nil, err
	}
	cases, ok := corpus.Items()
	if !ok {
		return nil, errInvalidCorpus
	}
	var output []byte
	for _, item := range cases {
		lines, err := editingCase(item)
		if err != nil {
			return nil, err
		}
		output = append(output, lines...)
	}
	return output, nil
}

func fieldText(record jsonx.Value, name string) (string, bool) {
	value, ok := record.Get(name)
	if !ok {
		return "", false
	}
	return value.AsString()
}

func editingSource(item jsonx.Value) ([]byte, bool, error) {
	if raw, ok := fieldText(item, "before_hex"); ok {
		data, err := hex.DecodeString(raw)
		return data, true, err
	}
	text, ok := fieldText(item, "before")
	return []byte(text), ok, nil
}

func editingCase(item jsonx.Value) ([]byte, error) {
	name, _ := fieldText(item, "name")
	format, _ := fieldText(item, "format")
	schema, _ := item.Get("schema")
	isSchema, _ := schema.AsBool()
	source, exists, err := editingSource(item)
	if err != nil {
		return nil, err
	}
	stepsValue, _ := item.Get("steps")
	steps, _ := stepsValue.Items()
	var ownership []flyrail.OwnedSelection
	var provenance flyrail.DocumentProvenance
	var output []byte
	for index, step := range steps {
		observed, observedExists := source, exists
		if raw, ok := step.Get("observed"); ok {
			text, isText := raw.AsString()
			observed, observedExists = []byte(text), isText
		}
		edit, err := editingStep(format, isSchema, step, observed, observedExists, ownership, provenance)
		var record jsonx.Value
		if err != nil {
			var failure *flyrail.ValueError
			if !errors.As(err, &failure) {
				return nil, err
			}
			record = jsonx.Object(jsonx.Member{Key: "error", Value: jsonx.String(failure.Message)})
		} else {
			if record, err = editingRecord(edit); err != nil {
				return nil, err
			}
			source, exists, ownership, provenance = edit.After, edit.AfterExists, edit.Ownership, edit.Provenance
		}
		output = append(output, name+"\t"+strconv.Itoa(index)+"\t"...)
		output = append(jsonx.AppendEncode(output, sortedJSON(record)), '\n')
	}
	return output, nil
}

func editingStep(format string, isSchema bool, step jsonx.Value, source []byte, exists bool, ownership []flyrail.OwnedSelection, provenance flyrail.DocumentProvenance) (flyrail.DocumentEdit, error) {
	records := []jsonx.Value{step}
	if raw, ok := step.Get("requests"); ok {
		records, _ = raw.Items()
	}
	requests := make([]flyrail.DocumentRequest, 0, len(records))
	for _, record := range records {
		request, err := editingRequest(format, record)
		if err != nil {
			return flyrail.DocumentEdit{}, err
		}
		requests = append(requests, request)
	}
	if !isSchema {
		return flyrail.EditDocument(source, exists, requests, ownership, provenance)
	}
	documentFormat, err := flyrail.ParseDocumentFormat(format)
	if err != nil {
		return flyrail.DocumentEdit{}, err
	}
	var schemas []flyrail.DocumentSchema
	if raw, ok := step.Get("schemas"); ok {
		items, _ := raw.Items()
		for _, item := range items {
			key, _ := fieldText(item, "key")
			rawValue, _ := item.Get("value")
			value, err := freezeValue(rawValue, 0)
			if err != nil {
				return flyrail.DocumentEdit{}, err
			}
			scalar, ok := value.(flyrail.Scalar)
			if !ok {
				return flyrail.DocumentEdit{}, errors.New("invalid schema fixture")
			}
			schema, err := flyrail.NewDocumentSchema(documentFormat, key, scalar)
			if err != nil {
				return flyrail.DocumentEdit{}, err
			}
			schemas = append(schemas, schema)
		}
	}
	validate := true
	if raw, ok := step.Get("validate"); ok {
		validate, _ = raw.AsBool()
	}
	result, err := interopbridge.EditSchemaDocument(source, exists, requests, slices.Clone(ownership), provenance, schemas, validate)
	if err != nil {
		return flyrail.DocumentEdit{}, err
	}
	edit, _ := result.(flyrail.DocumentEdit)
	return edit, nil
}

func editingMember(record jsonx.Value) (flyrail.Member, error) {
	var identity flyrail.Value
	var err error
	if semantic, ok := record.Get("semantic_identity"); ok {
		identity, err = flyrail.DecodeValue(jsonx.Encode(semantic))
	} else {
		raw, _ := record.Get("identity")
		identity, err = freezeValue(raw, 0)
	}
	if err != nil {
		return flyrail.Member{}, err
	}
	var key []string
	if raw, ok := record.Get("key"); ok {
		items, _ := raw.Items()
		for _, item := range items {
			text, _ := item.AsString()
			key = append(key, text)
		}
	}
	return flyrail.NewMember(identity, key)
}

func editingSelector(format string, raw jsonx.Value) (flyrail.ClaimSelector, error) {
	if format == "text" {
		start, _ := fieldText(raw, "start")
		end, _ := fieldText(raw, "end")
		return flyrail.NewSectionBoundaries(start, end)
	}
	items, _ := raw.Items()
	parts := make([]flyrail.SelectorPart, 0, len(items))
	for _, item := range items {
		var part flyrail.SelectorPart
		var err error
		if name, ok := item.AsString(); ok {
			part, err = flyrail.NewKey(name)
		} else {
			part, err = editingMember(item)
		}
		if err != nil {
			return nil, err
		}
		parts = append(parts, part)
	}
	return flyrail.NewSelector(parts...)
}

func editingRequest(format string, record jsonx.Value) (flyrail.DocumentRequest, error) {
	rawSelector, _ := record.Get("selector")
	selector, err := editingSelector(format, rawSelector)
	if err != nil {
		return flyrail.DocumentRequest{}, err
	}
	kind := flyrail.OwnershipStructured
	if format == "text" {
		kind = flyrail.OwnershipSection
	}
	claim, err := flyrail.NewClaim(kind, selector)
	if err != nil {
		return flyrail.DocumentRequest{}, err
	}
	if action, _ := fieldText(record, "action"); action == "retire" {
		return flyrail.NewRetireRequest(claim)
	}
	var content flyrail.Content
	if format == "text" {
		marker, ok := fieldText(record, "marker")
		if !ok {
			marker = "fixture"
		}
		text, _ := fieldText(record, "text")
		boundaries, _ := selector.(flyrail.SectionBoundaries)
		if content, err = flyrail.NewSectionContent(marker, text, &boundaries); err != nil {
			return flyrail.DocumentRequest{}, err
		}
	} else if content, err = editingStructured(format, selector, record); err != nil {
		return flyrail.DocumentRequest{}, err
	}
	acquisition := flyrail.AcquireConflict
	if raw, ok := fieldText(record, "acquisition"); ok {
		if acquisition, err = flyrail.ParseAcquisition(raw); err != nil {
			return flyrail.DocumentRequest{}, err
		}
	}
	return flyrail.NewSetRequest(content, acquisition)
}

func editingStructured(format string, selector flyrail.ClaimSelector, record jsonx.Value) (flyrail.Content, error) {
	documentFormat, err := flyrail.ParseDocumentFormat(format)
	if err != nil {
		return nil, err
	}
	var value flyrail.Value
	if semantic, ok := record.Get("semantic_value"); ok {
		value, err = flyrail.DecodeValue(jsonx.Encode(semantic))
	} else {
		raw, _ := record.Get("value")
		value, err = freezeValue(raw, 0)
	}
	if err != nil {
		return nil, err
	}
	var placement *flyrail.Placement
	if raw, ok := record.Get("placement"); ok {
		positionText, _ := fieldText(raw, "position")
		position, err := flyrail.ParsePosition(positionText)
		if err != nil {
			return nil, err
		}
		var anchor *flyrail.Member
		if rawAnchor, ok := raw.Get("anchor"); ok {
			member, err := editingMember(rawAnchor)
			if err != nil {
				return nil, err
			}
			anchor = &member
		}
		built, err := flyrail.NewPlacement(position, anchor)
		if err != nil {
			return nil, err
		}
		placement = &built
	}
	structured, _ := selector.(flyrail.Selector)
	return flyrail.NewStructuredContent(documentFormat, structured, value, placement)
}

func freezeValue(raw jsonx.Value, depth int) (flyrail.Value, error) {
	if depth > 512 {
		return nil, errInvalidCorpus
	}
	switch raw.Kind() {
	case jsonx.KindNull:
		return flyrail.Null{}, nil
	case jsonx.KindBool:
		value, _ := raw.AsBool()
		return flyrail.Bool(value), nil
	case jsonx.KindInt:
		value, _ := raw.AsInt()
		return flyrail.Int(value), nil
	case jsonx.KindFloat:
		value, _ := raw.AsFloat()
		return flyrail.NewFloat(value)
	case jsonx.KindString:
		value, _ := raw.AsString()
		return flyrail.NewString(value)
	case jsonx.KindArray:
		items, _ := raw.Items()
		values := make([]flyrail.Value, 0, len(items))
		for _, item := range items {
			value, err := freezeValue(item, depth+1)
			if err != nil {
				return nil, err
			}
			values = append(values, value)
		}
		return flyrail.NewArray(values...)
	case jsonx.KindObject:
		members, _ := raw.Members()
		values := make([]flyrail.ObjectMember, 0, len(members))
		for _, member := range members {
			value, err := freezeValue(member.Value, depth+1)
			if err != nil {
				return nil, err
			}
			values = append(values, flyrail.ObjectMember{Key: member.Key, Value: value})
		}
		return flyrail.NewObject(values...)
	}
	return nil, errInvalidCorpus
}

func hexOrNull(data []byte, exists bool) jsonx.Value {
	if !exists {
		return jsonx.Null()
	}
	return jsonx.String(hex.EncodeToString(data))
}

func hexValue(data []byte) jsonx.Value {
	return jsonx.String(hex.EncodeToString(data))
}

func member(key string, value jsonx.Value) jsonx.Member {
	return jsonx.Member{Key: key, Value: value}
}

func editingRecord(edit flyrail.DocumentEdit) (jsonx.Value, error) {
	ownership, err := ownedRecords(edit.Ownership)
	if err != nil {
		return jsonx.Value{}, err
	}
	provenance := edit.Provenance
	var containers []jsonx.Value
	for _, container := range provenance.Containers() {
		selector, err := selectorRecord(container.Selector())
		if err != nil {
			return jsonx.Value{}, err
		}
		containers = append(containers, jsonx.Object(member("selector", selector), member("empty_syntax", hexValue(container.EmptySyntax()))))
	}
	var separators []jsonx.Value
	for _, digest := range provenance.TrailingSeparators() {
		separators = append(separators, jsonx.String(digest))
	}
	fields, err := ownedRecords(provenance.SchemaFields())
	if err != nil {
		return jsonx.Value{}, err
	}
	empty, hasEmpty := provenance.EmptyDocument()
	schemaEmpty, hasSchemaEmpty := provenance.SchemaEmptyDocument()
	var observations []jsonx.Value
	for _, observation := range edit.Observations {
		claim, err := claimRecord(observation.Claim)
		if err != nil {
			return jsonx.Value{}, err
		}
		current, err := snapshotRecord(observation.Current)
		if err != nil {
			return jsonx.Value{}, err
		}
		observations = append(observations, jsonx.Object(
			member("claim", claim), member("status", jsonx.String(string(observation.Status))),
			member("current", current), member("message", jsonx.String(observation.Message)),
		))
	}
	return jsonx.Object(
		member("before", hexOrNull(edit.Before, edit.BeforeExists)),
		member("after", hexOrNull(edit.After, edit.AfterExists)),
		member("applicable", jsonx.Bool(edit.Applicable())),
		member("ownership", jsonx.Array(ownership...)),
		member("provenance", jsonx.Object(
			member("empty_document", hexOrNull(empty, hasEmpty)),
			member("containers", jsonx.Array(containers...)),
			member("trailing_separators", jsonx.Array(separators...)),
			member("schema_fields", jsonx.Array(fields...)),
			member("schema_empty_document", hexOrNull(schemaEmpty, hasSchemaEmpty)),
		)),
		member("observations", jsonx.Array(observations...)),
	), nil
}

func ownedRecords(items []flyrail.OwnedSelection) ([]jsonx.Value, error) {
	records := make([]jsonx.Value, 0, len(items))
	for _, item := range items {
		claim, err := claimRecord(item.Claim())
		if err != nil {
			return nil, err
		}
		installed := item.Installed()
		installedRecord, err := snapshotRecord(&installed)
		if err != nil {
			return nil, err
		}
		var baseline *flyrail.SelectionSnapshot
		if value, ok := item.Baseline(); ok {
			baseline = &value
		}
		baselineRecord, err := snapshotRecord(baseline)
		if err != nil {
			return nil, err
		}
		format := jsonx.Null()
		if item.Format() != "" {
			format = jsonx.String(string(item.Format()))
		}
		records = append(records, jsonx.Object(
			member("claim", claim), member("format", format), member("installed", installedRecord),
			member("disposition", jsonx.String(string(item.Disposition()))), member("baseline", baselineRecord),
			member("separator", hexValue(item.Separator())),
		))
	}
	return records, nil
}

func snapshotRecord(snapshot *flyrail.SelectionSnapshot) (jsonx.Value, error) {
	if snapshot == nil {
		return jsonx.Null(), nil
	}
	var value jsonx.Value
	switch typed := snapshot.Value().(type) {
	case flyrail.SnapshotBytes:
		value = jsonx.Object(member("bytes", hexValue(typed.Bytes())))
	case flyrail.SnapshotTyped:
		semantic, err := flyrail.EncodeValue(typed.Value())
		if err != nil {
			return jsonx.Value{}, err
		}
		value = jsonx.Object(member("semantic", hexValue(semantic)))
	}
	position := jsonx.Null()
	if selected, ok := snapshot.Position(); ok {
		previous, err := optionalMemberRecord(selected.Previous())
		if err != nil {
			return jsonx.Value{}, err
		}
		following, err := optionalMemberRecord(selected.Following())
		if err != nil {
			return jsonx.Value{}, err
		}
		position = jsonx.Object(member("previous", previous), member("following", following), member("prefix", hexValue(selected.Prefix())))
	}
	return jsonx.Object(
		member("value", value), member("syntax", hexValue(snapshot.Syntax())),
		member("owned_syntax", hexValue(snapshot.OwnedSyntax())), member("layout", hexValue(snapshot.Layout())),
		member("position", position),
	), nil
}

func optionalMemberRecord(item flyrail.Member, ok bool) (jsonx.Value, error) {
	if !ok {
		return jsonx.Null(), nil
	}
	return memberRecord(item)
}

func memberRecord(item flyrail.Member) (jsonx.Value, error) {
	identity, err := flyrail.EncodeValue(item.Identity())
	if err != nil {
		return jsonx.Value{}, err
	}
	keys := make([]jsonx.Value, 0, len(item.Key()))
	for _, key := range item.Key() {
		keys = append(keys, jsonx.String(key))
	}
	return jsonx.Object(member("identity", hexValue(identity)), member("key", jsonx.Array(keys...))), nil
}

func claimRecord(claim flyrail.Claim) (jsonx.Value, error) {
	var selector jsonx.Value
	switch typed := claim.Selector().(type) {
	case flyrail.SectionBoundaries:
		selector = jsonx.Object(member("start", jsonx.String(typed.Start())), member("end", jsonx.String(typed.End())))
	case flyrail.Selector:
		var err error
		if selector, err = selectorRecord(typed); err != nil {
			return jsonx.Value{}, err
		}
	default:
		return jsonx.Value{}, errors.New("invalid differential claim")
	}
	return jsonx.Object(member("kind", jsonx.String(string(claim.Kind()))), member("selector", selector)), nil
}

func selectorRecord(selector flyrail.Selector) (jsonx.Value, error) {
	parts := make([]jsonx.Value, 0, len(selector.Parts()))
	for _, part := range selector.Parts() {
		switch typed := part.(type) {
		case flyrail.Key:
			parts = append(parts, jsonx.String(typed.Name()))
		case flyrail.Member:
			record, err := memberRecord(typed)
			if err != nil {
				return jsonx.Value{}, err
			}
			parts = append(parts, record)
		}
	}
	return jsonx.Array(parts...), nil
}

func sortedJSON(value jsonx.Value) jsonx.Value {
	if items, ok := value.Items(); ok {
		sorted := make([]jsonx.Value, 0, len(items))
		for _, item := range items {
			sorted = append(sorted, sortedJSON(item))
		}
		return jsonx.Array(sorted...)
	}
	if members, ok := value.Members(); ok {
		sorted := make([]jsonx.Member, 0, len(members))
		for _, item := range members {
			sorted = append(sorted, jsonx.Member{Key: item.Key, Value: sortedJSON(item.Value)})
		}
		slices.SortFunc(sorted, func(a, b jsonx.Member) int { return strings.Compare(a.Key, b.Key) })
		return jsonx.Object(sorted...)
	}
	return value
}
