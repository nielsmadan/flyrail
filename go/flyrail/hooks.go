package flyrail

import (
	"encoding/binary"
	"errors"
	"maps"
	"math"
	"slices"
	"strconv"
	"strings"
	"unicode/utf16"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/hookruntime"
)

// The hook protocol limits.
const (
	HookInputLimit              = 1_048_576
	HookOutputLimit             = 65_536
	HookErrorLimit              = 16_384
	HookTextLimit               = 8_192
	HookJSONDepthLimit          = 64
	HookHandlerLimit            = 64
	HookEventTimeoutLimitMillis = 300_000
)

// HookError is a hook protocol failure identified by its wire code.
type HookError struct {
	Code string
}

// Error returns "flyrail-hook:" followed by the code.
func (e *HookError) Error() string {
	if e == nil {
		return ""
	}
	return "flyrail-hook:" + e.Code
}

func hookError(code string) error {
	return &HookError{Code: code}
}

// HookShell is the shell a hook launcher is written for.
type HookShell string

// The hook launcher shells.
const (
	ShellPOSIX HookShell = "posix"
	ShellCmd   HookShell = "cmd"
)

// ParseHookShell returns the hook shell spelled s.
func ParseHookShell(s string) (HookShell, error) {
	return parseEnum(s, "HookShell", ShellPOSIX, ShellCmd)
}

// HookRuntime is the Node runtime configuration and the embedded runtime modules hooks are rendered with.
type HookRuntime struct {
	node      string
	shell     HookShell
	resources []BundleEntry
}

// NewHookRuntime validates node as an absolute executable path without running it; "" means none for either argument.
func NewHookRuntime(node string, shell HookShell) (*HookRuntime, error) {
	if node != "" && !validNodePath(node) {
		return nil, newValueError("hook runtime requires an absolute Node 24+ executable path")
	}
	if shell != "" {
		if err := checkEnum(shell, "HookShell", ShellPOSIX, ShellCmd); err != nil {
			return nil, err
		}
	}
	files := hookruntime.Files()
	resources := make([]BundleEntry, 0, len(files))
	for _, file := range files {
		entry, err := newEntry(file.Name, file.Data, false, false)
		if err != nil {
			return nil, err
		}
		resources = append(resources, entry)
	}
	return &HookRuntime{node: node, shell: shell, resources: resources}, nil
}

func validNodePath(path string) bool {
	if _, err := validateRoute(path); err != nil {
		return false
	}
	if strings.ContainsFunc(path, func(r rune) bool { return r < ' ' }) {
		return false
	}
	return slices.ContainsFunc(strings.Split(path, "/"), func(part string) bool { return part != "" && part != "." })
}

// Node returns the Node executable path, or "" for none.
func (r *HookRuntime) Node() string {
	if r == nil {
		return ""
	}
	return r.node
}

// Shell returns the launcher shell, or "" for none.
func (r *HookRuntime) Shell() HookShell {
	if r == nil {
		return ""
	}
	return r.shell
}

// Resources returns copies of the embedded runtime modules.
func (r *HookRuntime) Resources() []BundleEntry {
	if r == nil {
		return nil
	}
	return copyEntries(r.resources)
}

// HookText is JavaScript string text: UTF-16 code units that may contain lone surrogates.
type HookText struct {
	units  []uint16
	scalar string
	lone   bool
}

// HookTextFromUTF16 returns text holding a copy of units.
func HookTextFromUTF16(units []uint16) HookText {
	return newHookText(slices.Clone(units))
}

// HookTextFromString returns the UTF-16 text of s, which must be valid UTF-8.
func HookTextFromString(s string) (HookText, error) {
	if err := validText(s, "hook text"); err != nil {
		return HookText{}, err
	}
	return hookTextOf(s), nil
}

func newHookText(units []uint16) HookText {
	for index := 0; index < len(units); index++ {
		unit := units[index]
		switch {
		case unit >= 0xd800 && unit <= 0xdbff && index+1 < len(units) && units[index+1] >= 0xdc00 && units[index+1] <= 0xdfff:
			index++
		case unit >= 0xd800 && unit <= 0xdfff:
			return HookText{units: units, lone: true}
		}
	}
	return HookText{units: units, scalar: string(utf16.Decode(units))}
}

func hookTextOf(s string) HookText {
	units := make([]uint16, 0, len(s))
	for _, r := range s {
		units = utf16.AppendRune(units, r)
	}
	return HookText{units: units, scalar: s}
}

// UTF8 returns the text as a string, or false when it contains a lone surrogate.
func (t HookText) UTF8() (string, bool) {
	if t.lone {
		return "", false
	}
	return t.scalar, true
}

// Units returns a copy of the UTF-16 code units.
func (t HookText) Units() []uint16 { return slices.Clone(t.units) }

// IsEmpty reports whether the text has no code units.
func (t HookText) IsEmpty() bool { return len(t.units) == 0 }

func (t HookText) isBlank() bool {
	return !slices.ContainsFunc(t.units, func(unit uint16) bool { return !isJavaScriptWhitespace(unit) })
}

func (t HookText) utf8Len() int {
	length := 0
	for index := 0; index < len(t.units); index++ {
		unit := t.units[index]
		switch {
		case unit < 0x80:
			length++
		case unit < 0x800:
			length += 2
		case unit >= 0xd800 && unit <= 0xdbff && index+1 < len(t.units) && t.units[index+1] >= 0xdc00 && t.units[index+1] <= 0xdfff:
			length += 4
			index++
		default:
			length += 3
		}
	}
	return length
}

func (t HookText) equals(s string) bool {
	index := 0
	for _, r := range s {
		for _, unit := range utf16.AppendRune(nil, r) {
			if index >= len(t.units) || t.units[index] != unit {
				return false
			}
			index++
		}
	}
	return index == len(t.units)
}

func (t HookText) key() string {
	key := make([]byte, 0, 2*len(t.units))
	for _, unit := range t.units {
		key = binary.BigEndian.AppendUint16(key, unit)
	}
	return string(key)
}

