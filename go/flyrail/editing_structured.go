package flyrail

import (
	"bytes"
	"slices"
	"strings"
)

const (
	snapshotFrameBudget = 990
	identityEntryFrames = 5
	objectLevelFrames   = 5
	arrayLevelFrames    = 4
	documentNesting     = "document nesting is too deep"
)

type pathPart struct {
	key     string
	index   int
	isIndex bool
}

func segmentKey(name string) pathPart { return pathPart{key: name} }

func segmentIndex(index int) pathPart { return pathPart{index: index, isIndex: true} }

func appendPath(path []pathPart, part pathPart) []pathPart {
	return append(slices.Clip(path), part)
}

// A nil syntax or prefix means none.
type documentBackend interface {
	text() string
	clone() documentBackend
	native(path []pathPart) (Value, error)
	syntax(path []pathPart) ([]byte, error)
	ownedSyntax(path []pathPart) ([]byte, error)
	layout(path []pathPart) ([]byte, error)
	positionPrefix(path []pathPart) ([]byte, error)
	emptySyntax(path []pathPart) ([]byte, error)
	put(path []pathPart, value Value, syntax, layout []byte) error
	insert(path []pathPart, index int, value Value, syntax, prefix, layout []byte) error
	delete(path []pathPart) error
	relocate(path []pathPart, index int, value Value, syntax, prefix, layout []byte) error
}

func parseStructured(text string, format DocumentFormat) (documentBackend, error) {
	switch format {
	case FormatJSON, FormatJSONC:
		return parseJSONDocument(text, format == FormatJSONC)
	case FormatTOML:
		return parseTOMLDocument(text)
	}
	return nil, newValueError("editing " + string(format) + " documents is not supported")
}

func present(b []byte) []byte {
	if b == nil {
		return []byte{}
	}
	return b
}

func checkSnapshotFrames(value Value) error { return framesUsed(value, 0) }

func checkIdentityFrames(value Value) error { return framesUsed(value, identityEntryFrames) }

func framesUsed(value Value, used int) error {
	switch v := value.(type) {
	case Object:
		used += objectLevelFrames
		if used >= snapshotFrameBudget {
			return newNestingError(documentNesting)
		}
		for _, member := range v.members {
			if err := framesUsed(member.Value, used); err != nil {
				return err
			}
		}
	case Array:
		used += arrayLevelFrames
		if used >= snapshotFrameBudget {
			return newNestingError(documentNesting)
		}
		for _, item := range v.items {
			if err := framesUsed(item, used); err != nil {
				return err
			}
		}
	}
	return nil
}

func identityAt(value Value, member Member) (Value, bool, error) {
	current := value
	for _, name := range member.key {
		object, ok := current.(Object)
		if !ok {
			return nil, false, nil
		}
		if current, ok = object.Get(name); !ok {
			return nil, false, nil
		}
	}
	if err := checkIdentityFrames(current); err != nil {
		return nil, false, err
	}
	if _, scalar := current.(Scalar); len(member.key) == 0 || scalar {
		return current, true, nil
	}
	return nil, false, nil
}

func memberIndex(array []Value, member Member) (int, bool, error) {
	var seen []Value
	found := -1
	for index, item := range array {
		identity, ok, err := identityAt(item, member)
		if err != nil {
			return 0, false, err
		}
		if !ok {
			continue
		}
		if slices.ContainsFunc(seen, func(other Value) bool { return ValuesEqual(other, identity) }) {
			return 0, false, newValueError("duplicate or ambiguous array member identities")
		}
		if ValuesEqual(identity, member.identity) {
			found = index
		}
		seen = append(seen, identity)
	}
	return found, found >= 0, nil
}

func resolve(backend documentBackend, selector Selector) ([]pathPart, bool, error) {
	var path []pathPart
	for _, part := range selector.parts {
		node, err := backend.native(path)
		if err != nil {
			return nil, false, err
		}
		switch p := part.(type) {
		case Key:
			object, ok := node.(Object)
			if !ok {
				return nil, false, newValueError("object key selector crosses a non-object")
			}
			if _, ok := object.Get(p.name); !ok {
				return nil, false, nil
			}
			path = appendPath(path, segmentKey(p.name))
		case Member:
			array, ok := node.(Array)
			if !ok {
				return nil, false, newValueError("member selector requires an array")
			}
			index, found, err := memberIndex(array.items, p)
			if err != nil || !found {
				return nil, false, err
			}
			path = appendPath(path, segmentIndex(index))
		}
	}
	return path, true, nil
}

