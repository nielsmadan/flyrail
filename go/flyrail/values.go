package flyrail

import (
	"fmt"
	"iter"
	"math"
	"slices"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

const maxValueDepth = 10_000

// Value is an immutable semantic configuration value: a Scalar, an Array or an Object.
type Value interface {
	isValue()
}

// Scalar is a semantic value that is not a container.
type Scalar interface {
	Value
	isScalar()
}

// Null is the null scalar.
type Null struct{}

// Bool is a boolean scalar.
type Bool bool

// Int is a signed 64-bit integer scalar.
type Int int64

// Float is a finite binary64 scalar; the zero value is positive zero.
type Float struct {
	bits uint64
}

// String is a scalar of valid UTF-8 text; the zero value is the empty string.
type String struct {
	text string
}

// Array is an immutable ordered sequence of values; the zero value is empty.
type Array struct {
	items []Value
	depth int
}

// Object is an immutable set of uniquely keyed values ordered by key bytes; the zero value is empty.
type Object struct {
	members []ObjectMember
	depth   int
}

// ObjectMember is one key and value of an Object.
type ObjectMember struct {
	Key   string
	Value Value
}

func (Null) isValue()     {}
func (Bool) isValue()     {}
func (Int) isValue()      {}
func (Float) isValue()    {}
func (String) isValue()   {}
func (Date) isValue()     {}
func (Time) isValue()     {}
func (DateTime) isValue() {}
func (Array) isValue()    {}
func (Object) isValue()   {}

func (Null) isScalar()     {}
func (Bool) isScalar()     {}
func (Int) isScalar()      {}
func (Float) isScalar()    {}
func (String) isScalar()   {}
func (Date) isScalar()     {}
func (Time) isScalar()     {}
func (DateTime) isScalar() {}

// NewFloat returns the float scalar f, which must be finite.
func NewFloat(f float64) (Float, error) {
	if math.IsNaN(f) || math.IsInf(f, 0) {
		return Float{}, newValueError("floats must be finite")
	}
	return Float{bits: math.Float64bits(f)}, nil
}

// Float64 returns the number.
func (f Float) Float64() float64 {
	return math.Float64frombits(f.bits)
}

// NewString returns the string scalar s, which must be valid UTF-8.
func NewString(s string) (String, error) {
	if !utf8.ValidString(s) {
		return String{}, newValueError("scalar must contain Unicode scalar values")
	}
	return String{text: s}, nil
}

// String returns the text.
func (s String) String() string {
	return s.text
}

// NewArray returns an array of items in order.
func NewArray(items ...Value) (Array, error) {
	depth := 0
	for _, item := range items {
		itemDepth, err := checkValue(item)
		if err != nil {
			return Array{}, err
		}
		depth = max(depth, itemDepth)
	}
	if depth >= maxValueDepth {
		return Array{}, newNestingError("value nesting is too deep")
	}
	return Array{items: slices.Clone(items), depth: depth + 1}, nil
}

// Items returns a copy of the items.
func (a Array) Items() []Value {
	return slices.Clone(a.items)
}

// Len returns the number of items.
func (a Array) Len() int {
	return len(a.items)
}

// NewObject returns an object of members sorted by key bytes; keys must be unique valid UTF-8.
func NewObject(members ...ObjectMember) (Object, error) {
	depth := 0
	for _, member := range members {
		if !utf8.ValidString(member.Key) {
			return Object{}, newValueError("object key must contain Unicode scalar values")
		}
		memberDepth, err := checkValue(member.Value)
		if err != nil {
			return Object{}, err
		}
		depth = max(depth, memberDepth)
	}
	if depth >= maxValueDepth {
		return Object{}, newNestingError("value nesting is too deep")
	}
	sorted := slices.Clone(members)
	slices.SortStableFunc(sorted, func(a, b ObjectMember) int { return strings.Compare(a.Key, b.Key) })
	for index := 1; index < len(sorted); index++ {
		if sorted[index-1].Key == sorted[index].Key {
			return Object{}, newValueError("duplicate object key")
		}
	}
	return Object{members: sorted, depth: depth + 1}, nil
}

// All yields the members in key byte order.
func (o Object) All() iter.Seq2[string, Value] {
	return func(yield func(string, Value) bool) {
		for _, member := range o.members {
			if !yield(member.Key, member.Value) {
				return
			}
		}
	}
}

// Get returns the value stored under key.
func (o Object) Get(key string) (Value, bool) {
	index := sort.Search(len(o.members), func(i int) bool { return o.members[i].Key >= key })
	if index < len(o.members) && o.members[index].Key == key {
		return o.members[index].Value, true
	}
	return nil, false
}

// Len returns the number of members.
func (o Object) Len() int {
	return len(o.members)
}

func checkValue(v Value) (int, error) {
	switch value := v.(type) {
	case Null, Bool, Int, Float, String, Time:
		return 0, nil
	case Date:
		return 0, value.validate()
	case DateTime:
		return 0, value.validate()
	case Array:
		return max(value.depth, 1), nil
	case Object:
		return max(value.depth, 1), nil
	}
	return 0, newTypeError("values must be Scalar, ObjectValue or ArrayValue")
}

// ValuesEqual reports whether a and b are the same semantic value; floats compare by bits and member order is insignificant.
func ValuesEqual(a, b Value) bool {
	switch left := a.(type) {
	case nil:
		return b == nil
	case Array:
		right, ok := b.(Array)
		return ok && slices.EqualFunc(left.items, right.items, ValuesEqual)
	case Object:
		right, ok := b.(Object)
		return ok && slices.EqualFunc(left.members, right.members, func(x, y ObjectMember) bool {
			return x.Key == y.Key && ValuesEqual(x.Value, y.Value)
		})
	case Null, Bool, Int, Float, String, Date, Time, DateTime:
		return a == b
	}
	return false
}

// EncodeValue returns the canonical semantic bytes of v.
func EncodeValue(v Value) ([]byte, error) {
	if _, err := checkValue(v); err != nil {
		return nil, err
	}
	return jsonx.Encode(semanticRecord(v)), nil
}

// DecodeValue parses canonical semantic bytes.
func DecodeValue(b []byte) (Value, error) {
	if !utf8.Valid(b) {
		return nil, newValueError("semantic values must be UTF-8")
	}
	record, err := jsonx.Parse(b)
	if err != nil {
		return nil, valueErrorFromJSON(err)
	}
	return valueFromRecord(record)
}

func tagged(tag string, payload jsonx.Value) jsonx.Value {
	return jsonx.Array(jsonx.String(tag), payload)
}

func semanticRecord(v Value) jsonx.Value {
	switch value := v.(type) {
	case Object:
		members := make([]jsonx.Value, 0, len(value.members))
		for _, member := range value.members {
			members = append(members, jsonx.Array(jsonx.String(member.Key), semanticRecord(member.Value)))
		}
		return tagged("object", jsonx.Array(members...))
	case Array:
		items := make([]jsonx.Value, 0, len(value.items))
		for _, item := range value.items {
			items = append(items, semanticRecord(item))
		}
		return tagged("array", jsonx.Array(items...))
	case Bool:
		return tagged("boolean", jsonx.Bool(bool(value)))
	case Int:
		return tagged("integer", jsonx.String(strconv.FormatInt(int64(value), 10)))
	case Float:
		return tagged("float", jsonx.String(floatHex(value.Float64())))
	case String:
		return tagged("string", jsonx.String(value.text))
	case Date:
		return tagged("date", jsonx.String(value.String()))
	case Time:
		return tagged("time", jsonx.String(value.String()))
	case DateTime:
		return tagged("datetime", jsonx.String(value.String()))
	}
	return jsonx.Array(jsonx.String("null"))
}

func valueFromJSON(v jsonx.Value) (Value, error) {
	switch v.Kind() {
	case jsonx.KindNull:
		return Null{}, nil
	case jsonx.KindBool:
		b, _ := v.AsBool()
		return Bool(b), nil
	case jsonx.KindInt:
		i, _ := v.AsInt()
		return Int(i), nil
	case jsonx.KindBigInt:
		return nil, newValueError("integers must fit signed 64 bits")
	case jsonx.KindFloat:
		f, _ := v.AsFloat()
		return NewFloat(f)
	case jsonx.KindString:
		s, _ := v.AsString()
		return String{text: s}, nil
	case jsonx.KindArray:
		raw, _ := v.Items()
		items := make([]Value, 0, len(raw))
		for _, item := range raw {
			value, err := valueFromJSON(item)
			if err != nil {
				return nil, err
			}
			items = append(items, value)
		}
		return NewArray(items...)
	}
	raw, _ := v.Members()
	members := make([]ObjectMember, 0, len(raw))
	for _, member := range raw {
		value, err := valueFromJSON(member.Value)
		if err != nil {
			return nil, err
		}
		members = append(members, ObjectMember{Key: member.Key, Value: value})
	}
	return NewObject(members...)
}

func valueFromRecord(record jsonx.Value) (Value, error) {
	entries, _ := record.Items()
	if len(entries) == 0 {
		return nil, newValueError("semantic values must be tagged arrays")
	}
	kind, ok := entries[0].AsString()
	if !ok {
		return nil, newValueError("semantic values must be tagged arrays")
	}
	if kind == "null" && len(entries) == 1 {
		return Null{}, nil
	}
	if len(entries) != 2 {
		return nil, newValueError("semantic value has an invalid field count")
	}
	raw := entries[1]
	if members, ok := raw.Items(); ok && kind == "array" {
		items := make([]Value, 0, len(members))
		for _, member := range members {
			item, err := valueFromRecord(member)
			if err != nil {
				return nil, err
			}
			items = append(items, item)
		}
		return NewArray(items...)
	}
	if members, ok := raw.Items(); ok && kind == "object" {
		return objectFromRecord(members)
	}
	if b, ok := raw.AsBool(); ok && kind == "boolean" {
		return Bool(b), nil
	}
	payload, ok := raw.AsString()
	if !ok {
		return nil, newValueError("semantic scalar payload has the wrong type")
	}
	value, err := scalarFromPayload(kind, payload)
	if err != nil {
		return nil, err
	}
	if !jsonx.Equal(semanticRecord(value), record) {
		return nil, newValueError("semantic scalar encoding is not canonical")
	}
	return value, nil
}

func objectFromRecord(members []jsonx.Value) (Value, error) {
	const shape = "semantic object members must be name/value pairs"
	pairs := make([]ObjectMember, 0, len(members))
	for _, member := range members {
		pair, _ := member.Items()
		if len(pair) != 2 {
			return nil, newValueError(shape)
		}
		name, ok := pair[0].AsString()
		if !ok {
			return nil, newValueError(shape)
		}
		item, err := valueFromRecord(pair[1])
		if err != nil {
			return nil, err
		}
		pairs = append(pairs, ObjectMember{Key: name, Value: item})
	}
	return NewObject(pairs...)
}

func scalarFromPayload(kind, payload string) (Value, error) {
	switch kind {
	case "string":
		return String{text: payload}, nil
	case "integer":
		i, err := semanticInteger(payload)
		return Int(i), err
	case "float":
		f, err := floatFromHex(payload)
		if err != nil {
			return nil, err
		}
		return NewFloat(f)
	case "date":
		return ParseDate(payload)
	case "time":
		return ParseTime(payload)
	case "datetime":
		return ParseDateTime(payload)
	}
	return nil, newValueError("unknown semantic scalar kind")
}

func semanticInteger(text string) (int64, error) {
	body := strings.TrimPrefix(text, "-")
	if body == "" || strings.IndexFunc(body, func(r rune) bool { return r < '0' || r > '9' }) >= 0 {
		return 0, newValueError("invalid literal for int() with base 10: " + jsonx.PythonStrRepr(text))
	}
	value, err := strconv.ParseInt(text, 10, 64)
	if err != nil {
		return 0, newValueError("integers must fit signed 64 bits")
	}
	return value, nil
}

func floatHex(value float64) string {
	if value == 0 {
		if math.Signbit(value) {
			return "-0x0.0p+0"
		}
		return "0x0.0p+0"
	}
	bits := math.Float64bits(value)
	sign := ""
	if bits>>63 == 1 {
		sign = "-"
	}
	biased := int((bits >> 52) & 0x7FF)
	fraction := bits & 0x000F_FFFF_FFFF_FFFF
	lead, exponent := 1, biased-1023
	if biased == 0 {
		lead, exponent = 0, -1022
	}
	return fmt.Sprintf("%s0x%x.%013xp%+d", sign, lead, fraction, exponent)
}

func floatFromHex(text string) (float64, error) {
	malformed := newValueError("float hexadecimal literal is malformed")
	negative := strings.HasPrefix(text, "-")
	body := strings.TrimPrefix(text, "-")
	if !negative {
		body = strings.TrimPrefix(body, "+")
	}
	if rest, ok := strings.CutPrefix(body, "0x"); ok {
		body = rest
	} else if rest, ok := strings.CutPrefix(body, "0X"); ok {
		body = rest
	} else {
		return 0, malformed
	}
	digitText, exponentText, hasExponent := body, "", false
	if position := strings.IndexAny(body, "pP"); position >= 0 {
		digitText, exponentText, hasExponent = body[:position], body[position+1:], true
	}
	whole, fraction, _ := strings.Cut(digitText, ".")
	if whole == "" && fraction == "" {
		return 0, malformed
	}
	if len(whole)+len(fraction) > 16 {
		return 0, newValueError("float hexadecimal literal has too many digits")
	}
	var mantissa uint64
	for _, digit := range []byte(whole + fraction) {
		nibble, ok := hexNibble(digit)
		if !ok {
			return 0, malformed
		}
		mantissa = mantissa*16 + uint64(nibble)
	}
	exponent := int64(0)
	if hasExponent {
		sign := int64(1)
		if strings.HasPrefix(exponentText, "-") {
			sign = -1
		}
		magnitude := exponentText
		if len(magnitude) > 0 && (magnitude[0] == '-' || magnitude[0] == '+') {
			magnitude = magnitude[1:]
		}
		if magnitude == "" || strings.IndexFunc(magnitude, func(r rune) bool { return r < '0' || r > '9' }) >= 0 {
			return 0, malformed
		}
		parsed, err := strconv.ParseInt(magnitude, 10, 32)
		if err != nil {
			return 0, newValueError("float exponent is out of range")
		}
		exponent = sign * parsed
	}
	value := scaleByPowerOfTwo(float64(mantissa), exponent-4*int64(len(fraction)))
	if math.IsInf(value, 0) || math.IsNaN(value) {
		return 0, newValueError("float hexadecimal literal is out of range")
	}
	if negative {
		return -value, nil
	}
	return value, nil
}

func hexNibble(digit byte) (byte, bool) {
	switch {
	case digit >= '0' && digit <= '9':
		return digit - '0', true
	case digit >= 'a' && digit <= 'f':
		return digit - 'a' + 10, true
	case digit >= 'A' && digit <= 'F':
		return digit - 'A' + 10, true
	}
	return 0, false
}

func scaleByPowerOfTwo(value float64, exponent int64) float64 {
	up := math.Float64frombits(0x7FE0_0000_0000_0000)
	down := math.Float64frombits(0x0360_0000_0000_0000)
	exponent = min(max(exponent, -4_096), 4_096)
	for exponent > 1_023 {
		value *= up
		exponent -= 1_023
		if math.IsInf(value, 0) {
			return value
		}
	}
	for exponent < -1_022 {
		value *= down
		exponent += 969
		if value == 0 {
			return value
		}
	}
	return value * math.Float64frombits(uint64(1_023+exponent)<<52)
}
