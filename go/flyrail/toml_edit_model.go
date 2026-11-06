package flyrail

import (
	"slices"
	"strings"
)

const tkMaxDepth = 1000

type tkSuper uint8

const (
	tkSuperUnset tkSuper = iota
	tkSuperFalse
	tkSuperTrue
)

// Items have reference identity: edits mutate them in place, as tomlkit does.
type tkItem struct {
	kind        tomlItemKind
	raw         string
	value       Value
	trivia      tomlTrivia
	fixed       bool
	items       []*tkItem
	spans       [][2]int
	commas      []int
	body        *tkContainer
	inlineNew   bool
	aotElement  bool
	super       tkSuper
	displayName *string
	aot         []*tkItem
}

type tkEntry struct {
	key  *tomlKey
	item *tkItem
}

type tkContainer struct {
	body   []tkEntry
	parsed bool
}

func tkNull() *tkItem {
	return &tkItem{kind: tomlNullItem}
}

func tkWhitespace(text string, fixed bool) *tkItem {
	return &tkItem{kind: tomlWhitespaceItem, raw: text, fixed: fixed}
}

func tkTableLike(item *tkItem) bool {
	return item.kind == tomlTableItem || item.kind == tomlAOTItem
}

func stringRef(text string) *string {
	return &text
}

func tkFromContainer(c *tomlContainer, parsed bool, depth int) (*tkContainer, error) {
	if depth > tkMaxDepth {
		return nil, newNestingError(documentNesting)
	}
	result := &tkContainer{parsed: parsed, body: make([]tkEntry, 0, len(c.body))}
	for index := range c.body {
		item, err := tkFromItem(&c.body[index].item, depth+1)
		if err != nil {
			return nil, err
		}
		result.body = append(result.body, tkEntry{key: c.body[index].key.clone(), item: item})
	}
	return result, nil
}

func tkFromTable(t *tomlTable, depth int) (*tkItem, error) {
	body, err := tkFromContainer(&t.body, false, depth+1)
	if err != nil {
		return nil, err
	}
	item := &tkItem{kind: tomlTableItem, body: body, trivia: t.trivia, aotElement: t.aotElement, super: tkSuperFalse}
	if t.implicit {
		item.super = tkSuperTrue
	}
	if t.displayName != "" {
		item.displayName = stringRef(t.displayName)
	}
	return item, nil
}

func tkFromItem(i *tomlItem, depth int) (*tkItem, error) {
	if depth > tkMaxDepth {
		return nil, newNestingError(documentNesting)
	}
	switch i.kind {
	case tomlTableItem:
		return tkFromTable(&i.table, depth)
	case tomlAOTItem:
		item := &tkItem{kind: tomlAOTItem, trivia: tomlTrivia{}}
		for index := range i.aot {
			table, err := tkFromTable(&i.aot[index], depth+1)
			if err != nil {
				return nil, err
			}
			item.aot = append(item.aot, table)
		}
		return item, nil
	case tomlArrayItem:
		item := &tkItem{kind: tomlArrayItem, raw: i.raw, value: i.value, trivia: i.trivia, spans: slices.Clone(i.spans), commas: slices.Clone(i.commas)}
		for index := range i.items {
			member, err := tkFromItem(&i.items[index], depth+1)
			if err != nil {
				return nil, err
			}
			item.items = append(item.items, member)
		}
		return item, nil
	case tomlInlineItem:
		body, err := tkInlineBody(i.raw, depth+1)
		if err != nil {
			return nil, err
		}
		return &tkItem{kind: tomlInlineItem, raw: i.raw, value: i.value, trivia: i.trivia, body: body}, nil
	case tomlWhitespaceItem:
		return tkWhitespace(i.raw, false), nil
	case tomlCommentItem:
		return &tkItem{kind: tomlCommentItem, trivia: i.trivia}, nil
	case tomlNullItem:
		return tkNull(), nil
	}
	return &tkItem{kind: i.kind, raw: i.raw, value: i.value, trivia: i.trivia}, nil
}