func isJavaScriptWhitespace(unit uint16) bool {
	switch {
	case unit >= 0x09 && unit <= 0x0d, unit >= 0x2000 && unit <= 0x200a:
		return true
	}
	switch unit {
	case 0x20, 0xa0, 0x1680, 0x2028, 0x2029, 0x202f, 0x205f, 0x3000, 0xfeff:
		return true
	}
	return false
}

func joinedUTF8Len(values []HookText) int {
	length := max(len(values)-1, 0)
	for _, value := range values {
		length += value.utf8Len()
	}
	return length
}

type hookKind uint8

const (
	hookNull hookKind = iota
	hookBool
	hookInt
	hookFloat
	hookString
	hookArray
	hookObject
)

type hookValue struct {
	kind    hookKind
	boolean bool
	integer int64
	number  float64
	text    HookText
	items   []hookValue
	members []hookPair
}

type hookPair struct {
	key   HookText
	value hookValue
}

func hookStringValue(text HookText) hookValue {
	return hookValue{kind: hookString, text: text}
}

func hookTextValue(s string) hookValue {
	return hookStringValue(hookTextOf(s))
}

func hookObjectValue(members ...hookPair) hookValue {
	return hookValue{kind: hookObject, members: members}
}

func hookMemberOf(key string, value hookValue) hookPair {
	return hookPair{key: hookTextOf(key), value: value}
}

func (v hookValue) scalar() (string, bool) {
	if v.kind != hookString {
		return "", false
	}
	return v.text.UTF8()
}

func (v hookValue) field(name string) (hookValue, bool) {
	for _, member := range v.members {
		if member.key.equals(name) {
			return member.value, true
		}
	}
	return hookValue{}, false
}

func (v hookValue) isOne() bool {
	return v.kind == hookInt && v.integer == 1 || v.kind == hookFloat && math.Float64bits(v.number) == math.Float64bits(1)
}

// HookMember is one member of a hook JSON object.
type HookMember struct {
	Key   string
	Value HookJSON
}

// HookJSON is a JSON value exchanged with hook handlers; the zero value is JSON null.
type HookJSON struct {
	value hookValue
}

// ParseHookJSON parses one JSON value under the hook input limit.
func ParseHookJSON(data []byte) (HookJSON, error) {
	value, code := parseHookValue(data, HookInputLimit)
	if code != "" {
		return HookJSON{}, hookError(code)
	}
	return HookJSON{value: value}, nil
}

// HookObject returns a JSON object with the members in order.
func HookObject(members ...HookMember) (HookJSON, error) {
	pairs := make([]hookPair, 0, len(members))
	for _, member := range members {
		if !utf8.ValidString(member.Key) {
			return HookJSON{}, hookError("json-key")
		}
		pairs = append(pairs, hookMemberOf(member.Key, member.Value.value))
	}
	return checkedHookJSON(hookObjectValue(pairs...))
}

// HookString returns a JSON string.
func HookString(t HookText) (HookJSON, error) {
	return checkedHookJSON(hookStringValue(t))
}

// HookBool returns a JSON boolean.
func HookBool(b bool) HookJSON {
	return HookJSON{value: hookValue{kind: hookBool, boolean: b}}
}

// HookInt returns i as a JavaScript number, rounded to the nearest binary64 value.
func HookInt(i int64) (HookJSON, error) {
	return checkedHookJSON(hookValue{kind: hookFloat, number: float64(i)})
}

// HookArray returns a JSON array of items.
func HookArray(items ...HookJSON) (HookJSON, error) {
	values := make([]hookValue, 0, len(items))
	for _, item := range items {
		values = append(values, item.value)
	}
	return checkedHookJSON(hookValue{kind: hookArray, items: values})
}

// HookNull returns JSON null.
func HookNull() HookJSON {
	return HookJSON{}
}

func checkedHookJSON(value hookValue) (HookJSON, error) {
	if code := validateEncodedHookValue(value, HookInputLimit); code != "" {
		return HookJSON{}, hookError(code)
	}
	return HookJSON{value: value}, nil
}

// Bytes returns the canonical encoding.
func (j HookJSON) Bytes() []byte { return encodeHookValue(j.value) }

// IsObject reports whether the value is a JSON object.
func (j HookJSON) IsObject() bool { return j.value.kind == hookObject }

// UTF8 returns the string value, or false for a non-string or a string with a lone surrogate.
func (j HookJSON) UTF8() (string, bool) { return j.value.scalar() }

// Text returns the string value as text, or false for a non-string.
func (j HookJSON) Text() (HookText, bool) {
	if j.value.kind != hookString {
		return HookText{}, false
	}
	return j.value.text, true
}

// HookRequest is the canonical packet a hook handler receives.
type HookRequest struct {
	event     HookEvent
	context   HookJSON
	tool      HookText
	hasTool   bool
	input     HookJSON
	hasInput  bool
	result    HookJSON
	hasResult bool
}

// NewHookRequest returns a validated request; nil tool, input or result means absent.
func NewHookRequest(event HookEvent, context HookJSON, tool *HookText, input, result *HookJSON) (HookRequest, error) {
	if !slices.Contains(hookEvents, event) {
		return HookRequest{}, hookError("input-event")
	}
	if !context.IsObject() {
		return HookRequest{}, hookError("host-input")
	}
	toolEvent := event == EventBeforeTool || event == EventAfterTool
	if toolEvent && (tool == nil || tool.IsEmpty()) {
		return HookRequest{}, hookError("host-tool")
	}
	if toolEvent && input == nil {
		return HookRequest{}, hookError("host-arguments")
	}
	if event == EventAfterTool && result == nil {
		return HookRequest{}, hookError("host-result")
	}
	if event == EventPrompt && (input == nil || input.value.kind != hookString) {
		return HookRequest{}, hookError("host-prompt")
	}
	if !toolEvent && tool != nil || !toolEvent && event != EventPrompt && input != nil || event != EventAfterTool && result != nil {
		return HookRequest{}, hookError("input-fields")
	}
	request := HookRequest{event: event, context: context}
	if tool != nil {
		request.tool, request.hasTool = *tool, true
	}
	if input != nil {
		request.input, request.hasInput = *input, true
	}
	if result != nil {
		request.result, request.hasResult = *result, true
	}
	if code := validateEncodedHookValue(request.json(), HookInputLimit); code != "" {
		return HookRequest{}, hookError(code)
	}
	return request, nil
}

