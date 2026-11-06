package jsonx

// Kind identifies the JSON type of a Value.
type Kind uint8

// The JSON value kinds; KindBigInt is an integer outside the signed 64-bit range, kept as its source text.
const (
	KindNull Kind = iota
	KindBool
	KindInt
	KindBigInt
	KindFloat
	KindString
	KindArray
	KindObject
)

// Value is a parsed JSON value whose object members keep their source order.
type Value struct {
	kind    Kind
	boolean bool
	integer int64
	float   float64
	text    string
	items   []Value
	members []Member
}

// Member is one name/value pair of a JSON object.
type Member struct {
	Key   string
	Value Value
}

// Error is a JSON reading failure carrying Flyrail's reference message.
type Error struct {
	Message string
}

// Error returns the reference message.
func (e *Error) Error() string {
	return e.Message
}

func fail(message string) *Error {
	return &Error{Message: message}
}

// Null returns the JSON null value.
func Null() Value {
	return Value{kind: KindNull}
}

// Bool returns a JSON boolean.
func Bool(b bool) Value {
	return Value{kind: KindBool, boolean: b}
}

// Int returns a JSON integer.
func Int(i int64) Value {
	return Value{kind: KindInt, integer: i}
}

// BigInt returns a JSON integer outside the signed 64-bit range, written as text.
func BigInt(text string) Value {
	return Value{kind: KindBigInt, text: text}
}

// Float returns a JSON floating-point number.
func Float(f float64) Value {
	return Value{kind: KindFloat, float: f}
}

// String returns a JSON string.
func String(s string) Value {
	return Value{kind: KindString, text: s}
}

// Array returns a JSON array that shares items.
func Array(items ...Value) Value {
	return Value{kind: KindArray, items: items}
}

// Object returns a JSON object that shares members.
func Object(members ...Member) Value {
	return Value{kind: KindObject, members: members}
}

// Kind returns the value's JSON type.
func (v Value) Kind() Kind {
	return v.kind
}

// AsBool returns the boolean and whether the value is one.
func (v Value) AsBool() (bool, bool) {
	return v.boolean, v.kind == KindBool
}

// AsInt returns the integer and whether the value is a signed 64-bit integer.
func (v Value) AsInt() (int64, bool) {
	return v.integer, v.kind == KindInt
}

// AsFloat returns the number and whether the value is a floating-point number.
func (v Value) AsFloat() (float64, bool) {
	return v.float, v.kind == KindFloat
}

// AsString returns the string and whether the value is one.
func (v Value) AsString() (string, bool) {
	return v.text, v.kind == KindString
}

// BigIntText returns the integer text and whether the value is an out-of-range integer.
func (v Value) BigIntText() (string, bool) {
	return v.text, v.kind == KindBigInt
}

// Items returns the array items, shared with the value; callers must not modify them.
func (v Value) Items() ([]Value, bool) {
	return v.items, v.kind == KindArray
}

// Members returns the object members in source order, shared with the value; callers must not modify them.
func (v Value) Members() ([]Member, bool) {
	return v.members, v.kind == KindObject
}

// IsNull reports whether the value is JSON null.
func (v Value) IsNull() bool {
	return v.kind == KindNull
}

// Get returns the first object member named key.
func (v Value) Get(key string) (Value, bool) {
	if v.kind != KindObject {
		return Value{}, false
	}
	for _, member := range v.members {
		if member.Key == key {
			return member.Value, true
		}
	}
	return Value{}, false
}

// Equal reports whether two values are structurally equal, comparing numbers with IEEE equality.
func Equal(a, b Value) bool {
	if a.kind != b.kind {
		return false
	}
	switch a.kind {
	case KindNull:
		return true
	case KindBool:
		return a.boolean == b.boolean
	case KindInt:
		return a.integer == b.integer
	case KindFloat:
		return a.float == b.float
	case KindBigInt, KindString:
		return a.text == b.text
	case KindArray:
		if len(a.items) != len(b.items) {
			return false
		}
		for index := range a.items {
			if !Equal(a.items[index], b.items[index]) {
				return false
			}
		}
		return true
	case KindObject:
		if len(a.members) != len(b.members) {
			return false
		}
		for index := range a.members {
			if a.members[index].Key != b.members[index].Key || !Equal(a.members[index].Value, b.members[index].Value) {
				return false
			}
		}
		return true
	}
	return false
}
