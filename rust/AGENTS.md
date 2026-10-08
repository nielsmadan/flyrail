# Flyrail for Rust

Flyrail is a synchronous Rust library for reversible skills, instructions, MCP
registrations and hooks at explicit local destinations. Keep dependencies small
and public values immutable. Library calls must never print, prompt, exit,
execute bundled commands, make network requests, or select installed agents.

Read README.md and the relevant docs before changing an interface. Keep public
API, lifecycle, translation and example docs consistent with behavioral
changes. Public prose belongs in documentation. Prefer clear names over
comments and doc comments.

Use `just check` for formatting, Clippy and the unit test tier; it is the
git-hook gate and must stay fast. Use `just check-integration` for the full
suite with fixtures, interop pairs and the hook runtime. `just format` formats
Rust code, `just lint` runs Clippy, and `just test` runs the whole suite.
Supported platforms are macOS and Linux; Windows is unsupported.

Keep caches, temporary files, downloaded tools, and test installations inside
this checkout; cargo runs with CARGO_HOME at the repository-root
`.cache/cargo-home`. Tests must use temporary roots and explicit environment
overrides, never the real user's agent configuration. Do not change HOME,
activate hooks, or install global tools. The repository-root lefthook.yml is
configuration only.

Validate public input behavior and observable filesystem outcomes. Do not add
tests that just repeat enum members or implementation details. Run the
complete shared check and read its exit status and full test summary before
reporting.

Shared conformance fixtures live in `../spec/fixtures`; when the fixture
directory is absent, conformance tests skip with a notice. Keep fixture bytes
portable.

Payload and metadata writes use exclusive create with
`OpenOptions::create_new`; do not use `File::create`, because it truncates
recovery evidence. Library code never panics on public paths; restriction
lints deny panic-prone operations.

Hook runtime checks require Node, npm and Bun; without them those tests skip
with a notice.