// ParseHookRequest parses and validates a request packet.
func ParseHookRequest(data []byte) (HookRequest, error) {
	value, code := parseHookValue(data, HookInputLimit)
	if code != "" {
		return HookRequest{}, hookError(code)
	}
	if value.kind != hookObject {
		return HookRequest{}, hookError("host-input")
	}
	expected := []string{"version", "event", "context", "tool", "input", "result"}
	for _, member := range value.members {
		if !slices.ContainsFunc(expected, member.key.equals) {
			return HookRequest{}, hookError("input-fields")
		}
	}
	if version, ok := value.field("version"); !ok || !version.isOne() {
		return HookRequest{}, hookError("input-version")
	}
	eventValue, _ := value.field("event")
	eventText, ok := eventValue.scalar()
	event := HookEvent(eventText)
	if !ok || !slices.Contains(hookEvents, event) {
		return HookRequest{}, hookError("input-event")
	}
	contextValue, ok := value.field("context")
	if !ok {
		return HookRequest{}, hookError("host-input")
	}
	var tool *HookText
	if toolValue, ok := value.field("tool"); ok {
		if toolValue.kind != hookString {
			return HookRequest{}, hookError("host-tool")
		}
		tool = &toolValue.text
	}
	return NewHookRequest(event, HookJSON{value: contextValue}, tool, optionalHookJSON(value, "input"), optionalHookJSON(value, "result"))
}

func optionalHookJSON(object hookValue, name string) *HookJSON {
	value, ok := object.field(name)
	if !ok {
		return nil
	}
	return &HookJSON{value: value}
}

// Event returns the hook event.
func (r HookRequest) Event() HookEvent { return r.event }

// Context returns the host context object.
func (r HookRequest) Context() HookJSON { return r.context }

// Tool returns the tool name, or false when absent.
func (r HookRequest) Tool() (HookText, bool) { return r.tool, r.hasTool }

// Input returns the tool input or prompt, or false when absent.
func (r HookRequest) Input() (HookJSON, bool) { return r.input, r.hasInput }

// Result returns the tool result, or false when absent.
func (r HookRequest) Result() (HookJSON, bool) { return r.result, r.hasResult }

// Bytes returns the canonical encoding.
func (r HookRequest) Bytes() []byte { return encodeHookValue(r.json()) }

func (r HookRequest) json() hookValue {
	members := []hookPair{
		hookMemberOf("version", hookValue{kind: hookInt, integer: 1}),
		hookMemberOf("event", hookTextValue(string(r.event))),
		hookMemberOf("context", r.context.value),
	}
	if r.hasTool {
		members = append(members, hookMemberOf("tool", hookStringValue(r.tool)))
	}
	if r.hasInput {
		members = append(members, hookMemberOf("input", r.input.value))
	}
	if r.hasResult {
		members = append(members, hookMemberOf("result", r.result.value))
	}
	return hookObjectValue(members...)
}

// HookResponse is the outcome a hook handler returns.
type HookResponse struct {
	outcome  HookOutcome
	value    HookJSON
	hasValue bool
}

// ContinueResponse returns the continue outcome.
func ContinueResponse() HookResponse {
	return HookResponse{outcome: OutcomeContinue}
}

// BlockResponse returns a block outcome with a nonblank reason.
func BlockResponse(reason HookText) (HookResponse, error) {
	return textResponse(OutcomeBlock, reason, "reason")
}

// AskResponse returns an ask outcome with a nonblank reason.
func AskResponse(reason HookText) (HookResponse, error) {
	return textResponse(OutcomeAsk, reason, "reason")
}

// ContextResponse returns a context outcome with nonblank text.
func ContextResponse(text HookText) (HookResponse, error) {
	return textResponse(OutcomeContext, text, "context")
}

// ModifyInputResponse returns a modify-input outcome: a string for prompt events, an object otherwise.
func ModifyInputResponse(event HookEvent, input HookJSON) (HookResponse, error) {
	if !slices.Contains(hookEvents, event) {
		return HookResponse{}, hookError("input-event")
	}
	valid := input.IsObject()
	if event == EventPrompt {
		valid = input.value.kind == hookString
	}
	if !valid {
		return HookResponse{}, hookError("replacement")
	}
	response := HookResponse{outcome: OutcomeModifyInput, value: input, hasValue: true}
	if code := validateEncodedHookValue(response.json(), HookInputLimit); code != "" {
		return HookResponse{}, hookError(code)
	}
	return response, nil
}

func textResponse(outcome HookOutcome, text HookText, code string) (HookResponse, error) {
	if text.isBlank() || text.utf8Len() > HookTextLimit {
		return HookResponse{}, hookError(code)
	}
	value, err := checkedHookJSON(hookStringValue(text))
	if err != nil {
		return HookResponse{}, err
	}
	return HookResponse{outcome: outcome, value: value, hasValue: true}, nil
}