func anchorMember(array []Value, index int, key []string) (Member, error) {
	ambiguous := newValueError("array position has an ambiguous anchor")
	if index < 0 || index >= len(array) {
		return Member{}, ambiguous
	}
	item := array[index]
	probe, err := NewMember(Null{}, key)
	if err != nil {
		return Member{}, err
	}
	identity, ok, err := identityAt(item, probe)
	if err != nil {
		return Member{}, err
	}
	var result Member
	if ok {
		result, err = NewMember(identity, key)
	} else {
		if err := checkIdentityFrames(item); err != nil {
			return Member{}, err
		}
		result, err = NewMember(item, nil)
	}
	if err != nil {
		return Member{}, err
	}
	found, ok, err := memberIndex(array, result)
	if err != nil {
		return Member{}, err
	}
	if !ok || found != index {
		return Member{}, ambiguous
	}
	return result, nil
}

func splitIndex(path []pathPart) ([]pathPart, int, bool) {
	if len(path) == 0 || !path[len(path)-1].isIndex {
		return nil, 0, false
	}
	return path[:len(path)-1], path[len(path)-1].index, true
}

func arrayPosition(backend documentBackend, path []pathPart, member Member) (ArrayPosition, error) {
	invalid := newValueError("array position requires an array member")
	parentPath, index, ok := splitIndex(path)
	if !ok {
		return ArrayPosition{}, invalid
	}
	parent, err := backend.native(parentPath)
	if err != nil {
		return ArrayPosition{}, err
	}
	array, ok := parent.(Array)
	if !ok {
		return ArrayPosition{}, invalid
	}
	var previous, following *Member
	if index > 0 {
		anchor, err := anchorMember(array.items, index-1, member.key)
		if err != nil {
			return ArrayPosition{}, err
		}
		previous = &anchor
	}
	if index+1 < len(array.items) {
		anchor, err := anchorMember(array.items, index+1, member.key)
		if err != nil {
			return ArrayPosition{}, err
		}
		following = &anchor
	}
	prefix, err := backend.positionPrefix(path)
	if err != nil {
		return ArrayPosition{}, err
	}
	return NewArrayPosition(previous, following, prefix)
}

func structuredSnapshot(backend documentBackend, selector Selector) (*SelectionSnapshot, error) {
	path, found, err := resolve(backend, selector)
	if err != nil || !found {
		return nil, err
	}
	value, err := backend.native(path)
	if err != nil {
		return nil, err
	}
	if err := checkSnapshotFrames(value); err != nil {
		return nil, err
	}
	var position *ArrayPosition
	if member, ok := selector.parts[len(selector.parts)-1].(Member); ok {
		computed, err := arrayPosition(backend, path, member)
		if err != nil {
			return nil, err
		}
		position = &computed
	}
	syntax, err := backend.syntax(path)
	if err != nil {
		return nil, err
	}
	owned, err := backend.ownedSyntax(path)
	if err != nil {
		return nil, err
	}
	layout, err := backend.layout(path)
	if err != nil {
		return nil, err
	}
	snapshot, err := NewSelectionSnapshot(SnapshotTyped{value: value}, syntax, owned, position, layout)
	if err != nil {
		return nil, err
	}
	return &snapshot, nil
}

func insertionIndex(array []Value, placement *Placement) (int, error) {
	if placement == nil {
		return len(array), nil
	}
	switch placement.Position() {
	case PositionFirst:
		return 0, nil
	case PositionLast:
		return len(array), nil
	}
	anchor, ok := placement.Anchor()
	if !ok {
		return 0, newValueError("array placement requires an anchor")
	}
	index, found, err := memberIndex(array, anchor)
	if err != nil {
		return 0, err
	}
	if !found {
		return 0, newValueError("array placement anchor is missing")
	}
	if placement.Position() == PositionAfter {
		index++
	}
	return index, nil
}

