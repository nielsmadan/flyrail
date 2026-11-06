package flyrail

import (
	"slices"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

type layoutPart struct {
	field   string
	index   uint64
	isIndex bool
}

type layoutHeader struct {
	path []layoutPart
	name string
}

func invalidHeader() error {
	return newValueError("invalid TOML restoration header")
}

func headerPath(name string) ([]string, error) {
	var parts []string
	rest := skipBlanks(name)
	for {
		part, remaining, err := headerKey(rest)
		if err != nil {
			return nil, err
		}
		parts = append(parts, part)
		rest = skipBlanks(remaining)
		afterDot, dotted := strings.CutPrefix(rest, ".")
		if !dotted {
			if rest == "" {
				return parts, nil
			}
			return nil, invalidHeader()
		}
		rest = skipBlanks(afterDot)
	}
}

func skipBlanks(text string) string {
	return strings.TrimLeft(text, " \t")
}

func isBareKeyCharacter(r rune) bool {
	return r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '_' || r == '-'
}

func isHeaderControl(r rune) bool {
	return r < 0x20 || r == 0x7F
}

func headerKey(text string) (string, string, error) {
	if text == "" {
		return "", "", invalidHeader()
	}
	first, _ := utf8.DecodeRuneInString(text)
	switch {
	case first == '\'':
		return literalKey(text)
	case first == '"':
		return basicKey(text)
	case isBareKeyCharacter(first):
		end := strings.IndexFunc(text, func(r rune) bool { return !isBareKeyCharacter(r) })
		if end < 0 {
			end = len(text)
		}
		return text[:end], text[end:], nil
	}
	return "", "", invalidHeader()
}

func literalKey(text string) (string, string, error) {
	for index, r := range text[1:] {
		if r == '\'' {
			return text[1 : index+1], text[index+2:], nil
		}
		if isHeaderControl(r) {
			return "", "", invalidHeader()
		}
	}
	return "", "", invalidHeader()
}

func basicKey(text string) (string, string, error) {
	var out strings.Builder
	rest := text[1:]
	for rest != "" {
		r, size := utf8.DecodeRuneInString(rest)
		if r == '"' {
			return out.String(), rest[1:], nil
		}
		if isHeaderControl(r) {
			return "", "", invalidHeader()
		}
		rest = rest[size:]
		if r != '\\' {
			out.WriteRune(r)
			continue
		}
		if rest == "" {
			return "", "", invalidHeader()
		}
		escape, size := utf8.DecodeRuneInString(rest)
		rest = rest[size:]
		var err error
		switch escape {
		case 'b':
			out.WriteByte('\b')
		case 't':
			out.WriteByte('\t')
		case 'n':
			out.WriteByte('\n')
		case 'f':
			out.WriteByte('\f')
		case 'r':
			out.WriteByte('\r')
		case '"':
			out.WriteByte('"')
		case '\\':
			out.WriteByte('\\')
		case 'u':
			rest, err = unicodeEscape(rest, 4, &out)
		case 'U':
			rest, err = unicodeEscape(rest, 8, &out)
		default:
			err = invalidHeader()
		}
		if err != nil {
			return "", "", err
		}
	}
	return "", "", invalidHeader()
}

func unicodeEscape(rest string, digits int, out *strings.Builder) (string, error) {
	if len(rest) < digits {
		return "", invalidHeader()
	}
	code, err := strconv.ParseUint(rest[:digits], 16, 32)
	if err != nil || code > utf8.MaxRune || code >= 0xD800 && code <= 0xDFFF {
		return "", invalidHeader()
	}
	out.WriteRune(rune(code))
	return rest[digits:], nil
}

func layoutHeaders(layout []byte) ([]layoutHeader, error) {
	if len(layout) == 0 {
		return nil, nil
	}
	invalidLayout := newValueError("invalid TOML restoration layout")
	if !utf8.Valid(layout) {
		return nil, invalidLayout
	}
	record, err := jsonx.Parse(layout)
	if err != nil {
		return nil, invalidLayout
	}
	members, ok := record.Members()
	if !ok || len(members) != 2 || slices.ContainsFunc(members, func(m jsonx.Member) bool {
		return m.Key != "version" && m.Key != "headers"
	}) {
		return nil, invalidLayout
	}
	version, _ := record.Get("version")
	if number, isInt := version.AsInt(); !isInt || number != 1 {
		return nil, invalidLayout
	}
	rawHeaders, _ := record.Get("headers")
	entries, ok := rawHeaders.Items()
	if !ok {
		return nil, invalidLayout
	}
	var result []layoutHeader
	for _, entry := range entries {
		path, rawName, err := layoutEntry(entry)
		if err != nil {
			return nil, err
		}
		if slices.ContainsFunc(result, func(seen layoutHeader) bool { return slices.Equal(seen.path, path) }) {
			return nil, newValueError("duplicate TOML restoration path")
		}
		name, ok := rawName.AsString()
		if !ok {
			return nil, invalidHeader()
		}
		if _, err := headerPath(name); err != nil {
			return nil, err
		}
		result = append(result, layoutHeader{path: path, name: name})
	}
	return result, nil
}

func layoutEntry(entry jsonx.Value) ([]layoutPart, jsonx.Value, error) {
	pair, ok := entry.Items()
	if !ok || len(pair) != 2 {
		return nil, jsonx.Value{}, newValueError("invalid TOML restoration header entry")
	}
	invalidPath := newValueError("invalid TOML restoration path")
	relative, ok := pair[0].Items()
	if !ok || len(relative) == 0 {
		return nil, jsonx.Value{}, invalidPath
	}
	path := make([]layoutPart, 0, len(relative))
	for _, part := range relative {
		if field, isString := part.AsString(); isString {
			path = append(path, layoutPart{field: field})
			continue
		}
		index, isInt := part.AsInt()
		if !isInt || index < 0 {
			return nil, jsonx.Value{}, invalidPath
		}
		path = append(path, layoutPart{index: uint64(index), isIndex: true})
	}
	return path, pair[1], nil
}

type tomlDocument struct {
	source  string
	newline string
	root    tomlContainer
	tree    *tkContainer
}

func parseTOMLDocument(text string) (*tomlDocument, error) {
	root, err := parseTOMLContainer(text)
	if err != nil {
		return nil, err
	}
	document := &tomlDocument{newline: "\n", root: coalesceTOML(root)}
	if strings.Contains(text, "\r\n") {
		document.newline = "\r\n"
	}
	document.source = renderTOMLRoot(&document.root)
	return document, nil
}

func (d *tomlDocument) text() string {
	return d.source
}

func (d *tomlDocument) native(path []pathPart) (Value, error) {
	if len(path) == 0 {
		return unwrapTOMLBody(&d.root)
	}
	located, err := locateTOMLPath(&d.root, path, "document selector path is missing")
	if err != nil {
		return nil, err
	}
	return located.unwrap()
}

type tomlLocatedKind uint8

const (
	tomlLocatedItem tomlLocatedKind = iota
	tomlLocatedTable
	tomlLocatedTables
	tomlLocatedAOT
	tomlLocatedInline
)

type tomlLocated struct {
	kind   tomlLocatedKind
	item   *tomlItem
	tables []*tomlTable
	inline []tomlInlineRef
}

type tomlInlineRef struct {
	parts []tomlKeyPart
	item  *tomlItem
}

func (l tomlLocated) body() *tomlContainer {
	switch {
	case l.kind == tomlLocatedTable:
		return &l.tables[0].body
	case l.kind == tomlLocatedItem && l.item.kind == tomlTableItem:
		return &l.item.table.body
	}
	return nil
}

func (l tomlLocated) unwrap() (Value, error) {
	switch l.kind {
	case tomlLocatedTable:
		return unwrapTOMLBody(&l.tables[0].body)
	case tomlLocatedTables:
		proxy, err := newTOMLProxy(l.tables)
		if err != nil {
			return nil, err
		}
		return unwrapTOMLGroups(proxy.groups)
	case tomlLocatedAOT:
		return unwrapTOMLMembers(l.tables)
	case tomlLocatedInline:
		root := &inlineNode{children: map[string]*inlineNode{}, open: true}
		for _, ref := range l.inline {
			names := make([]string, len(ref.parts))
			for index, part := range ref.parts {
				names[index] = part.name
			}
			if err := root.assign(names, ref.item.value); err != nil {
				return nil, err
			}
		}
		return root.object()
	}
	return l.item.unwrapValue()
}

func locateTOML(root *tomlContainer, path []pathPart) (tomlLocated, error) {
	return locateTOMLPath(root, path, "TOML selector path is missing")
}

func locateTOMLPath(root *tomlContainer, path []pathPart, missing string) (tomlLocated, error) {
	var current *tomlLocated
	for _, part := range path {
		var found tomlLocated
		var err error
		if part.isIndex {
			found, err = searchTOMLIndex(current, part.index)
		} else {
			found, err = searchTOMLKey(current, root, part.key, missing)
		}
		if err != nil {
			return tomlLocated{}, err
		}
		current = &found
	}
	if current == nil {
		return tomlLocated{}, newValueError("document selector path is missing")
	}
	return *current, nil
}

func searchTOMLKey(current *tomlLocated, root *tomlContainer, name, missing string) (tomlLocated, error) {
	var bodies []*tomlContainer
	switch {
	case current == nil:
		bodies = []*tomlContainer{root}
	case current.kind == tomlLocatedItem && current.item.kind == tomlInlineItem:
		refs := make([]tomlInlineRef, len(current.item.entries))
		for index := range current.item.entries {
			entry := &current.item.entries[index]
			refs[index] = tomlInlineRef{parts: entry.key.parts, item: &entry.item}
		}
		return searchTOMLInline(refs, name, missing)
	case current.kind == tomlLocatedInline:
		return searchTOMLInline(current.inline, name, missing)
	case current.kind == tomlLocatedTables:
		for _, table := range current.tables {
			bodies = append(bodies, &table.body)
		}
	default:
		body := current.body()
		if body == nil {
			return tomlLocated{}, newValueError("TOML selector crosses a scalar")
		}
		bodies = []*tomlContainer{body}
	}
	groups, err := groupTOMLEntries(bodies)
	if err != nil {
		return tomlLocated{}, err
	}
	index := slices.IndexFunc(groups, func(group *tomlGroup) bool { return group.name == name })
	if index < 0 {
		return tomlLocated{}, newValueError(missing)
	}
	group := groups[index]
	if len(group.entries) > 1 {
		proxy, err := newTOMLProxy(group.tables())
		if err != nil {
			return tomlLocated{}, err
		}
		return tomlLocated{kind: tomlLocatedTables, tables: proxy.tables}, nil
	}
	item := &group.entries[0].item
	switch item.kind {
	case tomlAOTItem:
		return tomlLocated{kind: tomlLocatedAOT, tables: group.members}, nil
	case tomlTableItem:
		return tomlLocated{kind: tomlLocatedTable, tables: []*tomlTable{&item.table}}, nil
	}
	return tomlLocated{kind: tomlLocatedItem, item: item}, nil
}

func searchTOMLInline(refs []tomlInlineRef, name, missing string) (tomlLocated, error) {
	var nested []tomlInlineRef
	for _, ref := range refs {
		if ref.parts[0].name != name {
			continue
		}
		if len(ref.parts) == 1 {
			return tomlLocated{kind: tomlLocatedItem, item: ref.item}, nil
		}
		nested = append(nested, tomlInlineRef{parts: ref.parts[1:], item: ref.item})
	}
	if len(nested) == 0 {
		return tomlLocated{}, newValueError(missing)
	}
	return tomlLocated{kind: tomlLocatedInline, inline: nested}, nil
}

func searchTOMLIndex(current *tomlLocated, index int) (tomlLocated, error) {
	missing := newValueError("document selector path is missing")
	switch {
	case current != nil && current.kind == tomlLocatedAOT:
		if index < 0 || index >= len(current.tables) {
			return tomlLocated{}, missing
		}
		return tomlLocated{kind: tomlLocatedTable, tables: current.tables[index : index+1]}, nil
	case current != nil && current.kind == tomlLocatedItem && current.item.kind == tomlArrayItem:
		if index < 0 || index >= len(current.item.items) {
			return tomlLocated{}, missing
		}
		return tomlLocated{kind: tomlLocatedItem, item: &current.item.items[index]}, nil
	}
	return tomlLocated{}, newValueError("TOML selector crosses a scalar")
}

func (d *tomlDocument) layout(path []pathPart) ([]byte, error) {
	if len(path) == 0 {
		return []byte{}, nil
	}
	located, err := locateTOML(&d.root, path)
	if err != nil {
		return nil, err
	}
	var headers []jsonx.Value
	collectTOMLHeaders(located, nil, &headers)
	if len(headers) == 0 {
		return []byte{}, nil
	}
	return jsonx.Encode(jsonx.Object(
		jsonx.Member{Key: "version", Value: jsonx.Int(1)},
		jsonx.Member{Key: "headers", Value: jsonx.Array(headers...)},
	)), nil
}

func collectTOMLHeaders(located tomlLocated, relative []jsonx.Value, headers *[]jsonx.Value) {
	switch {
	case located.kind == tomlLocatedTable || located.kind == tomlLocatedTables:
		walkTOMLLayout(located.tables, relative, headers)
	case located.kind == tomlLocatedItem && located.item.kind == tomlTableItem:
		walkTOMLLayout([]*tomlTable{&located.item.table}, relative, headers)
	case located.kind == tomlLocatedAOT:
		for index, member := range located.tables {
			collectTOMLHeaders(tomlLocated{kind: tomlLocatedTable, tables: []*tomlTable{member}}, appendLayoutPart(relative, jsonx.Int(int64(index))), headers)
		}
	case located.kind == tomlLocatedItem && located.item.kind == tomlAOTItem:
		for index := range located.item.aot {
			member := &located.item.aot[index]
			collectTOMLHeaders(tomlLocated{kind: tomlLocatedTable, tables: []*tomlTable{member}}, appendLayoutPart(relative, jsonx.Int(int64(index))), headers)
		}
	}
}

func appendLayoutPart(relative []jsonx.Value, part jsonx.Value) []jsonx.Value {
	return append(slices.Clip(relative), part)
}

func walkTOMLLayout(tables []*tomlTable, relative []jsonx.Value, headers *[]jsonx.Value) {
	for _, table := range tables {
		if len(relative) > 0 && !table.implicit && table.displayName != "" {
			*headers = append(*headers, jsonx.Array(jsonx.Array(relative...), jsonx.String(table.displayName)))
		}
		for index := range table.body.body {
			entry := &table.body.body[index]
			if entry.key != nil {
				collectTOMLHeaders(tomlLocated{kind: tomlLocatedItem, item: &entry.item}, appendLayoutPart(relative, jsonx.String(entry.key.dottedString())), headers)
			}
		}
	}
}