// ParseHookResponse parses a handler's output for event, accepting only the declared outcomes.
func ParseHookResponse(data []byte, event HookEvent, declared []HookOutcome) (HookResponse, error) {
	if !slices.Contains(hookEvents, event) {
		return HookResponse{}, hookError("input-event")
	}
	if len(data) > HookOutputLimit {
		return HookResponse{}, hookError("stdout-limit")
	}
	raw, code := parseHookValue(data, HookOutputLimit)
	if code != "" {
		return HookResponse{}, hookError("invalid-output")
	}
	if raw.kind != hookObject {
		return HookResponse{}, hookError("undeclared-outcome")
	}
	if version, ok := raw.field("version"); !ok || !version.isOne() {
		return HookResponse{}, hookError("undeclared-outcome")
	}
	outcomeValue, _ := raw.field("outcome")
	outcomeText, ok := outcomeValue.scalar()
	outcome := HookOutcome(outcomeText)
	if !ok || !slices.Contains(hookOutcomes, outcome) || !slices.Contains(declared, outcome) {
		return HookResponse{}, hookError("undeclared-outcome")
	}
	fieldName := responseField(outcome)
	expected := 2
	if fieldName != "" {
		expected = 3
	}
	if len(raw.members) != expected || slices.ContainsFunc(raw.members, func(member hookPair) bool {
		return !member.key.equals("version") && !member.key.equals("outcome") && (fieldName == "" || !member.key.equals(fieldName))
	}) {
		return HookResponse{}, hookError("output-fields")
	}
	value, _ := raw.field(fieldName)
	switch outcome {
	case OutcomeBlock:
		return BlockResponse(value.text)
	case OutcomeAsk:
		return AskResponse(value.text)
	case OutcomeContext:
		return ContextResponse(value.text)
	case OutcomeModifyInput:
		return ModifyInputResponse(event, HookJSON{value: value})
	default:
		return ContinueResponse(), nil
	}
}

func responseField(outcome HookOutcome) string {
	switch outcome {
	case OutcomeBlock, OutcomeAsk:
		return "reason"
	case OutcomeContext:
		return "text"
	case OutcomeModifyInput:
		return "input"
	default:
		return ""
	}
}

// Outcome returns the outcome.
func (r HookResponse) Outcome() HookOutcome { return r.outcome }

// Value returns the reason, text or replacement input, or false for continue.
func (r HookResponse) Value() (HookJSON, bool) { return r.value, r.hasValue }

// Bytes returns the canonical encoding.
func (r HookResponse) Bytes() []byte { return encodeHookValue(r.json()) }

func (r HookResponse) json() hookValue {
	members := []hookPair{
		hookMemberOf("version", hookValue{kind: hookInt, integer: 1}),
		hookMemberOf("outcome", hookTextValue(string(r.outcome))),
	}
	if name := responseField(r.outcome); r.hasValue && name != "" {
		members = append(members, hookMemberOf(name, r.value.value))
	}
	return hookObjectValue(members...)
}

// HookEffects are the combined effects the runtime reports back to a host bridge.
type HookEffects struct {
	input     HookJSON
	hasInput  bool
	contexts  []HookText
	decision  HookOutcome
	reason    HookText
	hasReason bool
}

// ParseHookEffects parses and validates an effects record.
func ParseHookEffects(data []byte) (HookEffects, error) {
	raw, code := parseHookValue(data, HookOutputLimit)
	if code != "" {
		return HookEffects{}, hookError("invalid-output")
	}
	if raw.kind != hookObject {
		return HookEffects{}, hookError("output-fields")
	}
	allowed := []string{"input", "contexts", "decision", "reason"}
	for _, member := range raw.members {
		if !slices.ContainsFunc(allowed, member.key.equals) {
			return HookEffects{}, hookError("output-fields")
		}
	}
	var effects HookEffects
	if input, ok := raw.field("input"); ok {
		effects.input, effects.hasInput = HookJSON{value: input}, true
	}
	contexts, ok := raw.field("contexts")
	if !ok || contexts.kind != hookArray {
		return HookEffects{}, hookError("output-fields")
	}
	effects.contexts = make([]HookText, 0, len(contexts.items))
	for _, item := range contexts.items {
		if item.kind != hookString {
			return HookEffects{}, hookError("context")
		}
		effects.contexts = append(effects.contexts, item.text)
	}
	if slices.ContainsFunc(effects.contexts, HookText.isBlank) || joinedUTF8Len(effects.contexts) > HookTextLimit {
		return HookEffects{}, hookError("context-limit")
	}
	if decision, ok := raw.field("decision"); ok {
		switch {
		case decision.kind == hookString && decision.text.equals("block"):
			effects.decision = OutcomeBlock
		case decision.kind == hookString && decision.text.equals("ask"):
			effects.decision = OutcomeAsk
		default:
			return HookEffects{}, hookError("undeclared-outcome")
		}
	}
	if reason, ok := raw.field("reason"); ok {
		if reason.kind != hookString || reason.text.isBlank() || reason.text.utf8Len() > HookTextLimit {
			return HookEffects{}, hookError("reason")
		}
		effects.reason, effects.hasReason = reason.text, true
	}
	if (effects.decision != "") != effects.hasReason {
		return HookEffects{}, hookError("output-fields")
	}
	return effects, nil
}

// Input returns the replacement input, or false when absent.
func (e HookEffects) Input() (HookJSON, bool) { return e.input, e.hasInput }

// Contexts returns a copy of the added context texts.
func (e HookEffects) Contexts() []HookText { return slices.Clone(e.contexts) }

// Decision returns the block or ask decision, or "" for none.
func (e HookEffects) Decision() HookOutcome { return e.decision }

// Reason returns the decision reason, or false when absent.
func (e HookEffects) Reason() (HookText, bool) { return e.reason, e.hasReason }

// Bytes returns the canonical encoding.
func (e HookEffects) Bytes() []byte {
	contexts := make([]hookValue, 0, len(e.contexts))
	for _, text := range e.contexts {
		contexts = append(contexts, hookStringValue(text))
	}
	members := []hookPair{hookMemberOf("contexts", hookValue{kind: hookArray, items: contexts})}
	if e.hasInput {
		members = append(members, hookMemberOf("input", e.input.value))
	}
	if e.decision != "" {
		members = append(members, hookMemberOf("decision", hookTextValue(string(e.decision))))
	}
	if e.hasReason {
		members = append(members, hookMemberOf("reason", hookStringValue(e.reason)))
	}
	return encodeHookValue(hookObjectValue(members...))
}

// HookHandlerParams are the inputs of NewHookHandler.
type HookHandlerParams struct {
	ID            string                      // required
	Event         HookEvent                   // required
	Argv          []string                    // required, nonempty
	Environment   map[string]EnvironmentValue // nil = no variables
	Cwd           string                      // required, absolute
	TimeoutMillis int                         // required, 1-300000
	Outcomes      []HookOutcome               // required, nonempty, unique
	Tools         []string                    // nil = every tool; only for tool events
}

