# Global Loadout

A Loadout installation changes the inputs of a user's Loadout source. The host
offers the destination, the user selects it, and after a successful change the
host runs Loadout sync. The library never runs sync. The format, the
compatibility baseline and the transaction rules are in
[spec/loadout.md](../../spec/loadout.md).

```ts
import { discoverLoadout, previewLoadout, applyLoadout, DirectoryStore } from 'flyrail';

const found = await discoverLoadout({ home, environment: { XDG_CONFIG_HOME: configRoot } });
if (found.status === 'available') {
  const destination = found.setup.select('local', ['claude']);
  const store = await DirectoryStore.open(stateDirectory);
  const proposal = await previewLoadout(bundle, destination, store, { platform: 'macos' });
  if (proposal.applicable) {
    const result = await applyLoadout(proposal, { signal });
    if (result.syncRequired) {
      // run Loadout sync here
    }
  }
}
```

## Discovery and selection

`home` and `configHome` are absolute. A supplied `configHome` selects the
registration and `environment` is then ignored; otherwise a nonempty
`XDG_CONFIG_HOME` in `environment` does, and failing that it is
`.config/loadout/config.toml` under `home`. `status` is `absent` (no registration), `available` (a `LoadoutSetup`)
or `invalid` (a `LoadoutDiagnostic`).

`setup.eligibleSources(bundle, harnesses)` lists the declared sources that can
carry the bundle for the requested harnesses. `setup.select(name, harnesses,
{ acceptSharedScope })` returns a `LoadoutDestination` holding the chosen source.
A narrower selection than the source or selector can represent needs
`acceptSharedScope: true`; otherwise the preview carries the `shared-scope`
diagnostic.

## Preview, apply, inspect, recover and remove

- `previewLoadout(bundle, destination, store, options)` plans the source edits.
  `impact` lists the paths and harnesses affected, `scopeNotice` the shared-scope
  warning, `diagnostic` why a preview is not applicable.
- `previewLoadoutRemoval(bundleId, location, store)` plans a source-free removal
  from the recorded installation; apply it with `applyLoadout`.
- `applyLoadout(preview, { lockTimeoutMs, signal })` revalidates the recorded
  reads and publishes. Cancellation follows the
  [lifecycle checkpoints](lifecycle.md#abortsignal): an abort seen up to the
  post-lock check rejects with `signal.reason`; a later one resolves with the
  real result.
- `inspectLoadout(bundleId, location, store)` observes the installation.
- `recoverLoadout(bundleId, location, store, options)` completes or rolls back an
  interrupted transaction.

A `LoadoutSetup` or `LoadoutDestination` captures the source as it was read at
discovery. Once the source changes, including through this library's own
`applyLoadout` or removal, a preview from that setup or destination is not
applicable: its `diagnostic` has code `concurrent-change`. Run
`discoverLoadout` again and select a fresh destination before the next preview.

Keep the `LoadoutLocation` (`root`, `profile`, `sourceName`, `sourcePath`)
alongside the state store: it identifies the installation for later reads and
removal. `location.installation(store)` gives the `InstallationTarget`.

## Sync responsibility

Every successful change, removal included, needs Loadout sync before the
generated destinations reflect it. An update or removal can retire assets that
generated hooks and MCP servers still reference, so coordinate the change and the
sync. The result's `lifecycle.notices` carries `loadout-sync-required` for
removals.

## Coverage

| Rust                                                                                               | TypeScript                                                                       |
| -------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| `discover_loadout`                                                                                 | `discoverLoadout`                                                                |
| `LoadoutDiscovery::{Absent, Available, Invalid}`                                                   | `LoadoutDiscovery` tagged by `status`                                            |
| `LoadoutDiagnostic` `code`, `message`, `path`                                                      | `LoadoutDiagnostic` `code`, `message`, `path`                                    |
| `LoadoutSetup` `registry`, `root`, `profile`, `profile_chain`, `sources`, `harnesses`, `selectors` | `LoadoutSetup` properties of the same names in camel case                        |
| `LoadoutSetup::eligible_sources`, `select`                                                         | `eligibleSources`, `select`                                                      |
| `LoadoutSource`, `LoadoutHarness`, `LoadoutSelector`                                               | readonly interfaces; sets and maps become frozen arrays and records              |
| `LoadoutDestination::location`, `source`, `harnesses`                                              | `LoadoutDestination` properties                                                  |
| `LoadoutLocation::new`, accessors, `installation`                                                  | `new LoadoutLocation(...)`, properties, `installation(store)`                    |
| `LoadoutOptions`                                                                                   | `LoadoutOptions` (`platform`, `hookRuntime`, `executionRoot`, `replaceModified`) |
| `preview_loadout`, `preview_loadout_removal`                                                       | `previewLoadout`, `previewLoadoutRemoval`                                        |
| `LoadoutPreview` accessors, `applicable`, `scope_notice`                                           | `LoadoutPreview` properties                                                      |
| `LoadoutImpact`                                                                                    | `LoadoutImpact`                                                                  |
| `apply_loadout`, `LoadoutResult`                                                                   | `applyLoadout`, `LoadoutResult` (`syncRequired`)                                 |
| `inspect_loadout`, `recover_loadout`                                                               | `inspectLoadout`, `recoverLoadout`                                               |
