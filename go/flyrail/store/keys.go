package store

import "strings"

const maxKeyLength = 200

// ValidateKey returns a *KeyError unless key is at most 200 bytes of slash-separated segments of [a-z0-9._-] starting with [a-z0-9].
func ValidateKey(key string) error {
	return validate(key, "state key")
}

// ValidateLockName returns a *KeyError unless name follows the state key grammar.
func ValidateLockName(name string) error {
	return validate(name, "lock name")
}

func validate(value, kind string) error {
	if len(value) > maxKeyLength {
		return &KeyError{Kind: kind, Value: value}
	}
	for segment := range strings.SplitSeq(value, "/") {
		if !validSegment(segment) {
			return &KeyError{Kind: kind, Value: value}
		}
	}
	return nil
}

func validSegment(segment string) bool {
	if segment == "" || !lowerOrDigit(segment[0]) {
		return false
	}
	for index := range len(segment) {
		if character := segment[index]; !lowerOrDigit(character) && character != '.' && character != '_' && character != '-' {
			return false
		}
	}
	return true
}

func lowerOrDigit(character byte) bool {
	return 'a' <= character && character <= 'z' || '0' <= character && character <= '9'
}