// HookHandler is one translated handler the runtime runs for an event.
type HookHandler struct {
	id            string
	event         HookEvent
	argv          []string
	environment   []environmentVariable
	cwd           string
	timeoutMillis int
	outcomes      []HookOutcome
	tools         []string
}

// NewHookHandler returns a validated handler with outcomes, tools and environment names in byte order.
func NewHookHandler(p HookHandlerParams) (HookHandler, error) {
	if err := validateIdentifier(p.ID, "artifact id"); err != nil {
		return HookHandler{}, err
	}
	if err := checkEnum(p.Event, "HookEvent", hookEvents...); err != nil {
		return HookHandler{}, err
	}
	if len(p.Argv) == 0 {
		return HookHandler{}, newValueError("argv requires a command")
	}
	for _, argument := range p.Argv {
		if err := validText(argument, "command argument"); err != nil {
			return HookHandler{}, err
		}
		if strings.Contains(argument, "\x00") {
			return HookHandler{}, newValueError("command arguments cannot contain NUL")
		}
	}
	if p.Argv[0] == "" {
		return HookHandler{}, newValueError("command cannot be empty")
	}
	cwd, err := validateRoute(p.Cwd)
	if err != nil {
		return HookHandler{}, err
	}
	if p.TimeoutMillis < 1 || p.TimeoutMillis > HookEventTimeoutLimitMillis {
		return HookHandler{}, newValueError("hook timeout must be 1-300000 integer milliseconds")
	}
	if len(p.Outcomes) == 0 {
		return HookHandler{}, newValueError("hook requires declared HookOutcome values")
	}
	for index, outcome := range p.Outcomes {
		if err := checkEnum(outcome, "HookOutcome", hookOutcomes...); err != nil {
			return HookHandler{}, err
		}
		if slices.Contains(p.Outcomes[:index], outcome) {
			return HookHandler{}, newValueError("hook outcomes must be unique")
		}
	}
	for _, name := range p.Tools {
		if err := validText(name, "tool name"); err != nil {
			return HookHandler{}, err
		}
		if name == "" || strings.Contains(name, "\x00") {
			return HookHandler{}, newValueError("tool names must be nonempty and cannot contain NUL")
		}
	}
	for index, name := range p.Tools {
		if slices.Contains(p.Tools[:index], name) {
			return HookHandler{}, newValueError("tool names must be unique")
		}
	}
	if len(p.Tools) > 0 && p.Event != EventBeforeTool && p.Event != EventAfterTool {
		return HookHandler{}, newValueError("tool selection requires a tool event")
	}
	environment, err := handlerEnvironment(p.Environment)
	if err != nil {
		return HookHandler{}, err
	}
	outcomes := slices.Clone(p.Outcomes)
	slices.Sort(outcomes)
	tools := slices.Clone(p.Tools)
	slices.Sort(tools)
	return HookHandler{
		id:            p.ID,
		event:         p.Event,
		argv:          slices.Clone(p.Argv),
		environment:   environment,
		cwd:           cwd,
		timeoutMillis: p.TimeoutMillis,
		outcomes:      outcomes,
		tools:         tools,
	}, nil
}

func handlerEnvironment(env map[string]EnvironmentValue) ([]environmentVariable, error) {
	names := slices.Sorted(maps.Keys(env))
	variables := make([]environmentVariable, 0, len(names))
	for _, name := range names {
		if _, err := NewEnvRef(name); err != nil {
			return nil, err
		}
		switch value := env[name].(type) {
		case EnvText:
			if err := validText(string(value), "environment value"); err != nil {
				return nil, err
			}
			if strings.Contains(string(value), "\x00") {
				return nil, newValueError("environment values cannot contain NUL")
			}
		case EnvRef:
			if _, err := NewEnvRef(value.name); err != nil {
				return nil, err
			}
		default:
			return nil, newValueError("environment values must be EnvText or EnvRef values")
		}
		variables = append(variables, environmentVariable{name: name, value: env[name]})
	}
	return variables, nil
}

// ID returns the handler id.
func (h HookHandler) ID() string { return h.id }

// Event returns the hook event.
func (h HookHandler) Event() HookEvent { return h.event }

// Argv returns a copy of the command arguments.
func (h HookHandler) Argv() []string { return slices.Clone(h.argv) }

// Environment returns a copy of the environment.
func (h HookHandler) Environment() map[string]EnvironmentValue {
	env := make(map[string]EnvironmentValue, len(h.environment))
	for _, variable := range h.environment {
		env[variable.name] = variable.value
	}
	return env
}

// Cwd returns the absolute working directory.
func (h HookHandler) Cwd() string { return h.cwd }

// TimeoutMillis returns the timeout in milliseconds.
func (h HookHandler) TimeoutMillis() int { return h.timeoutMillis }

// Outcomes returns a copy of the declared outcomes.
func (h HookHandler) Outcomes() []HookOutcome { return slices.Clone(h.outcomes) }

// Tools returns a copy of the selected tool names; empty means every tool.
func (h HookHandler) Tools() []string { return slices.Clone(h.tools) }

func (h HookHandler) json() hookValue {
	argv := make([]hookValue, 0, len(h.argv))
	for _, argument := range h.argv {
		argv = append(argv, hookTextValue(argument))
	}
	env := make([]hookPair, 0, len(h.environment))
	for _, variable := range h.environment {
		value := hookValue{}
		switch v := variable.value.(type) {
		case EnvText:
			value = hookTextValue(string(v))
		case EnvRef:
			value = hookObjectValue(hookMemberOf("ref", hookTextValue(v.name)))
		}
		env = append(env, hookMemberOf(variable.name, value))
	}
	outcomes := make([]hookValue, 0, len(h.outcomes))
	for _, outcome := range h.outcomes {
		outcomes = append(outcomes, hookTextValue(string(outcome)))
	}
	tools := make([]hookValue, 0, len(h.tools))
	for _, tool := range h.tools {
		tools = append(tools, hookTextValue(tool))
	}
	return hookObjectValue(
		hookMemberOf("id", hookTextValue(h.id)),
		hookMemberOf("event", hookTextValue(string(h.event))),
		hookMemberOf("argv", hookValue{kind: hookArray, items: argv}),
		hookMemberOf("cwd", hookTextValue(h.cwd)),
		hookMemberOf("env", hookObjectValue(env...)),
		hookMemberOf("timeout_ms", hookValue{kind: hookInt, integer: int64(h.timeoutMillis)}),
		hookMemberOf("outcomes", hookValue{kind: hookArray, items: outcomes}),
		hookMemberOf("tools", hookValue{kind: hookArray, items: tools}),
	)
}