func optionalMemberIndex(array []Value, member *Member) (int, bool, error) {
	if member == nil {
		return 0, false, nil
	}
	return memberIndex(array, *member)
}

func restorationIndex(array []Value, position ArrayPosition) (int, error) {
	previous, hasPrevious, err := optionalMemberIndex(array, position.previous)
	if err != nil {
		return 0, err
	}
	following, hasFollowing, err := optionalMemberIndex(array, position.following)
	if err != nil {
		return 0, err
	}
	switch {
	case hasPrevious && hasFollowing && previous >= following:
		return 0, newValueError("foreign array order prevents safe restoration")
	case hasPrevious:
		return previous + 1, nil
	case hasFollowing:
		return following, nil
	case len(array) > 0:
		return 0, newValueError("original array position has no surviving anchors")
	}
	return 0, nil
}

func withoutIndex(array []Value, skip int) []Value {
	return slices.Delete(slices.Clone(array), skip, skip+1)
}

func parents(backend documentBackend, selector Selector, containers []CreatedContainer) ([]pathPart, []CreatedContainer, error) {
	parts := selector.parts
	var path []pathPart
	for index, part := range parts[:len(parts)-1] {
		node, err := backend.native(path)
		if err != nil {
			return nil, nil, err
		}
		switch p := part.(type) {
		case Key:
			object, ok := node.(Object)
			if !ok {
				return nil, nil, newValueError("object key selector crosses a non-object")
			}
			path = appendPath(path, segmentKey(p.name))
			if _, ok := object.Get(p.name); ok {
				continue
			}
			value, err := emptyContainer(parts[index+1])
			if err != nil {
				return nil, nil, err
			}
			if err := backend.put(path, value, nil, []byte{}); err != nil {
				return nil, nil, err
			}
			empty, err := backend.emptySyntax(path)
			if err != nil {
				return nil, nil, err
			}
			containers = append(containers, NewCreatedContainer(Selector{parts: slices.Clone(parts[:index+1])}, empty))
		case Member:
			array, ok := node.(Array)
			if !ok {
				return nil, nil, newValueError("member selector requires an array")
			}
			found, ok, err := memberIndex(array.items, p)
			if err != nil {
				return nil, nil, err
			}
			if !ok {
				return nil, nil, newValueError("intermediate array member is missing")
			}
			path = appendPath(path, segmentIndex(found))
		}
	}
	return path, containers, nil
}

func emptyContainer(next SelectorPart) (Value, error) {
	if _, isKey := next.(Key); isKey {
		return NewObject()
	}
	return NewArray()
}

func putStructured(backend documentBackend, content StructuredContent, containers []CreatedContainer) ([]CreatedContainer, error) {
	parts := content.selector.parts
	if len(parts) == 0 {
		return nil, newValueError("a selector must select a key or member")
	}
	value := content.value
	parent, containers, err := parents(backend, content.selector, containers)
	if err != nil {
		return nil, err
	}
	current, err := backend.native(parent)
	if err != nil {
		return nil, err
	}
	switch part := parts[len(parts)-1].(type) {
	case Key:
		if _, ok := current.(Object); !ok {
			return nil, newValueError("object key selector crosses a non-object")
		}
		return containers, backend.put(appendPath(parent, segmentKey(part.name)), value, nil, []byte{})
	case Member:
		array, ok := current.(Array)
		if !ok {
			return nil, newValueError("member selector requires an array")
		}
		identity, ok, err := identityAt(value, part)
		if err != nil {
			return nil, err
		}
		if !ok || !ValuesEqual(identity, part.identity) {
			return nil, newValueError("generated array member does not match its selector")
		}
		index, found, err := memberIndex(array.items, part)
		if err != nil {
			return nil, err
		}
		if !found {
			insertion, err := insertionIndex(array.items, content.placement)
			if err != nil {
				return nil, err
			}
			return containers, backend.insert(parent, insertion, value, nil, nil, []byte{})
		}
		insertion := index
		if content.placement != nil {
			if insertion, err = insertionIndex(withoutIndex(array.items, index), content.placement); err != nil {
				return nil, err
			}
		}
		path := appendPath(parent, segmentIndex(index))
		if insertion == index {
			return containers, backend.put(path, value, nil, []byte{})
		}
		return containers, backend.relocate(path, insertion, value, nil, nil, []byte{})
	}
	return nil, newValueError("a selector must select a key or member")
}

