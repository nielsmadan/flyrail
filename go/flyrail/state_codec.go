package flyrail

import (
	"bytes"
	"encoding/hex"
	"errors"
	"slices"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

type stateRecord interface {
	toState() jsonx.Value
}

func encodeState(record jsonx.Value) []byte {
	return append(jsonx.Encode(canonicalJSON(record)), '\n')
}

func encodeRecord(value stateRecord) []byte {
	return encodeState(value.toState())
}

func canonicalJSON(value jsonx.Value) jsonx.Value {
	if items, ok := value.Items(); ok {
		sorted := make([]jsonx.Value, 0, len(items))
		for _, item := range items {
			sorted = append(sorted, canonicalJSON(item))
		}
		return jsonx.Array(sorted...)
	}
	if members, ok := value.Members(); ok {
		sorted := make([]jsonx.Member, 0, len(members))
		for _, member := range members {
			sorted = append(sorted, jsonx.Member{Key: member.Key, Value: canonicalJSON(member.Value)})
		}
		slices.SortFunc(sorted, func(a, b jsonx.Member) int { return strings.Compare(a.Key, b.Key) })
		return jsonx.Object(sorted...)
	}
	return value
}

func sameCanonical(a, b jsonx.Value) bool {
	return jsonx.Equal(canonicalJSON(a), canonicalJSON(b))
}

func invalidStateRecord() *ValueError {
	return newValueError("invalid state record")
}

func unknownStateRecord() *ValueError {
	return newValueError("unknown state record")
}

func fieldsMismatch() *ValueError {
	return newValueError("state record fields do not match schema")
}

func pythonTypeError(err error) error {
	if failure, ok := errors.AsType[*ValueError](err); ok && failure.pythonTypeError {
		return invalidStateRecord()
	}
	return err
}

func parseState(data []byte) (jsonx.Value, error) {
	if !utf8.Valid(data) {
		return jsonx.Value{}, invalidStateRecord()
	}
	raw, err := jsonx.Parse(bytes.TrimPrefix(data, []byte{0xEF, 0xBB, 0xBF}))
	if err != nil {
		return jsonx.Value{}, valueErrorFromJSON(err)
	}
	return raw, nil
}

func decodeState[T stateRecord](data []byte, from func(jsonx.Value) (T, error)) (T, error) {
	var zero T
	raw, err := parseState(data)
	if err != nil {
		return zero, err
	}
	value, err := from(raw)
	if err != nil {
		return zero, pythonTypeError(err)
	}
	if !sameCanonical(value.toState(), raw) {
		return zero, newValueError("state record is not canonical")
	}
	return value, nil
}

func decodeMetadata[T stateRecord](data []byte, tag string, from func(jsonx.Value) (T, error)) (T, error) {
	var zero T
	raw, err := parseState(data)
	if err != nil {
		return zero, err
	}
	if _, err := decoded(raw); err != nil {
		return zero, pythonTypeError(err)
	}
	if recorded, _ := raw.Get("type"); stringOf(recorded) != tag {
		canonical, err := canonicalState(raw)
		if err != nil {
			return zero, pythonTypeError(err)
		}
		if !sameCanonical(canonical, raw) {
			return zero, newValueError("state record is not canonical")
		}
		return zero, newValueError("metadata record has the wrong type")
	}
	return decodeState(data, from)
}

func stringOf(value jsonx.Value) string {
	text, _ := value.AsString()
	return text
}

func canonicalState(value jsonx.Value) (jsonx.Value, error) {
	switch value.Kind() {
	case jsonx.KindFloat:
		return jsonx.Value{}, newValueError("invalid state scalar")
	case jsonx.KindArray:
		items, _ := value.Items()
		result := make([]jsonx.Value, 0, len(items))
		for _, item := range items {
			canonical, err := canonicalState(item)
			if err != nil {
				return jsonx.Value{}, err
			}
			result = append(result, canonical)
		}
		return jsonx.Array(result...), nil
	case jsonx.KindObject:
		members, _ := value.Members()
		switch {
		case onlyMember(members, "value"):
			semantic, err := valueFromRecord(members[0].Value)
			if err != nil {
				return jsonx.Value{}, err
			}
			return encodedSemantic(semantic), nil
		case onlyMember(members, "bytes"):
			text, ok := members[0].Value.AsString()
			if !ok {
				return jsonx.Value{}, unknownStateRecord()
			}
			data, err := parseHexBytes(text)
			if err != nil {
				return jsonx.Value{}, err
			}
			return bytesState(data), nil
		case onlyMember(members, "path"):
			path, err := decodePath(value)
			if err != nil {
				return jsonx.Value{}, err
			}
			return encodedPath(path), nil
		}
		result := make([]jsonx.Member, 0, len(members))
		for _, member := range members {
			canonical, err := canonicalState(member.Value)
			if err != nil {
				return jsonx.Value{}, err
			}
			result = append(result, jsonx.Member{Key: member.Key, Value: canonical})
		}
		return jsonx.Object(result...), nil
	}
	return value, nil
}

func onlyMember(members []jsonx.Member, name string) bool {
	return len(members) == 1 && members[0].Key == name
}

func hasMembers(members []jsonx.Member, first, second string) bool {
	has := func(name string) bool {
		return slices.ContainsFunc(members, func(member jsonx.Member) bool { return member.Key == name })
	}
	return len(members) == 2 && has(first) && has(second)
}

func stateRecordObject(typeName string, fields ...jsonx.Member) jsonx.Value {
	return jsonx.Object(
		jsonx.Member{Key: "type", Value: jsonx.String(typeName)},
		jsonx.Member{Key: "fields", Value: jsonx.Object(fields...)},
	)
}

func member(key string, value jsonx.Value) jsonx.Member {
	return jsonx.Member{Key: key, Value: value}
}

func stateTag(value jsonx.Value, key string) (string, bool, error) {
	item, ok := value.Get(key)
	if !ok {
		return "", false, nil
	}
	switch item.Kind() {
	case jsonx.KindString:
		text, _ := item.AsString()
		return text, true, nil
	case jsonx.KindArray:
		return "", false, newTypeError("unhashable type: 'list'")
	case jsonx.KindObject:
		return "", false, newTypeError("unhashable type: 'dict'")
	}
	return "", false, nil
}

func recordFields(value jsonx.Value, typeName string, expected ...string) ([]jsonx.Member, error) {
	tag, ok, err := stateTag(value, "type")
	if err != nil {
		return nil, err
	}
	members, _ := value.Members()
	if !ok || tag != typeName || len(members) != 2 {
		return nil, unknownStateRecord()
	}
	rawFields, _ := value.Get("fields")
	fields, ok := rawFields.Members()
	if !ok {
		return nil, invalidStateRecord()
	}
	if len(fields) != len(expected) || slices.ContainsFunc(expected, func(name string) bool {
		return !slices.ContainsFunc(fields, func(field jsonx.Member) bool { return field.Key == name })
	}) {
		return nil, fieldsMismatch()
	}
	for _, field := range fields {
		if _, err := decoded(field.Value); err != nil {
			return nil, err
		}
	}
	return fields, nil
}

func stateField(fields []jsonx.Member, name string) jsonx.Value {
	for _, field := range fields {
		if field.Key == name {
			return field.Value
		}
	}
	return jsonx.Value{}
}

type decodedKind uint8

const (
	decodedScalar decodedKind = iota
	decodedTuple
	decodedBytes
	decodedPath
	decodedEnum
	decodedRecord
	decodedSemantic
)

type decodedValue struct {
	kind     decodedKind
	data     []byte
	path     string
	semantic Value
}

func decoded(value jsonx.Value) (decodedValue, error) {
	switch value.Kind() {
	case jsonx.KindFloat:
		return decodedValue{}, newValueError("invalid state scalar")
	case jsonx.KindArray:
		items, _ := value.Items()
		for _, item := range items {
			if _, err := decoded(item); err != nil {
				return decodedValue{}, err
			}
		}
		return decodedValue{kind: decodedTuple}, nil
	case jsonx.KindObject:
		return decodedObject(value)
	}
	return decodedValue{kind: decodedScalar}, nil
}

var stateEnumNames = []string{"Ownership", "DocumentFormat", "Disposition", "ResourceKind"}

var stateRecordTags = []string{
	"Claim", "DocumentSchema", "Key", "Member", "SectionBoundaries", "Selector", "ArrayPosition",
	"CreatedContainer", "DocumentProvenance", "OwnedSelection", "SelectionSnapshot", "Ancestor", "Node",
	"Revision", "ResourceRef", "OwnedClaim", "Receipt", "Generation", "Index", "Journal", "Preparation",
}

func decodedObject(value jsonx.Value) (decodedValue, error) {
	members, _ := value.Members()
	if onlyMember(members, "value") {
		semantic, err := valueFromRecord(members[0].Value)
		if err != nil {
			return decodedValue{}, err
		}
		return decodedValue{kind: decodedSemantic, semantic: semantic}, nil
	}
	if text, ok := members0String(members, "bytes"); ok {
		data, err := parseHexBytes(text)
		if err != nil {
			return decodedValue{}, err
		}
		return decodedValue{kind: decodedBytes, data: data}, nil
	}
	if _, ok := members0String(members, "path"); ok {
		path, err := decodePath(value)
		if err != nil {
			return decodedValue{}, err
		}
		return decodedValue{kind: decodedPath, path: path}, nil
	}
	if hasMembers(members, "enum", "value") {
		tag, ok, err := stateTag(value, "enum")
		if err != nil {
			return decodedValue{}, err
		}
		if ok {
			if slices.Contains(stateEnumNames, tag) {
				if _, err := enumFromState(value, tag); err != nil {
					return decodedValue{}, err
				}
				return decodedValue{kind: decodedEnum}, nil
			}
			if slices.Contains(stateRecordTags, tag) {
				return decodedValue{kind: decodedRecord}, nil
			}
		}
		return decodedValue{}, unknownStateRecord()
	}
	if hasMembers(members, "type", "fields") {
		tag, ok, err := stateTag(value, "type")
		if err != nil {
			return decodedValue{}, err
		}
		if ok {
			if slices.Contains(stateEnumNames, tag) {
				return decodedValue{}, invalidStateRecord()
			}
			if slices.Contains(stateRecordTags, tag) {
				if err := checkStateRecord(tag, value); err != nil {
					return decodedValue{}, err
				}
				return decodedValue{kind: decodedRecord}, nil
			}
		}
	}
	return decodedValue{}, unknownStateRecord()
}

func members0String(members []jsonx.Member, name string) (string, bool) {
	if !onlyMember(members, name) {
		return "", false
	}
	return members[0].Value.AsString()
}

func checkStateRecord(tag string, value jsonx.Value) error {
	var err error
	switch tag {
	case "Claim":
		_, err = claimFromState(value)
	case "DocumentSchema":
		_, err = documentSchemaFromState(value)
	case "Key":
		_, err = keyFromState(value)
	case "Member":
		_, err = memberFromState(value)
	case "SectionBoundaries":
		_, err = sectionBoundariesFromState(value)
	case "Selector":
		_, err = selectorFromState(value)
	case "ArrayPosition":
		_, err = arrayPositionFromState(value)
	case "CreatedContainer":
		_, err = createdContainerFromState(value)
	case "DocumentProvenance":
		_, err = documentProvenanceFromState(value)
	case "OwnedSelection":
		_, err = ownedSelectionFromState(value)
	case "SelectionSnapshot":
		_, err = selectionSnapshotFromState(value)
	case "Ancestor":
		_, err = ancestorFromState(value)
	case "Node":
		_, err = nodeFromState(value)
	case "Revision":
		_, err = revisionFromState(value)
	case "ResourceRef":
		_, err = resourceRefFromState(value)
	case "OwnedClaim":
		_, err = ownedClaimFromState(value)
	case "Receipt":
		_, err = receiptFromState(value)
	case "Generation":
		_, err = generationFromState(value)
	case "Index":
		_, err = indexFromState(value)
	case "Journal":
		_, err = journalFromState(value)
	case "Preparation":
		_, err = preparationFromState(value)
	}
	return err
}

func wrongKind(value jsonx.Value, wrong error) error {
	if _, err := decoded(value); err != nil {
		return err
	}
	return wrong
}

func classified[T any](value jsonx.Value, wrong error, from func(jsonx.Value) (T, error)) (T, error) {
	result, err := from(value)
	if err != nil {
		var zero T
		return zero, wrongKind(value, wrong)
	}
	return result, nil
}

func expectBytes(value jsonx.Value, wrong error) ([]byte, error) {
	item, err := decoded(value)
	if err != nil {
		return nil, err
	}
	if item.kind != decodedBytes {
		return nil, wrong
	}
	return item.data, nil
}

func expectPath(value jsonx.Value, wrong error) (string, error) {
	item, err := decoded(value)
	if err != nil {
		return "", err
	}
	if item.kind != decodedPath {
		return "", wrong
	}
	return item.path, nil
}

func textLike(value jsonx.Value, wrong error) (string, error) {
	if text, ok := value.AsString(); ok {
		return text, nil
	}
	return "", wrongKind(value, wrong)
}

func semanticOf(value jsonx.Value, wrong error) (Value, error) {
	item, err := decoded(value)
	if err != nil {
		return nil, err
	}
	if item.kind != decodedSemantic {
		return nil, wrong
	}
	return item.semantic, nil
}

func scalarSemantic(value jsonx.Value, wrong error) (Scalar, error) {
	item, err := decoded(value)
	if err != nil {
		return nil, err
	}
	scalar, ok := item.semantic.(Scalar)
	if item.kind != decodedSemantic || !ok {
		return nil, wrong
	}
	return scalar, nil
}

func metadataCount(value jsonx.Value, wrong error) (count, error) {
	if small, ok := value.AsInt(); ok {
		return count{small: small}, nil
	}
	if big, ok := value.BigIntText(); ok {
		return count{big: big}, nil
	}
	return count{}, wrongKind(value, wrong)
}

func schemaVersionOf(value jsonx.Value, wrong error) (int64, error) {
	if version, ok := value.AsInt(); ok {
		return version, nil
	}
	if _, ok := value.BigIntText(); ok {
		return 0, wrong
	}
	return 0, wrongKind(value, wrong)
}

func sequence[T any](value jsonx.Value, decodeItem func(jsonx.Value) (T, error)) ([]T, error) {
	items, ok := value.Items()
	if !ok {
		return nil, wrongKind(value, invalidStateRecord())
	}
	result := make([]T, 0, len(items))
	for _, item := range items {
		decodedItem, err := decodeItem(item)
		if err != nil {
			return nil, err
		}
		result = append(result, decodedItem)
	}
	return result, nil
}

func optional[T any](value jsonx.Value, decodeItem func(jsonx.Value) (T, error)) (*T, error) {
	if value.IsNull() {
		return nil, nil
	}
	item, err := decodeItem(value)
	if err != nil {
		return nil, err
	}
	return &item, nil
}

func optionalState[T stateRecord](value *T) jsonx.Value {
	if value == nil {
		return jsonx.Null()
	}
	return (*value).toState()
}

func optionalBytes(data []byte, present bool) jsonx.Value {
	if !present {
		return jsonx.Null()
	}
	return bytesState(data)
}

func parseHexBytes(text string) ([]byte, error) {
	invalid := func(position int) error {
		return newValueError("non-hexadecimal number found in fromhex() arg at position " + strconv.Itoa(position))
	}
	out := []byte{}
	index := 0
	for {
		for index < len(text) && strings.IndexByte(" \t\n\r\v\f", text[index]) >= 0 {
			index++
		}
		if index == len(text) {
			return out, nil
		}
		high, ok := hexNibble(text[index])
		if !ok {
			return nil, invalid(index)
		}
		index++
		if index == len(text) {
			return nil, invalid(index)
		}
		low, ok := hexNibble(text[index])
		if !ok {
			return nil, invalid(index)
		}
		index++
		out = append(out, high<<4|low)
	}
}

func bytesState(data []byte) jsonx.Value {
	return jsonx.Object(member("bytes", jsonx.String(hex.EncodeToString(data))))
}

func decodePath(value jsonx.Value) (string, error) {
	members, _ := value.Members()
	text, ok := members0String(members, "path")
	if !ok {
		return "", unknownStateRecord()
	}
	if !pathx.IsAbsolute(text) || hasParentDir(text) {
		return "", newValueError("state paths must be absolute without traversal")
	}
	return text, nil
}

func encodedPath(path string) jsonx.Value {
	return jsonx.Object(member("path", jsonx.String(posixPath(path))))
}

func encodedSemantic(value Value) jsonx.Value {
	return jsonx.Object(member("value", semanticRecord(value)))
}

func enumText(value jsonx.Value, enumName string) (string, error) {
	members, ok := value.Members()
	if !ok || len(members) != 2 {
		return "", unknownStateRecord()
	}
	tag, present, err := stateTag(value, "enum")
	if err != nil {
		return "", err
	}
	if !present || tag != enumName {
		return "", unknownStateRecord()
	}
	item, ok := value.Get("value")
	if !ok {
		return "", unknownStateRecord()
	}
	if text, isString := item.AsString(); isString {
		return text, nil
	}
	return "", newValueError(jsonx.PythonRepr(item) + " is not a valid " + enumName)
}

func encodedEnum(enumName, value string) jsonx.Value {
	return jsonx.Object(member("enum", jsonx.String(enumName)), member("value", jsonx.String(value)))
}

func enumFromState(value jsonx.Value, enumName string) (string, error) {
	text, err := enumText(value, enumName)
	if err != nil {
		return "", err
	}
	switch enumName {
	case "Ownership":
		_, err = ParseOwnership(text)
	case "DocumentFormat":
		_, err = ParseDocumentFormat(text)
	case "Disposition":
		_, err = ParseDisposition(text)
	case "ResourceKind":
		_, err = ParseResourceKind(text)
	}
	return text, err
}

func ownershipFromState(value jsonx.Value) (Ownership, error) {
	text, err := enumFromState(value, "Ownership")
	return Ownership(text), err
}

func documentFormatFromState(value jsonx.Value) (DocumentFormat, error) {
	text, err := enumFromState(value, "DocumentFormat")
	return DocumentFormat(text), err
}

func dispositionFromState(value jsonx.Value) (Disposition, error) {
	text, err := enumFromState(value, "Disposition")
	return Disposition(text), err
}

func resourceKindFromState(value jsonx.Value) (ResourceKind, error) {
	text, err := enumFromState(value, "ResourceKind")
	return ResourceKind(text), err
}

func (o Ownership) toState() jsonx.Value      { return encodedEnum("Ownership", string(o)) }
func (f DocumentFormat) toState() jsonx.Value { return encodedEnum("DocumentFormat", string(f)) }
func (d Disposition) toState() jsonx.Value    { return encodedEnum("Disposition", string(d)) }
func (k ResourceKind) toState() jsonx.Value   { return encodedEnum("ResourceKind", string(k)) }

func pair(value jsonx.Value) (jsonx.Value, jsonx.Value, error) {
	items, ok := value.Items()
	if !ok {
		return jsonx.Value{}, jsonx.Value{}, wrongKind(value, invalidStateRecord())
	}
	for _, item := range items {
		if _, err := decoded(item); err != nil {
			return jsonx.Value{}, jsonx.Value{}, err
		}
	}
	if len(items) > 2 {
		return jsonx.Value{}, jsonx.Value{}, newValueError("too many values to unpack (expected 2)")
	}
	if len(items) < 2 {
		return jsonx.Value{}, jsonx.Value{}, newValueError("not enough values to unpack (expected 2, got " + strconv.Itoa(len(items)) + ")")
	}
	return items[0], items[1], nil
}

func hexIdentity(value jsonx.Value) (string, error) {
	return textLike(value, newValueError("invalid hexadecimal identity"))
}

func requirementFromState(value jsonx.Value) (requirement, error) {
	first, second, err := pair(value)
	if err != nil {
		return requirement{}, err
	}
	path, err := expectPath(first, newValueError("expected an absolute concrete path"))
	if err != nil {
		return requirement{}, err
	}
	identifier, err := hexIdentity(second)
	if err != nil {
		return requirement{}, err
	}
	return requirement{path: path, identifier: identifier}, nil
}

func (r requirement) toState() jsonx.Value {
	return jsonx.Array(encodedPath(r.path), jsonx.String(r.identifier))
}

func membershipFromState(value jsonx.Value) (membership, error) {
	first, second, err := pair(value)
	if err != nil {
		return membership{}, err
	}
	reference, err := classified(first, newTypeError("generation requires resource references"), resourceRefFromState)
	if err != nil {
		return membership{}, err
	}
	identifiers, err := sequence(second, hexIdentity)
	if err != nil {
		return membership{}, err
	}
	return membership{resource: reference, claimIDs: identifiers}, nil
}

func selectorPartFromState(value jsonx.Value) (SelectorPart, error) {
	tag, _ := value.Get("type")
	switch stringOf(tag) {
	case "Key":
		return keyFromState(value)
	case "Member":
		return memberFromState(value)
	}
	return nil, wrongKind(value, invalidStateRecord())
}

func claimSelectorFromState(value jsonx.Value, kind Ownership) (ClaimSelector, error) {
	if value.IsNull() {
		return WholeSelector{}, nil
	}
	if path, ok := value.AsString(); ok {
		return SubtreeSelector(path), nil
	}
	tag, _ := value.Get("type")
	switch stringOf(tag) {
	case "SectionBoundaries":
		return sectionBoundariesFromState(value)
	case "Selector":
		return selectorFromState(value)
	}
	var wrong error
	switch kind {
	case OwnershipStructured:
		wrong = newTypeError("structured claims require a Selector")
	case OwnershipSection:
		wrong = newTypeError("section claims require SectionBoundaries or a marker name")
	case OwnershipSubtree:
		wrong = newTypeError("subtree claims require a string selector")
	default:
		wrong = newValueError("whole-resource claims cannot have selectors")
	}
	return nil, wrongKind(value, wrong)
}

func snapshotValueFromState(value jsonx.Value) (SnapshotValue, error) {
	item, err := decoded(value)
	if err != nil {
		return nil, err
	}
	switch item.kind {
	case decodedBytes:
		return SnapshotBytes{data: item.data}, nil
	case decodedSemantic:
		return SnapshotTyped{value: item.semantic}, nil
	}
	return nil, invalidStateRecord()
}

func snapshotValueToState(value SnapshotValue) jsonx.Value {
	switch v := value.(type) {
	case SnapshotBytes:
		return bytesState(v.data)
	case SnapshotTyped:
		return encodedSemantic(v.value)
	}
	return jsonx.Null()
}

func (k Key) toState() jsonx.Value {
	return stateRecordObject("Key", member("name", jsonx.String(k.name)))
}

func keyFromState(value jsonx.Value) (Key, error) {
	fields, err := recordFields(value, "Key", "name")
	if err != nil {
		return Key{}, err
	}
	name, err := textLike(stateField(fields, "name"), newTypeError("selector key must be a string"))
	if err != nil {
		return Key{}, err
	}
	return NewKey(name)
}

func stringsState(values []string) jsonx.Value {
	items := make([]jsonx.Value, 0, len(values))
	for _, value := range values {
		items = append(items, jsonx.String(value))
	}
	return jsonx.Array(items...)
}

func listState[T any](values []T, encode func(T) jsonx.Value) jsonx.Value {
	items := make([]jsonx.Value, 0, len(values))
	for _, value := range values {
		items = append(items, encode(value))
	}
	return jsonx.Array(items...)
}

func (m Member) toState() jsonx.Value {
	return stateRecordObject("Member",
		member("identity", encodedSemantic(m.identity)),
		member("key", stringsState(m.key)),
	)
}

func memberFromState(value jsonx.Value) (Member, error) {
	fields, err := recordFields(value, "Member", "identity", "key")
	if err != nil {
		return Member{}, err
	}
	identity, err := semanticOf(stateField(fields, "identity"), newTypeError("values must be Scalar, ObjectValue or ArrayValue"))
	if err != nil {
		return Member{}, err
	}
	keys, err := sequence(stateField(fields, "key"), func(item jsonx.Value) (string, error) {
		return textLike(item, newTypeError("selector key must be a string"))
	})
	if err != nil {
		return Member{}, err
	}
	return NewMember(identity, keys)
}

func (b SectionBoundaries) toState() jsonx.Value {
	return stateRecordObject("SectionBoundaries",
		member("end", jsonx.String(b.end)),
		member("start", jsonx.String(b.start)),
	)
}

func sectionBoundariesFromState(value jsonx.Value) (SectionBoundaries, error) {
	fields, err := recordFields(value, "SectionBoundaries", "start", "end")
	if err != nil {
		return SectionBoundaries{}, err
	}
	start, err := textLike(stateField(fields, "start"), newTypeError("section boundary must be a string"))
	if err != nil {
		return SectionBoundaries{}, err
	}
	end, err := textLike(stateField(fields, "end"), newTypeError("section boundary must be a string"))
	if err != nil {
		return SectionBoundaries{}, err
	}
	return NewSectionBoundaries(start, end)
}

func selectorPartToState(part SelectorPart) jsonx.Value {
	switch p := part.(type) {
	case Key:
		return p.toState()
	case Member:
		return p.toState()
	}
	return jsonx.Null()
}

func (s Selector) toState() jsonx.Value {
	return stateRecordObject("Selector", member("parts", listState(s.parts, selectorPartToState)))
}

func selectorFromState(value jsonx.Value) (Selector, error) {
	fields, err := recordFields(value, "Selector", "parts")
	if err != nil {
		return Selector{}, err
	}
	parts, err := sequence(stateField(fields, "parts"), selectorPartFromState)
	if err != nil {
		return Selector{}, err
	}
	return NewSelector(parts...)
}

func (s DocumentSchema) toState() jsonx.Value {
	return stateRecordObject("DocumentSchema",
		member("format", s.format.toState()),
		member("key", jsonx.String(s.key)),
		member("value", encodedSemantic(s.value)),
	)
}

func documentSchemaFromState(value jsonx.Value) (DocumentSchema, error) {
	fields, err := recordFields(value, "DocumentSchema", "format", "key", "value")
	if err != nil {
		return DocumentSchema{}, err
	}
	format, err := classified(stateField(fields, "format"), newValueError("document schema scaffolds require JSON or JSONC"), documentFormatFromState)
	if err != nil {
		return DocumentSchema{}, err
	}
	key, err := textLike(stateField(fields, "key"), newTypeError("selector key must be a string"))
	if err != nil {
		return DocumentSchema{}, err
	}
	scalar, err := scalarSemantic(stateField(fields, "value"), newTypeError("document schema fields require a scalar value"))
	if err != nil {
		return DocumentSchema{}, err
	}
	return NewDocumentSchema(format, key, scalar)
}

func optionalMemberState(m *Member) jsonx.Value {
	if m == nil {
		return jsonx.Null()
	}
	return m.toState()
}

func (p ArrayPosition) toState() jsonx.Value {
	return stateRecordObject("ArrayPosition",
		member("following", optionalMemberState(p.following)),
		member("prefix", bytesState(p.prefix)),
		member("previous", optionalMemberState(p.previous)),
	)
}

func arrayPositionFromState(value jsonx.Value) (ArrayPosition, error) {
	fields, err := recordFields(value, "ArrayPosition", "previous", "following", "prefix")
	if err != nil {
		return ArrayPosition{}, err
	}
	anchor := func(item jsonx.Value) (Member, error) {
		return classified(item, newTypeError("array position anchors must be Members"), memberFromState)
	}
	previous, err := optional(stateField(fields, "previous"), anchor)
	if err != nil {
		return ArrayPosition{}, err
	}
	following, err := optional(stateField(fields, "following"), anchor)
	if err != nil {
		return ArrayPosition{}, err
	}
	prefix, err := expectBytes(stateField(fields, "prefix"), newTypeError("array position prefix must be immutable bytes"))
	if err != nil {
		return ArrayPosition{}, err
	}
	return NewArrayPosition(previous, following, prefix)
}

func (s SelectionSnapshot) toState() jsonx.Value {
	return stateRecordObject("SelectionSnapshot",
		member("layout", bytesState(s.layout)),
		member("owned_syntax", bytesState(s.ownedSyntax)),
		member("position", optionalState(s.position)),
		member("syntax", bytesState(s.syntax)),
		member("value", snapshotValueToState(s.value)),
	)
}

func selectionSnapshotFromState(value jsonx.Value) (SelectionSnapshot, error) {
	fields, err := recordFields(value, "SelectionSnapshot", "value", "syntax", "owned_syntax", "position", "layout")
	if err != nil {
		return SelectionSnapshot{}, err
	}
	syntax := func(item jsonx.Value) ([]byte, error) {
		return expectBytes(item, newTypeError("selection syntax must be immutable bytes"))
	}
	snapshot, err := snapshotValueFromState(stateField(fields, "value"))
	if err != nil {
		return SelectionSnapshot{}, err
	}
	full, err := syntax(stateField(fields, "syntax"))
	if err != nil {
		return SelectionSnapshot{}, err
	}
	owned, err := syntax(stateField(fields, "owned_syntax"))
	if err != nil {
		return SelectionSnapshot{}, err
	}
	position, err := optional(stateField(fields, "position"), func(item jsonx.Value) (ArrayPosition, error) {
		return classified(item, newTypeError("selection position must be an ArrayPosition"), arrayPositionFromState)
	})
	if err != nil {
		return SelectionSnapshot{}, err
	}
	layout, err := syntax(stateField(fields, "layout"))
	if err != nil {
		return SelectionSnapshot{}, err
	}
	return NewSelectionSnapshot(snapshot, full, owned, position, layout)
}

func optionalFormatState(format DocumentFormat) jsonx.Value {
	if format == "" {
		return jsonx.Null()
	}
	return format.toState()
}

func (s OwnedSelection) toState() jsonx.Value {
	return stateRecordObject("OwnedSelection",
		member("baseline", optionalState(s.baseline)),
		member("claim", s.claim.toState()),
		member("disposition", s.disposition.toState()),
		member("format", optionalFormatState(s.format)),
		member("installed", s.installed.toState()),
		member("separator", bytesState(s.separator)),
	)
}

func ownedSelectionFromState(value jsonx.Value) (OwnedSelection, error) {
	fields, err := recordFields(value, "OwnedSelection", "claim", "format", "installed", "disposition", "baseline", "separator")
	if err != nil {
		return OwnedSelection{}, err
	}
	claim, err := classified(stateField(fields, "claim"), newTypeError("ownership requires a Claim"), claimFromState)
	if err != nil {
		return OwnedSelection{}, err
	}
	format, err := optional(stateField(fields, "format"), func(item jsonx.Value) (DocumentFormat, error) {
		return classified(item, newValueError("shared claim and document format disagree"), documentFormatFromState)
	})
	if err != nil {
		return OwnedSelection{}, err
	}
	wrongOwnership := newTypeError("ownership requires an installed snapshot and disposition")
	installed, err := classified(stateField(fields, "installed"), wrongOwnership, selectionSnapshotFromState)
	if err != nil {
		return OwnedSelection{}, err
	}
	disposition, err := classified(stateField(fields, "disposition"), wrongOwnership, dispositionFromState)
	if err != nil {
		return OwnedSelection{}, err
	}
	baseline, err := optional(stateField(fields, "baseline"), func(item jsonx.Value) (SelectionSnapshot, error) {
		return classified(item, newTypeError("takeover baseline must be a SelectionSnapshot"), selectionSnapshotFromState)
	})
	if err != nil {
		return OwnedSelection{}, err
	}
	separator, err := expectBytes(stateField(fields, "separator"), newTypeError("owned separators must be immutable bytes"))
	if err != nil {
		return OwnedSelection{}, err
	}
	params := OwnedSelectionParams{Claim: claim, Installed: installed, Disposition: disposition, Baseline: baseline, Separator: separator}
	if format != nil {
		params.Format = *format
	}
	return NewOwnedSelection(params)
}

func (c CreatedContainer) toState() jsonx.Value {
	return stateRecordObject("CreatedContainer",
		member("empty_syntax", bytesState(c.emptySyntax)),
		member("selector", c.selector.toState()),
	)
}

func createdContainerFromState(value jsonx.Value) (CreatedContainer, error) {
	fields, err := recordFields(value, "CreatedContainer", "selector", "empty_syntax")
	if err != nil {
		return CreatedContainer{}, err
	}
	wrong := newTypeError("container provenance requires a selector and immutable syntax")
	selector, err := classified(stateField(fields, "selector"), wrong, selectorFromState)
	if err != nil {
		return CreatedContainer{}, err
	}
	emptySyntax, err := expectBytes(stateField(fields, "empty_syntax"), wrong)
	if err != nil {
		return CreatedContainer{}, err
	}
	return NewCreatedContainer(selector, emptySyntax), nil
}

func (p DocumentProvenance) toState() jsonx.Value {
	return stateRecordObject("DocumentProvenance",
		member("containers", listState(p.containers, CreatedContainer.toState)),
		member("empty_document", optionalBytes(p.emptyDocument, p.hasEmptyDocument)),
		member("schema_empty_document", optionalBytes(p.schemaEmptyDocument, p.hasSchemaEmpty)),
		member("schema_fields", listState(p.schemaFields, OwnedSelection.toState)),
		member("trailing_separators", stringsState(p.trailingSeparators)),
	)
}

func documentProvenanceFromState(value jsonx.Value) (DocumentProvenance, error) {
	fields, err := recordFields(value, "DocumentProvenance", "empty_document", "containers", "trailing_separators", "schema_fields", "schema_empty_document")
	if err != nil {
		return DocumentProvenance{}, err
	}
	emptyDocument, err := optional(stateField(fields, "empty_document"), func(item jsonx.Value) ([]byte, error) {
		return expectBytes(item, newTypeError("created document evidence must be immutable bytes"))
	})
	if err != nil {
		return DocumentProvenance{}, err
	}
	containers, err := sequence(stateField(fields, "containers"), func(item jsonx.Value) (CreatedContainer, error) {
		return classified(item, newTypeError("container provenance must contain CreatedContainer values"), createdContainerFromState)
	})
	if err != nil {
		return DocumentProvenance{}, err
	}
	separators, err := sequence(stateField(fields, "trailing_separators"), func(item jsonx.Value) (string, error) {
		return textLike(item, newValueError("separator evidence must be SHA-256 digests"))
	})
	if err != nil {
		return DocumentProvenance{}, err
	}
	schemaFields, err := sequence(stateField(fields, "schema_fields"), func(item jsonx.Value) (OwnedSelection, error) {
		return classified(item, newValueError("schema provenance requires created top-level scalar fields"), ownedSelectionFromState)
	})
	if err != nil {
		return DocumentProvenance{}, err
	}
	schemaEmpty, err := optional(stateField(fields, "schema_empty_document"), func(item jsonx.Value) ([]byte, error) {
		return expectBytes(item, newValueError("schema empty-document evidence must be whitespace bytes"))
	})
	if err != nil {
		return DocumentProvenance{}, err
	}
	params := DocumentProvenanceParams{Containers: containers, TrailingSeparators: separators, SchemaFields: schemaFields}
	if emptyDocument != nil {
		params.EmptyDocument, params.HasEmptyDocument = *emptyDocument, true
	}
	if schemaEmpty != nil {
		params.SchemaEmptyDocument, params.HasSchemaEmptyDocument = *schemaEmpty, true
	}
	return NewDocumentProvenance(params)
}

func claimSelectorToState(selector ClaimSelector) jsonx.Value {
	switch s := selector.(type) {
	case SubtreeSelector:
		return jsonx.String(string(s))
	case SectionBoundaries:
		return s.toState()
	case Selector:
		return s.toState()
	}
	return jsonx.Null()
}

func (c Claim) toState() jsonx.Value {
	return stateRecordObject("Claim",
		member("kind", c.kind.toState()),
		member("selector", claimSelectorToState(c.selector)),
	)
}

func claimFromState(value jsonx.Value) (Claim, error) {
	fields, err := recordFields(value, "Claim", "kind", "selector")
	if err != nil {
		return Claim{}, err
	}
	kind, err := classified(stateField(fields, "kind"), newTypeError("claim kind must be an Ownership"), ownershipFromState)
	if err != nil {
		return Claim{}, err
	}
	selector, err := claimSelectorFromState(stateField(fields, "selector"), kind)
	if err != nil {
		return Claim{}, err
	}
	return NewClaim(kind, selector)
}

func (a ancestor) toState() jsonx.Value {
	return stateRecordObject("Ancestor",
		member("device", a.device.json()),
		member("gid", a.gid.json()),
		member("inode", a.inode.json()),
		member("mode", a.mode.json()),
		member("path", encodedPath(a.path)),
		member("security", bytesState(a.security)),
		member("uid", a.uid.json()),
	)
}

func identityFromState(fields []jsonx.Member, wrong error, order ...string) (fileIdentity, error) {
	var identity fileIdentity
	slots := map[string]*count{
		"device": &identity.device, "inode": &identity.inode, "mode": &identity.mode, "uid": &identity.uid, "gid": &identity.gid,
	}
	for _, name := range order {
		item, err := metadataCount(stateField(fields, name), wrong)
		if err != nil {
			return fileIdentity{}, err
		}
		*slots[name] = item
	}
	return identity, nil
}

func ancestorFromState(value jsonx.Value) (ancestor, error) {
	fields, err := recordFields(value, "Ancestor", "path", "device", "inode", "mode", "uid", "gid", "security")
	if err != nil {
		return ancestor{}, err
	}
	path, err := expectPath(stateField(fields, "path"), newValueError("ancestor path must be absolute"))
	if err != nil {
		return ancestor{}, err
	}
	identity, err := identityFromState(fields, newValueError("invalid ancestor metadata"), "device", "inode", "mode", "uid", "gid")
	if err != nil {
		return ancestor{}, err
	}
	security, err := expectBytes(stateField(fields, "security"), newTypeError("ancestor security must be immutable"))
	if err != nil {
		return ancestor{}, err
	}
	return newAncestor(path, identity, security)
}

func (n node) toState() jsonx.Value {
	return stateRecordObject("Node",
		member("data", optionalBytes(n.data, n.hasData)),
		member("device", n.device.json()),
		member("gid", n.gid.json()),
		member("inode", n.inode.json()),
		member("mode", n.mode.json()),
		member("path", jsonx.String(n.path)),
		member("security", bytesState(n.security)),
		member("uid", n.uid.json()),
	)
}

func nodeFromState(value jsonx.Value) (node, error) {
	fields, err := recordFields(value, "Node", "path", "data", "mode", "uid", "gid", "device", "inode", "security")
	if err != nil {
		return node{}, err
	}
	path, err := textLike(stateField(fields, "path"), newTypeError("revision path must be a string"))
	if err != nil {
		return node{}, err
	}
	data, err := optional(stateField(fields, "data"), func(item jsonx.Value) ([]byte, error) {
		return expectBytes(item, newTypeError("revision bytes must be immutable"))
	})
	if err != nil {
		return node{}, err
	}
	identity, err := identityFromState(fields, newValueError("invalid revision metadata"), "mode", "uid", "gid", "device", "inode")
	if err != nil {
		return node{}, err
	}
	security, err := expectBytes(stateField(fields, "security"), newTypeError("security metadata must be immutable"))
	if err != nil {
		return node{}, err
	}
	var payload []byte
	if data != nil {
		payload = *data
	}
	return newNode(path, payload, data != nil, identity, security)
}

func (r revision) toState() jsonx.Value {
	return stateRecordObject("Revision", member("nodes", listState(r.nodes, node.toState)))
}

func revisionFromState(value jsonx.Value) (revision, error) {
	fields, err := recordFields(value, "Revision", "nodes")
	if err != nil {
		return revision{}, err
	}
	nodes, err := sequence(stateField(fields, "nodes"), func(item jsonx.Value) (node, error) {
		return classified(item, newTypeError("expected Node values"), nodeFromState)
	})
	if err != nil {
		return revision{}, err
	}
	return newRevision(nodes)
}

func (r ResourceRef) toState() jsonx.Value {
	return stateRecordObject("ResourceRef",
		member("destination", encodedPath(r.Destination)),
		member("kind", r.Kind.toState()),
		member("schema_version", jsonx.Int(stateSchemaVersion)),
	)
}

func resourceRefFromState(value jsonx.Value) (ResourceRef, error) {
	fields, err := recordFields(value, "ResourceRef", "destination", "kind", "schema_version")
	if err != nil {
		return ResourceRef{}, err
	}
	destination, err := expectPath(stateField(fields, "destination"), newValueError("expected an absolute concrete path"))
	if err != nil {
		return ResourceRef{}, err
	}
	kind, err := classified(stateField(fields, "kind"), newTypeError("resource kind must be a ResourceKind"), resourceKindFromState)
	if err != nil {
		return ResourceRef{}, err
	}
	version, err := schemaVersionOf(stateField(fields, "schema_version"), newValueError("authority schema must be 2"))
	if err != nil {
		return ResourceRef{}, err
	}
	return newResourceRef(destination, kind, version)
}

func (c ownedClaim) toState() jsonx.Value {
	return stateRecordObject("OwnedClaim",
		member("artifact_id", jsonx.String(c.artifactID)),
		member("baseline", optionalState(c.baseline)),
		member("bundle_digest", jsonx.String(c.bundleDigest)),
		member("bundle_id", jsonx.String(c.bundleID)),
		member("claim", c.claim.toState()),
		member("claim_id", jsonx.String(c.claimID)),
		member("disposition", c.disposition.toState()),
		member("installed", optionalState(c.installed)),
		member("render_digest", jsonx.String(c.renderDigest)),
		member("requirements", listState(c.requirements, requirement.toState)),
		member("schema_requirements", listState(c.schemaRequirements, DocumentSchema.toState)),
		member("selection", optionalState(c.selection)),
		member("stable_requirements", listState(c.stableRequirements, requirement.toState)),
		member("version", jsonx.String(c.version)),
	)
}

func ownedClaimFromState(value jsonx.Value) (ownedClaim, error) {
	fields, err := recordFields(value, "OwnedClaim", "claim", "claim_id", "bundle_id", "artifact_id", "version",
		"bundle_digest", "render_digest", "selection", "installed", "disposition", "baseline", "requirements",
		"stable_requirements", "schema_requirements")
	if err != nil {
		return ownedClaim{}, err
	}
	wrongOwnership := newTypeError("ownership requires a claim and disposition")
	revisionOf := func(item jsonx.Value) (revision, error) {
		return classified(item, newTypeError("invalid state record"), revisionFromState)
	}
	var result ownedClaim
	if result.claim, err = classified(stateField(fields, "claim"), wrongOwnership, claimFromState); err != nil {
		return ownedClaim{}, err
	}
	if result.claimID, err = hexIdentity(stateField(fields, "claim_id")); err != nil {
		return ownedClaim{}, err
	}
	if result.bundleID, err = textLike(stateField(fields, "bundle_id"), newTypeError("owner must be a string")); err != nil {
		return ownedClaim{}, err
	}
	if result.artifactID, err = textLike(stateField(fields, "artifact_id"), newTypeError("artifact must be a string")); err != nil {
		return ownedClaim{}, err
	}
	if result.version, err = textLike(stateField(fields, "version"), newTypeError("version must be a string")); err != nil {
		return ownedClaim{}, err
	}
	if result.bundleDigest, err = hexIdentity(stateField(fields, "bundle_digest")); err != nil {
		return ownedClaim{}, err
	}
	if result.renderDigest, err = hexIdentity(stateField(fields, "render_digest")); err != nil {
		return ownedClaim{}, err
	}
	result.selection, err = optional(stateField(fields, "selection"), func(item jsonx.Value) (OwnedSelection, error) {
		return classified(item, newTypeError("invalid state record"), ownedSelectionFromState)
	})
	if err != nil {
		return ownedClaim{}, err
	}
	if result.installed, err = optional(stateField(fields, "installed"), revisionOf); err != nil {
		return ownedClaim{}, err
	}
	if result.disposition, err = classified(stateField(fields, "disposition"), wrongOwnership, dispositionFromState); err != nil {
		return ownedClaim{}, err
	}
	if result.baseline, err = optional(stateField(fields, "baseline"), revisionOf); err != nil {
		return ownedClaim{}, err
	}
	if result.requirements, err = sequence(stateField(fields, "requirements"), requirementFromState); err != nil {
		return ownedClaim{}, err
	}
	if result.stableRequirements, err = sequence(stateField(fields, "stable_requirements"), requirementFromState); err != nil {
		return ownedClaim{}, err
	}
	result.schemaRequirements, err = sequence(stateField(fields, "schema_requirements"), func(item jsonx.Value) (DocumentSchema, error) {
		return classified(item, newTypeError("expected DocumentSchema values"), documentSchemaFromState)
	})
	if err != nil {
		return ownedClaim{}, err
	}
	return newOwnedClaim(result)
}

func (r receipt) toState() jsonx.Value {
	return stateRecordObject("Receipt",
		member("claims", listState(r.claims, ownedClaim.toState)),
		member("created_directories", listState(r.createdDirectories, node.toState)),
		member("provenance", r.provenance.toState()),
		member("resource", r.resource.toState()),
		member("schema_version", jsonx.Int(stateSchemaVersion)),
		member("transaction_id", jsonx.String(r.transactionID)),
	)
}

func receiptFromState(value jsonx.Value) (receipt, error) {
	fields, err := recordFields(value, "Receipt", "resource", "transaction_id", "claims", "provenance", "created_directories", "schema_version")
	if err != nil {
		return receipt{}, err
	}
	wrong := newTypeError("invalid receipt resource or provenance")
	resource, err := classified(stateField(fields, "resource"), wrong, resourceRefFromState)
	if err != nil {
		return receipt{}, err
	}
	transactionID, err := hexIdentity(stateField(fields, "transaction_id"))
	if err != nil {
		return receipt{}, err
	}
	claims, err := sequence(stateField(fields, "claims"), func(item jsonx.Value) (ownedClaim, error) {
		return classified(item, newTypeError("expected OwnedClaim values"), ownedClaimFromState)
	})
	if err != nil {
		return receipt{}, err
	}
	provenance, err := classified(stateField(fields, "provenance"), wrong, documentProvenanceFromState)
	if err != nil {
		return receipt{}, err
	}
	directories, err := sequence(stateField(fields, "created_directories"), func(item jsonx.Value) (node, error) {
		return classified(item, newTypeError("expected Node values"), nodeFromState)
	})
	if err != nil {
		return receipt{}, err
	}
	version, err := schemaVersionOf(stateField(fields, "schema_version"), newValueError("receipt schema must be 2"))
	if err != nil {
		return receipt{}, err
	}
	return newReceipt(resource, transactionID, claims, provenance, directories, version)
}

func (m membership) toState() jsonx.Value {
	return jsonx.Array(m.resource.toState(), stringsState(m.claimIDs))
}

func (g generation) toState() jsonx.Value {
	return stateRecordObject("Generation",
		member("bundle_digest", jsonx.String(g.bundleDigest)),
		member("membership", listState(g.membership, membership.toState)),
		member("render_digest", jsonx.String(g.renderDigest)),
		member("version", jsonx.String(g.version)),
	)
}

func generationFromState(value jsonx.Value) (generation, error) {
	fields, err := recordFields(value, "Generation", "version", "bundle_digest", "render_digest", "membership")
	if err != nil {
		return generation{}, err
	}
	version, err := textLike(stateField(fields, "version"), newTypeError("version must be a string"))
	if err != nil {
		return generation{}, err
	}
	bundleDigest, err := hexIdentity(stateField(fields, "bundle_digest"))
	if err != nil {
		return generation{}, err
	}
	renderDigest, err := hexIdentity(stateField(fields, "render_digest"))
	if err != nil {
		return generation{}, err
	}
	members, err := sequence(stateField(fields, "membership"), membershipFromState)
	if err != nil {
		return generation{}, err
	}
	return newGeneration(version, bundleDigest, renderDigest, members)
}

func (x index) toState() jsonx.Value {
	return stateRecordObject("Index",
		member("bundle_id", jsonx.String(x.bundleID)),
		member("context_digest", jsonx.String(x.contextDigest)),
		member("current", optionalState(x.current)),
		member("pending", optionalState(x.pending)),
		member("previous", optionalState(x.previous)),
		member("residual", listState(x.residual, ResourceRef.toState)),
		member("schema_version", jsonx.Int(stateSchemaVersion)),
	)
}

func indexFromState(value jsonx.Value) (index, error) {
	fields, err := recordFields(value, "Index", "bundle_id", "context_digest", "current", "pending", "previous", "residual", "schema_version")
	if err != nil {
		return index{}, err
	}
	generationOf := func(item jsonx.Value) (generation, error) {
		return classified(item, newTypeError("index requires generation records"), generationFromState)
	}
	bundleID, err := textLike(stateField(fields, "bundle_id"), newTypeError("index owner must be a string"))
	if err != nil {
		return index{}, err
	}
	contextDigest, err := hexIdentity(stateField(fields, "context_digest"))
	if err != nil {
		return index{}, err
	}
	generations := make([]*generation, 0, 3)
	for _, name := range []string{"current", "pending", "previous"} {
		item, err := optional(stateField(fields, name), generationOf)
		if err != nil {
			return index{}, err
		}
		generations = append(generations, item)
	}
	residual, err := sequence(stateField(fields, "residual"), func(item jsonx.Value) (ResourceRef, error) {
		return classified(item, newTypeError("expected ResourceRef values"), resourceRefFromState)
	})
	if err != nil {
		return index{}, err
	}
	version, err := schemaVersionOf(stateField(fields, "schema_version"), newValueError("index schema must be 2"))
	if err != nil {
		return index{}, err
	}
	return newIndex(bundleID, contextDigest, generations[0], generations[1], generations[2], residual, version)
}

func (j journal) toState() jsonx.Value {
	return stateRecordObject("Journal",
		member("ancestors", listState(j.ancestors, ancestor.toState)),
		member("backup", j.backup.toState()),
		member("before", j.before.toState()),
		member("previous", optionalState(j.previous)),
		member("published", j.published.toState()),
		member("receipt", j.receipt.toState()),
		member("resource", j.resource.toState()),
		member("schema_version", jsonx.Int(stateSchemaVersion)),
		member("staged", j.staged.toState()),
	)
}

func journalFromState(value jsonx.Value) (journal, error) {
	fields, err := recordFields(value, "Journal", "resource", "previous", "receipt", "before", "staged", "published", "backup", "ancestors", "schema_version")
	if err != nil {
		return journal{}, err
	}
	wrong := newTypeError("journal requires resource and receipt")
	var result journal
	if result.resource, err = classified(stateField(fields, "resource"), wrong, resourceRefFromState); err != nil {
		return journal{}, err
	}
	result.previous, err = optional(stateField(fields, "previous"), func(item jsonx.Value) (receipt, error) {
		return classified(item, newTypeError("journal previous receipt is invalid"), receiptFromState)
	})
	if err != nil {
		return journal{}, err
	}
	if result.receipt, err = classified(stateField(fields, "receipt"), wrong, receiptFromState); err != nil {
		return journal{}, err
	}
	for _, target := range []struct {
		name     string
		revision *revision
	}{{"before", &result.before}, {"staged", &result.staged}, {"published", &result.published}, {"backup", &result.backup}} {
		if *target.revision, err = classified(stateField(fields, target.name), newTypeError("expected Revision values"), revisionFromState); err != nil {
			return journal{}, err
		}
	}
	result.ancestors, err = sequence(stateField(fields, "ancestors"), func(item jsonx.Value) (ancestor, error) {
		return classified(item, newTypeError("expected Ancestor values"), ancestorFromState)
	})
	if err != nil {
		return journal{}, err
	}
	version, err := schemaVersionOf(stateField(fields, "schema_version"), newValueError("journal schema must be 2"))
	if err != nil {
		return journal{}, err
	}
	return newJournal(result, version)
}

func (p preparation) toState() jsonx.Value {
	return stateRecordObject("Preparation",
		member("directory", encodedPath(p.directory)),
		member("resource", p.resource.toState()),
		member("schema_version", jsonx.Int(stateSchemaVersion)),
		member("transaction_id", jsonx.String(p.transactionID)),
	)
}

func preparationFromState(value jsonx.Value) (preparation, error) {
	fields, err := recordFields(value, "Preparation", "resource", "transaction_id", "directory", "schema_version")
	if err != nil {
		return preparation{}, err
	}
	resource, err := classified(stateField(fields, "resource"), newTypeError("preparation requires a resource"), resourceRefFromState)
	if err != nil {
		return preparation{}, err
	}
	transactionID, err := hexIdentity(stateField(fields, "transaction_id"))
	if err != nil {
		return preparation{}, err
	}
	directory, err := expectPath(stateField(fields, "directory"), newValueError("expected an absolute concrete path"))
	if err != nil {
		return preparation{}, err
	}
	version, err := schemaVersionOf(stateField(fields, "schema_version"), newValueError("preparation schema must be 2"))
	if err != nil {
		return preparation{}, err
	}
	return newPreparation(resource, transactionID, directory, version)
}