// HookConfig is the handler configuration the runtime reads for one host.
type HookConfig struct {
	host     string
	handlers []HookHandler
}

// NewHookConfig returns a validated configuration with handlers ordered by id.
func NewHookConfig(host string, handlers []HookHandler) (HookConfig, error) {
	if err := validText(host, "hook host"); err != nil {
		return HookConfig{}, err
	}
	if strings.TrimSpace(host) == "" {
		return HookConfig{}, newValueError("hook host must be a nonblank string")
	}
	if slices.ContainsFunc(handlers, func(handler HookHandler) bool { return handler.id == "" }) {
		return HookConfig{}, newValueError("hook handlers must be HookHandler values")
	}
	sorted := slices.Clone(handlers)
	slices.SortStableFunc(sorted, func(left, right HookHandler) int { return strings.Compare(left.id, right.id) })
	for _, event := range hookEvents {
		count, total := 0, 0
		for _, handler := range sorted {
			if handler.event == event {
				count++
				total += handler.timeoutMillis
			}
		}
		if count > HookHandlerLimit {
			return HookConfig{}, newValueError("hook event cannot have more than 64 handlers")
		}
		if total > HookEventTimeoutLimitMillis {
			return HookConfig{}, newValueError("hook event timeout sum cannot exceed 300000 milliseconds")
		}
	}
	return HookConfig{host: host, handlers: sorted}, nil
}

// Host returns the host name.
func (c HookConfig) Host() string { return c.host }

// Handlers returns a copy of the handlers in id order.
func (c HookConfig) Handlers() []HookHandler { return slices.Clone(c.handlers) }

// Bytes returns the canonical encoding the runtime reads.
func (c HookConfig) Bytes() []byte {
	handlers := make([]hookValue, 0, len(c.handlers))
	for _, handler := range c.handlers {
		handlers = append(handlers, handler.json())
	}
	return encodeHookValue(hookObjectValue(
		hookMemberOf("host", hookTextValue(c.host)),
		hookMemberOf("handlers", hookValue{kind: hookArray, items: handlers}),
	))
}

func parseHookValue(data []byte, limit int) (hookValue, string) {
	if len(data) > limit {
		return hookValue{}, "input-limit"
	}
	if !utf8.Valid(data) {
		return hookValue{}, "invalid-json"
	}
	parser := hookParser{data: data}
	if code := parser.validateSyntax(); code != "" {
		return hookValue{}, code
	}
	parser.pos = 0
	value, code := parser.value(0)
	if code != "" {
		return hookValue{}, code
	}
	parser.whitespace()
	if parser.pos != len(data) {
		return hookValue{}, "invalid-json"
	}
	if code := validateHookValue(value, 0); code != "" {
		return hookValue{}, code
	}
	return value, ""
}

type syntaxExpectation uint8

const (
	expectValue syntaxExpectation = iota
	expectArrayFirst
	expectArrayNext
	expectObjectFirst
	expectObjectKey
	expectColon
	expectObjectNext
	expectEnd
)

var reservedHookKeys = []string{"__proto__", "constructor", "prototype"}

type hookParser struct {
	data []byte
	pos  int
}

func (p *hookParser) validateSyntax() string {
	stack := []syntaxExpectation{expectEnd, expectValue}
	var objectKeys []map[string]struct{}
	depth := 0
	semantic := ""
	note := func(code string) {
		if semantic == "" {
			semantic = code
		}
	}
	for len(stack) > 0 {
		expectation := stack[len(stack)-1]
		stack = stack[:len(stack)-1]
		p.whitespace()
		switch expectation {
		case expectValue:
			switch c := p.peek(); {
			case c == '[':
				p.pos++
				depth++
				if depth > HookJSONDepthLimit {
					note("json-depth")
				}
				stack = append(stack, expectArrayFirst)
			case c == '{':
				p.pos++
				depth++
				if depth > HookJSONDepthLimit {
					note("json-depth")
				}
				objectKeys = append(objectKeys, map[string]struct{}{})
				stack = append(stack, expectObjectFirst)
			case c == '"':
				if _, code := p.string(); code != "" {
					return code
				}
			case c == 't' || c == 'f' || c == 'n':
				if code := p.literal(); code != "" {
					return code
				}
			case c == '-' || c >= '0' && c <= '9':
				text, code := p.numberText()
				if code != "" {
					return code
				}
				if _, ok := parseFiniteNumber(text); !ok {
					note("json-number")
				}
			default:
				return "invalid-json"
			}
		case expectArrayFirst:
			if p.peek() == ']' {
				p.pos++
				depth--
			} else {
				stack = append(stack, expectArrayNext, expectValue)
			}
		case expectArrayNext:
			switch p.peek() {
			case ',':
				p.pos++
				stack = append(stack, expectArrayNext, expectValue)
			case ']':
				p.pos++
				depth--
			default:
				return "invalid-json"
			}
		case expectObjectFirst:
			if p.peek() == '}' {
				p.pos++
				depth--
				objectKeys = objectKeys[:len(objectKeys)-1]
			} else {
				stack = append(stack, expectObjectKey)
			}
		case expectObjectKey:
			key, code := p.string()
			if code != "" {
				return code
			}
			keys := objectKeys[len(objectKeys)-1]
			_, duplicate := keys[key.key()]
			keys[key.key()] = struct{}{}
			if duplicate || slices.ContainsFunc(reservedHookKeys, key.equals) {
				note("json-key")
			}
			stack = append(stack, expectColon)
		case expectColon:
			if p.peek() != ':' {
				return "invalid-json"
			}
			p.pos++
			stack = append(stack, expectObjectNext, expectValue)
		case expectObjectNext:
			switch p.peek() {
			case ',':
				p.pos++
				stack = append(stack, expectObjectKey)
			case '}':
				p.pos++
				depth--
				objectKeys = objectKeys[:len(objectKeys)-1]
			default:
				return "invalid-json"
			}
		case expectEnd:
			if p.pos != len(p.data) {
				return "invalid-json"
			}
		}
	}
	return semantic
}

