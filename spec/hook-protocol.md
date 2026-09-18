# Portable command hooks, version 1

[Wire records](#wire-records) · [Ordering, bounds, and failures](#ordering-bounds-and-failures) · [Concrete support](#concrete-support) · [Runtime prerequisites and ownership](#runtime-prerequisites-and-ownership)

A hook command is application-owned behavior executed later by a host adapter.
Preparing a runtime snapshot and installing a bundle do not execute commands,
activate an agent, grant trust, change approval rules, or install prerequisites.
Portable translation uses the concrete host schemas below. Native authored hooks
remain available for other semantics.

## Wire records

One invocation receives one UTF-8 JSON object on stdin:

```json
{"version":1,"event":"before-tool","tool":"Bash","input":{"command":"pwd"},"context":{}}
```

The closed top-level fields are `version`, `event`, `context`, and event-specific
`tool`, `input`, `result`. `version` is integer 1. Events are:

- `session-start`: the host starts or resumes a session; no tool/input/result.
- `before-tool`: a pending tool call; exact native `tool` name and JSON `input`.
- `after-tool`: a successful completed tool call, with `tool`, `input`, and JSON
  `result`. It cannot reverse the tool's effects. Pi error results are skipped;
  native failure-only events are outside this mapping.
- `prompt`: submitted text before the mapped host stage; string `input`.
  Pi's event precedes prompt expansion, and records its available source.
- `stop`: the host reaches completion of an assistant run, without automatic
  retry/compaction work pending. This observes completion; it does not request
  shutdown or initiate another user turn. Pi uses `agent_settled`, not
  `agent_end`, `turn_end`, or `session_shutdown`. Cursor completion requires
  `status:completed`; Copilot requires `stopReason:end_turn`.

`context` contains only whitelisted fields actually available in that callback.
Native fields are `cwd`, `session_id`, `transcript_path`, `permission_mode`,
`model`, `stop_hook_active`, `conversation_id`, `generation_id`, `timestamp`,
`stop_reason`, and `tool_call_id`, where available. Null/missing optional fields
are omitted. Pi supplies `cwd`, `execution_mode`, `has_ui`, optional `source` and
`tool_call_id`; execution mode is never a permission mode. OpenCode supplies its
plugin directory as `cwd` and available `session_id`/`tool_call_id`. Adapters never
read transcripts or synthesize session IDs.

Cursor `tool_output` is parsed as JSON, rather than treated as terminal text.
Copilot `toolResult` retains its native `resultType` and `textResultForLlm` object.
Pi results contain available `content`, `details`, `is_error`, `usage` fields.
OpenCode results contain `title`, `output`, and `metadata`; before-tool arguments
come from the mutable **output.args**, not from callback input.

The command must exit zero and emit exactly one JSON object on stdout. Empty
stdout is a protocol error. Output has exactly these fields:

| Outcome | Complete record |
| --- | --- |
| continue | `{"version":1,"outcome":"continue"}` |
| block | `{"version":1,"outcome":"block","reason":"explanation"}` |
| ask | `{"version":1,"outcome":"ask","reason":"question"}` |
| modify-input | `{"version":1,"outcome":"modify-input","input":{"command":"pwd"}}` |
| context | `{"version":1,"outcome":"context","text":"extra context"}` |

`block` rejects a pending tool or prompt, not a completed result or an assistant
run. `ask` requests the host's ordinary user confirmation; it does not supply a
permission policy. `modify-input` replaces the complete argument object while
preserving normal host approval flow. For Pi prompt events it replaces the
complete string. Array/null/scalar tool replacements are rejected. The protocol
validates object shape; it cannot validate an arbitrary third-party tool schema.
Pi does not revalidate its tool schema after replacement, so hook authors must
supply complete valid tool arguments.

Reasons/context are nonblank strings of at most 8192 UTF-8 bytes. Unknown keys,
undeclared outcomes, duplicate object keys, non-finite numbers, malformed JSON,
invalid UTF-8, nesting beyond 64 containers and prototype-mutating property names
(`__proto__`, `constructor`, `prototype`) are rejected. Applications must not use
those reserved keys in portable event data or replacements.

## Ordering, bounds, and failures

Handlers run sequentially in ascending artifact-ID order within one bundle and
event. Input order in a manifest does not override this rule. An empty tool list
matches every tool. Otherwise matching uses exact case-sensitive native names;
`Bash` does not match `Bashful`, and adapters do not translate tool aliases.
Native groups receive all tools and perform matching inside the bridge.

A replacement is visible to subsequent handlers. Context strings concatenate in
order with one newline, with an 8192-byte combined cap. The first block or ask
short-circuits later handlers. Block suppresses pending replacement; ask retains
replacement where supported (Claude/Copilot). Effects are applied only after the
whole sequence validates. Any runtime failure discards accumulated host effects.
Side effects already performed by an application command cannot be rolled back.
Ordering with other bundles, foreign hooks, and concurrent host tool calls is
outside this guarantee. Codex's concurrent native hooks use one bridge per event
so its concurrency does not reorder handlers inside a bundle.

Each event permits at most 64 handlers and a sum of declared child timeouts of
300000 ms. The runner also enforces an event deadline, clamping each child to the
remaining wall-clock budget. Native entries have a fixed 310-second host timeout,
including a 3-second stdin deadline and startup/cleanup headroom. No handler is
silently dropped to fit the budget. The hosts document numeric command timeouts;
these declarations use that field without changing their default failure policy.

Stdin is bounded at 1048576 bytes; each command's stdout is bounded at 65536 bytes
and stderr at 16384 bytes. Capture stops immediately on overflow. Execution uses
literal argv with no child shell. Ordinary cwd is the explicit render execution
root plus the authored relative path. `AssetRef` binds a snapshotted installed
asset file or directory and a declared dependency. Environment inherits the host;
literal overrides remain literal, and only explicit `EnvRef` names are resolved
at execution. Missing references fail the invocation. Windows rejects authored
case-insensitive duplicate environment keys and replaces ambient keys by that
same case-insensitive identity. Every accepted environment name, including
`__proto__`, `constructor` and `toString`, retains its literal dictionary identity.
References require an actual own string entry in the host environment; inherited
JavaScript properties do not resolve as environment variables.

Failures are distinct from decisions: missing executable/cwd/environment, nonzero
exit, signal, timeout, malformed/oversized output and invalid outcomes produce a
fixed `flyrail-hook:<code>` diagnostic without input, argv, environment values or
child stderr. Native bridges return `{}` with exit zero; Pi callbacks return no
decision; OpenCode callbacks return without throwing. A deliberate OpenCode block
throws the validated reason; a deliberate Pi block returns its blocking result.
The native bootstrap catches module-load failure too. Missing Node or a host-killed
bootstrap remains a host failure: **Copilot CLI denies command crashes/nonzero
exits but fails open on timeouts**. These hooks are not an enforcement guarantee.

On POSIX, children have their own process group; the bridge kills remaining group
members on completion/failure and destroys pipe readers. Descendants that create
another session/group are outside that guarantee. On Windows only direct-child
termination is guaranteed; inherited pipes are destroyed at the timeout so they
cannot hold the bridge open. No process-tree service or shell is installed.

## Concrete support

`C` = continue, `B` = block, `A` = ask, `M` = modify-input, `X` = context.
Every requested outcome must be supported; otherwise the logical target has no
publishable resources and lifecycle preflight performs no writes.

| Host | Session start | Before tool | After tool | Prompt | Stop |
| --- | --- | --- | --- | --- | --- |
| Claude Code CLI | C X | C B A M X | C X | C B X | C |
| Codex CLI | C X | C B X | C X | C B X | C |
| Cursor CLI/IDE | C X | C B | C X | C B | C |
| Copilot CLI | C X | C B A M | C X | C | C |
| Pi | C | C B M | C X | C M | C |
| OpenCode | C | C B M | C X | unsupported | unsupported |

Claude uses `hookSpecificOutput.updatedInput` without an `allow` decision;
its `ask` may include that replacement. Codex documents updated input only with
explicit allow, so portable modification is unsupported. Cursor has no verified
replacement channel that preserves ordinary approval flow. Codex ask is parsed
but unsupported; Cursor ask is schema-accepted but ignored. Neither is emitted.
Cursor session start is fire-and-forget and cannot block. Its stop follow-up
message starts another turn and is not a generic context/denial result. Copilot
config-file prompt output is discarded. OpenCode permission callbacks lack tool
arguments, and text-part completion is not established as assistant-run completion.
Pi context after a tool appends a text content block; OpenCode appends context to
its actual mutable tool-output string. No generic result rewriting is inferred.

Copilot VS Code/cloud, native Cursor Windows shell execution, unverified OpenCode
prompt/stop channels, and all unlisted event/outcome pairs require authored native
content. Source/render checks do not establish that any installed host is active.
Ask behavior in unattended mode belongs to the host; Flyrail does not create a UI,
supply a default answer, or interpret Pi RPC's `hasUI:true` as TUI mode.

## Runtime prerequisites and ownership

Python `HookRuntime(...)` snapshots three packaged authored modules once:
`runtime/runner.mts`, `runtime/bridge.mts` and `runtime/process.mts`. Rendering
uses those immutable bytes and performs no file reads. An optional absolute Node
executable is required for native hosts; `HookRuntime()` is sufficient to prepare
OpenCode/Pi assets. Node 24+ supplies TypeScript stripping; OpenCode uses Bun
1.4.2+ and Pi 0.85.1 uses its Jiti loader. These are caller/host prerequisites,
never discovered, downloaded, or activated by the library. Python-only wheel,
source distribution and Git builds require neither Node nor npm.

Claude native handlers use direct `command` + `args`; Copilot CLI uses direct
`exec` + `args`. Windows requires a real executable, not a `.cmd`/`.bat` shim.
Claude launcher paths containing `${...}` are rejected because the host expands
placeholders. Codex and Cursor POSIX require an explicit `HookShell.POSIX`
assertion that the selected host hook shell accepts POSIX quoting. Codex Windows
supports `HookShell.CMD` for a selected `cmd.exe /C` shell and quoted paths with
spaces/non-ASCII; paths containing `% ! & | < > ^ ( ) "` or line breaks are refused.
The library does not alter the host shell. Cursor's Windows hook shell is not
established by its published schema, so that native bridge is unsupported.
TypeScript runtime destinations under a `node_modules` path component are refused
because Node refuses stripping there. `.mts` runtime files always use ESM,
independent of the surrounding project's `package.json` type.

There is one stable discovered `.ts` entry per bundle/host in OpenCode `plugins`
or Pi `extensions`. Runtime revisions live outside discovery, under
`<asset_root>/<bundle>/.hooks/<agent>/revisions/<digest>`. Native shared arrays
use immutable real event groups around a stable `.mjs` launcher; Cursor's member
identity is its documented command field. Copilot owns a dedicated hooks JSON
file. No invented host ID or semantically empty matcher identifies ownership.

Explicit `stable-reference` dependencies allow a whole-file routing entry to
update in place. Its own dependencies remain revision-bound. All ordinary edges
retain the previous revision guarantee. The distinction is persisted in receipts
and render digests. This promises stable resolvability, not atomic activation of
several resources: new runtime assets publish before their entry, references
retire before assets, and failed reference publication retains old assets.
Native outer timeout/launcher argv remain fixed across handler updates. First
acquisition baselines survive upgrades. Removal uses receipts without bundle
source or a Python installation in the host interpreter.

Cursor's top-level `version:1` is a `DocumentSchema` requirement, not an app-owned
field. Portable Cursor hooks use ordinary JSON, as shown in its documented
configuration examples. Comment and trailing-comma support is unverified; this
does not establish that Cursor rejects JSONC. Flyrail leaves input that does not
parse as ordinary JSON untouched and reports a parse/conflict result. Valid JSON
preserves foreign values and order under the shared editor's documented value
normalization policy. Explicit JSONC editors, schema requirements and native
authored payloads remain available.

Matching pre-existing schema values are preserved. An absent field is seeded in
the same document transaction, with resource-level creation evidence; many apps
can require it. Conflicting values and app-claim overlap block desired writes.
Status reports a mismatch without repair. Source-free removal can remove intact
hooks while preserving an edited schema. A created field is pruned only if no
remaining app/foreign content needs the document. Retained foreign contents
relinquish cleanup evidence; later installations treat that field as pre-existing.
Once no remaining bundle requires a seeded field, a desired app claim acquires
that field as unowned existing content. Default acquisition conflicts even when
the value matches; exact adoption acquires it without a baseline, while takeover
records its current value as the first baseline. Successful handover relinquishes
the scaffold's cleanup rights. Document creation and empty-source provenance
survive while app claims remain so later source-free removal can safely restore
an empty source or remove a created document. Another bundle's active requirement
always blocks an overlapping app claim.

Primary schema references, inspected 2026-09-12:
[Claude hooks](https://code.claude.com/docs/en/hooks),
[Codex hooks](https://developers.openai.com/codex/hooks),
[Codex command runner](https://github.com/openai/codex/blob/ee6814bfa4889fe9b2b3dcc9cc8bdd91effa8ab8/codex-rs/hooks/src/engine/command_runner.rs),
[Cursor hooks](https://cursor.com/docs/hooks),
[Copilot CLI hooks](https://docs.github.com/en/copilot/reference/hooks-reference),
[Pi 0.85.1 extensions](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/extensions.md),
[Pi 0.85.1 loader](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/extensions/loader.ts),
[OpenCode plugin interface](https://github.com/anomalyco/opencode/blob/dev/packages/plugin/src/index.ts),
[Bun 1.4.2 subprocess API](https://github.com/oven-sh/bun/blob/bun-v1.4.2/packages/bun-types/bun.d.ts),
[Node 24 TypeScript](https://nodejs.org/docs/v24.21.0/api/typescript.html).
Claude command+args and current Codex/Cursor/Copilot/OpenCode APIs are capability
prerequisites; these sources do not establish minimum released versions for every
field. No installed agent was activated to infer support.

`fixtures/hooks.json` supplies neutral positive wire/native vectors. The Python
gate runs real child processes, direct native exec, Bun OpenCode entries and Pi's
pinned Jiti loader. Local POSIX probes use non-login `/bin/sh -c`; CI sets
`FLYRAIL_TEST_CODEX_LOGIN_SHELL=1` to execute Codex's documented default `/bin/sh -lc`.
The default startup depends on readable shell profiles. The optional local command
and its limits are in [Python development](../python/docs/development.md).
The probe option does not establish a host setting for arbitrary shell arguments.
Native Windows cmd/direct-exec and Linux checks are wired into the CI matrix;
running tests on macOS does not establish those platforms locally. TypeScript
strict checking, syntax checks and auditing the exact locked development tool
graph are part of the shared gate.