func tkInlineBody(raw string, depth int) (*tkContainer, error) {
	s := &tomlScanner{src: raw, line: 1}
	s.step()
	body := &tkContainer{parsed: true}
	expectKey := true
	for {
		for {
			mark := s.pos
			for s.cur() == ' ' || s.cur() == '\t' || s.cur() == '\n' || s.cur() == '\r' {
				s.step()
			}
			if s.pos > mark {
				body.body = append(body.body, tkEntry{item: tkWhitespace(raw[mark:s.pos], false)})
			}
			if s.cur() != '#' {
				break
			}
			trivia, err := s.commentTrail(false)
			if err != nil {
				return nil, err
			}
			body.body = append(body.body, tkEntry{item: &tkItem{kind: tomlCommentItem, trivia: trivia}})
		}
		if s.cur() == '}' || s.end() {
			return body, nil
		}
		if !expectKey {
			body.body = append(body.body, tkEntry{item: tkWhitespace(",", false)})
			s.step()
			expectKey = true
			continue
		}
		key, item, err := parseTOMLKeyValue(s, false, depth/2)
		if err != nil {
			return nil, err
		}
		converted, err := tkFromItem(&item, depth+1)
		if err != nil {
			return nil, err
		}
		body.addParsed(key, converted)
		expectKey = false
	}
}

func (c *tkContainer) addParsed(key *tomlKey, item *tkItem) {
	if len(key.parts) == 1 {
		single := key.clone()
		single.dotted = false
		c.body = append(c.body, tkEntry{key: single, item: item})
		return
	}
	last := len(key.parts) - 1
	leaf := &tomlKey{parts: []tomlKeyPart{key.parts[last]}, sep: key.sep}
	node := item
	nodeKey := leaf
	for index := last - 1; index >= 1; index-- {
		level := &tkItem{kind: tomlTableItem, body: &tkContainer{parsed: true}, trivia: defaultTrivia(), super: tkSuperTrue}
		level.body.body = append(level.body.body, tkEntry{key: nodeKey, item: node})
		node, nodeKey = level, &tomlKey{parts: []tomlKeyPart{key.parts[index]}, dotted: true}
	}
	head := &tkItem{kind: tomlTableItem, body: &tkContainer{parsed: true}, trivia: defaultTrivia(), super: tkSuperTrue}
	head.body.body = append(head.body.body, tkEntry{key: nodeKey, item: node})
	c.body = append(c.body, tkEntry{key: &tomlKey{parts: []tomlKeyPart{key.parts[0]}, dotted: true}, item: head})
}

func (i *tkItem) clone() *tkItem {
	copied := *i
	copied.spans = slices.Clone(i.spans)
	copied.commas = slices.Clone(i.commas)
	if i.items != nil {
		copied.items = make([]*tkItem, len(i.items))
		for index, member := range i.items {
			copied.items[index] = member.clone()
		}
	}
	if i.body != nil {
		copied.body = i.body.clone()
	}
	if i.displayName != nil {
		copied.displayName = stringRef(*i.displayName)
	}
	if i.aot != nil {
		copied.aot = make([]*tkItem, len(i.aot))
		for index, member := range i.aot {
			copied.aot[index] = member.clone()
		}
	}
	return &copied
}

func (c *tkContainer) clone() *tkContainer {
	copied := &tkContainer{parsed: c.parsed, body: make([]tkEntry, len(c.body))}
	for index, entry := range c.body {
		copied.body[index] = tkEntry{key: entry.key.clone(), item: entry.item.clone()}
	}
	return copied
}

func (c *tkContainer) positions(name string) []int {
	var positions []int
	for index, entry := range c.body {
		if entry.key != nil && entry.key.name() == name {
			positions = append(positions, index)
		}
	}
	return positions
}

func (c *tkContainer) contains(name string) bool {
	return slices.ContainsFunc(c.body, func(entry tkEntry) bool { return entry.key != nil && entry.key.name() == name })
}

type tkGroup struct {
	name      string
	positions []int
}

func (c *tkContainer) groups() []tkGroup {
	var groups []tkGroup
	index := map[string]int{}
	for position, entry := range c.body {
		if entry.key == nil {
			continue
		}
		name := entry.key.name()
		if at, seen := index[name]; seen {
			groups[at].positions = append(groups[at].positions, position)
			continue
		}
		index[name] = len(groups)
		groups = append(groups, tkGroup{name: name, positions: []int{position}})
	}
	return groups
}

func (c *tkContainer) names() []string {
	groups := c.groups()
	names := make([]string, len(groups))
	for index, group := range groups {
		names[index] = group.name
	}
	return names
}

func (c *tkContainer) length() int {
	return len(c.names())
}

