package flyrail

import (
	"slices"
	"strings"
)

// Valid only while first and length still describe the body.
type tomlIndex struct {
	first      *tomlEntry
	length     int
	positions  map[string][]int
	outOfOrder []string
}

type tomlValidation struct {
	count int
	temp  *tomlContainer
}

func (x *tomlIndex) add(key *tomlKey, position int) {
	if key == nil {
		return
	}
	name := key.name()
	existing := x.positions[name]
	if len(existing) == 1 {
		x.outOfOrder = append(x.outOfOrder, name)
	}
	x.positions[name] = append(existing, position)
}

func (c *tomlContainer) lookup() *tomlIndex {
	if index := c.index; index != nil && index.length == len(c.body) && (len(c.body) == 0 || index.first == &c.body[0]) {
		return index
	}
	index := &tomlIndex{positions: map[string][]int{}, length: len(c.body)}
	for position := range c.body {
		index.add(c.body[position].key, position)
	}
	if len(c.body) > 0 {
		index.first = &c.body[0]
	}
	c.index = index
	return index
}

func (c *tomlContainer) positions(name string) []int {
	return c.lookup().positions[name]
}

func (c *tomlContainer) tablesAt(positions []int) []*tomlTable {
	tables := make([]*tomlTable, 0, len(positions))
	for _, position := range positions {
		if item := &c.body[position].item; item.kind == tomlTableItem {
			tables = append(tables, &item.table)
		}
	}
	return tables
}

func (c *tomlContainer) rawAppend(key *tomlKey, item tomlItem) {
	index := c.lookup()
	if item.kind == tomlTableItem && key != nil {
		c.tableKeys = append(c.tableKeys, key.name())
	}
	c.body = append(c.body, tomlEntry{key: key, item: item})
	index.add(key, len(c.body)-1)
	index.length, index.first = len(c.body), &c.body[0]
}

func (c *tomlContainer) append(key *tomlKey, item tomlItem, parsed bool) error {
	return c.appendValidated(key, item, parsed, true)
}

func (c *tomlContainer) tableAppend(key *tomlKey, item tomlItem, parsed, validate bool) error {
	if err := c.appendValidated(key, item, parsed, validate); err != nil {
		return err
	}
	if key == nil {
		return nil
	}
	if positions := c.positions(key.name()); len(positions) > 1 {
		return c.checkProxy(key.name(), positions)
	}
	return nil
}

// validate=false is tomlkit's Table.raw_append, which defers out-of-order checks.
func (c *tomlContainer) appendValidated(key *tomlKey, item tomlItem, parsed, validate bool) error {
	if key != nil && len(key.parts) > 1 {
		return c.appendDotted(key, item, parsed)
	}
	if key != nil {
		if positions := c.positions(key.name()); len(positions) > 0 {
			if err := c.checkContains(key.name(), positions, parsed); err != nil {
				return err
			}
			return c.appendExisting(key, item, positions, parsed, validate)
		}
		if len(c.body) > 0 && !parsed && (!item.isTable() || key.dotted) {
			index := lastIndexBeforeTable(c.body)
			if index < len(c.body) {
				if index > 0 {
					trailBeforeInsert(&c.body[index-1].item)
				}
				c.body = slices.Insert(c.body, index, tomlEntry{key: key, item: item})
				c.mutations++
				return nil
			}
			trailBeforeAppend(&c.body[len(c.body)-1].item)
			c.rawAppend(key, item)
			return nil
		}
	}
	c.rawAppend(key, glueTOMLItem(c, item, parsed))
	return nil
}

func (c *tomlContainer) checkContains(name string, positions []int, parsed bool) error {
	if len(positions) < 2 {
		return nil
	}
	if cached := c.validation[name]; parsed && cached != nil && cached.count == len(positions) {
		return nil
	}
	return c.checkProxy(name, positions)
}

func (c *tomlContainer) appendDotted(key *tomlKey, item tomlItem, parsed bool) error {
	if item.isTable() {
		return newValueError("Can't add a table to a dotted key")
	}
	head, leafPart := key.parts[:len(key.parts)-1], key.parts[len(key.parts)-1]
	leafKey := partKey(leafPart, false)
	leafKey.sep = key.sep
	node := item
	for index := len(head) - 1; index >= 1; index-- {
		level := tomlTable{trivia: defaultTrivia(), implicit: true}
		level.body.rawAppend(leafKey, node)
		leafKey, node = partKey(head[index], true), tableItem(level)
	}
	chain := tomlTable{trivia: defaultTrivia(), implicit: true}
	chain.body.rawAppend(leafKey, node)
	return c.append(partKey(head[0], true), tableItem(chain), parsed)
}