func restoreStructured(backend documentBackend, selector Selector, baseline *SelectionSnapshot) error {
	path, found, err := resolve(backend, selector)
	if err != nil {
		return err
	}
	if !found {
		return newValueError("owned selection is missing")
	}
	if baseline == nil {
		return backend.delete(path)
	}
	typed, isTyped := baseline.value.(SnapshotTyped)
	if _, isKey := selector.parts[len(selector.parts)-1].(Key); isKey {
		if !isTyped {
			return newValueError("structured baseline requires a typed value")
		}
		return backend.put(path, typed.value, present(baseline.syntax), baseline.layout)
	}
	if baseline.position == nil || !isTyped {
		return newValueError("array baseline requires its original position and typed value")
	}
	parentPath, index, ok := splitIndex(path)
	if !ok {
		return newValueError("member selector requires an array")
	}
	native, err := backend.native(parentPath)
	if err != nil {
		return err
	}
	array, ok := native.(Array)
	if !ok {
		return newValueError("member selector requires an array")
	}
	insertion, err := restorationIndex(withoutIndex(array.items, index), *baseline.position)
	if err != nil {
		return err
	}
	if insertion == index {
		return backend.put(path, typed.value, present(baseline.syntax), baseline.layout)
	}
	return backend.relocate(path, insertion, typed.value, present(baseline.syntax), present(baseline.position.prefix), baseline.layout)
}

type restoreEdge struct {
	index        int
	dependencies []int
}

func addEdge(edges []restoreEdge, index, dependency int) {
	for position := range edges {
		if edges[position].index == index && !slices.Contains(edges[position].dependencies, dependency) {
			edges[position].dependencies = append(edges[position].dependencies, dependency)
		}
	}
}

type indexedSnapshot struct {
	index    int
	snapshot SelectionSnapshot
}

