# Configuration bundles, schema 2

A configuration bundle snapshots skills, instructions, MCP registrations, hooks,
and the support assets those artifacts explicitly reference. Native declarations
belong to one of those four families. This schema does not define permissions,
tool-approval policy, general settings, package installation, or plugin management.

Python accepts these bundles through `Bundle`; both schemas use the same resource
lifecycle. Skill convenience functions accept skill-only bundles. Other families
use explicit rendered resources; agent translation consumes the same contracts.
Schema 1 remains the skill format described in [bundle-format.md](bundle-format.md).
There is no receipt migration or version-label ordering.

## Sources and snapshot identity

Filesystem directories, package resources, explicitly selected ZIP directories,
memory manifests with source entries, and immutable artifact construction produce
the same artifact bytes and digest. A filesystem or archive snapshot also retains
its source roots for overlap checks. Memory/generated snapshots have no filesystem
source roots. Source roots do not contribute to content identity.

The existing portable path, Unicode, collision, regular-file, reparse-point and
explicit executable-intent rules apply. Manifest source paths and native relative
destinations reject traversal, absolute paths and ambiguous spelling. Tree snapshots
include empty directories, infer missing parent directory entries and reject a file
that is also a parent. Native file modes are exactly `private`, `readable` or
`executable`; physical source mode bits never supply intent. These are creation
intents, not permission overrides for existing shared documents.

## Manifest records

The root has required `schema_version: 2`, `id`, `version`, and a nonempty
`artifacts` array. Optional `assets` and `dependencies` default to empty arrays.
Every JSON object has an exact field set: unknown and duplicate keys, unsupported
numbers, wrong types and implicit coercions are rejected. Bundle labels retain
the schema-1 opaque Unicode-string equality semantics.

Every artifact has a unique `id` and one of these `kind` values:

| Kind | Required fields after `id`, `kind` | Optional fields |
| --- | --- | --- |
| `skill` | `name`, `path` | `executables` |
| `instruction` | Exactly one of `text`, `path` | None |
| `mcp` | `name`, `transport` | None |
| `hook` | `event`, `command`, `protocol_version: 1` | `timeout_ms`, `outcomes`, `tools` |
| `native` | `family`, `audience`, `destination`, `content` | None |

Skill names are unique and every selected tree contains an exact regular
`SKILL.md`. Instruction source files decode as UTF-8 without newline rewriting.
Support asset records contain `id`, `family`, `path`, and optional `executables`.
Their IDs share the artifact namespace. Each asset has an explicit dependent;
unused assets are errors. Dependencies contain `dependent` and `required` IDs,
with optional `mode` defaulting to `"revision"`; `"stable-reference"` selects a
stable routing entry as described below.
Unknown IDs, duplicate edges and cycles are errors. Sources are not scanned for
script references. Asset delivery paths are a rendering decision.

Commands contain a nonempty `argv` array, optional `env` object, and optional
execution-root-relative `cwd`. Arguments preserve spaces and shell-looking text literally.
The render context supplies that absolute execution anchor (project root or explicit
user home by default); cwd never refers back to bundle source storage. Arguments
and cwd can instead use `{"asset":"ID","path":"relative/path"}` with optional
`path` to reference a checked installed support-tree revision. See the
[translation contract](translation.md) for validation and revision paths.
Environment values are literal strings or `{"env":"VARIABLE_NAME"}` references;
references remain unresolved. Commands, values and cwd reject NUL. Installation
does not run a command or fetch a dependency.

MCP transport is either `{"kind":"stdio","command":<command>}` or
`{"kind":"http","url":"https://example.test/mcp"}` with optional
`bearer_token_env`. HTTP(S) URLs require a host and reject credentials, fragments,
whitespace, control characters and invalid ports. Bearer tokens are environment
references, never an instruction to read the environment during snapshotting.

Hook events are `session-start`, `before-tool`, `after-tool`, `prompt`, and `stop`.
Declared outcomes are `continue`, `block`, `ask`, `modify-input`, and `context`;
the default is `continue`. `tools` is a set of exact names, available only for tool
events. Timeout is an integer from 1 to 300000 milliseconds, default 10000.
Protocol and host capability checks occur before lifecycle mutation. The model's
ability to express an outcome is not a claim that every agent supports it.

Native families are exactly `skills`, `instructions`, `mcp`, and `hooks`.
Audience requires one existing agent name and `project` or `user` scope. Surface
defaults to `cli`; `ide` is Cursor-specific and `vscode` is Copilot-specific.
Optional platform is `macos`, `linux` or `windows`. A native relative destination
is resolved against the selected agent context by the renderer.

