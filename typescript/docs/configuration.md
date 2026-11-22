# Configuration values and bundles

[Values](#values) · [Bundles](#bundles) · [Entries](#entries)

This page describes the inputs a bundle is built from. The API itself is
specified in [api.md](api.md); the manifest format and digests belong to the
Rust crate and the shared fixtures in `../spec/fixtures`.

## Values

A semantic value is `null`, a boolean, a `bigint` (a 64-bit integer), a finite
`number` (a binary64 float), a string, a `DateValue`, `TimeValue` or
`DateTimeValue`, an array of semantic values, or an `ObjectValue`.

```ts
import { DateValue, ObjectValue, decodeValue, encodeValue } from 'flyrail';

const value = new ObjectValue([
  ['count', 3n],
  ['ratio', 0.5],
  ['since', new DateValue(2026, 1, 2)],
]);
const bytes = encodeValue(value);
decodeValue(bytes);
```

`encodeValue` produces the canonical tagged JSON that the crate hashes, so
equal values encode to equal bytes in every language. Floats keep their bit
patterns, which makes `0` and `-0` different values, and `valuesEqual`
compares them that way. An `ObjectValue` sorts its members by the UTF-8 bytes
of the key and refuses a duplicate key.

## Bundles

A bundle is a manifest plus the files it names. Load one from a directory or a
zip archive, which are asynchronous and take an `AbortSignal`, or build one in
memory:

```ts
import { Bundle, directoryEntry, fileEntry } from 'flyrail';

const fromDisk = await Bundle.fromDirectory('/abs/path/to/bundle');
const inMemory = Bundle.fromMemory(manifestJsonBytes, [
  directoryEntry('review'),
  fileEntry('review/SKILL.md', skillBytes),
]);
inMemory.contentDigest;
```

Paths given to `fromDirectory` and `fromZip` must be absolute. A bundle read
from disk records its `sourceRoots`; a bundle built in memory has none.
`contentDigest` hashes the entries, so the same content gives the same digest
whichever way it was loaded.

## Entries

A `BundleEntry` is a plain object with a `path`, a `kind` of `'file'` or
`'directory'`, the file `data` (`null` for a directory) and an `executable`
flag. `fileEntry` and `directoryEntry` build them; `fileEntry` copies its data.
An entry is validated when a bundle consumes it, so a bad path or an
executable directory throws `ValueError` from `Bundle.fromMemory`, not from the
helper.

## Artifacts

A bundle can also be built from typed artifacts instead of a manifest.
`Bundle.fromArtifacts` takes the identity, at least one artifact, and optional
support assets and dependencies. Rust writes the manifest and entries, so the
digest matches the same bundle loaded from disk:

```ts
import { Bundle, BundleIdentity, InstructionArtifact } from 'flyrail';

const bundle = Bundle.fromArtifacts(new BundleIdentity('team', '1'), [
  new InstructionArtifact('guide', 'Prefer small commits.'),
]);
bundle.artifacts[0]; // an InstructionArtifact
```

Skills hold a `TreeContent`, MCP servers a `Command` or `HttpTransport`, hooks a
`Command` with a timeout and declared outcomes, and native artifacts an
`Audience`, a destination and `Content`. A `Dependency` orders one artifact or
asset after another; `dependencyOrder` returns the order Rust would use.

## Content

`Content` is what an artifact writes to a destination: `FileContent` for a whole
file, `TreeContent` for a directory, `SectionContent` for a marked section of a
text file, and `StructuredContent` for a value at a `Selector` inside a JSON,
JSONC or TOML document. A `Selector` is a list of `Key` and `Member` parts; a
`Member` addresses an array element by identity. `SectionBoundaries.fromMarker`
returns the comment lines Rust uses to fence a section.

## Claims

A `Claim` states which part of a resource an installation owns: the whole file or
tree, a subtree, a section or a structured selector. `claim.overlaps(other)`
tells whether two claims can conflict. `claim.identity(authority)` is the stable
hash Rust and the other languages share for a claim on a destination, where the
`ResourceAuthority` comes from `ResourceAuthority.create(destination)`.
`transactionPath(destination, id)` names the transaction file beside a
destination.

## Editing

`editDocument(source, requests, ownership?, provenance?)` applies `DocumentRequest`s to
the bytes of one document and returns a `DocumentEdit`. `source` is `null` when the
document does not exist, and `after` is `null` when the edit leaves no document. A request
sets a `SectionContent` or `StructuredContent` (`DocumentRequest.set`) or retires a `Claim`
(`DocumentRequest.retire`); `acquisition` chooses what to do with text that is already
there: `'conflict'` (the default), `'takeover'` or `'adopt'`.

Each call returns an `ownership` list and a `DocumentProvenance`. Pass both back into the
next call for the same document: they record what the earlier edit installed, so a later
set updates it, a retirement restores the foreign bytes, and a hand edit shows up as an
`EditObservation` with status `'modified'` instead of being overwritten. When
`applicable` is `false` (a `'conflict'` or `'modified'` observation), `after` is the
unchanged document and `ownership` and `provenance` are the ones you passed in.
`contentClaim(content)` returns the claim a section or structured content addresses.