func (p *hookParser) value(depth int) (hookValue, string) {
	p.whitespace()
	switch c := p.peek(); {
	case c == '[':
		return p.array(depth)
	case c == '{':
		return p.object(depth)
	case c == '"':
		text, code := p.string()
		return hookStringValue(text), code
	case c == 't' || c == 'f' || c == 'n':
		if code := p.literal(); code != "" {
			return hookValue{}, code
		}
		switch c {
		case 't':
			return hookValue{kind: hookBool, boolean: true}, ""
		case 'f':
			return hookValue{kind: hookBool}, ""
		default:
			return hookValue{}, ""
		}
	case c == '-' || c >= '0' && c <= '9':
		text, code := p.numberText()
		if code != "" {
			return hookValue{}, code
		}
		number, ok := parseFiniteNumber(text)
		if !ok {
			return hookValue{}, "json-number"
		}
		return hookValue{kind: hookFloat, number: number}, ""
	default:
		return hookValue{}, "invalid-json"
	}
}

func (p *hookParser) array(depth int) (hookValue, string) {
	if depth >= HookJSONDepthLimit {
		return hookValue{}, "json-depth"
	}
	p.pos++
	p.whitespace()
	items := []hookValue{}
	if p.peek() == ']' {
		p.pos++
		return hookValue{kind: hookArray, items: items}, ""
	}
	for {
		item, code := p.value(depth + 1)
		if code != "" {
			return hookValue{}, code
		}
		items = append(items, item)
		p.whitespace()
		switch p.peek() {
		case ',':
			p.pos++
		case ']':
			p.pos++
			return hookValue{kind: hookArray, items: items}, ""
		default:
			return hookValue{}, "invalid-json"
		}
	}
}

func (p *hookParser) object(depth int) (hookValue, string) {
	if depth >= HookJSONDepthLimit {
		return hookValue{}, "json-depth"
	}
	p.pos++
	p.whitespace()
	members := []hookPair{}
	if p.peek() == '}' {
		p.pos++
		return hookObjectValue(members...), ""
	}
	for {
		p.whitespace()
		key, code := p.string()
		if code != "" {
			return hookValue{}, code
		}
		p.whitespace()
		if p.peek() != ':' {
			return hookValue{}, "invalid-json"
		}
		p.pos++
		item, code := p.value(depth + 1)
		if code != "" {
			return hookValue{}, code
		}
		members = append(members, hookPair{key: key, value: item})
		p.whitespace()
		switch p.peek() {
		case ',':
			p.pos++
		case '}':
			p.pos++
			return hookObjectValue(members...), ""
		default:
			return hookValue{}, "invalid-json"
		}
	}
}

func (p *hookParser) string() (HookText, string) {
	if p.peek() != '"' {
		return HookText{}, "invalid-json"
	}
	p.pos++
	units := []uint16{}
	for {
		c := p.peek()
		switch {
		case c == '"':
			p.pos++
			return newHookText(units), ""
		case c == '\\':
			p.pos++
			if p.pos >= len(p.data) {
				return HookText{}, "invalid-json"
			}
			escape := p.data[p.pos]
			p.pos++
			switch escape {
			case '"', '\\', '/':
				units = append(units, uint16(escape))
			case 'b':
				units = append(units, 8)
			case 'f':
				units = append(units, 12)
			case 'n':
				units = append(units, 10)
			case 'r':
				units = append(units, 13)
			case 't':
				units = append(units, 9)
			case 'u':
				unit, ok := p.hex4()
				if !ok {
					return HookText{}, "invalid-json"
				}
				units = append(units, unit)
			default:
				return HookText{}, "invalid-json"
			}
		case c < 0x20:
			return HookText{}, "invalid-json"
		case c < 0x80:
			for p.pos < len(p.data) && p.data[p.pos] >= 0x20 && p.data[p.pos] < 0x80 && p.data[p.pos] != '"' && p.data[p.pos] != '\\' {
				units = append(units, uint16(p.data[p.pos]))
				p.pos++
			}
		default:
			r, size := utf8.DecodeRune(p.data[p.pos:])
			units = utf16.AppendRune(units, r)
			p.pos += size
		}
	}
}

func (p *hookParser) numberText() (string, string) {
	start := p.pos
	if p.peek() == '-' {
		p.pos++
	}
	switch c := p.peek(); {
	case c == '0':
		p.pos++
		if p.isDigit() {
			return "", "invalid-json"
		}
	case c >= '1' && c <= '9':
		p.digits()
	default:
		return "", "invalid-json"
	}
	if p.peek() == '.' {
		p.pos++
		if !p.isDigit() {
			return "", "invalid-json"
		}
		p.digits()
	}
	if c := p.peek(); c == 'e' || c == 'E' {
		p.pos++
		if c := p.peek(); c == '+' || c == '-' {
			p.pos++
		}
		if !p.isDigit() {
			return "", "invalid-json"
		}
		p.digits()
	}
	return string(p.data[start:p.pos]), ""
}

func (p *hookParser) isDigit() bool {
	c := p.peek()
	return c >= '0' && c <= '9'
}

func (p *hookParser) digits() {
	for p.isDigit() {
		p.pos++
	}
}