func (c *tomlContainer) appendExisting(key *tomlKey, item tomlItem, positions []int, parsed, validate bool) error {
	name := key.name()
	existing := &c.body[positions[len(positions)-1]]
	existingDotted := existing.key != nil && existing.key.dotted
	existingSuper := existing.item.kind == tomlTableItem && existing.item.table.implicit
	lastTableKeyIsEntry := len(c.tableKeys) > 0 && existing.key != nil && c.tableKeys[len(c.tableKeys)-1] == existing.key.name()
	switch item.kind {
	case tomlTableItem:
		table := item.table
		if !existing.item.isTable() {
			return tomlDuplicateError(name)
		}
		if table.aotElement {
			if existing.item.kind == tomlAOTItem {
				existing.item.aot = append(existing.item.aot, table)
				return nil
			}
			return c.replaceWithAOT(key, positions, aotItem([]tomlTable{existing.item.table, table}))
		}
		if existing.item.kind == tomlAOTItem {
			if !table.implicit || len(existing.item.aot) == 0 {
				return tomlDuplicateError(name)
			}
			last := &existing.item.aot[len(existing.item.aot)-1]
			for _, entry := range table.body.body {
				if err := last.body.append(entry.key, entry.item, parsed); err != nil {
					return err
				}
			}
			return nil
		}
		if existingSuper {
			if table.implicit {
				if key.dotted || existingDotted || !lastTableKeyIsEntry {
					position := len(c.body)
					if key.dotted && !parsed {
						position = lastIndexBeforeTable(c.body)
					}
					glued := glueTOMLItem(c, item, parsed)
					if position < len(c.body) {
						c.body = slices.Insert(c.body, position, tomlEntry{key: key, item: glued})
						c.mutations++
					} else {
						c.rawAppend(key, glued)
					}
					return c.validateKeyIf(name, parsed, validate)
				}
				for _, entry := range table.body.body {
					if err := existing.item.table.body.tableAppend(entry.key, entry.item, parsed, true); err != nil {
						return err
					}
				}
				return nil
			}
			if existingDotted {
				return tomlRedefinitionError()
			}
		} else if !table.implicit {
			return tomlDuplicateError(name)
		}
		if err := validateTOMLCandidate(&existing.item.table, &table); err != nil {
			return err
		}
		c.rawAppend(key, glueTOMLItem(c, item, parsed))
		return c.validateKeyIf(name, parsed, validate)
	case tomlAOTItem:
		if existing.item.kind == tomlAOTItem {
			existing.item.aot = append(existing.item.aot, item.aot...)
			return nil
		}
	}
	return tomlDuplicateError(name)
}

// The array lands at the key's first position; its other positions become null.
func (c *tomlContainer) replaceWithAOT(key *tomlKey, positions []int, aot tomlItem) error {
	for index := range aot.aot {
		if err := invalidateTOMLDisplayNames(&aot.aot[index]); err != nil {
			return err
		}
	}
	for _, position := range positions[1:] {
		c.body[position] = tomlEntry{item: tomlItem{kind: tomlNullItem}}
	}
	c.body[positions[0]] = tomlEntry{key: key, item: aot}
	c.index, c.validation, c.proxies = nil, nil, nil
	c.mutations++
	return nil
}

func invalidateTOMLDisplayNames(table *tomlTable) error {
	table.displayName = ""
	groups, err := groupTOMLEntries([]*tomlContainer{&table.body})
	if err != nil {
		return err
	}
	for _, group := range groups {
		if len(group.entries) > 1 {
			if _, err := newTOMLProxy(group.tables()); err != nil {
				return err
			}
			continue
		}
		item := &group.entries[0].item
		switch item.kind {
		case tomlTableItem:
			err = invalidateTOMLDisplayNames(&item.table)
		case tomlAOTItem:
			for index := range item.aot {
				if err = invalidateTOMLDisplayNames(&item.aot[index]); err != nil {
					break
				}
			}
		}
		if err != nil {
			return err
		}
	}
	return nil
}

func (c *tomlContainer) validateKeyIf(name string, parsed, validate bool) error {
	if !validate {
		return nil
	}
	return c.validateKey(name, parsed)
}