func restoreBatch(backend documentBackend, selections []OwnedSelection) error {
	var snapshots []indexedSnapshot
	var arrayPath []pathPart
	var array []Value
	for _, selection := range selections {
		selector, ok := selection.claim.selector.(Selector)
		if !ok || selection.baseline == nil {
			return newValueError("array restoration requires a baseline and selector")
		}
		path, found, err := resolve(backend, selector)
		if err != nil {
			return err
		}
		parent, index, ok := splitIndex(path)
		if !found || !ok {
			return newValueError("owned array member is missing")
		}
		arrayPath = slices.Clone(parent)
		native, err := backend.native(arrayPath)
		if err != nil {
			return err
		}
		items, ok := native.(Array)
		if !ok {
			return newValueError("member selector requires an array")
		}
		array = items.items
		existing := slices.IndexFunc(snapshots, func(item indexedSnapshot) bool { return item.index == index })
		if existing >= 0 {
			snapshots[existing].snapshot = *selection.baseline
		} else {
			snapshots = append(snapshots, indexedSnapshot{index: index, snapshot: *selection.baseline})
		}
	}
	length := len(array)
	tracked := func(index int) bool {
		return slices.ContainsFunc(snapshots, func(item indexedSnapshot) bool { return item.index == index })
	}
	edges := make([]restoreEdge, 0, length)
	var foreign []int
	for index := range length {
		edges = append(edges, restoreEdge{index: index})
		if !tracked(index) {
			foreign = append(foreign, index)
		}
	}
	for index := 1; index < len(foreign); index++ {
		addEdge(edges, foreign[index], foreign[index-1])
	}
	for _, item := range snapshots {
		position := item.snapshot.position
		if position == nil {
			return newValueError("array baseline requires its original position")
		}
		previous, hasPrevious, err := optionalMemberIndex(array, position.previous)
		if err != nil {
			return err
		}
		following, hasFollowing, err := optionalMemberIndex(array, position.following)
		if err != nil {
			return err
		}
		if !hasPrevious && !hasFollowing && length > 1 {
			return newValueError("original array position has no surviving anchors")
		}
		if hasPrevious {
			addEdge(edges, item.index, previous)
		}
		if hasFollowing {
			addEdge(edges, following, item.index)
		}
	}
	order := make([]int, 0, length)
	for len(edges) > 0 {
		ready := -1
		for _, edge := range edges {
			if len(edge.dependencies) > 0 {
				continue
			}
			if ready < 0 || readyBefore(edge.index, ready, tracked) {
				ready = edge.index
			}
		}
		if ready < 0 {
			return newValueError("foreign array order prevents safe restoration")
		}
		order = append(order, ready)
		edges = slices.DeleteFunc(edges, func(edge restoreEdge) bool { return edge.index == ready })
		for position := range edges {
			edges[position].dependencies = slices.DeleteFunc(edges[position].dependencies, func(dependency int) bool { return dependency == ready })
		}
	}
	current := make([]int, 0, length)
	for index := range length {
		current = append(current, index)
	}
	for target, identity := range order {
		if !tracked(identity) {
			for target < len(current) && current[target] != identity {
				path := appendPath(arrayPath, segmentIndex(target))
				value, err := backend.native(path)
				if err != nil {
					return err
				}
				syntax, err := backend.syntax(path)
				if err != nil {
					return err
				}
				layout, err := backend.layout(path)
				if err != nil {
					return err
				}
				if err := backend.relocate(path, max(len(current)-1, 0), value, present(syntax), nil, layout); err != nil {
					return err
				}
				moved := current[target]
				current = append(slices.Delete(current, target, target+1), moved)
			}
			continue
		}
		found := slices.IndexFunc(snapshots, func(item indexedSnapshot) bool { return item.index == identity })
		if found < 0 {
			continue
		}
		baseline := snapshots[found].snapshot
		typed, ok := baseline.value.(SnapshotTyped)
		if !ok || baseline.position == nil {
			return newValueError("array baseline requires a typed value and position")
		}
		index := slices.Index(current, identity)
		if index < 0 {
			return newValueError("owned array member is missing")
		}
		path := appendPath(arrayPath, segmentIndex(index))
		if index == target {
			if err := backend.put(path, typed.value, present(baseline.syntax), baseline.layout); err != nil {
				return err
			}
			continue
		}
		if err := backend.relocate(path, target, typed.value, present(baseline.syntax), present(baseline.position.prefix), baseline.layout); err != nil {
			return err
		}
		current = slices.Insert(slices.Delete(current, index, index+1), target, identity)
	}
	return nil
}

func readyBefore(candidate, best int, tracked func(int) bool) bool {
	if tracked(candidate) != tracked(best) {
		return tracked(candidate)
	}
	return candidate < best
}

func prune(backend documentBackend, containers []CreatedContainer, owned []OwnedSelection) ([]CreatedContainer, error) {
	ordered := slices.Clone(containers)
	slices.SortStableFunc(ordered, func(a, b CreatedContainer) int { return len(b.selector.parts) - len(a.selector.parts) })
	var retained []CreatedContainer
	for _, container := range ordered {
		path, found, err := resolve(backend, container.selector)
		if err != nil {
			return nil, err
		}
		if !found {
			continue
		}
		claim, err := NewClaim(OwnershipStructured, container.selector)
		if err != nil {
			return nil, err
		}
		value, err := backend.native(path)
		if err != nil {
			return nil, err
		}
		empty := false
		switch v := value.(type) {
		case Object:
			empty = v.Len() == 0
		case Array:
			empty = v.Len() == 0
		}
		if empty {
			syntax, err := backend.emptySyntax(path)
			if err != nil {
				return nil, err
			}
			empty = bytes.Equal(syntax, container.emptySyntax) &&
				!slices.ContainsFunc(owned, func(item OwnedSelection) bool { return claim.Overlaps(item.claim) })
		}
		if empty {
			if err := backend.delete(path); err != nil {
				return nil, err
			}
		} else {
			retained = append(retained, container)
		}
	}
	slices.Reverse(retained)
	return retained, nil
}

