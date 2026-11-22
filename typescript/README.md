# Flyrail for TypeScript

Reversible skills, instructions, MCP registrations and hooks for AI agents, as
an ESM-only, async TypeScript API. The engine is the Flyrail Rust crate, bound
through a napi-rs native addon, so behaviour and durable formats match the
other Flyrail implementations.

## Status

Alpha and unreleased. Every domain of the Rust crate that the Go module also
exposes is bound, except host-implemented stores and store locks (see
[support](docs/support.md#deferred)). Nothing is published to npm yet.

## Platforms

- Node.js 24.14 or newer, and Bun 1.4.2 or newer (run scripts with `bun run`).
- Electron main processes, with `flyrail` kept external to the bundle and
  `**/*.node` unpacked from the asar archive.
- macOS and Linux (glibc) on arm64 and x64. Windows and musl are unsupported.

## Building from a checkout

The build needs Rust 1.99.0 (selected by the repository's `rust-toolchain.toml`),
Node.js with npm, and [just](https://github.com/casey/just). From `typescript/`:

```sh
just setup         # npm ci
just native-build  # debug addon in build/native/
just build         # dist/
just check         # the full fast gate
```

## Installation

Until release automation exists, the package is installed from two local
tarballs: the `flyrail` package and the platform package that carries the
native addon for your OS and CPU (`flyrail-darwin-arm64`, `flyrail-darwin-x64`,
`flyrail-linux-arm64` or `flyrail-linux-x64`). From a checkout, `just pack`
builds the release addon and `dist/`, `npm pack`s `typescript/`, generates the
platform package around the release addon, `npm pack`s that too and prints both
tarball paths (under `../.cache/typescript/pack/`, or the directory you pass as
`just pack DIR`). Install both tarballs together in your project:

```sh
npm install /path/to/flyrail-0.1.0.tgz /path/to/flyrail-darwin-arm64-0.1.0.tgz
```

The package needs Node.js 24.14 or newer, Bun 1.4.2 or newer, or an Electron
main process; [support](docs/support.md) covers each runtime and Electron
packaging.

## Quickstart

`examples/quickstart.ts` installs, inspects and uninstalls a skill bundle built
in memory, then renders an instruction bundle for Codex, previews it, applies
the preview to `AGENTS.md` and removes it again:

```ts
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import {
  Bundle,
  BundleIdentity,
  DirectoryStore,
  InstructionArtifact,
  RenderContext,
  Target,
  applyPreview,
  fileEntry,
  inspect,
  install,
  preview,
  remove,
  render,
  uninstall,
} from 'flyrail';

const encode = (text: string): Uint8Array => new TextEncoder().encode(text);

export async function run(root: string): Promise<void> {
  const store = await DirectoryStore.open(join(root, '.flyrail-state'));

  const manifest = {
    schema_version: 1,
    id: 'team-skills',
    version: '1',
    skills: [{ name: 'review', path: 'review' }],
  };
  const skills = Bundle.fromMemory(encode(JSON.stringify(manifest)), [
    fileEntry('review/SKILL.md', encode('# Review\n\nReview the change for correctness.\n')),
  ]);
  const targets = [Target.directory(join(root, 'skills'))];
  const [installed] = await install(skills, targets, store, { lockTimeoutMs: 5000 });
  console.log('install:', installed?.status);
  const [inspection] = await inspect(skills, targets, store);
  console.log('current:', inspection?.observation.isCurrent);
  const [uninstalled] = await uninstall(skills.id, targets, store, { lockTimeoutMs: 5000 });
  console.log('uninstall:', uninstalled?.status);

  const guide = Bundle.fromArtifacts(new BundleIdentity('project-guide', '1'), [
    new InstructionArtifact('guide', 'Run the project checks before committing.\n'),
  ]);
  const context = new RenderContext(Target.project('codex', root), 'linux');
  const rendered = render(guide, context);
  const target = context.installation(store);
  const proposal = await preview(guide, rendered, target);
  const applied = await applyPreview(proposal, { lockTimeoutMs: 5000 });
  console.log('apply:', applied.status);
  console.log(await readFile(join(root, 'AGENTS.md'), 'utf8'));
  const removed = await remove(guide.id, target, { lockTimeoutMs: 5000 });
  console.log('remove:', removed.status);
}
```

## Documentation

- [API reference](docs/api.md)
- [Architecture](docs/architecture.md)
- [Lifecycle](docs/lifecycle.md)
- [Configuration](docs/configuration.md)
- [Inspection](docs/inspection.md)
- [Translation](docs/translation.md)
- [Hooks](docs/hooks.md)
- [Global Loadout](docs/loadout.md)
- [Examples](docs/examples.md)
- [Support and packaging](docs/support.md)
- [Development](docs/development.md)

## License

MIT