func (c *tomlContainer) validateKey(name string, parsed bool) error {
	positions := c.positions(name)
	if len(positions) < 2 {
		return nil
	}
	if !parsed {
		_, err := validateTOMLParts(c.tablesAt(positions), nil)
		return err
	}
	start, temp := 0, (*tomlContainer)(nil)
	if cached := c.validation[name]; cached != nil && cached.count <= len(positions) {
		start, temp = cached.count, cached.temp
	}
	temp, err := validateTOMLParts(c.tablesAt(positions[start:]), temp)
	if err != nil {
		delete(c.validation, name)
		return err
	}
	if c.validation == nil {
		c.validation = map[string]*tomlValidation{}
	}
	c.validation[name] = &tomlValidation{count: len(positions), temp: temp}
	return nil
}

func (c *tomlContainer) validateAll(parsed bool) error {
	for _, name := range slices.Clone(c.lookup().outOfOrder) {
		if err := c.validateKey(name, parsed); err != nil {
			return err
		}
	}
	return nil
}

func validateTOMLParts(parts []*tomlTable, temp *tomlContainer) (*tomlContainer, error) {
	if temp == nil {
		temp = &tomlContainer{}
	}
	for _, part := range parts {
		for index := range part.body.body {
			entry := &part.body.body[index]
			if err := temp.append(entry.key.clone(), entry.item.clone(), true); err != nil {
				return nil, err
			}
		}
	}
	return temp, temp.validateAll(true)
}

type tomlGroup struct {
	name    string
	entries []*tomlEntry
	members []*tomlTable
}

func (g *tomlGroup) tables() []*tomlTable {
	tables := make([]*tomlTable, 0, len(g.entries))
	for _, entry := range g.entries {
		if entry.item.kind == tomlTableItem {
			tables = append(tables, &entry.item.table)
		}
	}
	return tables
}

func aotMembers(item *tomlItem) []*tomlTable {
	members := make([]*tomlTable, len(item.aot))
	for index := range item.aot {
		members[index] = &item.aot[index]
	}
	return members
}

func groupTOMLEntries(bodies []*tomlContainer) ([]*tomlGroup, error) {
	var groups []*tomlGroup
	byName := map[string]*tomlGroup{}
	for _, body := range bodies {
		for index := range body.body {
			entry := &body.body[index]
			if entry.key == nil {
				continue
			}
			name := entry.key.name()
			group := byName[name]
			switch {
			case group == nil:
				group = &tomlGroup{name: name, entries: []*tomlEntry{entry}}
				if entry.item.kind == tomlAOTItem {
					group.members = aotMembers(&entry.item)
				}
				byName[name] = group
				groups = append(groups, group)
			case entry.item.kind == tomlAOTItem && len(group.entries) == 1 && group.entries[0].item.kind == tomlAOTItem:
				group.members = append(group.members, aotMembers(&entry.item)...)
			case group.entries[len(group.entries)-1].item.kind != tomlTableItem:
				return nil, tomlDuplicateError(name)
			default:
				group.entries = append(group.entries, entry)
			}
		}
	}
	return groups, nil
}

type tomlProxyCache struct {
	parts     []tomlProxyPart
	groups    map[string]*tomlGroupState
	chained   bool
	mutations int
}

type tomlProxyPart struct {
	position, entries, mutations int
}

type tomlGroupState struct {
	entries  int
	lastKind tomlItemKind
}

// Only the newest part can grow between reads.
func (c *tomlContainer) checkProxy(name string, positions []int) error {
	cache := c.proxies[name]
	if !cache.extends(c, positions) {
		cache = &tomlProxyCache{groups: map[string]*tomlGroupState{}, mutations: c.mutations}
	}
	if err := cache.extend(c, positions); err != nil {
		delete(c.proxies, name)
		return err
	}
	if c.proxies == nil {
		c.proxies = map[string]*tomlProxyCache{}
	}
	c.proxies[name] = cache
	if !cache.chained {
		return nil
	}
	_, err := newTOMLProxy(c.tablesAt(positions))
	return err
}

func (x *tomlProxyCache) extends(c *tomlContainer, positions []int) bool {
	if x == nil || x.mutations != c.mutations || len(x.parts) == 0 || len(x.parts) > len(positions) {
		return false
	}
	last := x.parts[len(x.parts)-1]
	if last.position != positions[len(x.parts)-1] || c.body[last.position].item.kind != tomlTableItem {
		return false
	}
	body := &c.body[last.position].item.table.body
	return last.mutations == body.mutations && last.entries <= len(body.body)
}

func (x *tomlProxyCache) extend(c *tomlContainer, positions []int) error {
	start := max(len(x.parts)-1, 0)
	for index := start; index < len(positions); index++ {
		item := &c.body[positions[index]].item
		if item.kind != tomlTableItem {
			x.chained, x.parts = true, nil
			return nil
		}
		body := &item.table.body
		if index == len(x.parts) {
			x.parts = append(x.parts, tomlProxyPart{position: positions[index], mutations: body.mutations})
		}
		part := &x.parts[index]
		for ; part.entries < len(body.body); part.entries++ {
			entry := &body.body[part.entries]
			if entry.key == nil {
				continue
			}
			if err := x.add(entry.key.name(), entry.item.kind); err != nil {
				return err
			}
		}
	}
	return nil
}

