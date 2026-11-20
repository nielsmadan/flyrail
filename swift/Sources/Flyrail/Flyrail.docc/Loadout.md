# Installing into global Loadout

Offer an existing global Loadout setup as a destination the user can select.
Flyrail supports the Loadout 0.11.0 source format. The app or user runs `loadout sync`
after source installation or removal to update generated harness configuration.

## Discover, choose, and install

Call ``discoverLoadout(home:configHome:environment:)`` with an absolute home,
an optional `configHome`, and the host's environment snapshot. Discovery reads
`loadout/config.toml` under `configHome`, nonempty `XDG_CONFIG_HOME`, or
`<home>/.config`, in that order.
`.absent` means the registration is missing. `.invalid` provides a diagnostic
with a code, message, and path. `.available` exposes the active profile chain,
declared sources, harness slices, and effective selectors.

Use discovery when building the destination offer. This example checks again
after the user opts in. The host provides callbacks to show an invalid setup,
choose a declared source, review the preview, and save the locator alongside
its state store and bundle ID.

```swift
import Flyrail

func installIntoLoadout(
    bundle: FlyrailBundle,
    home: ResourcePath,
    environment: [String: String],
    harnesses: [Agent],
    store: any StateStore,
    options: LoadoutOptions,
    reportInvalid: (LoadoutDiagnostic) -> Void,
    chooseSource: ([LoadoutSource]) -> String?,
    review: (LoadoutPreview) -> Bool,
    saveLocation: (LoadoutLocation) throws -> Void
) throws -> LoadoutResult? {
    switch try discoverLoadout(home: home, environment: environment) {
    case .absent:
        return nil
    case let .invalid(_, diagnostic):
        reportInvalid(diagnostic)
        return nil
    case let .available(setup):
        let choices = setup.eligibleSources(bundle: bundle, harnesses: harnesses)
        guard !choices.isEmpty, let name = chooseSource(choices) else { return nil }
        let destination = try setup.select(source: name, harnesses: harnesses)
        let plan = try previewLoadout(bundle: bundle, destination: destination, store: store, options: options)
        guard review(plan), plan.applicable else { return nil }
        try saveLocation(plan.location)
        return applyLoadout(plan)
    }
}
```

Eligibility filters by the requested slices and source `use`. Preview checks
the content, edits, and scope. Use `summarize(plan.lifecycle)` for planned
resource paths and actions. Show any diagnostic or resource errors, `impact`,
and `scopeNotice` before applying.
Other profiles and projects that use a shared source may receive its content.
If preview reports `shared-scope`, show the impact and obtain explicit approval
before selecting again with `acceptSharedScope: true` and reviewing a fresh preview.
Use `LoadoutOptions(platform: .macOS)` for a skills-only bundle. Supply a
``HookRuntime`` for hook bundles, including an explicit Node 24+ executable for
Claude, Codex, and Cursor hooks.

## Supported source mappings

| Bundle content | Loadout source mapping |
| --- | --- |
| Skills | `skills/<name>/` with authored files and a manifest policy for harness coverage and `render = false`. |
| Instructions | Bundle-qualified Markdown fragments in `instructions/` and individual `source/fragment` entries in configured selector arrays. |
| MCP | Named `mcp.toml` tables. Stdio accepts command, arguments, and literal environment values. HTTP accepts a URL and an optional bearer environment variable name. |
| Hooks | JSON fragments and selectors for Claude, Codex, and Cursor. OpenCode plugins and Pi extensions under `module-config/` use Flyrail's bridges. |
| Native files | Single `.js`, `.mjs`, or `.ts` files directly under the OpenCode user CLI plugin directory or Pi user CLI extension directory, mapped through `module-config/`. |

Preview refuses disabled slices, conflicting source content, unsupported native
paths, MCP `cwd` and stdio environment references, Copilot, and Droid hooks.
Hook events and outcomes must also fit the chosen harness. Separate hook selector
arrays are required where the harnesses use different hook protocols.

## Apply, inspect, and remove

Apply rechecks the registration, profile, and relevant source inputs. Rediscover
and preview again if they changed. Read ``LoadoutResult/lifecycle`` for status,
errors, and recovery details. Use ``LoadoutResult/syncRequired`` to surface the
activation step. Report an applied install as a source change awaiting sync.
Handle partial results using the per-resource outcomes and the existing
<doc:Transactions> guarantees.

Persist the ``LoadoutLocation`` fields `root`, `profile`, `sourceName`, and
`sourcePath` with the bundle ID and the identity of the same <doc:Stores> store.
Reconstruct the locator with `LoadoutLocation(root:profile:sourceName:sourcePath:)`.
Use it with `inspectLoadout`, `recoverLoadout`, or `previewLoadoutRemoval` followed
by `applyLoadout`. These calls address the original installation even if the
registration changes or the bundle directory disappears. Successful removal
also requires sync for the generated destinations to reflect the source changes.