func (i *tkItem) isSuperTable() bool {
	switch i.super {
	case tkSuperTrue:
		return true
	case tkSuperFalse:
		return false
	}
	groups := i.body.groups()
	if len(groups) == 0 {
		return false
	}
	for _, group := range groups {
		if len(group.positions) > 1 {
			return false
		}
		entry := i.body.body[group.positions[0]]
		if !tkTableLike(entry.item) || entry.key.dotted {
			return false
		}
	}
	return true
}

func (i *tkItem) arrayString() string {
	if len(i.items) == 0 || len(i.items) != len(i.spans) {
		return i.raw
	}
	var text strings.Builder
	previous := 0
	for index, member := range i.items {
		text.WriteString(i.raw[previous:i.spans[index][0]])
		text.WriteString(member.asString())
		previous = i.spans[index][1]
	}
	text.WriteString(i.raw[previous:])
	return text.String()
}

func (i *tkItem) asString() string {
	switch i.kind {
	case tomlArrayItem:
		return i.arrayString()
	case tomlValueItem, tomlWhitespaceItem:
		return i.raw
	case tomlCommentItem:
		return i.trivia.indent + i.trivia.comment + i.trivia.trail
	case tomlInlineItem:
		return i.renderInline()
	case tomlTableItem:
		return i.body.asString()
	case tomlAOTItem:
		var text strings.Builder
		for _, member := range i.aot {
			text.WriteString(member.body.asString())
		}
		return text.String()
	}
	return ""
}

func tkNeedsSeparator(text, indent string) bool {
	trimmed := strings.TrimRight(text, " ")
	return trimmed != "" && !strings.HasSuffix(trimmed, "\n") && !strings.Contains(indent, "\n")
}

func (c *tkContainer) asString() string {
	var text strings.Builder
	for _, entry := range c.body {
		switch {
		case entry.key != nil && entry.item.kind == tomlTableItem:
			if tkNeedsSeparator(text.String(), entry.item.trivia.indent) {
				text.WriteByte('\n')
			}
			text.WriteString(renderTkTable(entry.key, entry.item, nil))
		case entry.key != nil && entry.item.kind == tomlAOTItem:
			if tkNeedsSeparator(text.String(), entry.item.trivia.indent) {
				text.WriteByte('\n')
			}
			text.WriteString(renderTkAOT(entry.key, entry.item, nil))
		default:
			text.WriteString(renderTkSimple(entry.key, entry.item, nil))
		}
	}
	return text.String()
}

func renderTkSimple(key *tomlKey, item *tkItem, prefix *string) string {
	if key == nil {
		return item.asString()
	}
	name := key.asString()
	if prefix != nil {
		name = *prefix + "." + name
	}
	return item.trivia.indent + name + key.sep + item.asString() + item.trivia.commentWS + item.trivia.comment + item.trivia.trail
}

func renderTkTable(key *tomlKey, table *tkItem, prefix *string) string {
	var name string
	if table.displayName != nil {
		name = *table.displayName
	} else {
		name = key.asString()
		if prefix != nil {
			name = *prefix + "." + name
		}
	}
	header := !table.isSuperTable()
	if !header && !key.dotted {
		for _, entry := range table.body.body {
			switch entry.item.kind {
			case tomlTableItem:
				header = header || entry.key != nil && entry.key.dotted
			case tomlAOTItem, tomlWhitespaceItem, tomlNullItem:
			default:
				header = true
			}
		}
	}
	var text strings.Builder
	if header {
		open, closing := "[", "]"
		if table.aotElement {
			open, closing = "[[", "]]"
		}
		text.WriteString(table.trivia.indent + open + name + closing + table.trivia.commentWS + table.trivia.comment + table.trivia.trail)
		if !strings.Contains(table.trivia.trail, "\n") && table.body.length() > 0 {
			text.WriteByte('\n')
		}
	} else if table.trivia.indent == "\n" {
		text.WriteString(table.trivia.indent)
	}
	for _, entry := range table.body.body {
		switch entry.item.kind {
		case tomlTableItem:
			if tkNeedsSeparator(text.String(), entry.item.trivia.indent) {
				text.WriteByte('\n')
			}
			if entry.item.isSuperTable() && entry.key.dotted && !key.dotted {
				text.WriteString(renderTkTable(entry.key, entry.item, nil))
			} else {
				text.WriteString(renderTkTable(entry.key, entry.item, &name))
			}
		case tomlAOTItem:
			if tkNeedsSeparator(text.String(), entry.item.trivia.indent) {
				text.WriteByte('\n')
			}
			text.WriteString(renderTkAOT(entry.key, entry.item, &name))
		default:
			var simplePrefix *string
			if key.dotted {
				simplePrefix = &name
			}
			text.WriteString(renderTkSimple(entry.key, entry.item, simplePrefix))
		}
	}
	return text.String()
}