func (x *tomlProxyCache) add(name string, kind tomlItemKind) error {
	group := x.groups[name]
	switch {
	case group == nil:
		x.groups[name] = &tomlGroupState{entries: 1, lastKind: kind}
	case kind == tomlAOTItem && group.entries == 1 && group.lastKind == tomlAOTItem:
	case group.lastKind != tomlTableItem:
		return tomlDuplicateError(name)
	default:
		group.entries++
		group.lastKind = kind
		x.chained = true
	}
	return nil
}

type tomlProxy struct {
	tables []*tomlTable
	groups []*tomlGroup
}

func newTOMLProxy(tables []*tomlTable) (*tomlProxy, error) {
	bodies := make([]*tomlContainer, len(tables))
	for index, table := range tables {
		bodies[index] = &table.body
	}
	groups, err := groupTOMLEntries(bodies)
	if err != nil {
		return nil, err
	}
	for _, group := range groups {
		if len(group.entries) > 1 {
			if _, err := validateTOMLParts(group.tables(), nil); err != nil {
				return nil, err
			}
		}
	}
	return &tomlProxy{tables: tables, groups: groups}, nil
}

func unwrapTOMLGroups(groups []*tomlGroup) (Value, error) {
	members := make([]ObjectMember, 0, len(groups))
	for _, group := range groups {
		value, err := group.unwrap()
		if err != nil {
			return nil, err
		}
		members = append(members, ObjectMember{Key: group.name, Value: value})
	}
	return NewObject(members...)
}

func (g *tomlGroup) unwrap() (Value, error) {
	if len(g.entries) > 1 {
		proxy, err := newTOMLProxy(g.tables())
		if err != nil {
			return nil, err
		}
		return unwrapTOMLGroups(proxy.groups)
	}
	if g.entries[0].item.kind == tomlAOTItem {
		return unwrapTOMLMembers(g.members)
	}
	return g.entries[0].item.unwrapValue()
}

func unwrapTOMLMembers(members []*tomlTable) (Value, error) {
	items := make([]Value, 0, len(members))
	for _, member := range members {
		value, err := unwrapTOMLBody(&member.body)
		if err != nil {
			return nil, err
		}
		items = append(items, value)
	}
	return NewArray(items...)
}

func unwrapTOMLBody(body *tomlContainer) (Value, error) {
	groups, err := groupTOMLEntries([]*tomlContainer{body})
	if err != nil {
		return nil, err
	}
	return unwrapTOMLGroups(groups)
}

func (i *tomlItem) unwrapValue() (Value, error) {
	switch i.kind {
	case tomlValueItem, tomlArrayItem, tomlInlineItem:
		return i.value, nil
	case tomlTableItem:
		return unwrapTOMLBody(&i.table.body)
	case tomlAOTItem:
		return unwrapTOMLMembers(aotMembers(i))
	}
	return nil, newValueError("document selector path is missing")
}

func glueTOMLItem(container *tomlContainer, item tomlItem, parsed bool) tomlItem {
	if parsed || len(container.body) == 0 {
		return item
	}
	previousWhitespace := tomlEndsWithWhitespace(container.body)
	switch item.kind {
	case tomlTableItem:
		if item.table.trivia.indent == "" && !previousWhitespace {
			item.table.trivia.indent = "\n"
		}
	case tomlAOTItem:
		if len(item.aot) > 0 && !strings.Contains(item.aot[0].trivia.indent, "\n") && !previousWhitespace {
			item.aot[0].trivia.indent = "\n" + item.aot[0].trivia.indent
		}
	}
	return item
}

func tomlEndsWithWhitespace(body []tomlEntry) bool {
	for index := len(body) - 1; index >= 0; index-- {
		if body[index].item.kind != tomlNullItem {
			return tomlItemEndsWithWhitespace(&body[index].item)
		}
	}
	return false
}

func tomlItemEndsWithWhitespace(item *tomlItem) bool {
	switch item.kind {
	case tomlWhitespaceItem:
		return true
	case tomlTableItem:
		return tomlEndsWithWhitespace(item.table.body.body)
	case tomlAOTItem:
		return len(item.aot) > 0 && tomlEndsWithWhitespace(item.aot[len(item.aot)-1].body.body)
	}
	return false
}

