# Public TypeScript API

[Package](#package) · [Conventions](#conventions) · [Option and input types](#option-and-input-types) · [Errors](#errors) · [Enumerations](#enumerations) · [Domains](#domains)

This page is the reference for the `flyrail` package. Behaviour, durable
formats and error text follow the Rust crate; this document fixes the shape of
the TypeScript surface.

## Package

`flyrail` is ESM only (`"type": "module"`, no `require` condition). The single
entry point `flyrail` exports every public class, function and type. It needs
Node.js 24.14 or newer, Bun 1.4.2 or newer, or an Electron main process, on
macOS or Linux (glibc) for arm64 or x64. Loading the package loads the native
addon once; a missing or unsupported addon throws `NativeLoadError`.

## Conventions

These rules apply to every domain below; an entry states only where it departs
from them.

| Rust                                              | TypeScript                                                                                                            |
| ------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| accessor `foo_bar()` / `is_foo()` / `as_str()`    | readonly property `fooBar` / `isFoo` (booleans keep `is`) / the string-literal value                                  |
| `Option<T>`                                       | `T \| null` (never `undefined` in outputs); optional input fields accept `undefined` or `null`                        |
| `Vec<T>` / `&[T]`                                 | `readonly T[]` (frozen)                                                                                               |
| `Vec<(String, T)>` context/env maps               | `Readonly<Record<string, T>>`                                                                                         |
| `Vec<u8>` / `&[u8]`                               | `Uint8Array` (copied both ways)                                                                                       |
| `PathBuf` / `&Path`                               | absolute `string`                                                                                                     |
| enum with `as_str`                                | string-literal union from [Enumerations](#enumerations); unknown strings fail natively with Rust's text               |
| `FromStr` / `Display` on temporal types           | `X.parse(text)` / `toString()`                                                                                        |
| builder chains / more than five positional params | one `...Init` or `...Options` object                                                                                  |
| `Clone`/`Eq` on inputs                            | `equals(other)` where Go exposes equality (`Target`, `Claim`, `InstallationTarget`, `BundleIdentity`, temporal types) |
| `Result<T, E>` (synchronous)                      | returns `T` or throws the matching error class                                                                        |
| filesystem or lock function                       | `Promise<T>` taking `options.signal`                                                                                  |

Immutable input classes validate natively in their constructors (throwing
`ValueError`), and their accessors never call native code. Results and
observations are frozen plain objects, built once per call; they are classes
only where they carry methods or a native handle.

**Input validation.** Every path argument must be absolute (POSIX
`isAbsolute`), or the call throws `ValueError("<label> must be an absolute path")`.
Strings that cross into Rust must be well-formed UTF-16
(`String.prototype.isWellFormed`), or the call throws `ValueError`. Hook text is
exact UTF-16 and may contain lone surrogates. The library takes home,
environment, PATH and working directory only as explicit arguments and never
reads them from the process.

**Numbers.** Semantic integers, hook JSON integers and any `u64`/`i64`
quantity are `bigint`. Binary64 values are JavaScript `number`s
(`type Binary64 = number`), finite only, with their bit patterns preserved
(including `-0` and subnormals). Bounded small integers (dates, times,
millisecond timeouts, schema versions, errno, alias indexes, offsets) are
`number`. Lock timeouts are `lockTimeoutMs: number`, finite and at least 0,
default 0 (one attempt per lock); anything else throws
`ValueError("lock timeout must be finite and nonnegative")`.

**AbortSignal.** Asynchronous calls accept `options.signal`. An
already-aborted signal rejects with `signal.reason` before any native work
starts.

- Mutations (`applyPreview`, `sync`, `recoverInstallation`, `remove`,
  `install`, `update`, `uninstall`, `applyLoadout`, `recoverLoadout`) reject
  with `signal.reason` only when the native call reports Rust's
  `Err(Cancelled)`. `Err(Cancelled)` means no index, receipt, journal or preparation was written,
  no destination content was published or removed, and no recovery ran. Lock
  files, store/staging/destination parent directories and authority records
  (which the transaction protocol retains permanently anyway) may persist, the
  same footprint a busy timeout leaves. Otherwise the call resolves with its
  real result, even when the signal fired after the commit started.
- Read-only calls (previews, inspections, loading, discovery, store `read` and
  `keys`, authority canonicalization) reject with `signal.reason` as soon as the
  signal aborts; the native work finishes in the background and its result is
  discarded.
- Store `write` and `delete` check the signal only before they start, then
  resolve with their real result.

A pending call keeps the event loop alive until it settles, so await mutations
before `process.exit` or `app.exit`.

## Option and input types

Every option object is a plain object. `CallOptions` carries `signal`; `LockOptions`
adds `lockTimeoutMs`. `PreviewOptions` (`acquisition`, `replaceModified`),
`DetectAgentsOptions` (`searchPath`, `home`, `applications`, `noApplications`),
`DiscoverLoadoutOptions` (`home`, `configHome`, `environment`) and `LoadoutOptions`
extend `CallOptions`. Constructors that take many fields accept an init object:
`HookArtifactInit`, `HookHandlerInit`, `HookRequestInit`, `RenderedArtifactInit`,
`SelectionSnapshotInit`, `OwnedSelectionInit`, `DocumentProvenanceInit` and
`HookRuntimeOptions`. The
Global Loadout results are described by `LoadoutSource`, `LoadoutHarness`,
`LoadoutSelector`, `LoadoutImpact` and `LoadoutResult`.

## Errors

| Class                         | Raised for                                                                                         | Fields                                                        |
| ----------------------------- | -------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| `FlyrailError`                | Base class of every error below.                                                                   |                                                               |
| `ValueError`                  | Construction and validation failures, including JavaScript-side argument checks.                   |                                                               |
| `BundleError`                 | Bundle loading.                                                                                    | `kind`, `errno`                                               |
| `AuthorityError`              | Path authority.                                                                                    | `kind`, `path`, `errno`                                       |
| `StoreError`                  | State store access.                                                                                | `kind`, `lockName`, `path`, `errno`                           |
| `HookError`                   | Hook protocol parsing; the message is `flyrail-hook:<code>`.                                       | `code`                                                        |
| `UnsupportedTranslationError` | A translation the target agent cannot express.                                                     | `code`                                                        |
| `TargetError`                 | Data, not thrown: carried in previews, observations and results where Rust places a `TargetError`. | `code`, `path`, `errno`                                       |
| `NativeLoadError`             | The addon could not be loaded, or the platform is unsupported.                                     | `attempts` (`NativeLoadAttempt`: `{ path, message }` per try) |
| `NativePanicError`            | A panic inside the native addon, with the panic message.                                           |                                                               |
| `NativeError`                 | A native failure outside the domain, such as an argument napi-rs could not convert.                |                                                               |

`kind` is `BundleErrorKind` (`'invalid' | 'io'`) for `BundleError`,
`AuthorityErrorKind` (`'invalid' | 'unsafe' | 'io'`) for `AuthorityError` and
`StoreErrorKind` (`'busy' | 'invalid-key' | 'io'`) for `StoreError`. Every
class has a stable `name` equal to the class name. `TargetError` extends
`FlyrailError` so a caller can `throw result.error`.

Lifecycle calls report problems in two places, as Rust does. Invalid arguments
(a relative path, a bad lock timeout, an object that is not a built-in store,
a malformed bundle id) reject with `ValueError`, and a store method's own
failure rejects with `StoreError`. Everything that happens to a target, such as
a conflict, a busy lock, an I/O failure or interrupted earlier work, is a
`TargetError` in the result's `error` fields, with Rust's code: a store's
`busy` becomes `busy`, `invalid-key` becomes `invalid_state`, and `io` becomes
`io_error` or, for `ENOTSUP`, `ENOSYS`, `EXDEV` and `EINVAL`, `unsupported`.

## Enumerations

Rust enums with `as_str` are string-literal unions: `Agent`, `TargetScope`,
`Surface`, `Platform`, `Family`, `HookEvent`, `HookOutcome`, `HookShell`,
`DocumentFormat`, `FileMode`, `Position`, `Ownership`, `Acquisition`,
`Disposition`, `EditStatus`, `NoticeKind`, `DependencyMode`, `ResourceKind`,
`OperationStatus`, `ErrorCode`, `ObservationState`, `ModificationKind` and
`ChangeAction`. `AGENTS` lists every `Agent` in Rust's `Agent::ALL` order.

## Domains

### Values, temporal types and models

Values cross the native boundary as tagged trees and are converted with explicit
loops and a manual stack, never recursion, so a 512-level value cannot overflow
the JavaScript stack. Nesting deeper than 512 levels throws
`ValueError("value nesting is too deep")` before any native call, and the
addon checks the same bound again.

```ts
type Binary64 = number;
type SemanticScalar =
  null | boolean | bigint | Binary64 | string | DateValue | TimeValue | DateTimeValue;
type SemanticValue = SemanticScalar | readonly SemanticValue[] | ObjectValue;
```

A `bigint` is a semantic integer and must fit signed 64 bits; a `number` is a
semantic float and must be finite. Text must be well-formed UTF-16. Plain
objects are not semantic values: use `ObjectValue`.

| Rust                                                                                                      | TypeScript                                                                                         | Notes                                                                                                                                                                       |
| --------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Value` (`Scalar`, `ArrayValue`, `ObjectValue`)                                                           | `SemanticValue`                                                                                    | The three Rust variants flatten into one union. `Value`'s `From<bool/i64/&str/String>` are the literals `true`, `1n`, `'text'`.                                             |
| `Scalar::null`, `boolean`, `integer`, `float`, `text`                                                     | `null`, `boolean`, `bigint`, `number`, `string`                                                    | `Scalar::float` refuses non-finite input with Rust's text `floats must be finite`; `-0` and subnormals keep their bits.                                                     |
| `Scalar::date`, `time`, `datetime`                                                                        | `DateValue`, `TimeValue`, `DateTimeValue`                                                          |                                                                                                                                                                             |
| `Scalar::as_boolean`, `as_integer`, `as_float`, `as_text`, `as_date`, `as_time`, `as_datetime`, `is_null` | `typeof` and `instanceof` on the value                                                             | No accessor methods.                                                                                                                                                        |
| `ArrayValue::new`, `items`, `len`, `is_empty`                                                             | a `readonly SemanticValue[]`                                                                       | Arrays are plain, frozen when decoded.                                                                                                                                      |
| `ObjectValue::new`                                                                                        | `new ObjectValue(members?)`                                                                        | A duplicate key throws `ValueError("duplicate object key")`. Members sort by UTF-8 key bytes, so `'\uffff'` precedes `'\u{1F600}'`.                                         |
| `ObjectValue::items`, `get`, `len`, `is_empty`                                                            | `entries()`, `get(key)`, `size`, `[Symbol.iterator]`; `has(key)` is added                          | `get` returns `undefined` for a missing key.                                                                                                                                |
| `Value::semantic_bytes`                                                                                   | `encodeValue(value): Uint8Array`                                                                   | Runs on a 16 MiB native stack.                                                                                                                                              |
| `Value::from_semantic_bytes`                                                                              | `decodeValue(bytes): SemanticValue`                                                                | Rust's JSON reader refuses more than 512 JSON levels; each semantic array or object spends two, so decoding stops near 256 levels of nesting.                               |
| `Value: Eq`                                                                                               | `valuesEqual(a, b): boolean`                                                                       | Floats compare by bits: `valuesEqual(0, -0)` is false, and `1n` never equals `1`.                                                                                           |
| `ValueError`                                                                                              | `ValueError`                                                                                       | The message is Rust's text.                                                                                                                                                 |
| `Date::new`, `FromStr`, `Display`, `year`, `month`, `day`                                                 | `new DateValue(y, m, d)`, `DateValue.parse`, `toString()`, `year`, `month`, `day`, `equals(other)` | An out-of-range field throws Rust's range message. A non-integer field throws a binding text such as `date month must be an integer` (likewise for time and offset fields). |
| `Time::new`, `FromStr`, `Display`, `hour`, `minute`, `second`, `microsecond`                              | `new TimeValue(h, m, s, us = 0)`, `TimeValue.parse`, `toString()`, fields, `equals(other)`         | `toString()` always prints six fractional digits.                                                                                                                           |
| `DateTime::local`, `coordinated`, `with_offset`                                                           | `DateTimeValue.local`, `utc`, `withOffset(date, time, offsetSeconds)`                              | An offset is normalized to UTC on construction and not kept, so `withOffset(...).isUtc` is true. An offset of a day or more throws.                                         |
| `DateTime::date`, `time`, `is_coordinated`, `FromStr`, `Display`                                          | `date`, `time`, `isUtc`, `DateTimeValue.parse`, `toString()`, `equals(other)`                      | A trailing `Z` or any offset parses to a UTC value; no offset parses to a local value.                                                                                      |
| `BundleIdentity::new`, `id`, `version`                                                                    | `new BundleIdentity(id, version)`, `id`, `version`, `equals(other)`                                |                                                                                                                                                                             |
| `SkillSpec::new`, `name`, `path`, `executables`                                                           | `new SkillSpec(name, path, executables?)`, `name`, `path`, `executables`                           | The name must match the last path segment.                                                                                                                                  |
| `BundleEntry::new`, `directory`, `file`, `executable_file`                                                | `directoryEntry(path)`, `fileEntry(path, data, { executable? })`, or a `BundleEntry` literal       | Validated where consumed, as in Go; `fileEntry` copies `data`.                                                                                                              |
| `BundleEntry::path`, `data`, `executable`, `is_directory`                                                 | `path`, `data` (`Uint8Array \| null`), `executable`, `kind: 'file' \| 'directory'`                 |                                                                                                                                                                             |
| `OperationStatus` and `as_str`                                                                            | `OperationStatus`                                                                                  | See [Enumerations](#enumerations).                                                                                                                                          |

```ts
interface BundleEntry {
  readonly path: string;
  readonly kind: 'file' | 'directory';
  readonly data: Uint8Array | null;
  readonly executable: boolean;
}
```

### Bundles

| Rust                                                                                                         | TypeScript                                                                                        | Notes                                                                                                                       |
| ------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `Bundle::from_directory`                                                                                     | `Bundle.fromDirectory(root, { signal? }): Promise<Bundle>`                                        | `root` must be absolute. Read-only: an abort rejects with `signal.reason` while the native work finishes in the background. |
| `Bundle::from_zip(path, resource)`                                                                           | `Bundle.fromZip(path, resource, { signal? }): Promise<Bundle>`                                    | `path` must be absolute; `resource` is the member directory inside the archive.                                             |
| `Bundle::from_memory`                                                                                        | `Bundle.fromMemory(manifest, entries): Bundle`                                                    | `manifest` is the manifest's JSON bytes.                                                                                    |
| `Bundle::from_memory_value`                                                                                  | `Bundle.fromMemoryValue(manifest, entries): Bundle`                                               | `manifest` is a `SemanticValue`.                                                                                            |
| `Bundle::from_artifacts`                                                                                     | `Bundle.fromArtifacts(identity, artifacts, assets?, dependencies?): Bundle`                       | Rust's checks (unique ids, dependency order, asset references) throw `BundleError` with `kind: 'invalid'`.                  |
| `Bundle::identity`, `id`, `version`, `skills`, `entries`, `content_digest`, `source_roots`, `schema_version` | `identity`, `id`, `version`, `skills`, `entries`, `contentDigest`, `sourceRoots`, `schemaVersion` | Frozen properties read once at construction. `contentDigest` is 64 lowercase hex characters. `schemaVersion` is a `number`. |
| `Bundle::artifacts`, `assets`, `dependencies`                                                                | `artifacts`, `assets`, `dependencies`                                                             | Frozen arrays of real class instances, rebuilt from the native bundle on first access and then cached.                      |
| `BundleError::Invalid`, `Io`                                                                                 | `BundleError` with `kind` `'invalid'` or `'io'` and `errno`                                       | A missing directory rejects with `kind: 'io'` and `errno: 2`.                                                               |

Not bound: the `Ord`, `PartialOrd` and `Hash` derives on `Date`, `Time`, `DateTime`, `BundleIdentity`, `SkillSpec` and `BundleEntry` (TypeScript exposes `equals` only), `ValueError::message()` (the error's `message`), and `BundleError`'s `From` impls and `source()`.

### Artifacts, content and dependencies

Every class below validates in its constructor by building the Rust value
natively, so a refusal is a `ValueError` carrying Rust's text, and the fields
it keeps are Rust's canonical form: a `Command` sorts its environment by name,
a `HookArtifact` sorts its outcomes and tools, and a `TreeContent` adds the
implied directories and sorts its entries by path bytes. Instances are frozen.
Sum types are discriminated by `kind` (`Artifact`, `Content`, `SelectorPart`,
`ClaimSelector`) or by `string` versus class (`CommandArgument`,
`EnvironmentValue`); `McpTransport` is `Command | HttpTransport`. Content that
holds a `SemanticValue` keeps the value you passed.

| Rust                                                                                                         | TypeScript                                                                                     | Notes                                                                                                                                                                                          |
| ------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Family`, `HookEvent`, `HookOutcome`, `DocumentFormat`, `FileMode`, `Position` and their `as_str`, `FromStr` | string-literal unions                                                                          | See [Enumerations](#enumerations); an unknown string throws Rust's `'x' is not a valid Family` text from the constructor that receives it.                                                     |
| `Audience::new`, `agent`, `scope`, `surface`, `platform`                                                     | `new Audience(agent, scope, surface, platform?)`, same properties                              | `directory` scope and a surface that does not belong to the agent are refused.                                                                                                                 |
| `EnvRef::new`, `name`                                                                                        | `new EnvRef(name)`, `name`, `equals(other)`                                                    |                                                                                                                                                                                                |
| `AssetRef::new`, `with_path`, `asset_id`, `path`                                                             | `new AssetRef(assetId, path?)`, `assetId`, `path`                                              | `path` is `null` when absent.                                                                                                                                                                  |
| `CommandArgument` (`Text`, `Asset`)                                                                          | `string \| AssetRef`                                                                           |                                                                                                                                                                                                |
| `EnvironmentValue` (`Text`, `Reference`)                                                                     | `string \| EnvRef`                                                                             |                                                                                                                                                                                                |
| `Command::new`, `argv`, `env`, `cwd`                                                                         | `new Command(argv, env?, cwd?)`, `argv`, `env`, `cwd`                                          | `env` is a `Readonly<Record<string, EnvironmentValue>>`. An empty `argv` throws `argv requires a command`.                                                                                     |
| `HttpTransport::new`, `url`, `bearer_token`                                                                  | `new HttpTransport(url, bearerToken?)`, `url`, `bearerToken`                                   |                                                                                                                                                                                                |
| `McpTransport`                                                                                               | `Command \| HttpTransport`                                                                     |                                                                                                                                                                                                |
| `SkillArtifact::new`, `id`, `name`, `content`                                                                | `new SkillArtifact(id, name, content)`, `kind: 'skill'`                                        | The tree must contain a `SKILL.md` file.                                                                                                                                                       |
| `InstructionArtifact::new`, `id`, `text`                                                                     | `new InstructionArtifact(id, text)`, `kind: 'instruction'`                                     |                                                                                                                                                                                                |
| `McpArtifact::new`, `id`, `name`, `transport`                                                                | `new McpArtifact(id, name, transport)`, `kind: 'mcp'`                                          |                                                                                                                                                                                                |
| `HookArtifact::new`, `id`, `event`, `command`, `timeout_ms`, `outcomes`, `tools`, `protocol_version`         | `new HookArtifact(init)`, `kind: 'hook'`, `timeoutMs`, `protocolVersion`                       | `init.tools` accepts `undefined` or `null`; the property is always an array. A non-integer `timeoutMs` throws the crate's range message, `hook timeout must be 1-300000 integer milliseconds`. |
| `NativeArtifact::new`, `id`, `family`, `audience`, `destination`, `content`                                  | `new NativeArtifact(id, family, audience, destination, content)`, `kind: 'native'`             |                                                                                                                                                                                                |
| `SupportAsset::new`, `id`, `family`, `content`                                                               | `new SupportAsset(id, family, content)`                                                        |                                                                                                                                                                                                |
| `Artifact`                                                                                                   | `Artifact`; `Artifact::id` is `id`                                                             |                                                                                                                                                                                                |
| `FileContent::new`, `data`, `mode`                                                                           | `new FileContent(data, mode)`, `kind: 'file'`                                                  | `data` is copied.                                                                                                                                                                              |
| `TreeContent::new`, `entries`; `snapshot_entries`                                                            | `new TreeContent(entries)`, `kind: 'tree'`                                                     | `snapshot_entries` runs inside the constructor.                                                                                                                                                |
| `Key::new`, `name`                                                                                           | `new Key(name)`, `kind: 'key'`                                                                 |                                                                                                                                                                                                |
| `Member::new`, `identity`, `key`                                                                             | `new Member(identity, key)`, `kind: 'member'`                                                  |                                                                                                                                                                                                |
| `SelectorPart`                                                                                               | `Key \| Member`                                                                                |                                                                                                                                                                                                |
| `Selector::new`, `parts`                                                                                     | `new Selector(parts)`, `kind: 'structured'`                                                    | An empty selector is refused.                                                                                                                                                                  |
| `Placement::new`, `Default`, `position`, `anchor`                                                            | `new Placement(position = 'last', anchor = null)`                                              | `before` and `after` require an anchor.                                                                                                                                                        |
| `SectionBoundaries::new`, `from_marker`, `start`, `end`                                                      | `new SectionBoundaries(start, end)`, `SectionBoundaries.fromMarker(marker)`, `kind: 'section'` |                                                                                                                                                                                                |
| `SectionContent::new`, `marker`, `text`, `boundaries`, `selector`                                            | `new SectionContent(marker, text, boundaries?)`, `kind: 'section'`, `selector`                 | `selector` is a property read from the native value.                                                                                                                                           |
| `StructuredContent::new`, `format`, `selector`, `value`, `placement`                                         | `new StructuredContent(format, selector, value, placement?)`, `kind: 'structured'`             |                                                                                                                                                                                                |
| `DocumentSchema::new`, `format`, `key`, `value`, `content`                                                   | `new DocumentSchema(format, key, value)`, `content()`                                          | A non-scalar value throws the binding text `document schema values must be scalars`.                                                                                                           |
| `Content`                                                                                                    | `Content`                                                                                      |                                                                                                                                                                                                |
| `Dependency::new`, `with_mode`, `dependent`, `required`, `mode`                                              | `new Dependency(dependent, required, mode = 'revision')`                                       |                                                                                                                                                                                                |
| `DependencyMode`                                                                                             | `DependencyMode`                                                                               |                                                                                                                                                                                                |
| `dependency_order`                                                                                           | `dependencyOrder(names, dependencies): readonly string[]`                                      | A cycle, a duplicate edge or an unknown name throws `ValueError`.                                                                                                                              |

Not bound: the `Eq`, `PartialEq`, `Ord`, `PartialOrd` and `Hash` derives (except `Claim` equality), `ModelValue` and
`model_value` (an internal record encoding), and the `From` impls on
`CommandArgument` and `EnvironmentValue`.

### Claims and authority

| Rust                                                          | TypeScript                                                                                      | Notes                                                                                       |
| ------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| `Ownership` and `as_str`, `FromStr`                           | `Ownership`                                                                                     | See [Enumerations](#enumerations).                                                          |
| `ClaimSelector` (`Whole`, `Subtree`, `Section`, `Structured`) | `ClaimSelector`: `WHOLE`, `{ kind: 'subtree', path }`, `SectionBoundaries`, `Selector`          | A `section` claim given a `subtree` selector converts the marker to boundaries, as in Rust. |
| `Claim::new`, `kind`, `selector`                              | `new Claim(kind, selector)`, `kind`, `selector`                                                 | A selector that does not fit the kind throws Rust's text.                                   |
| `Claim::overlaps`                                             | `claim.overlaps(other): boolean`                                                                |                                                                                             |
| `Claim::identity(authority)`                                  | `claim.identity(authority): string`                                                             | Runs on a 16 MiB native stack.                                                              |
| `Claim: Eq`                                                   | `claim.equals(other): boolean`                                                                  |                                                                                             |
| `ResourceAuthority::new`, `destination`                       | `ResourceAuthority.create(destination, { signal? }): Promise<ResourceAuthority>`, `destination` | Read-only filesystem work on the worker pool; `destination` must be absolute.               |
| `canonical_destination`                                       | `canonicalDestination(path, { signal? }): Promise<string>`                                      | A symlinked ancestor rejects with `AuthorityError` kind `'unsafe'`.                         |
| `transaction_path`                                            | `transactionPath(destination, transactionId): string`                                           | Synchronous and pure.                                                                       |
| `AuthorityError`                                              | `AuthorityError`                                                                                | See [Errors](#errors).                                                                      |

Not bound: `AuthorityError::message` and `path` (the error's `message` and
`path`), the `From` impls, and the hidden interop helpers `admit` and
`security_capture` (interop testing only).

### Targets and render contexts

| Rust                                                                                                                                                                                           | TypeScript                                                                                             | Notes                                                                                                                                                                                                         |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Target::directory`                                                                                                                                                                            | `Target.directory(root)`                                                                               | `root` must be absolute. A filesystem root throws `a filesystem root cannot be a skill container`.                                                                                                            |
| `Target::project(agent, root)`                                                                                                                                                                 | `Target.project(agent, root)`                                                                          | `root` must be absolute. `Target.project('codex', root).root` is `<root>/.agents/skills`.                                                                                                                     |
| `Target::user_with_home`, `user_with_home_and_environment`                                                                                                                                     | `Target.user(agent, home, environment?)`                                                               | `home` is required and absolute. Omitting `environment` calls `user_with_home`; passing a record calls `user_with_home_and_environment`.                                                                      |
| `Target::user`, `user_with_environment`                                                                                                                                                        | not bound                                                                                              | Rust reads the process environment and the account database for the first, and derives the home from `HOME` or `USERPROFILE` for the second. The binding never reads either; Go requires `home` the same way. |
| `Target::root`, `agent`, `scope`, `home`, `environment`                                                                                                                                        | `root`, `agent`, `scope`, `home`, `environment`                                                        | Frozen properties. `agent` and `home` are `null` when absent; `environment` is a frozen record of the retained variables.                                                                                     |
| `Target: Eq`                                                                                                                                                                                   | `target.equals(other)`                                                                                 |                                                                                                                                                                                                               |
| `HookRuntime::new`, `node`, `shell`, `resources`; `HookShell`                                                                                                                                  | `new HookRuntime(options?: HookRuntimeOptions)` with `{ node?, shell? }`, `node`, `shell`, `resources` | `node` must be absolute when given. `resources` are the embedded `runner.mts`, `bridge.mts` and `process.mts` entries; each read returns fresh copies.                                                        |
| `RenderContext::new`, `with_surface`, `with_*_root`, `with_*_path`, `with_hook_runtime`                                                                                                        | `new RenderContext(target, platform, options?)` with `RenderOptions`                                   | A directory target throws `render context requires a project or user agent Target`. Every option path must be absolute.                                                                                       |
| `RenderContext::target`, `platform`, `surface`, `root`, `audience`, `skills`, `execution_root`, `instruction_path`, `mcp_path`, `native_root`, `asset_root`, `hook_runtime`, `routing_context` | same names as frozen properties                                                                        | `surface` defaults to `'cli'`. `routingContext` is a frozen record.                                                                                                                                           |
| `instruction_destination`, `mcp_destination`                                                                                                                                                   | `instructionDestination(context)`, `mcpDestination(context)`: `string \| null`                         | `null` when the agent and context fix no path; a relative `CODEX_HOME`-style variable throws `ValueError`.                                                                                                    |
| `Capability`, `capabilities`                                                                                                                                                                   | `Capability`, `capabilities(context)`                                                                  | One entry per family, in `skills`, `instructions`, `mcp`, `hooks` order.                                                                                                                                      |
| `TRANSLATABLE_AGENTS`, `is_translatable`, `untranslatable_message`, `require_translatable`                                                                                                     | `TRANSLATABLE_AGENTS`, `isTranslatable`, `untranslatableMessage`, `requireTranslatable`                | `requireTranslatable` throws `UnsupportedTranslationError` carrying Rust's code (`agent-unsupported`).                                                                                                        |
| `UnsupportedTranslation::code`, `message`                                                                                                                                                      | `UnsupportedTranslationError` `code`, `message`                                                        | See [Errors](#errors).                                                                                                                                                                                        |

Not bound: `Capability::new` (`Capability` is a plain object returned by
`capabilities`, never constructed in JavaScript) and `UnsupportedTranslation::new`
and its `Display` impl (the error's `message` carries the same text).

`RenderContext.installation` is described with the installation targets below.

### Rendering and translation

Every call in this section is synchronous and pure: it performs no filesystem
access, and `render`, `renderMany` and the translations run on the 16 MiB
native stack. See [translation](translation.md) for the destination and notice
rules.

| Rust                                                         | TypeScript                                                                                                                | Notes                                                                                                                                                  |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `Notice::new`, `with_code`, `NoticeKind`                     | `Notice`: `{ artifactId, kind, message, code }`                                                                           | A plain frozen object; `code` is `null` when the crate's code is empty.                                                                                |
| `RenderedArtifact::new` and accessors                        | `new RenderedArtifact(init)`, same properties                                                                             | `init.subtree`, `requireExisting` and `schemaRequirements` are optional. `destination` is validated natively, so a relative one throws Rust's text.    |
| `RenderedBundle::new`, `with_notices`, `empty` and accessors | `new RenderedBundle(artifacts, dependencies?, notices?, routingContext?)`, `RenderedBundle.empty()`                       | The constructor sorts artifacts by id and dependencies by edge, and computes `contentDigest`. `supported` is `false` when any notice is `unsupported`. |
| `render`, `render_many`                                      | `render(bundle, context)`, `renderMany(bundle, contexts)`                                                                 | `renderMany([])` throws `render_many requires at least one explicit context`.                                                                          |
| `ArtifactTranslation::artifact`, `notices`, `supported`      | `ArtifactTranslation`: `artifact`, `notices`, `supported`                                                                 | Returned by the translation functions; not constructible.                                                                                              |
| `translate_instruction_artifact`, `translate_mcp_artifact`   | `translateInstructionArtifact(bundleId, artifact, context)`, `translateMcpArtifact(artifact, context, assetDestinations)` | `assetDestinations` maps an asset id to an absolute revision directory.                                                                                |
| `assemble_translations`                                      | `assembleTranslations(translations, dependencies, contexts, routingContext)`                                              | Dependency errors throw `ValueError` with Rust's text.                                                                                                 |

Not bound: `RenderedArtifact::with_id` and `mcp_translation::literal` (crate
internals), the `ModelValue` encodings, and the `Eq` and `Hash` derives.

### Document editing

`editDocument` is synchronous and pure: it never touches the filesystem and runs on the
16 MiB native stack. Every model below is validated natively on construction, so an
invalid combination throws `ValueError` with Rust's text, and each is a frozen handle
class that can be fed back into the next `editDocument` call. Byte accessors return
fresh copies.

| Rust                                                               | TypeScript                                                                                                                                                            | Notes                                                                                                         |
| ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `Disposition`, `Acquisition`, `EditStatus` and `as_str`, `FromStr` | `Disposition`, `Acquisition`, `EditStatus`                                                                                                                            | See [Enumerations](#enumerations).                                                                            |
| `SnapshotValue` (`Bytes`, `Typed`)                                 | `SnapshotValue`: `{ kind: 'bytes', bytes }` or `{ kind: 'typed', value }`                                                                                             | Dispatch is on `kind`.                                                                                        |
| `ArrayPosition::new`, `previous`, `following`, `prefix`            | `new ArrayPosition(previous, following, prefix)`, same properties                                                                                                     | Equal anchors throw `array position anchors must be distinct`.                                                |
| `SelectionSnapshot::new` and accessors                             | `new SelectionSnapshot(init)`, `value`, `syntax`, `ownedSyntax`, `position`, `layout`                                                                                 | `position` is `null` when absent.                                                                             |
| `OwnedSelection::new` and accessors                                | `new OwnedSelection(init)`, `claim`, `format`, `installed`, `disposition`, `baseline`, `separator`                                                                    | `separator` is `null` when empty. Only a takeover carries a baseline.                                         |
| `CreatedContainer::new`, `selector`, `empty_syntax`                | `new CreatedContainer(selector, emptySyntax)`, same properties                                                                                                        |                                                                                                               |
| `DocumentProvenance::new` and accessors                            | `new DocumentProvenance(init?)`, `emptyDocument`, `containers`, `trailingSeparators`, `schemaFields`, `schemaEmptyDocument`                                           | `Default` is `new DocumentProvenance()`. Optional bytes are `null` when absent.                               |
| `DocumentRequest::new`, `set`, `retire` and accessors              | `new DocumentRequest(claim, content, acquisition?)`, `DocumentRequest.set(content, acquisition?)`, `DocumentRequest.retire(claim)`, `claim`, `content`, `acquisition` | `acquisition` defaults to `'conflict'`; retirement has `content === null`.                                    |
| `EditObservation::new` and accessors                               | `EditObservation`: `{ claim, status, current, message }`                                                                                                              | A plain frozen object returned by `editDocument`.                                                             |
| `DocumentEdit` and accessors                                       | `DocumentEdit`: `{ before, after, ownership, provenance, observations, applicable }`                                                                                  | `before` and `after` are `Uint8Array \| null`; `after === null` means the document is deleted.                |
| `edit_document(data, requests, ownership, provenance)`             | `editDocument(source, requests, ownership?, provenance?)`                                                                                                             | `source === null` is an absent document (`None`). `ownership` defaults to `[]`, `provenance` to an empty one. |
| `content_claim`                                                    | `contentClaim(content): Claim`                                                                                                                                        | File and tree content throw `document content must be a section or structured selection`.                     |
| `SharedContent`                                                    | `SharedContent` (an alias of `Content`)                                                                                                                               |                                                                                                               |

Not bound: `EditObservation::new` and `DocumentEdit::new` (the crate constructs both),
`DocumentEdit::applicable` as a method (it is the `applicable` property), and the `Eq`
derives.

### Stores

Only the built-in stores are bound. Store methods return Promises and run on
the worker pool. See the shared [state store contract](../../spec/state-store.md)
for every operation's guarantees.

| Rust                                                            | TypeScript                                                                 | Notes                                                                                                                                                                               |
| --------------------------------------------------------------- | -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `DirectoryStore::new(path)`                                     | `DirectoryStore.open(path, options?)`: `Promise<DirectoryStore>`           | `path` must be absolute. Like Rust, opening does no I/O: a location that is not private to its owner is refused by the first operation, with `StoreError` kind `io`.                |
| `MemoryStore::new()`                                            | `new MemoryStore()`                                                        | Nothing survives the process.                                                                                                                                                       |
| `StateStore::location`, `staging_directory`                     | `location`, `stagingDirectory`                                             | `<path>` and `<path>/staging` for `DirectoryStore`; both `null` for `MemoryStore`.                                                                                                  |
| `StateStore::read`, `keys`                                      | `read(key, options?)`, `keys(prefix, options?)`                            | Read-only: an abort rejects at once. `read` resolves a fresh `Uint8Array` or `null`; `keys` resolves a frozen, sorted array.                                                        |
| `StateStore::write`, `delete`                                   | `write(key, value, expected, options?)`, `delete(key, expected, options?)` | Compare-and-set against `expected` (`null` for an absent record in `write`); they resolve `false` when the stored value differs. The signal is checked only before the call starts. |
| `StateStore` (built-in implementations)                         | `type StateStore = DirectoryStore \| MemoryStore`                          | Any other object passed as a store throws `ValueError("store must be a DirectoryStore or MemoryStore")`.                                                                            |
| `stores::validate_key(key, "state key")`, `(name, "lock name")` | `validateKey(key)`, `validateLockName(name)`                               | Throw `StoreError` kind `invalid-key`, for example `state key must be slash-separated lowercase segments`.                                                                          |
| `StoreError` (`Busy`, `Io`, `InvalidKey`)                       | `StoreError` with `kind` `'busy' \| 'io' \| 'invalid-key'`                 | `lockName` is set for `busy`; `path` and `errno` for `io`. Store methods throw it; lifecycle calls convert it into a `TargetError` (see [Errors](#errors)).                         |

Not bound: `StateStore::lock` and `StoreLock` (a borrowed lock guard cannot
cross the asynchronous boundary), stores implemented in JavaScript, and
`testing::check_store`.

### Installation targets, lifecycle and summaries

Previews and inspections are read-only: an abort rejects at once and the
native work finishes in the background. `applyPreview`, `sync`,
`recoverInstallation` and `remove` are mutations: they reject with
`signal.reason` only when Rust reports `Err(Cancelled)`, and otherwise resolve
with the real result. Every mutation takes `lockTimeoutMs` (default 0, one
attempt per lock). See [lifecycle](lifecycle.md) for the full semantics.

| Rust                                                                        | TypeScript                                                                                                                                                             | Notes                                                                                                                                                                                                                                         |
| --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `InstallationTarget::new`, `without_context`                                | `new InstallationTarget(store, context?, routingContext?)`                                                                                                             | Both records default to empty (`undefined` or `null`). Duplicate keys cannot occur in a record; the crate sorts the entries.                                                                                                                  |
| `InstallationTarget::store`, `context`, `routing_context`, `context_digest` | `store`, `context`, `routingContext`, `contextDigest`                                                                                                                  | `store` is the store object passed in. The records are frozen.                                                                                                                                                                                |
| `InstallationTarget: Eq`                                                    | `target.equals(other)`                                                                                                                                                 | True only for the same store object (Rust's `Arc` pointer equality) and equal contexts.                                                                                                                                                       |
| `RenderContext::installation(store, context)`                               | `context.installation(store, context?)`                                                                                                                                | Omitting `context` (or passing `null`) uses the stable `agent`, `scope`, `surface` and `root` routing entries.                                                                                                                                |
| `ResourceKind`, `ResourceRef::destination`, `kind`                          | `ResourceKind`, `ResourceRef`: `{ destination, kind }`                                                                                                                 | A plain frozen object.                                                                                                                                                                                                                        |
| `ResourcePlan` accessors                                                    | `ResourcePlan`: `{ resource, transactionDirectory, recoveryPaths, requireExisting, error }`                                                                            | The captured revisions and receipts stay native.                                                                                                                                                                                              |
| `LifecyclePreview` accessors                                                | `LifecyclePreview`: `bundleId`, `bundle`, `rendered`, `target`, `acquisition`, `replaceModified`, `resources`, `error`, `applicable`                                   | Not constructible. It holds the native proposal, so `applyPreview` applies exactly what was captured. `bundle`, `rendered` and `target` are the objects passed to `preview`; a removal preview has `bundle === null` and an empty `rendered`. |
| `preview`, `preview_removal`                                                | `preview(bundle, rendered, target, options?)`, `previewRemoval(bundleId, target, options?)`                                                                            | `options.acquisition` defaults to `'conflict'`, `options.replaceModified` to `false`.                                                                                                                                                         |
| `apply_preview`, `sync`, `recover_installation`, `remove`                   | `applyPreview(preview, options?)`, `sync(bundle, rendered, target, options?)`, `recoverInstallation(bundleId, target, options?)`, `remove(bundleId, target, options?)` | Resolve an `InstallationResult`. `SyncOptions` adds `acquisition` and `replaceModified`; `RemoveOptions` adds `replaceModified`.                                                                                                              |
| `inspect_installation`                                                      | `inspectInstallation(bundleId, target, options?)`: `Promise<InstallationObservation>`                                                                                  |                                                                                                                                                                                                                                               |
| `ClaimObservation`, `ResourceObservation` accessors                         | `ClaimObservation`, `ResourceObservation`                                                                                                                              | Plain frozen objects. `revisionDigest` is `null` when the resource could not be observed.                                                                                                                                                     |
| `InstallationObservation` accessors, `matches`                              | `InstallationObservation`: `bundleId`, `target`, `resources`, `isCurrent`, `pending`, `version`, `bundleDigest`, `renderDigest`, `error`; `matches(bundle, rendered)`  | `target` is the caller's `InstallationTarget`. `matches` is Rust's comparison against the captured observation.                                                                                                                               |
| `ResourceResult`, `InstallationResult` accessors                            | `ResourceResult`, `InstallationResult`: `{ status, observation, resources, notices, error, succeeded }`                                                                | `succeeded` is `true` exactly for `applied` and `unchanged`.                                                                                                                                                                                  |
| `ChangeAction`, `PlannedChange`, `PlanSummary`, `summarize`                 | `ChangeAction`, `PlannedChange`, `PlanSummary`, `summarize(preview)`                                                                                                   | `summarize` is synchronous. `agent`, `scope` and `surface` are `null` when the routing context does not name them.                                                                                                                            |

Not bound: `ResourceRef::new` and `ResourceRef::schema_version`, `PlannedChange::new` and `PlanSummary::new` (the crate
constructs them), `ResourcePlan::ancestors`, `ResourceKind: FromStr`, and the durable state records
(`Index`, `Receipt`, `Preparation`, `Journal` and their parts), as in Go;
`LifecyclePreview::index` and `generation`; and the `Eq` derives on results.

### Observations

Skill observations are plain frozen objects. Optional values are `null`, never
`undefined`. `InventoryEntry.size` is a `bigint` (or `null` for directories).

| Rust                                  | TypeScript                                                                                                                                                              | Notes                                                                                                                               |
| ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `InventoryEntry` accessors            | `InventoryEntry`: `{ path, directory, size, sha256, executable }`                                                                                                       | `directory` is `is_directory()`.                                                                                                    |
| `Installation` accessors              | `Installation`: `{ bundleId, version, contentDigest, transactionId, entries }`                                                                                          |                                                                                                                                     |
| `Modification`, `ModificationKind`    | `Modification`: `{ bundleId, path, kind }`                                                                                                                              | `path` names the owned skill.                                                                                                       |
| `Conflict`                            | `Conflict`: `{ path, owner }`                                                                                                                                           |                                                                                                                                     |
| `Observation` accessors, `is_current` | `Observation`: `{ state, installed, installations, versionMatches, recordedContentMatches, contentMatches, modifications, conflicts, error, recoveryPaths, isCurrent }` | The three `Option<bool>` checks are `null` when they could not be made.                                                             |
| `TargetInspection<T>`                 | `TargetInspection`: `{ target, root, aliasOf, observation }`                                                                                                            | `target` is the caller's `Target` at that index. `aliasOf` is `null` or the index of the first target with the same canonical root. |
| `TargetResult<T>`                     | `TargetResult`: `{ target, root, aliasOf, status, observation, error, recoveryPaths, succeeded }`                                                                       | `succeeded` is true exactly for `applied` and `unchanged`.                                                                          |

Not bound: the constructors of these types (the crate constructs them).

### Skills and inspection

`SkillTarget` is bound as `Target`; a plain path is `Target.directory(root)`.
`install`, `update` and `uninstall` are mutations (see
[lifecycle](lifecycle.md)); `inspect` and `renderSkills` are read-only. Results
come back in target order, and independent target failures stay data.

| Rust                                  | TypeScript                                                                          | Notes                                                           |
| ------------------------------------- | ----------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| `render_skills`                       | `renderSkills(bundle, target, options?)`: `Promise<RenderedBundle>`                 | Rejects with `ValueError` for a bundle that is not skills-only. |
| `inspect`                             | `inspect(bundle, targets, store, options?)`: `Promise<readonly TargetInspection[]>` | See [inspection](inspection.md).                                |
| `install`, `install_with_timeout`     | `install(bundle, targets, store, options?)`                                         | `options.lockTimeoutMs`; `signal`.                              |
| `update`, `update_with_options`       | `update(bundle, targets, store, options?)`                                          | Adds `replaceModified`.                                         |
| `uninstall`, `uninstall_with_options` | `uninstall(bundleId, targets, store, options?)`                                     | Adds `replaceModified`.                                         |
| `SkillOptions`                        | `SkillOptions`: `{ replaceModified?, lockTimeoutMs?, signal? }`                     |                                                                 |

### Agent discovery

`detectAgents(options)` takes the search path explicitly and never reads
`process.env`. `searchPath` is an array of absolute directories (no empty
entries, none containing `:`); `[]` searches nothing. Application directories
default to `/Applications` plus `<home>/Applications` when `home` is given;
`applications` replaces that list and `noApplications: true` clears it.

| Rust                                | TypeScript                                                                                                          | Notes                                                                            |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| `detect_agents(path, applications)` | `detectAgents({ searchPath, home?, applications?, noApplications?, signal? })`: `Promise<readonly AgentPresence[]>` | Always passes `Some(..)` for both Rust arguments. Results are in `AGENTS` order. |
| `AgentPresence` accessors           | `AgentPresence`: `{ agent, executable, application }`                                                               | At least one of the two paths is non-null.                                       |

Not bound: `AgentPresence::new`.

### Hooks

The protocol types are synchronous and immutable. Every protocol failure throws
`HookError` with Rust's `HookFailure` code; an invalid argument (an unknown
event, a handler cwd that is not absolute) throws `ValueError`. Hook text crosses
the native boundary as UTF-16 code units, so `HookJson.string('\uD800')` keeps its
lone surrogate and `text` returns it unchanged. Integers are `bigint`; the
protocol stores them as binary64 numbers, so values beyond 2^53 round the way
JSON numbers do. Parsing runs on the large native stack.

| Rust                                                                                                                      | TypeScript                                                                                                                                                   | Notes                                                                                              |
| ------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------- |
| `INPUT_LIMIT`, `OUTPUT_LIMIT`, `ERROR_LIMIT`, `TEXT_LIMIT`, `JSON_DEPTH_LIMIT`, `HANDLER_LIMIT`, `EVENT_TIMEOUT_LIMIT_MS` | `HOOK_INPUT_LIMIT`, `HOOK_OUTPUT_LIMIT`, `HOOK_ERROR_LIMIT`, `HOOK_TEXT_LIMIT`, `HOOK_JSON_DEPTH_LIMIT`, `HOOK_HANDLER_LIMIT`, `HOOK_EVENT_TIMEOUT_LIMIT_MS` | Numbers read from native when the module loads.                                                    |
| `HookJson::parse`, `object`, `string`, `boolean`, `integer`, `array`, `null`                                              | `HookJson.parse(bytes)`, `.object(members)`, `.string(text)`, `.boolean(v)`, `.integer(bigint)`, `.array(items)`, `.null()`                                  | Object members keep their order. Keys must be well-formed text.                                    |
| `HookJson::is_object`, `as_text`, `to_bytes`                                                                              | `isObject`, `text` (`string \| null`), `bytes()`                                                                                                             | `bytes()` returns a fresh `Uint8Array`.                                                            |
| `HookRequest::new_text`, `parse`, accessors, `to_bytes`                                                                   | `new HookRequest({ event, context, tool?, input?, result? })`, `HookRequest.parse(bytes)`, `bytes()`                                                         | `context` must be an object; tool events need a tool.                                              |
| `HookResponse::continuing`, `block`, `ask`, `context`, `modify_input`, `parse`                                            | `HookResponse.continuing()`, `.block(reason)`, `.ask(reason)`, `.context(text)`, `.modifyInput(event, input)`, `.parse(bytes, event, declared)`              | `outcome`, `value`, `bytes()`. An undeclared outcome throws `HookError` code `undeclared-outcome`. |
| `HookEffects::parse`, `Default`, accessors, `to_bytes`                                                                    | `HookEffects.parse(bytes)`, `HookEffects.none()`, `input`, `contexts`, `decision`, `reason`, `bytes()`                                                       |                                                                                                    |
| `HookEnvironment`                                                                                                         | `EnvironmentValue` (`string \| EnvRef`)                                                                                                                      | Reuses the artifact type.                                                                          |
| `HookHandler::new`, accessors                                                                                             | `new HookHandler({ id, event, argv, environment?, cwd, timeoutMs, outcomes, tools? })`                                                                       | Outcomes, tools and environment come back sorted. A bad timeout throws the crate's range text.     |
| `HookConfig::new`, `host`, `handlers`, `to_bytes`                                                                         | `new HookConfig(host, handlers)`, `host`, `handlers`, `bytes()`                                                                                              | Handlers are ordered by id.                                                                        |

Not bound: `HookText` as a type (text crosses as `Uint16Array` and `string`), `HookJson::as_str` (use `text`), `HookRequest::new` (only `new_text` is mapped), `HookFailure::new`, `code`, `Display` and `Error` (a failure surfaces as `HookError` with its `code`), and the `HookEnvironment` enum itself (its `Text` and `Reference` variants map to `EnvironmentValue`).

See [hooks](hooks.md) for the runtime and an example.

### Global Loadout

Discovery, selection, preview, apply, inspection and recovery of a Global Loadout
source installation. A discovery is a tagged result (`absent`, `available`,
`invalid`); an invalid registration is data (`LoadoutDiagnostic`, a `FlyrailError`
carrying `code` and `path`), never a throw. `discoverLoadout` takes `home`
(absolute), an optional `configHome` (absolute) and an explicit `environment`
record; the library reads no process environment. A relative path or a malformed
string throws `ValueError`. `LoadoutSetup` and `LoadoutDestination` hold engine
state behind a native handle, so `select` re-validates natively and an unknown
source or an empty or repeated harness list throws `ValueError` with Rust's text.
`previewLoadout` plans an installation for a `LoadoutDestination`, and `previewLoadoutRemoval` plans a source-free removal from the `LoadoutLocation` (`root`, `profile`, `sourceName`, `sourcePath`) of the installation. A `LoadoutPreview` carries its `LifecyclePreview`, so `summarize(preview.lifecycle)`
works. `applyLoadout` and `recoverLoadout` take `signal` with the mutation
semantics in [lifecycle](lifecycle.md); discovery, previews and `inspectLoadout`
take `signal` with the read-only semantics. `syncRequired` is true unless the
lifecycle status is `failed`: the host runs Loadout sync. See [loadout](loadout.md).

Not bound: `LoadoutOptions::new` (the options object supplies `platform` and
defaults the rest), the `Debug`, `Clone` and `Eq` derives, and `LoadoutDiscovery::Available`'s
`Box` (the setup is one shared handle).