func parseFiniteNumber(text string) (float64, bool) {
	number, err := strconv.ParseFloat(text, 64)
	if err != nil && !errors.Is(err, strconv.ErrRange) {
		return 0, false
	}
	return number, !math.IsInf(number, 0) && !math.IsNaN(number)
}

func (p *hookParser) literal() string {
	for _, literal := range []string{"true", "false", "null"} {
		if strings.HasPrefix(string(p.data[p.pos:min(p.pos+len(literal), len(p.data))]), literal) {
			p.pos += len(literal)
			return ""
		}
	}
	return "invalid-json"
}

func (p *hookParser) hex4() (uint16, bool) {
	if len(p.data)-p.pos < 4 {
		return 0, false
	}
	var value uint16
	for _, digit := range p.data[p.pos : p.pos+4] {
		switch {
		case digit >= '0' && digit <= '9':
			value = value<<4 | uint16(digit-'0')
		case digit >= 'a' && digit <= 'f':
			value = value<<4 | uint16(digit-'a'+10)
		case digit >= 'A' && digit <= 'F':
			value = value<<4 | uint16(digit-'A'+10)
		default:
			return 0, false
		}
	}
	p.pos += 4
	return value, true
}

func (p *hookParser) whitespace() {
	for {
		switch p.peek() {
		case ' ', '\t', '\n', '\r':
			p.pos++
		default:
			return
		}
	}
}

func (p *hookParser) peek() int {
	if p.pos >= len(p.data) {
		return -1
	}
	return int(p.data[p.pos])
}

func validateHookValue(value hookValue, depth int) string {
	switch value.kind {
	case hookFloat:
		if math.IsInf(value.number, 0) || math.IsNaN(value.number) {
			return "json-number"
		}
	case hookArray:
		if depth >= HookJSONDepthLimit {
			return "json-depth"
		}
		for _, item := range value.items {
			if code := validateHookValue(item, depth+1); code != "" {
				return code
			}
		}
	case hookObject:
		if depth >= HookJSONDepthLimit {
			return "json-depth"
		}
		keys := make(map[string]struct{}, len(value.members))
		for _, member := range value.members {
			key := member.key.key()
			if _, duplicate := keys[key]; duplicate || slices.ContainsFunc(reservedHookKeys, member.key.equals) {
				return "json-key"
			}
			keys[key] = struct{}{}
			if code := validateHookValue(member.value, depth+1); code != "" {
				return code
			}
		}
	default:
	}
	return ""
}

func validateEncodedHookValue(value hookValue, limit int) string {
	if code := validateHookValue(value, 0); code != "" {
		return code
	}
	if len(encodeHookValue(value)) > limit {
		return "input-limit"
	}
	return ""
}

func encodeHookValue(value hookValue) []byte {
	return appendHookValue(nil, value)
}

func appendHookValue(output []byte, value hookValue) []byte {
	switch value.kind {
	case hookBool:
		return strconv.AppendBool(output, value.boolean)
	case hookInt:
		return strconv.AppendInt(output, value.integer, 10)
	case hookFloat:
		return append(output, jsNumber(value.number)...)
	case hookString:
		return appendHookString(output, value.text)
	case hookArray:
		output = append(output, '[')
		for index, item := range value.items {
			if index != 0 {
				output = append(output, ',')
			}
			output = appendHookValue(output, item)
		}
		return append(output, ']')
	case hookObject:
		output = append(output, '{')
		for index, member := range value.members {
			if index != 0 {
				output = append(output, ',')
			}
			output = appendHookString(output, member.key)
			output = append(output, ':')
			output = appendHookValue(output, member.value)
		}
		return append(output, '}')
	default:
		return append(output, "null"...)
	}
}

func appendHookString(output []byte, text HookText) []byte {
	const hex = "0123456789abcdef"
	output = append(output, '"')
	units := text.units
	for index := 0; index < len(units); index++ {
		unit := units[index]
		switch {
		case unit == '"':
			output = append(output, '\\', '"')
		case unit == '\\':
			output = append(output, '\\', '\\')
		case unit == 0x08:
			output = append(output, '\\', 'b')
		case unit == 0x0c:
			output = append(output, '\\', 'f')
		case unit == 0x0a:
			output = append(output, '\\', 'n')
		case unit == 0x0d:
			output = append(output, '\\', 'r')
		case unit == 0x09:
			output = append(output, '\\', 't')
		case unit >= 0xd800 && unit <= 0xdbff && index+1 < len(units) && units[index+1] >= 0xdc00 && units[index+1] <= 0xdfff:
			output = utf8.AppendRune(output, utf16.DecodeRune(rune(unit), rune(units[index+1])))
			index++
		case unit < 0x20 || unit >= 0xd800 && unit <= 0xdfff:
			output = append(output, '\\', 'u', hex[unit>>12], hex[unit>>8&0xf], hex[unit>>4&0xf], hex[unit&0xf])
		default:
			output = utf8.AppendRune(output, rune(unit))
		}
	}
	return append(output, '"')
}

func jsNumber(value float64) string {
	if value == 0 {
		return "0"
	}
	sign := ""
	if value < 0 {
		sign, value = "-", -value
	}
	mantissa, exponentText, _ := strings.Cut(strconv.FormatFloat(value, 'e', -1, 64), "e")
	exponent, err := strconv.Atoi(exponentText)
	if err != nil {
		return sign + mantissa
	}
	digits := strings.Replace(mantissa, ".", "", 1)
	decimalIndex := exponent + 1
	if exponent >= -6 && exponent < 21 {
		switch {
		case decimalIndex <= 0:
			return sign + "0." + strings.Repeat("0", -decimalIndex) + digits
		case decimalIndex >= len(digits):
			return sign + digits + strings.Repeat("0", decimalIndex-len(digits))
		default:
			return sign + digits[:decimalIndex] + "." + digits[decimalIndex:]
		}
	}
	tail := ""
	if len(digits) > 1 {
		tail = "." + digits[1:]
	}
	exponentSign := ""
	if exponent >= 0 {
		exponentSign = "+"
	}
	return sign + digits[:1] + tail + "e" + exponentSign + strconv.Itoa(exponent)
}
