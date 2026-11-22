# Examples

[Quickstart](#quickstart) · [flyrail-notes](#flyrail-notes)

Both examples import `flyrail` by its package name. Inside this checkout Node
resolves that name to the package itself through its `exports`, so build
`dist/` first (`just native-build` and `just build`). The tests run both
examples against `src/` through the vitest alias.

## Quickstart

`examples/quickstart.ts` exports `run(root)`, which takes an absolute project
directory and:

1. builds a skill bundle in memory with `Bundle.fromMemory`, then `install`s
   it into `<root>/skills`, `inspect`s it (current) and `uninstall`s it;
2. builds an instruction bundle with `Bundle.fromArtifacts`, `render`s it for
   Codex in `<root>`, `preview`s and `applyPreview`s it (creating the managed
   section in `<root>/AGENTS.md`), prints the file, and `remove`s it again.

State lives in a `DirectoryStore` at `<root>/.flyrail-state`. The
[README](../README.md#quickstart) shows the file verbatim, and
`test/quickstart.test.ts` fails when the two differ.

## flyrail-notes

`examples/flyrail-notes` is a small command-line application that ships its
companion agent skill as a bundle directory (`bundle/`) and manages it with
Flyrail. It mirrors the Go example of the same name:

```sh
node examples/flyrail-notes/cli.ts --version
node examples/flyrail-notes/cli.ts words FILE
node examples/flyrail-notes/cli.ts ai status --source DIR --target DIR [--target DIR ...] --state DIR
node examples/flyrail-notes/cli.ts ai install --source DIR --target DIR [--target DIR ...] --state DIR
node examples/flyrail-notes/cli.ts ai update --source DIR --target DIR [--target DIR ...] --state DIR [--replace-modified]
node examples/flyrail-notes/cli.ts ai uninstall --bundle-id ID --target DIR [--target DIR ...] --state DIR [--replace-modified]
```

Each `ai` command prints one JSON report with the application version, the
bundle id and version, and one entry per target (its observation, status,
error and recovery paths). Exit status 0 means success (for `status`, that
every target is current), 1 that a target differs or an operation did not
complete, and 2 a usage or source error. `uninstall` needs only the bundle id
and the state directory, so it works after the bundle directory is gone.
Relative paths are resolved against the working directory by the application
before they reach `flyrail`, which accepts only absolute paths.

Every mutation waits up to 5 s (`lockTimeoutMs: 5000`) for a lock held by
another Flyrail run. The first Ctrl-C aborts the call through an
`AbortController`: a wait that has not committed rejects with the abort reason,
the command prints a one-line message to stderr instead of a JSON report (unlike
the Go example, which reports a `busy` target error), and exits 1. A second
Ctrl-C terminates the process.

`main.ts` holds the application as `run(argv, { stdout, stderr, signal })`,
returning the exit code; `cli.ts` passes `process.argv.slice(2)`,
`process.stdout`, `process.stderr` and the `AbortController`'s signal, and sets
`process.exitCode`. Node runs the TypeScript sources directly through its
built-in type stripping. `test/flyrail-notes.test.ts` drives `run` through
install, status, uninstall and a usage error.
