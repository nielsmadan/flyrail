# Bundled hooks

Render a hook artifact with a `HookRuntime` snapshot, install the rendered
bundle, and let the host run the generated configuration. The library never
executes hooks.

```ts
import {
  Bundle,
  BundleIdentity,
  Command,
  HookArtifact,
  HookRuntime,
  RenderContext,
  Target,
  render,
} from 'flyrail';

const bundle = Bundle.fromArtifacts(new BundleIdentity('project-hooks', 'one'), [
  new HookArtifact({
    id: 'audit',
    event: 'before-tool',
    command: new Command(['audit-command', '--json']),
    timeoutMs: 10_000,
    outcomes: ['continue'],
    tools: ['Bash'],
  }),
]);
const runtime = new HookRuntime({ node: '/opt/node/bin/node' });
const context = new RenderContext(Target.project('claude', '/work/app'), 'linux', {
  hookRuntime: runtime,
});
const rendered = render(bundle, context);
```

`new HookRuntime(options)` snapshots the packaged `runner.mts`, `bridge.mts` and
`process.mts` files (`runtime.resources`). A native host (Claude, Codex, Cursor,
Copilot, Droid) needs an absolute Node 24+ executable; Codex and Cursor shell
declarations also need `shell: 'posix' | 'cmd'`. The library validates the path
but never probes or runs it.

Pi and OpenCode run hooks in their own host runtimes (Bun 1.4.2 or newer for
OpenCode), so a render for those hosts needs no `node` path.

Installation writes configuration and support assets only. Trust, reload,
discovery and activation stay with the host; read `RenderedBundle.notices` for
prerequisites. Commands receive the canonical protocol JSON on stdin and must
print one of their declared outcomes. See the shared
[hook protocol](../../spec/hook-protocol.md) for payloads, bounds and the host
matrix.

## Protocol types

For integrations that speak the protocol, `HookJson`, `HookRequest`,
`HookResponse`, `HookEffects`, `HookHandler` and `HookConfig` mirror the Rust
types:

```ts
import { HookRequest, HookResponse } from 'flyrail';

const request = HookRequest.parse(stdin);
const response = HookResponse.parse(output, request.event, ['continue', 'block']);
if (response.outcome === 'block') {
  console.error(response.value?.text);
}
```

Hook text is UTF-16 and may hold lone surrogates; `text`, `tool`, `contexts` and
`reason` return it exactly. Integers are `bigint` and are stored as binary64
numbers. Failures throw `HookError` whose `code` is the Rust `HookFailure` code
(`json-depth`, `undeclared-outcome`, `host-tool` and so on). The limits are the
`HOOK_*_LIMIT` constants.

## Runtime checks

`test/hook-runtime.integration.test.ts` writes `HookRuntime.resources` into a
temporary directory and runs every `spec/fixtures/hooks.json` case through
`../rust/tests/hook_runtime_harness.mjs`, using Node, and Bun for OpenCode
cases (`bun run`, never `bun test`).