func renderTkAOT(key *tomlKey, aot *tkItem, prefix *string) string {
	name := key.asString()
	if prefix != nil {
		name = *prefix + "." + name
	}
	var text strings.Builder
	for _, member := range aot.aot {
		text.WriteString(renderTkAOTTable(member, name))
	}
	return text.String()
}

func renderTkAOTTable(table *tkItem, name string) string {
	var text strings.Builder
	text.WriteString(table.trivia.indent + "[[" + name + "]]" + table.trivia.commentWS + table.trivia.comment + table.trivia.trail)
	for _, entry := range table.body.body {
		switch entry.item.kind {
		case tomlTableItem:
			if entry.item.isSuperTable() && entry.key.dotted {
				text.WriteString(renderTkTable(entry.key, entry.item, nil))
			} else {
				text.WriteString(renderTkTable(entry.key, entry.item, &name))
			}
		case tomlAOTItem:
			text.WriteString(renderTkAOT(entry.key, entry.item, &name))
		default:
			text.WriteString(renderTkSimple(entry.key, entry.item, nil))
		}
	}
	return text.String()
}

func (i *tkItem) renderInline() string {
	body := i.body.body
	explicitCommas, lastItem, lastNull := false, -1, -1
	for index, entry := range body {
		switch {
		case entry.key != nil:
			lastItem = index
		case entry.item.kind == tomlWhitespaceItem && strings.Contains(entry.item.raw, ","):
			explicitCommas = true
		}
		if entry.item.kind == tomlNullItem {
			lastNull = index
		}
	}
	buf := "{"
	emittedKey, needsSeparator, pendingSeparator := false, false, false
	lastValueEnd := len(buf)
	for index, entry := range body {
		item := entry.item
		if entry.key == nil {
			text := item.asString()
			if item.kind == tomlWhitespaceItem && strings.Contains(item.raw, ",") {
				stripped := strings.Replace(text, ",", "", 1)
				if !emittedKey || lastNull > index && lastItem <= index || pendingSeparator {
					buf += stripped
					continue
				}
				pendingSeparator = true
			}
			if index == len(body)-1 {
				if i.inlineNew {
					buf = strings.TrimRight(buf, ", ")
				} else if !explicitCommas || strings.Contains(text, ",") {
					buf = strings.TrimRight(buf, ",")
				}
			}
			buf += text
			if strings.Contains(text, ",") {
				needsSeparator = false
			}
			continue
		}
		if explicitCommas && needsSeparator {
			buf = buf[:lastValueEnd] + "," + buf[lastValueEnd:]
			needsSeparator = false
		}
		var rendered string
		if entry.key.dotted && item.kind == tomlTableItem {
			rendered = strings.Join(renderTkDotted(entry.key, item), ", ")
		} else {
			name := entry.key.asString()
			if entry.key.dotted {
				name += "."
			}
			rendered = name + entry.key.sep + item.asString()
		}
		buf += item.trivia.indent + rendered
		lastValueEnd = len(buf)
		buf += item.trivia.comment + strings.ReplaceAll(item.trivia.trail, "\n", "")
		emittedKey, pendingSeparator = true, false
		if explicitCommas {
			needsSeparator = true
		}
		if !explicitCommas && lastItem >= 0 && index < lastItem {
			buf += ","
			if i.inlineNew {
				buf += " "
			}
		}
	}
	return buf + "}"
}

func renderTkDotted(key *tomlKey, table *tkItem) []string {
	prefix := key.asString() + "." + key.sep
	var parts []string
	for _, entry := range table.body.body {
		if entry.key == nil {
			continue
		}
		if entry.item.kind == tomlTableItem {
			for _, sub := range renderTkDotted(entry.key, entry.item) {
				parts = append(parts, prefix+sub)
			}
			continue
		}
		item := entry.item
		parts = append(parts, prefix+item.trivia.indent+entry.key.asString()+entry.key.sep+item.asString()+
			item.trivia.commentWS+item.trivia.comment+strings.ReplaceAll(item.trivia.trail, "\n", ""))
	}
	return parts
}