type structuredGroup struct {
	key       []SelectorPart
	selection []OwnedSelection
}

type structuredState struct {
	format     DocumentFormat
	containers []CreatedContainer
	owned      []OwnedSelection
	retired    []Claim
	groups     []structuredGroup
}

func (s *structuredState) request(backend documentBackend, request DocumentRequest, current *SelectionSnapshot, previous *OwnedSelection, status EditStatus) (documentBackend, EditStatus, error) {
	content, hasContent := request.content.(StructuredContent)
	reuse := (status == EditCurrent || status == EditAdopted) && hasContent && content.placement == nil
	var candidate documentBackend
	if reuse {
		candidate = backend.clone()
	} else {
		parsed, err := parseStructured(backend.text(), s.format)
		if err != nil {
			return nil, status, err
		}
		candidate = parsed
	}
	selector, ok := request.claim.selector.(Selector)
	if !ok {
		return nil, status, newValueError("structured editor requires a Selector")
	}
	if status == EditRemoved {
		if previous == nil {
			return nil, status, newValueError("retirement requires existing ownership")
		}
		claim := request.claim
		already := slices.ContainsFunc(s.retired, claim.Equal)
		batch := slices.IndexFunc(s.groups, func(group structuredGroup) bool {
			return len(group.selection) > 1 && slices.ContainsFunc(group.selection, func(item OwnedSelection) bool { return item.claim.Equal(claim) })
		})
		switch {
		case already:
		case batch >= 0:
			if err := restoreBatch(candidate, s.groups[batch].selection); err != nil {
				return nil, status, err
			}
			for _, item := range s.groups[batch].selection {
				s.retired = append(s.retired, item.claim)
			}
		default:
			if err := restoreStructured(candidate, selector, previous.baseline); err != nil {
				return nil, status, err
			}
		}
		s.owned = withoutOwned(s.owned, claim)
		return candidate, status, nil
	}
	if !hasContent {
		return candidate, status, nil
	}
	if status != EditAdopted && status != EditCurrent || content.placement != nil {
		before := candidate.text()
		containers, err := putStructured(candidate, content, s.containers)
		if err != nil {
			return nil, status, err
		}
		s.containers = containers
		if status == EditCurrent && candidate.text() != before {
			status = EditUpdated
		}
	}
	installed, err := structuredSnapshot(candidate, selector)
	if err != nil {
		return nil, status, err
	}
	if installed == nil || !desiredMatches(installed.value, desired{kind: desiredStructured, value: content.value}) {
		return nil, status, newValueError("published selection differs from the requested value")
	}
	selection, err := ownershipFor(request.claim, s.format, *installed, current, previous, status, nil)
	if err != nil {
		return nil, status, err
	}
	s.owned = upsertOwned(s.owned, request.claim, selection)
	return candidate, status, nil
}

func rootArrayRequested(claims []Claim) bool {
	return slices.ContainsFunc(claims, func(claim Claim) bool {
		selector, ok := claim.selector.(Selector)
		if !ok || len(selector.parts) == 0 {
			return false
		}
		_, member := selector.parts[0].(Member)
		return member
	})
}

func findPreimage(claims []Claim, preimages []*SelectionSnapshot, claim Claim) *SelectionSnapshot {
	index := slices.IndexFunc(claims, claim.Equal)
	if index < 0 {
		return nil
	}
	return preimages[index]
}

