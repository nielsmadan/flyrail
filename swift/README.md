# Flyrail for Swift

The `Flyrail` library product is the Swift implementation of Flyrail's synchronous
library for reversible skills, instructions, MCP registrations and hooks at
explicit local destinations. It requires macOS 13 or later and Swift 6.2 or later.

Like the other implementations, it never prints, prompts, starts bundled commands,
contacts a network service, or selects an installed agent automatically.

## Installation

Add the package to `Package.swift` and depend on the `Flyrail` product:

```swift
dependencies: [
    .package(url: "https://github.com/nielsmadan/flyrail", .upToNextMinor(from: "0.1.0")),
],
targets: [
    .target(name: "App", dependencies: [.product(name: "Flyrail", package: "flyrail")]),
]
```

Swift releases are the repository's `vX.Y.Z` tags. Swift versions are independent
of the Python and Rust packages. While the package is below 1.0, a minor release
may change the API, so `.upToNextMinor` accepts only patch releases.

## Quickstart

```swift
import Flyrail

let bundle = try FlyrailBundle(directory: ResourcePath("/path/to/flyrail"))
let target = try Target.directory(ResourcePath("/path/to/skills"))
let store = try DirectoryStore(ResourcePath("/path/to/app-state/flyrail"))

let installed = install(bundle, targets: [target], store: store)
let removed = uninstall(bundleID: bundle.identity.id, targets: [target], store: store)
```

There is no default state store. Construct one explicitly, for example a
`DirectoryStore` in the application's own state directory, and pass it to every
lifecycle call. `MemoryStore` suits tests and dry runs. Skill lifecycle functions
report each target's outcome as a `TargetResult` and never throw.

The DocC catalog in [`Sources/Flyrail/Flyrail.docc`](Sources/Flyrail/Flyrail.docc)
documents state stores, transactions, authority and editing. The shared formats
are described in the [specifications](../spec/README.md).

## Development

`just check` runs formatting, lint, a warnings-as-errors build and the unit
tests; `just check-integration` and `just check-package` run the cross-language
scripts and package QA. See [AGENTS.md](AGENTS.md) for tool versions and sandbox
notes.

## Releasing Swift

SwiftPM resolves packages from Git tags, so a Swift release is an annotated
`vX.Y.Z` tag on `main` plus a GitHub release; there is no registry upload. From
the repository root:

```sh
just release swift --dry-run
just release swift
just release swift patch
just release swift 0.2.0
```

The release runs the integration gates, so run `just setup` first for the Python
environment, and have Rust, Node and Bun installed. Pushing the tag publishes the
release to SwiftPM consumers immediately, so every gate runs before the push:
curtains considers commits touching `swift/`, `Package.swift` or
`Package.resolved`, runs Swift `just check`, `just check-integration` and the
workflow validation, writes [`CHANGELOG.md`](CHANGELOG.md), reruns package QA
against the release commit, then pushes `main` with the tag. Package QA builds a consumer against
the extracted package and needs network access to resolve `swift-system`.

[`release-swift.yml`](../.github/workflows/release-swift.yml) validates the tag,
reruns the Swift, integration and package gates on macOS 26, and creates the
"Flyrail for Swift" GitHub release. Releases below 1.0 are marked as
pre-releases. Retry a failed publication from the workflow's Actions page with
**Run workflow** on `main`, supplying the existing tag, or with the GitHub CLI:

```sh
gh workflow run release-swift.yml --ref main -f tag=v0.1.0
```

Never move a published tag: SwiftPM consumers pin its commit. The `v*` tag
pattern is shared with any other `v`-prefixed tag, so cut `vX.Y.Z` tags only for
Swift releases and protect them with a tag ruleset.