type tkProxy struct {
	container *tkContainer
	tables    []*tkItem
	internal  *tkContainer
	tablesMap map[string][]int
}

func newTkProxy(container *tkContainer, positions []int) (*tkProxy, error) {
	proxy := &tkProxy{container: container, internal: &tkContainer{parsed: true}, tablesMap: map[string][]int{}}
	for _, position := range positions {
		item := container.body[position].item
		if item.kind != tomlTableItem {
			continue
		}
		proxy.tables = append(proxy.tables, item)
		tableIndex := len(proxy.tables) - 1
		for _, entry := range item.body.body {
			if !proxy.mergeAOTFragment(entry) {
				if err := proxy.internal.rawAppend(entry.key, entry.item); err != nil {
					return nil, err
				}
			}
			if entry.key != nil {
				name := entry.key.name()
				if !slices.Contains(proxy.tablesMap[name], tableIndex) {
					proxy.tablesMap[name] = append(proxy.tablesMap[name], tableIndex)
				}
			}
		}
	}
	return proxy, nil
}

func (p *tkProxy) mergeAOTFragment(entry tkEntry) bool {
	if entry.key == nil || entry.item.kind != tomlAOTItem {
		return false
	}
	positions := p.internal.positions(entry.key.name())
	if len(positions) != 1 {
		return false
	}
	existing := p.internal.body[positions[0]].item
	if existing.kind != tomlAOTItem {
		return false
	}
	merged := &tkItem{kind: tomlAOTItem, aot: append(slices.Clone(existing.aot), entry.item.aot...)}
	p.internal.body[positions[0]].item = merged
	return true
}

func (c *tkContainer) unwrap(depth int) (Value, error) {
	if depth > tkMaxDepth {
		return nil, newNestingError(documentNesting)
	}
	groups := c.groups()
	members := make([]ObjectMember, 0, len(groups))
	for _, group := range groups {
		var value Value
		var err error
		if len(group.positions) > 1 {
			var proxy *tkProxy
			if proxy, err = newTkProxy(c, group.positions); err == nil {
				value, err = proxy.internal.unwrap(depth + 1)
			}
		} else {
			value, err = c.body[group.positions[0]].item.unwrap(depth + 1)
		}
		if err != nil {
			return nil, err
		}
		members = append(members, ObjectMember{Key: group.name, Value: value})
	}
	return NewObject(members...)
}

func (i *tkItem) unwrap(depth int) (Value, error) {
	if depth > tkMaxDepth {
		return nil, newNestingError(documentNesting)
	}
	switch i.kind {
	case tomlTableItem, tomlInlineItem:
		return i.body.unwrap(depth + 1)
	case tomlAOTItem:
		items := make([]Value, 0, len(i.aot))
		for _, member := range i.aot {
			value, err := member.body.unwrap(depth + 1)
			if err != nil {
				return nil, err
			}
			items = append(items, value)
		}
		return NewArray(items...)
	case tomlArrayItem:
		if len(i.items) == 0 {
			return i.value, nil
		}
		items := make([]Value, 0, len(i.items))
		for _, member := range i.items {
			value, err := member.unwrap(depth + 1)
			if err != nil {
				return nil, err
			}
			items = append(items, value)
		}
		return NewArray(items...)
	case tomlValueItem:
		return i.value, nil
	}
	return nil, newValueError("document selector path is missing")
}

// Compares as Python dicts do: object member order is irrelevant.
func tkSemanticEqual(a, b Value) bool {
	switch left := a.(type) {
	case Object:
		right, ok := b.(Object)
		if !ok || left.Len() != right.Len() {
			return false
		}
		for _, member := range left.members {
			other, ok := right.Get(member.Key)
			if !ok || !tkSemanticEqual(member.Value, other) {
				return false
			}
		}
		return true
	case Array:
		right, ok := b.(Array)
		if !ok || left.Len() != right.Len() {
			return false
		}
		for index := range left.items {
			if !tkSemanticEqual(left.items[index], right.items[index]) {
				return false
			}
		}
		return true
	}
	return ValuesEqual(a, b)
}
