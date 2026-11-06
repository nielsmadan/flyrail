package flyrail

import (
	"errors"
	"fmt"
	"math"
	"slices"
	"strconv"
	"strings"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

const tkMissingMessage = "document selector path is missing"

func tkMissing() error {
	return newValueError(tkMissingMessage)
}

func isTkMissing(err error) bool {
	var failure *ValueError
	return errors.As(err, &failure) && failure.Message == tkMissingMessage
}

type tkNode struct {
	root  *tkContainer
	item  *tkItem
	proxy *tkProxy
}

func (n tkNode) mapping() *tkContainer {
	switch {
	case n.root != nil:
		return n.root
	case n.proxy != nil:
		return n.proxy.internal
	case n.item != nil && (n.item.kind == tomlTableItem || n.item.kind == tomlInlineItem):
		return n.item.body
	}
	return nil
}

func (n tkNode) tableLike() bool {
	return n.proxy != nil || n.item != nil && tkTableLike(n.item)
}

func (c *tkContainer) item(name string) (tkNode, error) {
	positions := c.positions(name)
	switch len(positions) {
	case 0:
		return tkNode{}, tkMissing()
	case 1:
		return tkNode{item: c.body[positions[0]].item}, nil
	}
	proxy, err := newTkProxy(c, positions)
	if err != nil {
		return tkNode{}, err
	}
	return tkNode{proxy: proxy}, nil
}

func (c *tkContainer) rawAppend(key *tomlKey, item *tkItem) error {
	if key != nil {
		if positions := c.positions(key.name()); len(positions) > 0 && c.body[positions[len(positions)-1]].item.kind != tomlTableItem {
			return tomlDuplicateError(key.name())
		}
	}
	c.body = append(c.body, tkEntry{key: key, item: item})
	return nil
}

func (c *tkContainer) previousIndex() int {
	for index := len(c.body) - 1; index >= 0; index-- {
		if c.body[index].item.kind != tomlNullItem {
			return index
		}
	}
	return -1
}

func (c *tkContainer) previousItem() *tkItem {
	if index := c.previousIndex(); index >= 0 {
		return c.body[index].item
	}
	return nil
}

func tkEndsWithWhitespace(item *tkItem) bool {
	for depth := 0; item != nil && depth <= tkMaxDepth; depth++ {
		switch item.kind {
		case tomlWhitespaceItem:
			return true
		case tomlTableItem:
			item = item.body.previousItem()
		case tomlAOTItem:
			if len(item.aot) == 0 {
				return false
			}
			item = item.aot[len(item.aot)-1]
		default:
			return false
		}
	}
	return false
}

func tkRendersTableHeader(table *tkItem, depth int) bool {
	if depth > tkMaxDepth {
		return true
	}
	for _, entry := range table.body.body {
		switch entry.item.kind {
		case tomlAOTItem:
			return true
		case tomlTableItem:
			if entry.key == nil || !entry.key.dotted || !entry.item.isSuperTable() || tkRendersTableHeader(entry.item, depth+1) {
				return true
			}
		}
	}
	return false
}

func (c *tkContainer) lastIndexBeforeTable() int {
	last := -1
	for index, entry := range c.body {
		item := entry.item
		if item.kind == tomlNullItem || item.kind == tomlWhitespaceItem && !item.fixed {
			continue
		}
		if tkTableLike(item) && entry.key != nil && !entry.key.dotted {
			break
		}
		if item.kind == tomlTableItem && entry.key != nil && entry.key.dotted && tkRendersTableHeader(item, 0) {
			break
		}
		last = index
	}
	return last + 1
}

// Skips out-of-order children, as tomlkit does.
func (i *tkItem) invalidateDisplayName(depth int) {
	if depth > tkMaxDepth {
		return
	}
	switch i.kind {
	case tomlTableItem:
		i.displayName = nil
		for _, group := range i.body.groups() {
			if len(group.positions) == 1 {
				i.body.body[group.positions[0]].item.invalidateDisplayName(depth + 1)
			}
		}
	case tomlAOTItem:
		for _, member := range i.aot {
			member.invalidateDisplayName(depth + 1)
		}
	}
}

func (c *tkContainer) append(key *tomlKey, item *tkItem) error {
	prev := c.previousItem()
	prevWS := prev != nil && tkEndsWithWhitespace(prev)
	if item.kind == tomlTableItem {
		if !c.parsed {
			item.invalidateDisplayName(0)
		}
		if len(c.body) > 0 && !c.parsed && item.trivia.indent == "" && !prevWS && key != nil && !key.dotted {
			item.trivia.indent = "\n"
		}
	}
	if item.kind == tomlAOTItem && len(c.body) > 0 && !c.parsed {
		item.invalidateDisplayName(0)
		if len(item.aot) > 0 && !strings.Contains(item.aot[0].trivia.indent, "\n") && !prevWS {
			item.aot[0].trivia.indent = "\n" + item.aot[0].trivia.indent
		}
	}
	if key != nil && c.contains(key.name()) {
		return tomlDuplicateError(key.name())
	}
	if key != nil && len(c.body) > 0 && !c.parsed && (!tkTableLike(item) || key.dotted) {
		last := c.lastIndexBeforeTable()
		if last < len(c.body) {
			after := c.body[last].item
			if after.kind != tomlWhitespaceItem && !strings.Contains(after.trivia.indent, "\n") {
				after.trivia.indent = "\n" + after.trivia.indent
			}
			return c.insertAt(last, key, item)
		}
		previous := c.body[len(c.body)-1].item
		if previous.kind == tomlTableItem && previous.isSuperTable() {
			child := previous.body.previousItem()
			if child != nil && child.kind != tomlWhitespaceItem && strings.Contains(previous.trivia.trail, "\n") && !strings.Contains(child.trivia.trail, "\n") {
				child.trivia.trail += previous.trivia.trail
			}
		}
		if previous.kind != tomlWhitespaceItem && !tkEndsWithWhitespace(previous) && !strings.Contains(previous.trivia.trail, "\n") {
			previous.trivia.trail += "\n"
		}
	}
	c.body = append(c.body, tkEntry{key: key, item: item})
	return nil
}

func (c *tkContainer) insertAt(index int, key *tomlKey, item *tkItem) error {
	if index > len(c.body)-1 {
		return newValueError(fmt.Sprintf("Unable to insert at position %d", index))
	}
	if index > 0 {
		previous := c.body[index-1].item
		if previous.kind != tomlWhitespaceItem && !tkEndsWithWhitespace(previous) && !tkTableLike(item) && !strings.Contains(previous.trivia.trail, "\n") {
			previous.trivia.trail += "\n"
		}
	}
	c.body = slices.Insert(c.body, index, tkEntry{key: key, item: item})
	return nil
}

func (c *tkContainer) remove(name string) error {
	positions := c.positions(name)
	if len(positions) == 0 {
		return tkMissing()
	}
	for _, position := range positions {
		c.body[position] = tkEntry{item: tkNull()}
	}
	return nil
}

func (c *tkContainer) removeItem(item *tkItem) {
	for index, entry := range c.body {
		if entry.item == item {
			c.body[index] = tkEntry{item: tkNull()}
			return
		}
	}
}

func (c *tkContainer) setItem(name string, value *tkItem) error {
	if c.contains(name) {
		return c.replace(name, value)
	}
	return c.append(tkSingleKey(name), value)
}

func (c *tkContainer) replace(name string, value *tkItem) error {
	positions := c.positions(name)
	for _, position := range positions[1:] {
		c.body[position] = tkEntry{item: tkNull()}
	}
	index := positions[0]
	oldKey, old := c.body[index].key, c.body[index].item
	dottedToHeader := oldKey.dotted && (value.kind == tomlAOTItem || value.kind == tomlTableItem && !value.isSuperTable())
	reposition := false
	if dottedToHeader {
		for _, entry := range c.body[index+1:] {
			if entry.item.kind == tomlNullItem || entry.item.kind == tomlWhitespaceItem {
				continue
			}
			if tkTableLike(entry.item) && (entry.key == nil || !entry.key.dotted) {
				continue
			}
			reposition = true
			break
		}
	}
	changed := tkTableLike(value) != tkTableLike(old)
	newKey := oldKey
	if changed || dottedToHeader {
		newKey = tkSingleKey(name)
	}
	if changed || reposition {
		c.body[index] = tkEntry{item: tkNull()}
		placed := false
		if tkTableLike(value) {
			for position := index; position < len(c.body); position++ {
				entry := c.body[position]
				if tkTableLike(entry.item) && (entry.key == nil || !entry.key.dotted) {
					if err := c.insertAt(position, newKey, value); err != nil {
						return err
					}
					index, placed = position, true
					break
				}
			}
		}
		if !placed {
			index = -1
			if err := c.append(newKey, value); err != nil {
				return err
			}
		}
	} else {
		if value.kind != tomlWhitespaceItem && value.kind != tomlAOTItem {
			value.trivia.indent = old.trivia.indent
			if value.trivia.commentWS == "" {
				value.trivia.commentWS = old.trivia.commentWS
			}
			if value.trivia.comment == "" {
				value.trivia.comment = old.trivia.comment
			}
			value.trivia.trail = old.trivia.trail
		}
		c.body[index] = tkEntry{key: newKey, item: value}
	}
	if value != old {
		value.invalidateDisplayName(0)
	}
	if value.kind == tomlTableItem {
		last := c.previousIndex()
		if index < 0 {
			index = last
		}
		hasWS := tkEndsWithWhitespace(value)
		replaceWS := old.kind == tomlTableItem && len(old.body.body) > 0 && old.body.body[len(old.body.body)-1].item.kind == tomlWhitespaceItem
		nextWS := index < last && c.body[index+1].item.kind == tomlWhitespaceItem
		if (index < last || replaceWS) && !nextWS && !hasWS {
			value.body.body = append(value.body.body, tkEntry{item: tkWhitespace("\n", false)})
		}
	}
	return nil
}

func firstSpaceRun(text string) (string, bool) {
	start := strings.IndexByte(text, ' ')
	if start < 0 {
		return "", false
	}
	end := start
	for end < len(text) && text[end] == ' ' {
		end++
	}
	return text[start:end], true
}

func (i *tkItem) tableSetItem(name string, value *tkItem) error {
	if i.kind == tomlInlineItem {
		if value.kind != tomlWhitespaceItem && value.trivia.comment != "" {
			value.trivia.comment = ""
		}
		if value.kind == tomlTableItem {
			return newValueError("Inline tables cannot contain a table")
		}
	}
	replacing := i.body.contains(name)
	if err := i.body.setItem(name, value); err != nil {
		return err
	}
	if replacing {
		return nil
	}
	if run, ok := firstSpaceRun(i.trivia.indent); ok && value.kind != tomlWhitespaceItem {
		split := strings.IndexByte(value.trivia.indent, ' ')
		if split < 0 {
			split = len(value.trivia.indent)
		}
		value.trivia.indent = value.trivia.indent[:split] + run + value.trivia.indent[split:]
	}
	return nil
}

func (p *tkProxy) removeTable(table *tkItem) {
	if index := slices.Index(p.tables, table); index >= 0 {
		p.tables = slices.Delete(p.tables, index, index+1)
	}
	p.container.removeItem(table)
}

func tkHasPlainValue(table *tkItem) bool {
	return slices.ContainsFunc(table.body.groups(), func(group tkGroup) bool {
		return len(group.positions) == 1 && !tkTableLike(table.body.body[group.positions[0]].item)
	})
}

func (p *tkProxy) setItem(name string, value *tkItem, depth int) error {
	if depth > tkMaxDepth {
		return newNestingError(documentNesting)
	}
	if indices, ok := p.tablesMap[name]; ok {
		for len(indices) > 1 {
			table := p.tables[indices[len(indices)-1]]
			indices = indices[:len(indices)-1]
			p.removeTable(table)
		}
		p.tablesMap[name] = indices
		table := p.tables[indices[0]]
		old, err := table.body.item(name)
		if err != nil {
			return err
		}
		if old.tableLike() && !tkTableLike(value) {
			if err := table.body.remove(name); err != nil {
				return err
			}
			delete(p.tablesMap, name)
			return p.setItem(name, value, depth+1)
		}
		if err := table.tableSetItem(name, value); err != nil {
			return err
		}
	} else if len(p.tables) > 0 {
		target := p.tables[0]
		if !tkTableLike(value) {
			if index := slices.IndexFunc(p.tables, tkHasPlainValue); index >= 0 {
				target = p.tables[index]
			} else if index := slices.IndexFunc(p.tables, func(table *tkItem) bool { return !table.isSuperTable() }); index >= 0 {
				target = p.tables[index]
			}
		}
		if err := target.tableSetItem(name, value); err != nil {
			return err
		}
	} else if err := p.container.setItem(name, value); err != nil {
		return err
	}
	return p.internal.setItem(name, value)
}

func (p *tkProxy) delItem(name string) error {
	indices, ok := p.tablesMap[name]
	if !ok {
		return tkMissing()
	}
	for position := len(indices) - 1; position >= 0; position-- {
		table := p.tables[indices[position]]
		if err := table.body.remove(name); err != nil {
			return err
		}
		if table.body.length() == 0 && len(p.tables) > 1 {
			p.removeTable(table)
		}
	}
	delete(p.tablesMap, name)
	return p.internal.remove(name)
}

func clampAOTIndex(index, length int) int {
	if index < 0 {
		index += length
	}
	return max(min(index, length), 0)
}

func (i *tkItem) aotInsert(index int, table *tkItem) {
	index = clampAOTIndex(index, len(i.aot))
	if run, ok := firstSpaceRun(i.trivia.indent); ok {
		split := strings.IndexByte(table.trivia.indent, ' ')
		if split < 0 {
			split = len(table.trivia.indent)
		}
		table.trivia.indent = table.trivia.indent[:split] + run + table.trivia.indent[split:]
	}
	i.aot = slices.Insert(i.aot, index, table)
}

func isBareKeyName(name string) bool {
	if name == "" {
		return false
	}
	for index := range len(name) {
		if !isTOMLBareKeyByte(int(name[index])) {
			return false
		}
	}
	return true
}

func tkSingleKey(name string) *tomlKey {
	raw := name
	quoted := !isBareKeyName(name)
	if quoted {
		raw = `"` + tomlEscape(name) + `"`
	}
	return &tomlKey{parts: []tomlKeyPart{{name: name, raw: raw, quoted: quoted}}, sep: " = "}
}

func tomlEscape(text string) string {
	var out strings.Builder
	for _, character := range text {
		switch character {
		case '\b':
			out.WriteString(`\b`)
		case '\t':
			out.WriteString(`\t`)
		case '\n':
			out.WriteString(`\n`)
		case '\f':
			out.WriteString(`\f`)
		case '\r':
			out.WriteString(`\r`)
		case 0x1B:
			out.WriteString(`\e`)
		case '"':
			out.WriteString(`\"`)
		case '\\':
			out.WriteString(`\\`)
		default:
			if character < 0x20 || character == 0x7F {
				fmt.Fprintf(&out, `\u%04x`, character)
			} else {
				out.WriteRune(character)
			}
		}
	}
	return out.String()
}

func pythonIsoTime(t Time) string {
	text := fmt.Sprintf("%02d:%02d:%02d", t.hour, t.minute, t.second)
	if t.microsecond != 0 {
		text += fmt.Sprintf(".%06d", t.microsecond)
	}
	return text
}

func tkScalarText(value Value) (string, error) {
	switch v := value.(type) {
	case Bool:
		if v {
			return "true", nil
		}
		return "false", nil
	case Int:
		return strconv.FormatInt(int64(v), 10), nil
	case Float:
		number := v.Float64()
		switch {
		case math.IsNaN(number):
			return "nan", nil
		case math.IsInf(number, 1):
			return "inf", nil
		case math.IsInf(number, -1):
			return "-inf", nil
		}
		return jsonx.PythonFloatRepr(number), nil
	case String:
		return `"` + tomlEscape(v.text) + `"`, nil
	case Date:
		return v.String(), nil
	case Time:
		return pythonIsoTime(v), nil
	case DateTime:
		text := v.date.String() + "T" + pythonIsoTime(v.time)
		if v.utc {
			text += "Z"
		}
		return text, nil
	}
	return "", newValueError("Unable to convert an object of <class 'NoneType'> to a TOML item")
}

type tkParent uint8

const (
	tkParentNone tkParent = iota
	tkParentTable
	tkParentArray
	tkParentInline
)

type tkNativeKind uint8

const (
	tkNativeScalar tkNativeKind = iota
	tkNativeObject
	tkNativeArray
)

type tkNative struct {
	kind    tkNativeKind
	scalar  Value
	members []tkNativeMember
	items   []tkNative
}

type tkNativeMember struct {
	key   string
	value tkNative
}

func tkNativeOf(value Value) tkNative {
	switch v := value.(type) {
	case Object:
		members := make([]tkNativeMember, 0, len(v.members))
		for _, member := range v.members {
			members = append(members, tkNativeMember{key: member.Key, value: tkNativeOf(member.Value)})
		}
		return tkNative{kind: tkNativeObject, members: members}
	case Array:
		items := make([]tkNative, 0, len(v.items))
		for _, item := range v.items {
			items = append(items, tkNativeOf(item))
		}
		return tkNative{kind: tkNativeArray, items: items}
	}
	return tkNative{scalar: value}
}

func (n tkNative) toValue(depth int) (Value, error) {
	if depth > tkMaxDepth {
		return nil, newNestingError(documentNesting)
	}
	switch n.kind {
	case tkNativeObject:
		members := make([]ObjectMember, 0, len(n.members))
		for _, member := range n.members {
			value, err := member.value.toValue(depth + 1)
			if err != nil {
				return nil, err
			}
			members = append(members, ObjectMember{Key: member.key, Value: value})
		}
		return NewObject(members...)
	case tkNativeArray:
		items := make([]Value, 0, len(n.items))
		for _, item := range n.items {
			value, err := item.toValue(depth + 1)
			if err != nil {
				return nil, err
			}
			items = append(items, value)
		}
		return NewArray(items...)
	}
	return n.scalar, nil
}

func (c *tkContainer) unwrapNative(depth int) (tkNative, error) {
	if depth > tkMaxDepth {
		return tkNative{}, newNestingError(documentNesting)
	}
	groups := c.groups()
	members := make([]tkNativeMember, 0, len(groups))
	for _, group := range groups {
		var value tkNative
		var err error
		if len(group.positions) > 1 {
			var proxy *tkProxy
			if proxy, err = newTkProxy(c, group.positions); err == nil {
				value, err = proxy.internal.unwrapNative(depth + 1)
			}
		} else {
			value, err = c.body[group.positions[0]].item.unwrapNative(depth + 1)
		}
		if err != nil {
			return tkNative{}, err
		}
		members = append(members, tkNativeMember{key: group.name, value: value})
	}
	return tkNative{kind: tkNativeObject, members: members}, nil
}

func (i *tkItem) unwrapNative(depth int) (tkNative, error) {
	if depth > tkMaxDepth {
		return tkNative{}, newNestingError(documentNesting)
	}
	switch i.kind {
	case tomlTableItem, tomlInlineItem:
		return i.body.unwrapNative(depth + 1)
	case tomlAOTItem, tomlArrayItem:
		members := i.aot
		if i.kind == tomlArrayItem {
			members = i.items
		}
		items := make([]tkNative, 0, len(members))
		for _, member := range members {
			value, err := member.unwrapNative(depth + 1)
			if err != nil {
				return tkNative{}, err
			}
			items = append(items, value)
		}
		return tkNative{kind: tkNativeArray, items: items}, nil
	case tomlValueItem:
		return tkNative{scalar: i.value}, nil
	}
	return tkNative{}, newValueError("document selector path is missing")
}

// Object values move last; the rest keep their order.
func tkSortedMembers(members []tkNativeMember) []tkNativeMember {
	sorted := slices.Clone(members)
	slices.SortStableFunc(sorted, func(a, b tkNativeMember) int {
		left, right := a.value.kind == tkNativeObject, b.value.kind == tkNativeObject
		switch {
		case left == right:
			return 0
		case left:
			return 1
		}
		return -1
	})
	return sorted
}

func tkGenerate(value tkNative, parent tkParent, depth int) (*tkItem, error) {
	if depth > tkMaxDepth {
		return nil, newNestingError(documentNesting)
	}
	switch value.kind {
	case tkNativeObject:
		if parent == tkParentArray || parent == tkParentInline {
			table := &tkItem{kind: tomlInlineItem, body: &tkContainer{}, trivia: defaultTrivia()}
			return table, tkFillTable(table, value.members, tkParentInline, false, depth)
		}
		table := &tkItem{kind: tomlTableItem, body: &tkContainer{}, trivia: defaultTrivia()}
		return table, tkFillTable(table, value.members, tkParentTable, false, depth)
	case tkNativeArray:
		allObjects := len(value.items) > 0 && !slices.ContainsFunc(value.items, func(item tkNative) bool { return item.kind != tkNativeObject })
		if allObjects && (parent == tkParentNone || parent == tkParentTable) {
			aot := &tkItem{kind: tomlAOTItem}
			for _, item := range value.items {
				table := &tkItem{kind: tomlTableItem, body: &tkContainer{}, trivia: defaultTrivia(), aotElement: true}
				if err := tkFillTable(table, tkSortedMembers(item.members), tkParentTable, false, depth); err != nil {
					return nil, err
				}
				if len(aot.aot) > 0 && aot.aot[len(aot.aot)-1].body.length() > 0 && !strings.Contains(table.trivia.indent, "\n") {
					table.trivia.indent = "\n" + table.trivia.indent
				}
				aot.aot = append(aot.aot, table)
			}
			return aot, nil
		}
		parts := make([]string, 0, len(value.items))
		for _, item := range value.items {
			var member *tkItem
			var err error
			if item.kind == tkNativeObject {
				member = &tkItem{kind: tomlInlineItem, body: &tkContainer{}, trivia: defaultTrivia(), inlineNew: true}
				err = tkFillTable(member, tkSortedMembers(item.members), tkParentInline, true, depth)
			} else {
				member, err = tkGenerate(item, tkParentArray, depth+1)
			}
			if err != nil {
				return nil, err
			}
			parts = append(parts, member.asString())
		}
		array, err := value.toValue(depth)
		if err != nil {
			return nil, err
		}
		return &tkItem{kind: tomlArrayItem, raw: "[" + strings.Join(parts, ", ") + "]", value: array, trivia: defaultTrivia()}, nil
	}
	raw, err := tkScalarText(value.scalar)
	if err != nil {
		return nil, err
	}
	return &tkItem{kind: tomlValueItem, raw: raw, value: value.scalar, trivia: defaultTrivia()}, nil
}

func tkFillTable(table *tkItem, members []tkNativeMember, parent tkParent, clearTrail bool, depth int) error {
	for _, member := range members {
		child, err := tkGenerate(member.value, parent, depth+1)
		if err != nil {
			return err
		}
		if clearTrail {
			child.trivia.trail = ""
		}
		if err := table.tableSetItem(member.key, child); err != nil {
			return err
		}
	}
	return nil
}

func tkInline(node *tkItem) (*tkItem, error) {
	if node.kind != tomlTableItem {
		return node, nil
	}
	value, err := node.unwrapNative(0)
	if err != nil {
		return nil, err
	}
	inline := &tkItem{kind: tomlInlineItem, body: &tkContainer{}, trivia: defaultTrivia(), inlineNew: true}
	return inline, tkFillTable(inline, value.members, tkParentInline, false, 0)
}
