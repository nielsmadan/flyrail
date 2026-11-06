# Public Go API

[Packages](#packages) · [Conventions](#conventions) · [Merged and split functions](#merged-and-split-functions) · [Crosswalk](#crosswalk) · [Unexported in Go](#unexported-in-go)

Start with the [README quickstart](../README.md); this page is the reference.
It lists every exported item of the three packages, with the Rust item each one
corresponds to, so a reader who knows the Rust crate or the shared specifications
can find the Go name, and a Go reader can see where the shape departs from the
other implementations. Behaviour, durable formats and cross-language error text
follow Rust; this document fixes the shape of the Go surface. Signatures here
are checked against `go doc` for the packages.

## Packages

| Package | Import path | Holds |
| --- | --- | --- |
| `flyrail` | `github.com/nielsmadan/flyrail/go/flyrail` | Every public type and all domain logic: values, bundles, artifacts, rendering, editing, claims, observations, lifecycle, skills, discovery, summaries. |
| `store` | `github.com/nielsmadan/flyrail/go/flyrail/store` | The `Store` interface, its errors, `DirectoryStore` and `MemoryStore`. Imports nothing from `flyrail`. |
| `storetest` | `github.com/nielsmadan/flyrail/go/flyrail/store/storetest` | The conformance kit a custom store runs from its own tests. |

## Conventions

These rules apply to every row below; a row states only where it departs from them.

**Errors.** Fallible calls return `(T, error)`. Error values are exported types
usable with `errors.As`, or sentinels usable with `errors.Is`; a function never
returns a typed-nil pointer as an `error`. Construction and validation fail with
`*ValueError`, source loading with `*BundleError`, path authority with
`*AuthorityError`, hook protocol parsing with `*HookError`. Expected target
failures (conflicts, I/O, recovery) are not Go errors: they travel as
`*TargetError` inside previews, observations and results, exactly where Rust
places a `TargetError`. Nil or zero-value inputs that Rust's types make
impossible return `*ValueError`; library code never panics. Constructors
without an error result (`NewLocalDateTime`, `NewUTCDateTime`,
`NewCreatedContainer`) accept anything, and the value is validated where it is
consumed (`EncodeValue`, `NewArray`, the editor). Accessors on a nil pointer
receiver return zero values. `*TargetError` fields and `Err()` accessors are
nil when there is no error: never assign one to an `error` variable or return it
as an `error` (typed-nil); every type that carries one also has `Failure() error`,
which is a nil interface when there is no failure; `Error()` and `Unwrap()` on a nil `*TargetError`
return `""` and nil.

**Context.** Every call that touches the filesystem or a store takes
`ctx context.Context` first. The context carries cancellation (and a deadline
that can shorten lock waits) only; Rust has no equivalent parameter. Pure
functions (rendering, editing, summaries, constructors) take no context. A nil
`ctx` is an invalid argument to every function in package `flyrail` that takes
one: it returns a `*ValueError` with the message `context must not be nil`
(`ApplyPreview` reports it in its result instead). `store` implementations
treat a nil `ctx` as `context.Background()`.

**Lock timeouts.** Every `LockTimeout time.Duration` field replaces a Rust
`lock_timeout: Duration` argument. Zero means exactly one acquisition attempt,
as Rust's `Duration::ZERO`; each lock gets its own full timeout.

**Optional values.** Rust `Option<T>` maps by kind:

| Rust | Go parameter or field | Go accessor |
| --- | --- | --- |
| `Option<String>` / `Option<PathBuf>` where the empty string is never valid | `string`, `""` = `None` | `string`, `""` = `None` |
| `Option<Vec<u8>>` (empty and absent differ) | `[]byte` plus a `bool` presence flag | `([]byte, bool)` |
| `Option<T>` of an immutable Flyrail type | `*T`, nil = `None`, the pointee is copied | `(T, bool)` |
| `Option<enum>` | the string enum, `""` = `None` | the enum, `""` = `None` |
| `Option<bool>` in a result | `*bool`, nil = not determined | field |
| `Option<usize>` index in a result | `bool` flag plus `int` (zero value is `None`) | field |
| `Option<TargetError>` | `*TargetError`, nil = `None` | field `Err` or method `Err()` |
| `Option<i32>` errno | `syscall.Errno`, `0` = `None` | field |

**Immutable inputs and snapshots** (`Bundle`, `RenderedBundle`,
`LifecyclePreview`, `InstallationTarget`, `Target`, `RenderContext`, values,
artifacts, content, claims, editor inputs) have unexported fields, a validating
constructor that deep-copies its inputs, and accessors that return deep copies (every returned `[]byte`, and the `Data`
of every returned `BundleEntry`).
Accessor names drop Rust's `as_`/`is_` prefixes except for booleans
(`IsUTC`, `IsObject`) and use `Err()` for Rust's `error()`. Rust's `Clone` is
plain assignment: `*Bundle`, `*RenderedBundle`, `*LifecyclePreview`,
`*InstallationTarget` and `*HookRuntime` are shared by pointer, every other
immutable type is a small value.

**Results and observations** (`InstallationResult`, `InstallationObservation`,
`ResourceObservation`, `ClaimObservation`, `ResourceResult`, `TargetResult`,
`TargetInspection`, `Observation`, `Installation`, `Modification`, `Conflict`,
`InventoryEntry`, `PlanSummary`, `PlannedChange`, `AgentPresence`,
`Capability`, `Notice`, `EditObservation`, `DocumentEdit`) are plain structs
with exported fields owned by the caller. Rust's public constructors for these
types become struct literals; the library validates a value where it consumes
one.

**Constructors with more than five parameters** take a `…Params` struct whose
field comments give each zero value (`HookArtifactParams`, `HookHandlerParams`,
`RenderedArtifactParams`, `OwnedSelectionParams`, `DocumentProvenanceParams`).

**Enums** are string types whose constant values equal Rust's `as_str`
spellings; `string(v)` replaces `as_str`. Each constant carries the prefix
listed in its row (`AgentClaude`, `CodeBusy`, …). Where Rust implements
`FromStr`, Go has `ParseX(s string) (X, error)` returning `*ValueError` with
Rust's text (`'x' is not a valid Agent`). A constructor given an unknown
enum value returns `*ValueError`, so some Rust constructors that cannot fail
gain an `error` result in Go. Hook protocol functions are the exception: they
report every failure as `*HookError`, so an unknown `HookEvent` is
`input-event`, an invalid-UTF-8 `HookObject` key is `json-key`, and
`NewHookConfig` rejects a zero `HookHandler`.

**Sum types** are sealed interfaces with an unexported marker method; only
package `flyrail` implements them. Callers branch with a type switch.

**Context maps.** Rust's `Vec<(String, String)>` routing and identity contexts
(keys unique, sorted on construction) are `map[string]string`. A nil map and an
empty map both mean "no entries", except `RenderContext.Installation`, whose nil
`context` is Rust `None`; `InstallationWithContext` has no such case.

**Paths** are `string`s (any bytes, as Rust's `PathBuf`), absolute where Rust
requires it, made absolute like `std::path::absolute` (no `..` collapsing).
Identifiers, labels, messages and context entries must be valid UTF-8, as
Rust's `String`; constructors reject invalid UTF-8 with `*ValueError`.

**Equality.** Immutable types whose fields are all comparable (`BundleIdentity`,
`EnvRef`, `AssetRef`, `Audience`, `Key`,
`SectionBoundaries`, `Dependency`, `ResourceAuthority`, `ResourceRef`,
`Notice`, `Date`, `Time`, `DateTime`, `Float`, `String`) support `==` and map
keys with Rust's `Eq` semantics. Rust's derived or custom equality on the other
types is exposed only where callers need it: `ValuesEqual`, `Claim.Equal`,
`Target.Equal`, `(*InstallationTarget).Equal`.

**Iteration.** `iter.Seq2` appears only for ordered object members
(`Object.All`). Everything else returns slices.

## Merged and split functions

| Rust | Go | Why |
| --- | --- | --- |
| `Target::user`, `user_with_home`, `user_with_environment`, `user_with_home_and_environment` | `UserTarget(agent, home, lookupEnv)` | One constructor, as Python's `Target.user(agent, home=, env=)`; the library never reads the environment itself. |
| `RenderContext::new`, `with_surface`, `with_execution_root`, `with_instruction_path`, `with_mcp_path`, `with_native_root`, `with_asset_root`, `with_hook_runtime` | `NewRenderContext(target, platform, RenderOptions)` | Builder chain becomes an options struct. |
| `RenderContext::installation(store, None \| Some(context))` | `RenderContext.Installation(st, context)` and `RenderContext.InstallationWithContext(st, context)` | `Installation` with a nil `context` is Rust `None` (the default routing identity) and with a non-nil one, even empty, is `Some`; callers who build the map programmatically should use `InstallationWithContext`, which is always `Some` (a nil or empty `context` is an empty identity). |
| `InstallationTarget::new`, `without_context` | `NewInstallationTarget(st, context, routing)` | `without_context` is `NewInstallationTarget(st, nil, nil)`. |
| `install`, `install_with_timeout` | `Install(ctx, …, LockOptions)` | Zero `LockOptions` is `install`. |
| `update`, `update_with_options` | `Update(ctx, …, SkillOptions)` | Zero `SkillOptions` is `update`. |
| `uninstall`, `uninstall_with_options` | `Uninstall(ctx, …, SkillOptions)` | Zero `SkillOptions` is `uninstall`. |
| `preview(…, acquisition, replace_modified)`, `sync(…, acquisition, replace_modified, lock_timeout)`, `remove(…, replace_modified, lock_timeout)`, `apply_preview(…, lock_timeout)`, `recover_installation(…, lock_timeout)` | `PreviewOptions`, `SyncOptions`, `RemoveOptions`, `LockOptions` | Trailing flags become option structs. `PreviewRemoval` keeps a plain `replaceModified bool`. |
| `Bundle::from_directory`, `from_zip`, `from_memory`, `from_memory_value`, `from_artifacts` | `OpenBundle`, `OpenBundleZip`, `NewBundleFromEntries`, `NewBundleFromValue`, `NewBundle` | Go adds `LoadBundleFS` for `io/fs.FS` sources (for `embed.FS`); it has no Rust counterpart. |
| `BundleEntry::new`, `directory`, `file`, `executable_file` | `BundleEntry{…}` literal | `BundleEntry` is a plain struct, validated where consumed. |
| `Scalar::null`, `boolean`, `integer`, `float`, `text`, `date`, `time`, `datetime`; `Value::Scalar` | `Null{}`, `Bool`, `Int`, `NewFloat`, `NewString`, `Date`, `Time`, `DateTime` | `Value` and `Scalar` flatten into one sealed interface family. |
| `Scalar::as_boolean` … `as_datetime`, `is_null` | type switch on `Value` / `Scalar` | |
| `DateTime::local`, `coordinated`, `with_offset` | `NewLocalDateTime`, `NewUTCDateTime`, `NewDateTimeWithOffset` | Kept split. |
| `AssetRef::new`, `with_path` | `NewAssetRef(assetID, path)` | `path` `""` is `new`. |
| `SectionBoundaries::new`, `from_marker` | `NewSectionBoundaries`, `SectionBoundariesFromMarker` | Kept split. |
| `Dependency::new`, `with_mode` | `NewDependency(dependent, required, mode)` | `mode` `""` is `new` (revision). |
| `Notice::new`, `with_code` | `Notice{…}` literal | `Code` `""` is `new`. |
| `RenderedBundle::new`, `with_notices` | `NewRenderedBundle(artifacts, deps, notices, routing)` | nil `notices` is `new`. |
| `DocumentRequest::new`, `set`, `retire` | `NewDocumentRequest`, `NewSetRequest`, `NewRetireRequest` | Kept split. |
| `HookRequest::new`, `new_text` | `NewHookRequest(event, context, tool *HookText, …)` | A Rust `String` tool is `HookTextFromString`. |
| `HookEnvironment::{Text, Reference}` | `EnvironmentValue` (`EnvText`, `EnvRef`) | Same two cases with the same validation; one Go type serves artifacts and translated handlers. |
| `stores::validate_key(key, label)` | `store.ValidateKey(key)`, `store.ValidateLockName(name)` | The two labels Rust passes become two functions. |
| `StoreError::{Busy, InvalidKey, Io}` | `*store.BusyError`, `*store.KeyError`, `*store.IOError` | Enum variants become separate error types. |
| `AuthorityError::{Invalid, Unsafe, Io}`, `BundleError::{Invalid, Io}` | `*AuthorityError`, `*BundleError` with an `Err` cause | The variant is the cause's type. |
| `SkillTarget` trait over `Target`, `PathBuf`, `&Path` | `Target`; a path is `DirectoryTarget(root)` | No trait; `TargetResult<T>`/`TargetInspection<T>` lose their type parameter. |
| `Agent::ALL`, `TRANSLATABLE_AGENTS` | `Agents()`, `TranslatableAgents()` | Functions returning fresh slices instead of mutable package variables. |
| `testing::check_store(open) -> Result<(), String>` | `storetest.Run(t, open)` | Failures go to `t`. |

## Crosswalk

Each table lists the Rust item, its Go replacement, and the Go signatures.
Package `flyrail` unless a row names `store` or `storetest`.

### Values

```go
type Value interface{ isValue() }
type Scalar interface{ Value; isScalar() }

type Null struct{}
type Bool bool
type Int int64
type Float struct{ /* finite binary64 bits */ }
type String struct{ /* valid UTF-8 */ }
type Array struct{ /* immutable */ }
type Object struct{ /* immutable, members sorted by key bytes */ }
type ObjectMember struct {
	Key   string
	Value Value
}
```

| Rust | Go | Notes |
| --- | --- | --- |
| `Value` (enum `Scalar`/`Array`/`Object`) | `Value` sealed interface | Implemented by the scalar types, `Array`, `Object`. `From<bool/i64/&str/String>` become `Bool(b)`, `Int(i)`, `NewString(s)`. Rust's `Eq` is `func ValuesEqual(a, b Value) bool` (floats compare by bits). |
| `Value::semantic_bytes` | `func EncodeValue(v Value) ([]byte, error)` | Errors only for nil or zero-value input. |
| `Value::from_semantic_bytes` | `func DecodeValue(b []byte) (Value, error)` | |
| `Scalar` | `Scalar` sealed interface | Implemented by `Null`, `Bool`, `Int`, `Float`, `String`, `Date`, `Time`, `DateTime`. |
| `Scalar::float` | `func NewFloat(f float64) (Float, error)`; `func (Float) Float64() float64` | NaN and infinities: `floats must be finite`. |
| `Scalar::text` | `func NewString(s string) (String, error)`; `func (String) String() string` | Rejects invalid UTF-8. |
| `ArrayValue` | `Array`: `func NewArray(items ...Value) (Array, error)`; `Items() []Value`; `Len() int` | Zero `Array` is empty. |
| `ObjectValue` | `Object`: `func NewObject(members ...ObjectMember) (Object, error)`; `All() iter.Seq2[string, Value]`; `Get(key string) (Value, bool)`; `Len() int` | Duplicate keys: `duplicate object key`. Zero `Object` is empty. |
| `ValueError` | `type ValueError struct{ Message string }`; `Error() string` | Unexported fields carry Rust's crate-private Python-`TypeError` and nesting flags. |
| `Date` | `func NewDate(year, month, day int) (Date, error)`; `Year()`, `Month()`, `Day() int`; `String() string`; `func ParseDate(s string) (Date, error)` | Rust `Display`/`FromStr`. |
| `Time` | `func NewTime(hour, minute, second, microsecond int) (Time, error)`; `Hour()`, `Minute()`, `Second()`, `Microsecond() int`; `String()`; `func ParseTime(s string) (Time, error)` | |
| `DateTime` | `func NewLocalDateTime(d Date, t Time) DateTime`; `func NewUTCDateTime(d Date, t Time) DateTime`; `func NewDateTimeWithOffset(d Date, t Time, offsetSeconds int) (DateTime, error)`; `Date() Date`; `Time() Time`; `IsUTC() bool`; `String()`; `func ParseDateTime(s string) (DateTime, error)` | A value is local or UTC, as in Rust: an offset is normalized to UTC on construction and not retained. |

### Bundles and models

```go
type BundleEntry struct {
	Path       string // portable relative path
	Data       []byte // file bytes; nil for a directory
	Dir        bool   // a directory entry; Data must be nil and Executable false
	Executable bool   // declared executable intent; physical modes never set it
}
```

| Rust | Go | Notes |
| --- | --- | --- |
| `Bundle` | `*Bundle` | Accessors: `Identity() BundleIdentity`, `ID() string`, `Version() string`, `Skills() []SkillSpec`, `Entries() []BundleEntry`, `ContentDigest() string`, `SourceRoots() []string`, `Artifacts() []Artifact`, `Assets() []SupportAsset`, `Dependencies() []Dependency`, `SchemaVersion() int`. |
| `Bundle::from_directory` | `func OpenBundle(ctx context.Context, dir string) (*Bundle, error)` | |
| `Bundle::from_zip(path, resource)` | `func OpenBundleZip(ctx context.Context, path, root string) (*Bundle, error)` | `root`: member directory inside the archive (Rust `resource`); it must be a relative path naming that directory, so `""` is refused. |
| — | `func LoadBundleFS(fsys fs.FS) (*Bundle, error)` | Snapshots an `fs.FS` (e.g. `embed.FS`); ignores file modes; no source roots. |
| `Bundle::from_memory` | `func NewBundleFromEntries(manifest []byte, entries []BundleEntry) (*Bundle, error)` | |
| `Bundle::from_memory_value` | `func NewBundleFromValue(manifest Value, entries []BundleEntry) (*Bundle, error)` | |
| `Bundle::from_artifacts` | `func NewBundle(id BundleIdentity, artifacts []Artifact, assets []SupportAsset, deps []Dependency) (*Bundle, error)` | |
| `BundleError` | `type BundleError struct{ Message string; Err error }`; `Error()`; `Unwrap() error` | `Err` is the `*ValueError` (Rust `Invalid`) or the I/O error, usually `*fs.PathError` (Rust `Io`). |
| `BundleIdentity` | `func NewBundleIdentity(id, version string) (BundleIdentity, error)`; `ID()`, `Version() string` | |
| `SkillSpec` | `func NewSkillSpec(name, path string, executables []string) (SkillSpec, error)`; `Name()`, `Path() string`; `Executables() []string` | |
| `BundleEntry` | `BundleEntry` plain struct (above) | Rust `data()` is `Data`; `is_directory()` is `Dir`. |
| `OperationStatus` | `type OperationStatus string`: `StatusApplied`, `StatusUnchanged`, `StatusFailed`, `StatusPartial`, `StatusIncomplete` | No `FromStr` in Rust, no `Parse` in Go. |

### Targets and render contexts

```go
type RenderOptions struct {
	Surface         Surface      // "" = SurfaceCLI
	ExecutionRoot   string       // "" = the context's Root()
	InstructionPath string       // "" = the agent's default instruction destination
	MCPPath         string       // "" = the agent's default MCP destination
	NativeRoot      string       // "" = the context's Root()
	AssetRoot       string       // "" = Root()/.flyrail-assets
	HookRuntime     *HookRuntime // nil = no hook runtime; hooks render unsupported
}
```

| Rust | Go | Notes |
| --- | --- | --- |
| `Agent` | `type Agent string`: `AgentClaude`, `AgentCodex`, `AgentOpencode`, `AgentPi`, `AgentCursor`, `AgentCopilot`, `AgentDroid`; `func ParseAgent(s string) (Agent, error)` | |
| `Agent::ALL` | `func Agents() []Agent` | Rust order. |
| `TargetScope` | `type TargetScope string`: `ScopeDirectory`, `ScopeUser`, `ScopeProject`; `ParseTargetScope` | |
| `Surface` | `type Surface string`: `SurfaceCLI`, `SurfaceIDE`, `SurfaceVSCode`; `ParseSurface` | |
| `Platform` | `type Platform string`: `PlatformMacOS`, `PlatformLinux`, `PlatformWindows`; `ParsePlatform` | |
| `Target` | `Target` | Accessors: `Root() string`, `Agent() Agent` (`""` for a directory target), `Scope() TargetScope`, `Home() string` (`""` = none), `Environment() map[string]string` (retained relocation variables). `Equal(o Target) bool` is Rust's custom `PartialEq` (root, agent, scope, anchor, suffix; not home or environment). |
| `Target::directory` | `func DirectoryTarget(root string) (Target, error)` | Also replaces a Rust `PathBuf`/`&Path` skill target. |
| `Target::project` | `func ProjectTarget(agent Agent, root string) (Target, error)` | |
| `Target::user*` (four functions) | `func UserTarget(agent Agent, home string, lookupEnv func(string) (string, bool)) (Target, error)` | `home` `""`: home comes from `lookupEnv("HOME")`, then `USERPROFILE`, and must be absolute (Rust `user_with_environment`); neither set, or `lookupEnv` nil, fails with `an explicit env requires home, HOME, or USERPROFILE`. `lookupEnv` nil: no relocation variables (Rust `user_with_home`). Rust `Target::user()` differs (empty `HOME` becomes `/`, a relative `HOME` is made absolute, an unset `HOME` falls back to the account database, `USERPROFILE` is never read); Go has no ambient account lookup by design, so a caller wanting that passes `home` (`user.Current().HomeDir`) itself. |
| `RenderContext` | `RenderContext` | Accessors: `Target() Target`, `Platform() Platform`, `Surface() Surface`, `Root() string`, `Audience() Audience`, `Skills() string`, `ExecutionRoot()`, `InstructionPath()`, `MCPPath()`, `NativeRoot()`, `AssetRoot() string` (`""` = not set), `HookRuntime() *HookRuntime` (the exception to the `(T, bool)` rule: shared by pointer, nil = none), `RoutingContext() map[string]string`. |
| `RenderContext::new` and builders | `func NewRenderContext(target Target, platform Platform, opts RenderOptions) (RenderContext, error)` | Relative path options: `render routes must be absolute Paths`. |
| `RenderContext::installation` | `func (RenderContext) Installation(st store.Store, context map[string]string) (*InstallationTarget, error)`; `func (RenderContext) InstallationWithContext(st store.Store, context map[string]string) (*InstallationTarget, error)` | `Installation` with nil `context` uses the `agent`, `scope`, `surface`, `root` routing entries as the identity context (Rust `None`), otherwise `context` as given; `InstallationWithContext` always uses `context` as given, even when nil or empty (Rust `Some`). |
| `Capability` | `type Capability struct{ Family Family; Portable, Native bool; Limitations, Prerequisites []string }` | Plain struct. |
| `capabilities` | `func Capabilities(rc RenderContext) ([]Capability, error)` | |
| `instruction_destination` | `func InstructionDestination(rc RenderContext) (string, error)` | `""` is Rust `Ok(None)`. |
| `mcp_destination` | `func MCPDestination(rc RenderContext) (string, error)` | |
| `TRANSLATABLE_AGENTS` | `func TranslatableAgents() []Agent` | |
| `is_translatable` | `func IsTranslatable(a Agent) bool` | |
| `untranslatable_message` | `func UntranslatableMessage(a Agent) string` | |
| `require_translatable` | `func RequireTranslatable(rc RenderContext) error` | Returns `*UnsupportedTranslationError` or nil. |
| `UnsupportedTranslation` | `type UnsupportedTranslationError struct{ Code, Message string }`; `Error()` | Go error-type naming. |

### Artifacts

| Rust | Go | Notes |
| --- | --- | --- |
| `Family` | `type Family string`: `FamilySkills`, `FamilyInstructions`, `FamilyMCP`, `FamilyHooks`; `ParseFamily` | |
| `Audience` | `func NewAudience(agent Agent, scope TargetScope, surface Surface, platform Platform) (Audience, error)`; `Agent()`, `Scope()`, `Surface()`, `Platform()` | `platform` `""` = none. |
| `EnvRef` | `func NewEnvRef(name string) (EnvRef, error)`; `Name() string` | Implements `EnvironmentValue`. |
| `AssetRef` | `func NewAssetRef(assetID, path string) (AssetRef, error)`; `AssetID() string`; `Path() string` | `path` `""` = whole asset (Rust `new`). Implements `CommandArgument`. |
| `CommandArgument` | `CommandArgument` sealed interface: `type ArgText string`, `AssetRef` | `ArgText` is Rust `Text(String)`. |
| `EnvironmentValue` | `EnvironmentValue` sealed interface: `type EnvText string`, `EnvRef` | `EnvText` is Rust `Text(String)`. |
| `Command` | `func NewCommand(argv []CommandArgument, env map[string]EnvironmentValue, cwd CommandArgument) (Command, error)`; `Argv() []CommandArgument`; `Env() map[string]EnvironmentValue`; `Cwd() CommandArgument` | `cwd` nil = none. Implements `MCPTransport`. |
| `HttpTransport` | `func NewHTTPTransport(url string, bearerToken *EnvRef) (HTTPTransport, error)`; `URL() string`; `BearerToken() (EnvRef, bool)` | Implements `MCPTransport`. |
| `McpTransport` | `MCPTransport` sealed interface: `Command`, `HTTPTransport` | |
| `Artifact` | `type Artifact interface{ ID() string; isArtifact() }` | Implemented by the five artifact types. |
| `SkillArtifact` | `func NewSkillArtifact(id, name string, content TreeContent) (SkillArtifact, error)`; `ID()`, `Name() string`; `Content() TreeContent` | |
| `InstructionArtifact` | `func NewInstructionArtifact(id, text string) (InstructionArtifact, error)`; `ID()`, `Text() string` | |
| `McpArtifact` | `func NewMCPArtifact(id, name string, transport MCPTransport) (MCPArtifact, error)`; `ID()`, `Name() string`; `Transport() MCPTransport` | |
| `HookEvent` | `type HookEvent string`: `EventSessionStart`, `EventBeforeTool`, `EventAfterTool`, `EventPrompt`, `EventStop`; `ParseHookEvent` | |
| `HookOutcome` | `type HookOutcome string`: `OutcomeContinue`, `OutcomeBlock`, `OutcomeAsk`, `OutcomeModifyInput`, `OutcomeContext`; `ParseHookOutcome` | |
| `HookArtifact` | `func NewHookArtifact(p HookArtifactParams) (HookArtifact, error)`; `ID() string`; `Event() HookEvent`; `Command() Command`; `TimeoutMillis() int`; `Outcomes() []HookOutcome`; `Tools() []string`; `ProtocolVersion() int` | See `HookArtifactParams` below. |
| `NativeArtifact` | `func NewNativeArtifact(id string, family Family, audience Audience, destination string, content Content) (NativeArtifact, error)`; `ID()`, `Destination() string`; `Family()`; `Audience()`; `Content() Content` | |
| `SupportAsset` | `func NewSupportAsset(id string, family Family, content TreeContent) (SupportAsset, error)`; `ID()`; `Family()`; `Content() TreeContent` | |

```go
type HookArtifactParams struct {
	ID            string        // required
	Event         HookEvent     // required
	Command       Command       // required; the zero Command is rejected
	TimeoutMillis int           // required, 1-300000; zero is rejected
	Outcomes      []HookOutcome // required, nonempty, unique
	Tools         []string      // nil = every tool; only for tool events
}
```

### Content

| Rust | Go | Notes |
| --- | --- | --- |
| `Content` | `Content` sealed interface: `FileContent`, `TreeContent`, `SectionContent`, `StructuredContent` | |
| `DocumentFormat` | `type DocumentFormat string`: `FormatJSON`, `FormatJSONC`, `FormatTOML`; `ParseDocumentFormat` | |
| `FileMode` | `type FileMode string`: `ModePrivate`, `ModeReadable`, `ModeExecutable`; `ParseFileMode` | |
| `FileContent` | `func NewFileContent(data []byte, mode FileMode) (FileContent, error)`; `Data() []byte`; `Mode() FileMode` | Gains an error for an unknown mode. |
| `TreeContent` | `func NewTreeContent(entries []BundleEntry) (TreeContent, error)`; `Entries() []BundleEntry` | Normalizes like Rust `snapshot_entries` (inferred parents, byte order). Zero value is an empty tree. |
| `Key` | `func NewKey(name string) (Key, error)`; `Name() string` | Implements `SelectorPart`. |
| `Member` | `func NewMember(identity Value, key []string) (Member, error)`; `Identity() Value`; `Key() []string` | Implements `SelectorPart`. |
| `SelectorPart` | `SelectorPart` sealed interface: `Key`, `Member` | |
| `Selector` | `func NewSelector(parts ...SelectorPart) (Selector, error)`; `Parts() []SelectorPart` | Implements `ClaimSelector`. |
| `Position` | `type Position string`: `PositionFirst`, `PositionLast`, `PositionBefore`, `PositionAfter`; `ParsePosition` | |
| `Placement` | `func NewPlacement(position Position, anchor *Member) (Placement, error)`; `Position() Position`; `Anchor() (Member, bool)` | Zero `Placement` is Rust `Default` (last, no anchor). |
| `SectionBoundaries` | `func NewSectionBoundaries(start, end string) (SectionBoundaries, error)`; `func SectionBoundariesFromMarker(marker string) (SectionBoundaries, error)`; `Start()`, `End() string` | Implements `ClaimSelector`. |
| `SectionContent` | `func NewSectionContent(marker, text string, boundaries *SectionBoundaries) (SectionContent, error)`; `Marker()`, `Text() string`; `Boundaries() (SectionBoundaries, bool)`; `Selector() SectionBoundaries` | nil `boundaries` derive from the marker. Rust `selector()` returns `Result`; after validated construction it cannot fail, so Go returns the value. |
| `StructuredContent` | `func NewStructuredContent(format DocumentFormat, selector Selector, value Value, placement *Placement) (StructuredContent, error)`; `Format()`; `Selector()`; `Value() Value`; `Placement() (Placement, bool)` | nil `placement` differs from a default placement, as in Rust. |
| `DocumentSchema` | `func NewDocumentSchema(format DocumentFormat, key string, value Scalar) (DocumentSchema, error)`; `Format()`; `Key() string`; `Value() Scalar`; `Content() (StructuredContent, error)` | |

### Claims and authority

| Rust | Go | Notes |
| --- | --- | --- |
| `Ownership` | `type Ownership string`: `OwnershipFile`, `OwnershipTree`, `OwnershipSubtree`, `OwnershipSection`, `OwnershipStructured`; `ParseOwnership` | |
| `ClaimSelector` | `ClaimSelector` sealed interface: `WholeSelector struct{}`, `type SubtreeSelector string`, `SectionBoundaries`, `Selector` | Rust `Whole`, `Subtree(String)`, `Section`, `Structured`. |
| `Claim` | `func NewClaim(kind Ownership, selector ClaimSelector) (Claim, error)`; `Kind() Ownership`; `Selector() ClaimSelector`; `Overlaps(o Claim) bool`; `Identity(a ResourceAuthority) (string, error)`; `Equal(o Claim) bool` | A section claim given a `SubtreeSelector` marker converts it to boundaries, as Rust; `Claim` is deliberately not comparable with `==` or usable as a map key, so use `Equal`. |
| `ResourceAuthority` | `func NewResourceAuthority(ctx context.Context, destination string) (ResourceAuthority, error)`; `Destination() string` | Canonicalizes on the filesystem; fails with `*AuthorityError` (a nil `ctx` with `*ValueError`). |
| `authority::canonical_destination` | `func CanonicalDestination(ctx context.Context, path string) (string, error)` | Public module path in Rust. |
| `authority::transaction_path` | `func TransactionPath(destination, transactionID string) (string, error)` | Pure. |
| `AuthorityError` | `type AuthorityError struct{ Message, Path string; Err error }`; `Error()`; `Unwrap() error` | `Path` `""` = none. `Err` is the `*ValueError` (Rust `Invalid`), `ErrUnsafePath` (Rust `Unsafe`, `var ErrUnsafePath = errors.New("unsafe path")`), or the I/O cause carrying its `syscall.Errno` (Rust `Io`). |

### Editing

| Rust | Go | Notes |
| --- | --- | --- |
| `Acquisition` | `type Acquisition string`: `AcquireConflict`, `AcquireTakeover`, `AcquireAdopt`; `ParseAcquisition` | In option structs `""` means `AcquireConflict`. |
| `Disposition` | `type Disposition string`: `DispositionCreated`, `DispositionTakenOver`, `DispositionAdopted`; `ParseDisposition` | |
| `EditStatus` | `type EditStatus string`: `EditCreated`, `EditTakenOver`, `EditAdopted`, `EditCurrent`, `EditUpdated`, `EditRemoved`, `EditModified`, `EditConflict` | |
| `SnapshotValue` | `SnapshotValue` sealed interface: `SnapshotBytes`, `SnapshotTyped` | `func NewSnapshotBytes(b []byte) SnapshotBytes` with `Bytes() []byte`; `func NewSnapshotTyped(v Value) (SnapshotTyped, error)` with `Value() Value`. |
| `ArrayPosition` | `func NewArrayPosition(previous, following *Member, prefix []byte) (ArrayPosition, error)`; `Previous() (Member, bool)`; `Following() (Member, bool)`; `Prefix() []byte` | |
| `SelectionSnapshot` | `func NewSelectionSnapshot(value SnapshotValue, syntax, ownedSyntax []byte, position *ArrayPosition, layout []byte) (SelectionSnapshot, error)`; `Value() SnapshotValue`; `Syntax()`, `OwnedSyntax()`, `Layout() []byte`; `Position() (ArrayPosition, bool)` | |
| `OwnedSelection` | `func NewOwnedSelection(p OwnedSelectionParams) (OwnedSelection, error)`; `Claim() Claim`; `Format() DocumentFormat`; `Installed() SelectionSnapshot`; `Disposition() Disposition`; `Baseline() (SelectionSnapshot, bool)`; `Separator() []byte` | `Format()` `""` = section claim. |
| `CreatedContainer` | `func NewCreatedContainer(selector Selector, emptySyntax []byte) CreatedContainer`; `Selector() Selector`; `EmptySyntax() []byte` | |
| `DocumentProvenance` | `func NewDocumentProvenance(p DocumentProvenanceParams) (DocumentProvenance, error)`; `EmptyDocument() ([]byte, bool)`; `Containers() []CreatedContainer`; `TrailingSeparators() []string`; `SchemaFields() []OwnedSelection`; `SchemaEmptyDocument() ([]byte, bool)` | Zero `DocumentProvenance` is Rust `Default`. |
| `DocumentRequest` | `func NewDocumentRequest(claim Claim, content Content, acquisition Acquisition) (DocumentRequest, error)`; `func NewSetRequest(content Content, acquisition Acquisition) (DocumentRequest, error)`; `func NewRetireRequest(claim Claim) (DocumentRequest, error)`; `Claim()`; `Content() Content`; `Acquisition()` | nil `content` retires; `acquisition` `""` = `AcquireConflict`. |
| `EditObservation` | `type EditObservation struct{ Claim Claim; Status EditStatus; Current *SelectionSnapshot; Message string }` | Plain struct; `Current` nil = none. |
| `DocumentEdit` | `type DocumentEdit struct{ Before []byte; BeforeExists bool; After []byte; AfterExists bool; Ownership []OwnedSelection; Provenance DocumentProvenance; Observations []EditObservation }`; `func (DocumentEdit) Applicable() bool` | Plain struct; `AfterExists` false means the document is deleted. |
| `edit_document` | `func EditDocument(source []byte, sourceExists bool, requests []DocumentRequest, ownership []OwnedSelection, provenance DocumentProvenance) (DocumentEdit, error)` | |
| `content_claim` | `func ContentClaim(c Content) (Claim, error)` | |
| `SharedContent` | — | Rust type alias of `Content`; use `Content`. |

```go
type OwnedSelectionParams struct {
	Claim       Claim              // required
	Format      DocumentFormat     // "" = section (text) claim; set for structured claims
	Installed   SelectionSnapshot  // required
	Disposition Disposition        // required
	Baseline    *SelectionSnapshot // nil = none; required exactly for DispositionTakenOver
	Separator   []byte             // nil = none; section claims only
}

type DocumentProvenanceParams struct {
	EmptyDocument          []byte             // used only when HasEmptyDocument
	HasEmptyDocument       bool               // false = Rust None
	Containers             []CreatedContainer // nil = none
	TrailingSeparators     []string           // nil = none; SHA-256 hex digests
	SchemaFields           []OwnedSelection   // nil = none
	SchemaEmptyDocument    []byte             // used only when HasSchemaEmptyDocument
	HasSchemaEmptyDocument bool               // false = Rust None
}
```

### Rendering and translation

| Rust | Go | Notes |
| --- | --- | --- |
| `NoticeKind` | `type NoticeKind string`: `NoticePrerequisite`, `NoticeActivation`, `NoticeUnsupported`; `ParseNoticeKind` | |
| `Notice` | `type Notice struct{ ArtifactID string; Kind NoticeKind; Message string; Code string }` | Plain struct; `Code` `""` = none. Validated by `NewRenderedBundle` and `AssembleTranslations`. |
| `DependencyMode` | `type DependencyMode string`: `DependencyRevision`, `DependencyStableReference`; `ParseDependencyMode` | |
| `Dependency` | `func NewDependency(dependent, required string, mode DependencyMode) (Dependency, error)`; `Dependent()`, `Required() string`; `Mode() DependencyMode` | `mode` `""` = `DependencyRevision`. |
| `dependency_order` | `func DependencyOrder(names []string, deps []Dependency) ([]string, error)` | |
| `RenderedArtifact` | `func NewRenderedArtifact(p RenderedArtifactParams) (RenderedArtifact, error)`; `ID() string`; `Family()`; `Destination() string`; `Content() Content`; `Subtree() string` (`""` = none); `RequireExisting() bool`; `SchemaRequirements() []DocumentSchema` | |
| `RenderedBundle` | `*RenderedBundle`; `func NewRenderedBundle(artifacts []RenderedArtifact, deps []Dependency, notices []Notice, routing map[string]string) (*RenderedBundle, error)`; `func EmptyRenderedBundle() *RenderedBundle`; `Artifacts()`, `Dependencies()`, `Notices()`; `Supported() bool`; `ContentDigest() string`; `RoutingContext() map[string]string` | |
| `ModelValue` | unexported | See [Unexported in Go](#unexported-in-go). |
| `render` | `func Render(b *Bundle, rc RenderContext) (*RenderedBundle, error)` | No I/O. |
| `render_many` | `func RenderMany(b *Bundle, contexts ...RenderContext) (*RenderedBundle, error)` | No contexts: `render_many requires at least one explicit context`. |
| `ArtifactTranslation` | `ArtifactTranslation`: `Artifact() (RenderedArtifact, bool)`; `Notices() []Notice`; `Supported() bool` | No public constructor, as Rust. |
| `translate_instruction_artifact` | `func TranslateInstructionArtifact(bundleID string, a InstructionArtifact, rc RenderContext) (ArtifactTranslation, error)` | |
| `translate_mcp_artifact` | `func TranslateMCPArtifact(a MCPArtifact, rc RenderContext, assetDestinations map[string]string) (ArtifactTranslation, error)` | Map: asset id → absolute destination. |
| `assemble_translations` | `func AssembleTranslations(translations []ArtifactTranslation, deps []Dependency, contexts []RenderContext, routing map[string]string) (*RenderedBundle, error)` | |

```go
type RenderedArtifactParams struct {
	ID                 string           // required
	Family             Family           // required
	Destination        string           // required, absolute
	Content            Content          // required
	Subtree            string           // "" = none; tree content only
	RequireExisting    bool             // false = the destination may be created
	SchemaRequirements []DocumentSchema // nil = none
}
```

### Hooks

| Rust | Go | Notes |
| --- | --- | --- |
| `hooks::INPUT_LIMIT`, `OUTPUT_LIMIT`, `ERROR_LIMIT`, `TEXT_LIMIT`, `JSON_DEPTH_LIMIT`, `HANDLER_LIMIT`, `EVENT_TIMEOUT_LIMIT_MS` | untyped constants `HookInputLimit`, `HookOutputLimit`, `HookErrorLimit`, `HookTextLimit`, `HookJSONDepthLimit`, `HookHandlerLimit`, `HookEventTimeoutLimitMillis` | Rust values. |
| `HookShell` | `type HookShell string`: `ShellPOSIX`, `ShellCmd`; `ParseHookShell` | |
| `HookRuntime` | `func NewHookRuntime(node string, shell HookShell) (*HookRuntime, error)`; `Node() string`; `Shell() HookShell`; `Resources() []BundleEntry` | `node` `""` = none; `shell` `""` = none. Validates the path without running it; resources are the embedded `.mts` files. |
| `HookText` | `func HookTextFromUTF16(units []uint16) HookText`; `func HookTextFromString(s string) (HookText, error)`; `UTF8() (string, bool)`; `Units() []uint16`; `IsEmpty() bool` | `UTF8` is Rust `as_str` (false for lone surrogates). `From<String>` is `HookTextFromString`, which rejects invalid UTF-8. |
| `HookJson` | `HookJSON`: `func ParseHookJSON(data []byte) (HookJSON, error)`; `func HookObject(members ...HookMember) (HookJSON, error)`; `func HookString(t HookText) (HookJSON, error)`; `func HookBool(b bool) HookJSON`; `func HookInt(i int64) (HookJSON, error)`; `func HookArray(items ...HookJSON) (HookJSON, error)`; `func HookNull() HookJSON`; `Bytes() []byte`; `IsObject() bool`; `UTF8() (string, bool)`; `Text() (HookText, bool)` | `type HookMember struct{ Key string; Value HookJSON }`. The zero `HookJSON` is JSON `null`. Errors are `*HookError`, as Rust's `Result<_, HookFailure>`, including from functions that take a `HookJSON`. |
| `HookRequest` | `func NewHookRequest(event HookEvent, context HookJSON, tool *HookText, input, result *HookJSON) (HookRequest, error)`; `func ParseHookRequest(data []byte) (HookRequest, error)`; `Event()`; `Context() HookJSON`; `Tool() (HookText, bool)`; `Input()`, `Result() (HookJSON, bool)`; `Bytes() []byte` | |
| `HookResponse` | `func ContinueResponse() HookResponse`; `func BlockResponse(reason HookText) (HookResponse, error)`; `func AskResponse(reason HookText) (HookResponse, error)`; `func ContextResponse(text HookText) (HookResponse, error)`; `func ModifyInputResponse(event HookEvent, input HookJSON) (HookResponse, error)`; `func ParseHookResponse(data []byte, event HookEvent, declared []HookOutcome) (HookResponse, error)`; `Outcome() HookOutcome`; `Value() (HookJSON, bool)`; `Bytes() []byte` | Rust `continuing`, `block`, `ask`, `context`, `modify_input`, `parse`. |
| `HookEffects` | `func ParseHookEffects(data []byte) (HookEffects, error)`; `Input() (HookJSON, bool)`; `Contexts() []HookText`; `Decision() HookOutcome` (`""` = none); `Reason() (HookText, bool)`; `Bytes() []byte` | Zero value is Rust `Default` (no effects). |
| `HookEnvironment` | `EnvironmentValue` | See merged functions. |
| `HookHandler` | `func NewHookHandler(p HookHandlerParams) (HookHandler, error)`; `ID() string`; `Event()`; `Argv() []string`; `Environment() map[string]EnvironmentValue`; `Cwd() string`; `TimeoutMillis() int`; `Outcomes() []HookOutcome`; `Tools() []string` | |
| `HookConfig` | `func NewHookConfig(host string, handlers []HookHandler) (HookConfig, error)`; `Host() string`; `Handlers() []HookHandler`; `Bytes() []byte` | Rust `to_bytes`. |
| `HookFailure` | `type HookError struct{ Code string }`; `Error()` = `flyrail-hook:<code>` | Go error-type naming. |

```go
type HookHandlerParams struct {
	ID            string                      // required
	Event         HookEvent                   // required
	Argv          []string                    // required, nonempty
	Environment   map[string]EnvironmentValue // nil = no variables
	Cwd           string                      // required, absolute
	TimeoutMillis int                         // required, 1-300000
	Outcomes      []HookOutcome               // required, nonempty, unique
	Tools         []string                    // nil = every tool; only for tool events
}
```

### Lifecycle

```go
type PreviewOptions struct {
	Acquisition     Acquisition // "" = AcquireConflict
	ReplaceModified bool        // false = keep modified owned content
}
type SyncOptions struct {
	Acquisition     Acquisition   // "" = AcquireConflict
	ReplaceModified bool          // false = keep modified owned content
	LockTimeout     time.Duration // 0 = one acquisition attempt per lock
}
type RemoveOptions struct {
	ReplaceModified bool          // false = keep modified owned content
	LockTimeout     time.Duration // 0 = one acquisition attempt per lock
}
type LockOptions struct {
	LockTimeout time.Duration // 0 = one acquisition attempt per lock
}
```

The error result of lifecycle functions is only for invalid arguments
(`*ValueError`, Rust's `Err(ValueError)`); target failures are in the result.
A nil `ctx` (see **Context**) and a negative `LockTimeout` to `Sync`, `Remove`
or `RecoverInstallation` are invalid arguments; both return a `*ValueError`
(`ApplyPreview` reports them in its result instead).

| Rust | Go | Notes |
| --- | --- | --- |
| `InstallationTarget` | `func NewInstallationTarget(st store.Store, context, routing map[string]string) (*InstallationTarget, error)`; `Store() store.Store`; `Context()`, `RoutingContext() map[string]string`; `ContextDigest() string`; `Equal(o *InstallationTarget) bool` | `NewInstallationTarget` rejects with `*ValueError` a nil `st` and any `st` whose dynamic type is not a non-nil pointer (so typed-nil stores and non-comparable value stores are refused). `Equal` is Rust's `Arc::ptr_eq` plus equal contexts: same store by pointer `==`, which cannot panic. |
| `LifecyclePreview` | `*LifecyclePreview`: `BundleID() string`; `Bundle() *Bundle` (nil for a removal); `Rendered() *RenderedBundle`; `Target() *InstallationTarget`; `Acquisition()`; `ReplaceModified() bool`; `Resources() []ResourcePlan`; `Err() *TargetError`; `Failure() error`; `Applicable() bool` | Rust `index()`, `generation()` return state records and stay unexported. |
| `ResourcePlan` | `ResourcePlan`: `Resource() ResourceRef`; `TransactionDirectory() string`; `RecoveryPaths() []string`; `RequireExisting() bool`; `Err() *TargetError`; `Failure() error` | Rust `before`, `after`, `previous`, `receipt`, `ancestors` return state records and stay unexported. |
| `ResourceKind` | `type ResourceKind string`: `ResourceDocument`, `ResourceTree`, `ResourceSkillContainer`; `ParseResourceKind` | |
| `ResourceRef` | `type ResourceRef struct{ Destination string; Kind ResourceKind }`; `func (ResourceRef) SchemaVersion() int` | Plain comparable struct reported in results. `SchemaVersion` is Rust's `schema_version()` (always 2); `ResourceRef::new` validation happens where Flyrail reads one. |
| `ClaimObservation` | `type ClaimObservation struct{ ClaimID, BundleID, ArtifactID, Version, BundleDigest, RenderDigest string; Status EditStatus }` | |
| `ResourceObservation` | `type ResourceObservation struct{ Destination string; Claims []ClaimObservation; RevisionDigest string; Err *TargetError; RecoveryPaths []string }`; `func (ResourceObservation) Failure() error` | `RevisionDigest` `""` = none. `Failure` as on `InstallationResult`. |
| `InstallationObservation` | `type InstallationObservation struct{ BundleID string; Target *InstallationTarget; Resources []ResourceObservation; Current, Pending bool; Version, BundleDigest, RenderDigest string; Err *TargetError }`; `func (InstallationObservation) Matches(b *Bundle, r *RenderedBundle) bool`; `func (InstallationObservation) Failure() error` | `Current` is the stored Rust `is_current()` value, a field here, whereas `Observation.IsCurrent()` is computed. `Version`, digests `""` = no generation recorded. |
| `ResourceResult` | `type ResourceResult struct{ Resource ResourceRef; Status OperationStatus; Err *TargetError; RecoveryPaths []string }`; `func (ResourceResult) Succeeded() bool`; `func (ResourceResult) Failure() error` | As `InstallationResult`. |
| `InstallationResult` | `type InstallationResult struct{ Status OperationStatus; Observation InstallationObservation; Resources []ResourceResult; Notices []Notice; Err *TargetError }`; `func (InstallationResult) Succeeded() bool`; `func (InstallationResult) Failure() error` | `Succeeded` is true exactly for `StatusApplied` and `StatusUnchanged`, the statuses whose `Err` is nil. `Failure` returns `Err` as an `error` that is a nil interface when `Err` is nil, so `return result.Failure()` is safe where `return result.Err` would return a typed-nil `error`. |
| `preview` | `func Preview(ctx context.Context, b *Bundle, r *RenderedBundle, t *InstallationTarget, opts PreviewOptions) (*LifecyclePreview, error)` | |
| `preview_removal` | `func PreviewRemoval(ctx context.Context, bundleID string, t *InstallationTarget, replaceModified bool) (*LifecyclePreview, error)` | |
| `apply_preview` | `func ApplyPreview(ctx context.Context, p *LifecyclePreview, opts LockOptions) InstallationResult` | No error result, as Rust; a nil preview, a nil `ctx` (`context must not be nil`) or a negative `LockTimeout` (`lock timeout must be finite and nonnegative`) yields a `StatusFailed` result with `CodeInvalidState` and no changes. |
| `sync` | `func Sync(ctx context.Context, b *Bundle, r *RenderedBundle, t *InstallationTarget, opts SyncOptions) (InstallationResult, error)` | |
| `recover_installation` | `func RecoverInstallation(ctx context.Context, bundleID string, t *InstallationTarget, opts LockOptions) (InstallationResult, error)` | |
| `inspect_installation` | `func InspectInstallation(ctx context.Context, bundleID string, t *InstallationTarget) (InstallationObservation, error)` | |
| `remove` | `func Remove(ctx context.Context, bundleID string, t *InstallationTarget, opts RemoveOptions) (InstallationResult, error)` | |
| `summarize` | `func Summarize(p *LifecyclePreview) (PlanSummary, error)` | Pure. |
| `ChangeAction` | `type ChangeAction string`: `ActionCreate`, `ActionUpdate`, `ActionDelete`, `ActionUnchanged`, `ActionConflict`; `ParseChangeAction` | |
| `PlannedChange` | `type PlannedChange struct{ ArtifactIDs []string; Families []Family; Kind ResourceKind; Destination string; Action ChangeAction; Err *TargetError; RecoveryPaths []string }`; `func (PlannedChange) Failure() error` | `Failure` as on `InstallationResult`. |
| `PlanSummary` | `type PlanSummary struct{ Agent Agent; Scope TargetScope; Surface Surface; Applicable bool; Changes []PlannedChange; Notices []Notice; Err *TargetError }`; `func (PlanSummary) Failure() error` | `Agent`, `Scope`, `Surface` `""` = not in the routing context. `Failure` as on `InstallationResult`. |

### Errors and observations

| Rust | Go | Notes |
| --- | --- | --- |
| `ErrorCode` | `type ErrorCode string`: `CodeBusy`, `CodeConflict`, `CodeModified`, `CodeUpdateRequired`, `CodeUnsupported`, `CodeIOError`, `CodeUnsafePath`, `CodeInvalidState`, `CodeConcurrentChange`, `CodeRecoveryNeeded`; `ParseErrorCode` | |
| `TargetError` | `type TargetError struct{ Code ErrorCode; Message string; Path string; Errno syscall.Errno }`; `Error()`; `Unwrap() []error` | Rust `TargetError::new` is a struct literal. An unexported field holds the wrapped cause (`store.ErrBusy`, `ctx.Err()`, …); `Unwrap` returns `Errno` when nonzero plus that cause. For store errors `Message` is `err.Error()` and, for a `*store.IOError`, `Path` and `Errno` are copied from it. Store errors map as Rust: `store.ErrBusy` → busy; `store.ErrInvalidKey` → invalid_state; an `Errno` → io_error, except `ENOTSUP`, `ENOSYS`, `EXDEV`, `EINVAL` → unsupported; any other custom-store error → io_error with its text and `Errno` 0. |
| `ObservationState` | `type ObservationState string`: `StateAbsent`, `StateInstalled`, `StateRecoveryNeeded`, `StateUnknown`; `ParseObservationState` | |
| `ModificationKind` | `type ModificationKind string`: `ModificationMissing`, `ModificationAdded`, `ModificationTypeChanged`, `ModificationContentChanged`, `ModificationExecutableChanged`; `ParseModificationKind` | |
| `InventoryEntry` | `type InventoryEntry struct{ Path string; Dir bool; Size int64; SHA256 string; Executable bool }` | Rust `size`/`sha256` are `None` for a directory: `Dir` true, `Size` 0, `SHA256` `""`. |
| `Installation` | `type Installation struct{ BundleID, Version, ContentDigest, TransactionID string; Entries []InventoryEntry }` | |
| `Modification` | `type Modification struct{ BundleID, Path string; Kind ModificationKind }` | |
| `Conflict` | `type Conflict struct{ Path, Owner string }` | `Owner` `""` = unowned. |
| `Observation` | `type Observation struct{ State ObservationState; Installed *Installation; Installations []Installation; VersionMatches, RecordedContentMatches, ContentMatches *bool; Modifications []Modification; Conflicts []Conflict; Err *TargetError; RecoveryPaths []string }`; `func (Observation) IsCurrent() bool`; `func (Observation) Failure() error` | `Installed` nil = none; the `*bool` fields nil = not determined. |
| `TargetInspection<T>` | `type TargetInspection struct{ Target Target; Root string; IsAlias bool; AliasOf int; Observation Observation }` | `IsAlias` false = not an alias; `AliasOf` is meaningful only when `IsAlias` is true (the index of the earlier equivalent target). |
| `TargetResult<T>` | `type TargetResult struct{ Target Target; Root string; IsAlias bool; AliasOf int; Status OperationStatus; Observation Observation; Err *TargetError; RecoveryPaths []string }`; `func (TargetResult) Succeeded() bool`; `func (TargetResult) Failure() error` | As above; `Succeeded` and `Failure` as on `InstallationResult`. |

### Skills

```go
type SkillOptions struct {
	ReplaceModified bool          // false = keep a modified same-owner unit
	LockTimeout     time.Duration // 0 = one acquisition attempt per lock
}
```

| Rust | Go | Notes |
| --- | --- | --- |
| `SkillTarget` | `Target` | Rust `PathBuf`/`&Path` targets are `DirectoryTarget(root)`. Divergence: Rust reports a root that `ResourceAuthority::new` refuses (`/`, non-UTF-8) as a failed `TargetResult`; `DirectoryTarget` refuses it at construction with `*ValueError`, before the operation. |
| `SkillOptions` | `SkillOptions` plain struct | Rust `new`, `replace_modified()`, `lock_timeout()` become the fields; zero value is Rust `Default`. |
| `render_skills` | `func RenderSkills(ctx context.Context, b *Bundle, t Target) (*RenderedBundle, error)` | Canonicalizes the target root, hence the context; fails with `*AuthorityError` (a nil `ctx` with `*ValueError`). |
| `install`, `install_with_timeout` | `func Install(ctx context.Context, b *Bundle, targets []Target, st store.Store, opts LockOptions) ([]TargetResult, error)` | `st` must be a non-nil pointer store, as everywhere a store is taken. |
| `update`, `update_with_options` | `func Update(ctx context.Context, b *Bundle, targets []Target, st store.Store, opts SkillOptions) ([]TargetResult, error)` | `st` must be a non-nil pointer store, as everywhere a store is taken. |
| `uninstall`, `uninstall_with_options` | `func Uninstall(ctx context.Context, bundleID string, targets []Target, st store.Store, opts SkillOptions) ([]TargetResult, error)` | `st` must be a non-nil pointer store, as everywhere a store is taken. |
| `inspect` | `func Inspect(ctx context.Context, b *Bundle, targets []Target, st store.Store) ([]TargetInspection, error)` | `st` must be a non-nil pointer store, as everywhere a store is taken. |

The error result is for invalid arguments (empty `targets`, nil store, a
non-skill bundle); per-target failures are in the results, in request order.

### Discovery

```go
type DiscoveryOptions struct {
	SearchPath     []string // nil = search nothing; "" entries mean "."; pass filepath.SplitList(os.Getenv("PATH")) for the ambient PATH
	Applications   []string // application directories; nil or empty = /Applications and <home>/Applications (the latter only when home != "") unless NoApplications
	NoApplications bool     // true = skip application discovery
}
```

| Rust | Go | Notes |
| --- | --- | --- |
| `AgentPresence` | `type AgentPresence struct{ Agent Agent; Executable, Application string }` | `""` = no such evidence. |
| `detect_agents(path, application_directories)` | `func DetectAgents(ctx context.Context, home string, opts DiscoveryOptions) ([]AgentPresence, error)` | Rust reads `PATH` and the home directory itself; Go takes both explicitly. Application evidence is macOS-only, as Rust. A relative application directory (explicit, or derived from a relative `home`) fails up front with `*ValueError` `agent presence evidence must be an absolute Path`, whatever is installed. Results in `Agents()` order. |

### State stores

Package `store`:

```go
type Expected struct{ /* value []byte; present bool */ }
func Absent() Expected
func Equal(v []byte) Expected // copies v
func (Expected) Present() bool
func (Expected) Bytes() []byte // copy; nil when absent

type Unlocker interface{ Unlock() error } // idempotent
func NewUnlocker(release func() error) Unlocker

type Store interface {
	Location() (string, bool)
	StagingDirectory() (string, bool)
	Read(ctx context.Context, key string) (value []byte, ok bool, err error)
	Write(ctx context.Context, key string, value []byte, expected Expected) (bool, error)
	Delete(ctx context.Context, key string, expected []byte) (bool, error)
	Keys(ctx context.Context, prefix string) ([]string, error) // ascending byte order
	Lock(ctx context.Context, name string, timeout time.Duration) (Unlocker, error)
}

var (
	ErrBusy       = errors.New("state store lock is busy")
	ErrInvalidKey = errors.New("invalid state store key or lock name")
)
type BusyError struct{ Name string }
func NewBusyError(name string, cause error) *BusyError // cause (for example ctx.Err()) is wrapped; Error() ignores it
type IOError struct{ Message, Path string; Errno syscall.Errno }
type KeyError struct{ Kind, Value string }
func ValidateKey(key string) error
func ValidateLockName(name string) error

type DirectoryStore struct{ /* … */ }
func NewDirectoryStore(ctx context.Context, path string) (*DirectoryStore, error)
type MemoryStore struct{ /* … */ }
func NewMemoryStore() *MemoryStore
```

| Rust | Go | Notes |
| --- | --- | --- |
| `StateStore` | `store.Store` | Implementations are safe for concurrent use and copy values in and out. Rust's default `location`/`staging_directory` (`None`) is `("", false)`. |
| `StateStore::read` | `Read` | `ok` false = absent; an empty value is present. |
| `StateStore::write(key, value, expected: Option<&[u8]>)` | `Write(…, expected Expected)` | `Absent()` = Rust `None`: the key must be absent. The zero `Expected` is `Absent()`. |
| `StateStore::delete`, `keys` | `Delete`, `Keys` | |
| `StateStore::lock(name, timeout)` | `Lock(ctx, name, timeout)` | Zero timeout = one attempt; waits (`DirectoryStore` polls every 20 ms; `MemoryStore` wakes waiters directly) end at the earlier of `timeout` and the context deadline. Rust releases on drop; Go callers `Unlock`. |
| `StoreLock` | `store.Unlocker` | |
| `StoreLock::new(release)` | `func NewUnlocker(release func() error) Unlocker` | Runs `release` at most once. |
| `StoreError::Busy { name }` | `*store.BusyError` | `Error()` = `state store lock is busy: <name>`; `errors.Is(err, store.ErrBusy)`. A store may also return it from `Write` or `Delete`. `BusyError` has an unexported cause set by `NewBusyError` and returned by `Unwrap() error`; when the context ended the store passes `ctx.Err()` as the cause, so `errors.Is(err, context.Canceled)` holds while `Error()` stays the Rust text. |
| `StoreError::InvalidKey { message }` | `*store.KeyError` | `Kind` is `state key` or `lock name`; `Error()` = `<Kind> must be slash-separated lowercase segments`; `errors.Is(err, store.ErrInvalidKey)`. |
| `StoreError::Io { message, path, kind, errno }` | `*store.IOError` | `Error()` is `Message`, the complete Rust/Python text (`[Errno 2] No such file or directory: '<path>'`; the privacy refusal is `[Errno 13] state store must be private to its owner: '<path>'`). `Path` `""` = none; `Unwrap()` returns `*fs.PathError{Path, Err: Errno}` when `Errno` is nonzero (so `errors.Is(err, syscall.EACCES)` holds), else nil. Custom stores construct it by struct literal. |
| `stores::validate_key(key, label)` | `store.ValidateKey(key)`, `store.ValidateLockName(name)` | For custom stores. |
| `DirectoryStore::new` | `store.NewDirectoryStore(ctx, path)` | `Location()` is `path` made absolute (Rust `std::path::absolute`); `StagingDirectory()` is `<path>/staging`. Values over 4 GiB are refused by `Write` and `Read` (and by `MemoryStore.Write`) with `[Errno 27] File too large`; the limit is twice the journal of a resource at the 256 MiB resource limit, so no transaction within the resource limits reaches it. A zero-value `DirectoryStore` passed to a lifecycle or skills call is a `*ValueError` (`directory state store must come from NewDirectoryStore`). Divergence: Rust records a staging directory that is not valid UTF-8 lossily and the store stays incomplete; Go's mutating calls refuse it before any write with Python's `CodeInvalidState` message (`'utf-8' codec can't encode character '\udcfd' in position N: surrogates not allowed`). |
| `MemoryStore::new` | `store.NewMemoryStore()` | The zero `MemoryStore` is also an empty, ready store (Rust `Default`); it must not be copied after first use. |
| `testing::check_store(open)` | `func Run(t *testing.T, open func(t *testing.T) store.Store)` in `storetest` | Rust's checks plus: empty value, absent versus empty, caller mutation of passed and returned slices, double `Unlock`, zero-timeout busy, context cancellation. Same preconditions: nothing under `conformance/`. |

## Unexported in Go

| Rust | Go | Reason |
| --- | --- | --- |
| `encode`, `decode` (`codec`) | unexported state codec; reached by the interop driver through `internal/interopbridge` | State records are a durable format, not an API. |
| `Ancestor`, `Count`, `Generation`, `Index`, `Node`, `OwnedClaim`, `Preparation`, `Receipt`, `Revision`, `Journal` | unexported records; `internal/interopbridge` for the driver | State records; `ResourceKind` and `ResourceRef` stay public because results report them. |
| `ModelValue` | unexported `modelValue() Value` method on model types; `internal/interopbridge` for the differential driver | Differential record encoding, not consumer API. |
| `admit` (`#[doc(hidden)]`) | unexported; `internal/interopbridge` | Interop testing only. |
| `security_capture` (`#[doc(hidden)]`) | `internal/native` returns plain structs; `internal/interopbridge` | Interop testing only. |
| `InteropResourceLockGuard`, `interop_resource_lock`, `set_interop_test_barrier`, `set_interop_test_fault` (feature `interop-test-seams`) | `internal/seams` hooks carried in a context value; the lock through `internal/interopbridge` | No package-level mutable state; tests run in parallel. |
| `content::snapshot_entries` | folded into `NewTreeContent` | Only used to build tree content. |
| `SharedContent` | — | Alias of `Content`. |
