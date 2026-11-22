# Translation

`render` and `renderMany` turn a `Bundle` into a `RenderedBundle` for one or
more agent contexts, and `translateInstructionArtifact`, `translateMcpArtifact`
and `assembleTranslations` expose the same steps one artifact at a time. The
translation rules (destinations, interpolation, assets and activation notices)
are the crate's; the dated contract is in
[`spec/translation.md`](../../spec/translation.md). This page records what the
TypeScript surface adds.

## Contexts

A `RenderContext` pairs a `Target` that names an agent (`Target.project` or
`Target.user`) with a platform. `Target.directory` has no agent and cannot make a
context. The context never reads the process: the home directory, environment
variables, hook runtime and every explicit path (`executionRoot`,
`instructionPath`, `mcpPath`, `nativeRoot`, `assetRoot`) arrive as arguments and
must be absolute.

## Outcomes

An agent without a verified translation (`droid`) and any unsupported request do
not throw. They return an `unsupported` `Notice` with Rust's code and message:
`ArtifactTranslation.supported` and `RenderedBundle.supported` are `false`, and
a rendered bundle with an unsupported notice carries no artifacts. Only
`requireTranslatable` throws, with `UnsupportedTranslationError`.

Activation and prerequisite notices (`mcp-activation`, `instruction-loading`,
`pi-mcp-adapter-v2-32-1`, ...) are returned on supported results and do not make
a result unsupported.

## Assembly

`assembleTranslations` blocks incompatible claims on one destination
(`destination-collision`), incompatible document formats
(`document-format-collision`) and Copilot CLI precedence conflicts
(`copilot-mcp-precedence-collision`). Identical registrations on one destination
coexist. `renderMany` renders each context and merges the results, so its digest
does not depend on context order.

## Conformance

`spec/fixtures/translations.json` (every case on macOS, Linux and Windows, the
asset manifest and the combined cases) and the `renderings` and `capabilities`
sections of `spec/fixtures/configurations.json` run against this surface.

The `capabilities` vectors are acceptance contracts that predate the renderer;
the Rust crate differs from two of them, and the TypeScript test
asserts the crate's behaviour: the Copilot CLI stdio registration carries no
`type: "local"` entry, and the generic `preToolUse` `ask` outcome is reported
with the code `hook-outcome` rather than `generic-preToolUse-ask-is-ignored`.
