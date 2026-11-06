package main

import (
	"errors"
	"fmt"
	"math"
	"os"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

type optionSet struct {
	values []string
	flags  []string
}

var writeOptions = optionSet{
	values: []string{
		"--state", "--bundle-id", "--lock-timeout", "--destination", "--source", "--agent", "--node",
		"--version", "--payload", "--large", "--section", "--mode", "--kind", "--acquisition",
		"--barrier-phase", "--barrier-ready", "--barrier-release", "--barrier-timeout",
	},
	flags: []string{"--replace-modified", "--fail-index-completion"},
}

var commandOptions = map[string]optionSet{
	"encode":  {values: []string{"--type"}},
	"write":   writeOptions,
	"install": writeOptions,
	"update":  writeOptions,
	"inspect": {values: []string{"--state", "--bundle-id", "--lock-timeout"}},
	"recover": {values: []string{"--state", "--bundle-id", "--lock-timeout"}},
	"remove":  {values: []string{"--state", "--bundle-id", "--lock-timeout"}, flags: []string{"--replace-modified"}},
	"records": {values: []string{"--state"}},
	"hold": {values: []string{
		"--state", "--destination", "--barrier", "--kind", "--lock-timeout", "--wait-timeout",
	}},
	"observe": {values: []string{"--destination", "--kind", "--repeat"}},
}

var errUsageText = errors.New(usage)

type options struct {
	values map[string]string
	flags  map[string]bool
}

func parseOptions(command string, args []string) (options, error) {
	allowed, ok := commandOptions[command]
	if !ok {
		return options{}, errUsageText
	}
	parsed := options{values: map[string]string{}, flags: map[string]bool{}}
	for index := 0; index < len(args); index++ {
		argument := args[index]
		switch {
		case slices.Contains(allowed.flags, argument):
			if parsed.flags[argument] {
				return options{}, fmt.Errorf("duplicate option: %s", argument)
			}
			parsed.flags[argument] = true
		case slices.Contains(allowed.values, argument):
			if index+1 >= len(args) || strings.HasPrefix(args[index+1], "--") {
				return options{}, fmt.Errorf("missing value for %s", argument)
			}
			if _, seen := parsed.values[argument]; seen {
				return options{}, fmt.Errorf("duplicate option: %s", argument)
			}
			index++
			parsed.values[argument] = args[index]
		case strings.HasPrefix(argument, "--"):
			return options{}, fmt.Errorf("unknown option: %s", argument)
		default:
			return options{}, errUsageText
		}
	}
	return parsed, nil
}

func (o options) has(name string) bool {
	_, ok := o.values[name]
	return ok
}

func (o options) value(name, fallback string) string {
	if value, ok := o.values[name]; ok {
		return value
	}
	return fallback
}

func (o options) required(name string) (string, error) {
	value, ok := o.values[name]
	if !ok {
		return "", fmt.Errorf("missing %s", name)
	}
	return value, nil
}

// Joins without collapsing components, as Rust's driver does.
func absolute(path string) (string, error) {
	if strings.HasPrefix(path, "/") {
		return path, nil
	}
	current, err := os.Getwd()
	if err != nil {
		return "", err
	}
	return pathx.Join(current, path), nil
}

func (o options) absolute(name string) (string, error) {
	value, err := o.required(name)
	if err != nil {
		return "", err
	}
	return absolute(value)
}

func duration(name, text string) (time.Duration, error) {
	seconds, err := strconv.ParseFloat(text, 64)
	switch {
	case err != nil:
		return 0, fmt.Errorf("invalid %s: %s", name, strings.TrimPrefix(err.Error(), "strconv.ParseFloat: "))
	case math.IsNaN(seconds) || math.IsInf(seconds, 0) || seconds < 0:
		return 0, fmt.Errorf("invalid %s: must be finite and nonnegative", name)
	case seconds >= float64(math.MaxInt64)/float64(time.Second):
		return 0, fmt.Errorf("invalid %s: duration is too large", name)
	}
	return time.Duration(seconds * float64(time.Second)), nil
}

func (o options) duration(name, fallback string) (time.Duration, error) {
	return duration(name, o.value(name, fallback))
}