func trailBeforeInsert(item *tomlItem) {
	if item.kind == tomlWhitespaceItem || tomlItemEndsWithWhitespace(item) {
		return
	}
	if trivia := item.anyTrivia(); trivia != nil && !strings.Contains(trivia.trail, "\n") {
		trivia.trail += "\n"
	}
}

func trailBeforeAppend(item *tomlItem) {
	if item.kind == tomlTableItem && item.table.implicit && strings.Contains(item.table.trivia.trail, "\n") {
		body := item.table.body.body
		for index := len(body) - 1; index >= 0; index-- {
			if body[index].item.kind == tomlNullItem {
				continue
			}
			if body[index].item.kind != tomlWhitespaceItem {
				if trivia := body[index].item.anyTrivia(); trivia != nil && !strings.Contains(trivia.trail, "\n") {
					trivia.trail += item.table.trivia.trail
				}
			}
			break
		}
	}
	if item.kind == tomlWhitespaceItem || tomlItemEndsWithWhitespace(item) {
		return
	}
	if trivia := item.anyTrivia(); trivia != nil && !strings.Contains(trivia.trail, "\n") {
		trivia.trail += "\n"
	}
}

func lastIndexBeforeTable(body []tomlEntry) int {
	last := -1
	for index := range body {
		entry := &body[index]
		switch entry.item.kind {
		case tomlNullItem, tomlWhitespaceItem:
			continue
		case tomlTableItem, tomlAOTItem:
			if entry.key != nil && !entry.key.dotted {
				return last + 1
			}
			if entry.item.kind == tomlTableItem && entry.key != nil && entry.key.dotted && rendersTableHeader(&entry.item.table) {
				return last + 1
			}
		}
		last = index
	}
	return last + 1
}

func rendersTableHeader(table *tomlTable) bool {
	for index := range table.body.body {
		entry := &table.body.body[index]
		switch entry.item.kind {
		case tomlAOTItem:
			return true
		case tomlTableItem:
			if entry.key == nil || !entry.key.dotted || !entry.item.table.implicit || rendersTableHeader(&entry.item.table) {
				return true
			}
		}
	}
	return false
}

// An out-of-order existing key reads back as a proxy, never a table.
func validateTOMLCandidate(current, candidate *tomlTable) error {
	for index := range candidate.body.body {
		entry := &candidate.body.body[index]
		if entry.key == nil {
			continue
		}
		name := entry.key.name()
		positions := current.body.positions(name)
		if len(positions) == 0 {
			continue
		}
		existingTable, existing := false, (*tomlItem)(nil)
		if len(positions) > 1 {
			if _, err := newTOMLProxy(current.body.tablesAt(positions)); err != nil {
				return err
			}
		} else {
			existing = &current.body.body[positions[0]].item
			existingTable = existing.isTable()
		}
		if existingTable != entry.item.isTable() {
			return tomlDuplicateError(name)
		}
		if entry.key.dotted {
			return tomlRedefinitionError()
		}
		if existing != nil && existing.kind == tomlTableItem && entry.item.kind == tomlTableItem {
			if !existing.table.implicit && !entry.item.table.implicit {
				return tomlDuplicateError(name)
			}
			if err := validateTOMLCandidate(&existing.table, &entry.item.table); err != nil {
				return err
			}
		}
	}
	return nil
}

func coalesceTOML(container tomlContainer) tomlContainer {
	var result tomlContainer
	for _, entry := range container.body {
		switch entry.item.kind {
		case tomlTableItem:
			entry.item.table.body = coalesceTOML(entry.item.table.body)
		case tomlAOTItem:
			for index := range entry.item.aot {
				entry.item.aot[index].body = coalesceTOML(entry.item.aot[index].body)
			}
		}
		if len(result.body) > 0 && entry.key != nil && entry.key.dotted && entry.item.kind == tomlTableItem {
			index := len(result.body) - 1
			for index > 0 && (result.body[index].item.kind == tomlCommentItem || result.body[index].item.kind == tomlWhitespaceItem) {
				index--
			}
			previous := &result.body[index]
			if previous.key != nil && previous.key.dotted && previous.key.asString() == entry.key.asString() && previous.item.kind == tomlTableItem {
				merged := previous.item.table
				gaps := slices.Clone(result.body[index+1:])
				result.body = result.body[:index+1]
				body := tomlContainer{body: append(append(slices.Clip(merged.body.body), gaps...), entry.item.table.body.body...)}
				merged.body = coalesceTOML(body)
				result.body[index].item = tableItem(merged)
				continue
			}
		}
		result.rawAppend(entry.key, entry.item)
	}
	return result
}