func editStructured(data document, text string, hasBOM bool, format DocumentFormat, requests []DocumentRequest, previous []OwnedSelection, provenance DocumentProvenance) (DocumentEdit, error) {
	if !data.exists {
		switch {
		case format == FormatTOML:
			text = ""
		case rootArrayRequested(requestAndOwnedClaims(requests, previous)):
			text = "[]"
		default:
			text = "{}"
		}
	}
	backend, err := parseStructured(text, format)
	if err != nil {
		return DocumentEdit{}, err
	}
	state := structuredState{format: format, owned: slices.Clone(previous), containers: provenance.Containers()}
	var observations []EditObservation
	var preimageClaims []Claim
	var preimages []*SelectionSnapshot
	for _, request := range requests {
		if selector, ok := request.claim.selector.(Selector); ok {
			snapshot, err := structuredSnapshot(backend, selector)
			if err != nil {
				return DocumentEdit{}, err
			}
			preimageClaims = append(preimageClaims, request.claim)
			preimages = append(preimages, snapshot)
		}
	}
	for _, request := range requests {
		old := findOwned(state.owned, request.claim)
		selector, ok := request.claim.selector.(Selector)
		if old == nil || !ok || request.content != nil || old.baseline == nil {
			continue
		}
		if _, member := selector.parts[len(selector.parts)-1].(Member); !member {
			continue
		}
		if !slices.ContainsFunc(preimageClaims, request.claim.Equal) {
			continue
		}
		current := findPreimage(preimageClaims, preimages, request.claim)
		if selectionStatus(current, desired{kind: desiredRetired}, old, request.acquisition, format) != EditRemoved {
			continue
		}
		key := slices.Clone(selector.parts[:len(selector.parts)-1])
		group := slices.IndexFunc(state.groups, func(item structuredGroup) bool {
			return slices.EqualFunc(item.key, key, selectorPartsEqual)
		})
		if group >= 0 {
			state.groups[group].selection = append(state.groups[group].selection, *old)
		} else {
			state.groups = append(state.groups, structuredGroup{key: key, selection: []OwnedSelection{*old}})
		}
	}
	originalOwned := slices.Clone(state.owned)
	for _, request := range requests {
		if _, ok := request.claim.selector.(Selector); !ok {
			return DocumentEdit{}, newValueError("structured editor requires a Selector")
		}
		current := findPreimage(preimageClaims, preimages, request.claim)
		want := desired{kind: desiredRetired}
		if content, ok := request.content.(StructuredContent); ok {
			want = desired{kind: desiredStructured, value: content.value}
		}
		old := findOwned(originalOwned, request.claim)
		status := selectionStatus(current, want, old, request.acquisition, format)
		if blockingStatus(status) {
			observations = append(observations, EditObservation{Claim: request.claim, Status: status, Current: current})
			continue
		}
		candidate, final, err := state.request(backend, request, current, old, status)
		if err != nil {
			failure, ok := asValueError(err)
			if !ok || failure.nesting {
				return DocumentEdit{}, err
			}
			observations = append(observations, EditObservation{Claim: request.claim, Status: EditConflict, Current: current, Message: failure.Message})
			continue
		}
		backend = candidate
		observations = append(observations, EditObservation{Claim: request.claim, Status: final, Current: current})
	}
	if slices.ContainsFunc(observations, func(item EditObservation) bool { return blockingStatus(item.Status) }) {
		return newDocumentEdit(data, data, previous, provenance, observations), nil
	}
	containers, err := prune(backend, state.containers, state.owned)
	if err != nil {
		return DocumentEdit{}, err
	}
	var separators []string
	for _, digest := range provenance.trailingSeparators {
		if !slices.Contains(separators, digest) {
			separators = append(separators, digest)
		}
	}
	if format == FormatTOML && data.exists && text != "" && !strings.HasSuffix(text, "\r") && !strings.HasSuffix(text, "\n") &&
		(strings.HasSuffix(backend.text(), "\r") || strings.HasSuffix(backend.text(), "\n")) {
		if digest := sha256Hex(data.data); !slices.Contains(separators, digest) {
			separators = append(separators, digest)
		}
	}
	params := DocumentProvenanceParams{Containers: containers, TrailingSeparators: separators}
	if !data.exists && len(state.owned) > 0 {
		params.EmptyDocument, params.HasEmptyDocument = []byte(text), true
	} else {
		params.EmptyDocument, params.HasEmptyDocument = provenance.EmptyDocument()
	}
	creation, err := NewDocumentProvenance(params)
	if err != nil {
		return DocumentEdit{}, err
	}
	return editResult(data, withBOM(hasBOM, backend.text()), previous, state.owned, provenance, creation, observations), nil
}
