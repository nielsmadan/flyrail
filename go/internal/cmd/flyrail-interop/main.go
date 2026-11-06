// Command flyrail-interop drives the Go implementation for the cross-language interop and differential checks.
package main

import (
	"context"
	"fmt"
	"io"
	"os"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

const usage = "usage: flyrail-interop differential|edits|encode|write|install|update|inspect|remove|recover|records|hold|observe [options]"

func main() {
	os.Exit(run(os.Args[1:], os.Stdin, os.Stdout, os.Stderr))
}

func run(args []string, stdin io.Reader, stdout, stderr io.Writer) int {
	if len(args) > 0 && (args[0] == "differential" || args[0] == "edits") {
		command := differential
		if args[0] == "edits" {
			command = editsCommand
		}
		output, err := command(args[1:])
		if err == nil {
			_, err = stdout.Write(output)
		}
		if err != nil {
			_, _ = fmt.Fprintln(stderr, err)
			return 1
		}
		return 0
	}
	if len(args) == 0 || operations[args[0]] == nil {
		_, _ = stdout.Write(errorRecord(usage))
		return 1
	}
	err := runOperation(args[0], args[1:], stdin, stdout)
	if err == nil {
		return 0
	}
	if args[0] == "encode" {
		_, _ = fmt.Fprintln(stderr, failureMessage(err))
	} else {
		_, _ = stdout.Write(errorRecord(failureMessage(err)))
	}
	return 1
}

func runOperation(command string, args []string, stdin io.Reader, stdout io.Writer) error {
	parsed, err := parseOptions(command, args)
	if err != nil {
		return err
	}
	return operations[command](context.Background(), parsed, stdin, stdout)
}

func errorRecord(message string) []byte {
	record := jsonx.Object(jsonx.Member{Key: "error", Value: jsonx.Object(
		jsonx.Member{Key: "code", Value: jsonx.Null()},
		jsonx.Member{Key: "message", Value: jsonx.String(message)},
		jsonx.Member{Key: "path", Value: jsonx.Null()},
	)})
	return append(jsonx.Encode(record), '\n')
}