## Closed resource contents

| Content kind | Required fields after `kind` | Optional fields |
| --- | --- | --- |
| `file` | `path` | `mode` (default `private`) |
| `tree` | `path` | `executables` |
| `section` | `marker`, exactly one of `text`, `path` | `boundaries` |
| `structured` | `format`, `selector`, exactly one of `value`, `semantic_value` | `placement` |

Sections default to complete marker lines `<!-- flyrail:ID:start -->` and
`<!-- flyrail:ID:end -->`. A custom boundary object contains `start` and `end`:
distinct nonblank single lines without NUL. Content cannot contain its own
boundary lines. A marker ID is attribution; the physical section selector is the
actual boundary pair. Different IDs using the same pair address the same claim.
Editors additionally reject overlapping spans, repeated boundaries and malformed
or ambiguous pairs after reading the document.

Structured formats are `json`, `jsonc`, and `toml`. Selectors are nonempty arrays
of object-key strings and member objects. A member contains exactly one of
`identity` (JSON value) or `semantic_identity` (typed record below), plus optional
`key`, a path of object-key strings. Empty `key` means exact typed-value identity;
nonempty `key` selects a scalar native identity such as a `name` field. Numeric
array indexes are not selectors. Duplicate or multiply matching members are
ambiguous even when their payloads are equal.

Placement is `first`, `last`, `before`, or `after`; the last two require an `anchor`
member. Placement only applies to a final member selector. Self-anchors are invalid.
Exact identity changes are retirement plus acquisition with a fresh baseline.
Stable native identity with a changed payload preserves the original baseline.
Generated hooks should use a stable launcher or native key when available so a
payload update does not restore the original foreign hook prematurely.

## Semantic value encoding 1

Values are immutable tagged scalars, arrays and objects. Object keys are exact
Unicode scalar strings, sorted by UTF-8 bytes; duplicate keys are errors. Arrays
retain order and may contain equal values, although a selector cannot identify an
ambiguous duplicate. Scalars distinguish null, boolean, integer, float and string.
Integers are signed 64-bit values. Floats are finite IEEE-754 binary64, including
distinct positive and negative zero. Date/time values follow TOML's date, local
time, local datetime and offset datetime types. Offset datetimes normalize to UTC;
local times reject offsets. Scalar strings reject surrogate code points.

Canonical records are JSON arrays:

| Value | Record |
| --- | --- |
| Null | `["null"]` |
| Boolean | `["boolean",true]` |
| Integer | `["integer","-17"]` |
| Float | `["float","0x1.8000000000000p+0"]` |
| String | `["string","text"]` |
| Date | `["date","2026-01-02"]` |
| Local time | `["time","01:02:03.000000"]` |
| Local datetime | `["datetime","2026-01-02T01:02:03.000000"]` |
| Offset datetime | `["datetime","2026-01-02T00:02:03.000000+00:00"]` |
| Array | `["array",[<record>,...]]` |
| Object | `["object",[["key",<record>],...]]` |

Integer text uses ordinary decimal with no leading zero or plus sign. Float text
uses lowercase hexadecimal normalized as Python's binary64 `float.hex`: normal
values have one leading hexadecimal digit and thirteen fractional digits; zero is
`0x0.0p+0`, subnormals retain exponent `-1022`. Times always have six fractional
digits. Noncanonical scalar representations are rejected. Semantic bytes are
UTF-8 JSON with literal Unicode, no whitespace, no BOM, and no final newline.

## Bundle and rendering digests

Model records have object fields `type` (the stable tag below) and `fields` (an
object of every field listed below). Defaults are materialized before hashing:
nulls, false booleans and empty sequences remain encoded. In particular,
`Dependency.mode` is encoded as `"revision"` when omitted from input, and
`RenderedArtifact.schema_requirements` is encoded as an empty array when unused.
Values recursively use the semantic encoding above. Byte strings become an
object `{"bytes":"<lowercase hex>"}`;
sequences become arrays, enum values become strings, paths become slash-separated
absolute strings. Record tags match the public immutable model names:
`SkillArtifact`, `InstructionArtifact`, `McpArtifact`, `HookArtifact`,
`NativeArtifact`, `SupportAsset`, `Audience`, `Command`, `AssetRef`, `EnvRef`, `HttpTransport`,
`FileContent`, `TreeContent`, `BundleEntry`, `SectionContent`, `SectionBoundaries`,
`StructuredContent`, `DocumentSchema`, `Selector`, `Key`, `Member`, `Placement`,
`Dependency`, `RenderedArtifact`, and
the typed `Scalar`, `ObjectValue`, `ArrayValue` wrappers. These tags and fields are
wire names, not a requirement to expose those class names in another language:

| Model tag | Fields |
| --- | --- |
| `SkillArtifact` | `id`, `name`, `content` |
| `InstructionArtifact` | `id`, `text` |
| `McpArtifact` | `id`, `name`, `transport` |
| `HookArtifact` | `id`, `event`, `command`, `timeout_ms`, `outcomes`, `tools`, `protocol_version` |
| `NativeArtifact` | `id`, `family`, `audience`, `destination`, `content` |
| `SupportAsset` | `id`, `family`, `content` |
| `Audience` | `agent`, `scope`, `surface`, `platform` |
| `Command` | `argv`, `env` (sorted name/value pairs), `cwd` |
| `AssetRef` | `asset_id`, `path` (null selects tree root) |
| `EnvRef` | `name` |
| `HttpTransport` | `url`, `bearer_token` |
| `FileContent` | `data`, `mode` |
| `TreeContent` | `entries` |
| `BundleEntry` | `path`, `data` (null for directory), `executable` |
| `SectionContent` | `marker`, `text`, `boundaries` (null selects convention) |
| `SectionBoundaries` | `start`, `end` |
| `StructuredContent` | `format`, `selector`, `value`, `placement` |
| `DocumentSchema` | `format`, `key`, `value` (a `Scalar` model record) |
| `Selector` | `parts` |
| `Key` | `name` |
| `Member` | `identity`, `key` |
| `Placement` | `position`, `anchor` |
| `Dependency` | `dependent`, `required`, `mode` |
| `Scalar` | `value` |
| `ObjectValue` | `items` (sorted name/value pairs) |
| `ArrayValue` | `items` |
| `RenderedArtifact` | `id`, `family`, `destination`, `content`, `subtree`, `require_existing`, `schema_requirements` |

Configuration digest is SHA-256 of ASCII `flyrail-config-v2` plus NUL, followed
by semantic bytes of the model sequence `(artifacts, assets, dependencies)`.
Artifacts/assets sort by ID; edges sort by dependent then required. Bundle ID,
version and source location are excluded. Artifact IDs, payload bytes, native
audience/destination, creation mode, executable intent and dependencies contribute.

Rendering has a separate SHA-256 domain `flyrail-render-v1` plus NUL over
`(rendered_artifacts, dependencies)`. Rendered artifacts have `id`, `family`,
absolute `destination`, closed `content`, nullable `subtree`, boolean
`require_existing` and a `schema_requirements` sequence. A subtree is an owned
tree within a skill-container authority;
required-existing binds absence checks through publication. Notices identify prerequisite,
activation or unsupported results with an optional stable `code`. Immutable
`routing_context` string pairs retain the explicit rendering selection in previews. They do not change content identity. A renderer
must block unsupported requested behavior and must bind its capability/context
selection into the resulting preview, even when two legacy skill Targets compare
equal.

`fixtures/configurations.json` uses fixture schema 1 to collect schema-2 source,
render, claim, authority, receipt, transaction and capability vectors. Its `cases`
contain complete portable source inputs and expected configuration digests;
`semantic_values` pin the independent typed encodings. The fixture consumer
exercises directory, package, ZIP, memory and immutable artifact construction.
`renderings` pin complete rendered contents and their separate digest.
Rendered fixture file contents use `data_hex` bytes; schema requirements carry
`format`, `key` and a plain scalar `value` to construct the model records above.

`capabilities` records a single portable artifact request, an explicit
agent/scope/surface/platform context, and the expected supported registration or
blocking result. Supported MCP records specify destination, document format,
selector and native value where the path contract is fixed. The Pi case requires
the adapter registration fields and an explicit prerequisite with a schema pin
before rendering. It does not assert that the adapter is installed. Unsupported
requests must block mutation. These are acceptance contracts for the translation
layer; explicit resource rendering and lifecycle do not imply agent translation. They cover
CLI versus VSCode registration roots, environment references, prerequisites and
an unsupported hook outcome. The implemented renderer is exercised by the expanded positive and unsupported
`fixtures/translations.json` vectors; the dated [translation contract](translation.md)
defines its destination, interpolation, asset and activation boundaries.

A rendered `"stable-reference"` dependency requires a whole-file routing entry
whose own dependencies are revision-bound. `RenderedArtifact.schema_requirements`
contains immutable `DocumentSchema(format, key, Scalar)` values for top-level
JSON/JSONC schema scaffolding. They are not app-owned selectors. See the
[hook protocol](hook-protocol.md) for lifecycle and portable-outcome guarantees.
