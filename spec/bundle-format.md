# Bundle format, schema 1

A bundle source is a directory containing an exact `flyrail.json` filename and
one or more complete skill directories. A loader snapshots all selected names,
bytes, directories and explicit executable intent before installation begins.
Changing or deleting the source, closing a resource context or removing its
archive afterwards must not change that snapshot.

Entry paths are relative to the eventual skill container, for example `review`
and `review/scripts/run.py`. Directory entries include every skill root and empty
directory. Canonical source roots are retained for later overlap checks: the
selected directory for filesystem sources or the archive file for ZIP sources.
Loading detects observable concurrent changes but does not promise a transactional
snapshot against an arbitrary external writer.

The [Python API](../python/docs/api.md) describes its concrete constructors,
resource providers, immutable value types and error mapping.

## Manifest

```json
{
  "schema_version": 1,
  "id": "team-tools",
  "version": "2026-autumn",
  "skills": [
    {
      "name": "review",
      "path": "skills/review",
      "executables": ["scripts/run.py"]
    }
  ]
}
```

The manifest is UTF-8 JSON without a byte order mark. Its root must be an object.
Duplicate object keys at any depth, unknown fields, non-JSON numeric constants,
and incorrectly typed values are errors; values are never coerced. Object field
order and JSON whitespace do not matter.

| Field | Rule |
| --- | --- |
| `schema_version` | Required integer `1`; booleans, floats, and other versions are rejected. |
| `id` | Required bundle identifier string. |
| `version` | Required nonblank Unicode scalar string, preserved verbatim, with equality semantics only. |
| `skills` | Required nonempty array of skill objects with distinct names. |
| skill `name` | Required skill identifier string, equal to the last component of `path`. |
| skill `path` | Required source-relative directory path. Declared skill paths cannot overlap. |
| skill `executables` | Optional array of exact relative file paths inside that skill; defaults to `[]`. |

Identifiers contain 1–64 lowercase ASCII letters or digits, optionally separated
by single hyphens. Leading, trailing, and repeated hyphens are invalid. Windows
reserved device names are invalid. Versions reject surrogate code points and are
not Unicode-normalized. They have no semantic-version ordering;
`"  next release  "` is a valid label whose spaces are preserved. An empty skill
array is not a schema-1 bundle; removing an installation uses uninstall.

Every skill contains an exact `SKILL.md` regular file. Its Markdown, YAML, encoding,
line endings, and bytes are preserved without parsing or rewriting. Other files
may contain arbitrary binary bytes. Filesystem permission bits never imply
executable intent. Every executable path must name an existing regular file with
matching spelling and case; directories and missing files are invalid.

## Portable paths and sources

All metadata and included source paths use `/` separators and NFC-normalized
Unicode scalar values. Empty, absolute, dot, traversal, backslash, and empty
components are invalid. Components cannot end in a dot or space, contain Unicode
control characters, or contain `< > : " \\ | ? *`. Windows device names are
rejected case-insensitively, including names with extensions and spaces before
extensions: `CON`, `aux.txt`, `COM1 .exe`, and `LPT¹` are invalid. Device stems are
`con`, `prn`, `aux`, `nul`, `conin$`, `conout$`, and `com`/`lpt` followed by 1–9 or
the superscripts ¹, ², ³.

A portable collision key is NFC normalization of Unicode full case folding.
Distinct files or directories, including empty directories, cannot share that
key within a sibling directory. Executable declarations must also have distinct
portable keys. Actual source spelling is preserved and must match manifest paths
exactly, even on a case-insensitive host. Parent components used to reach selected
skills and resources cannot have ambiguous portable spellings.

Source roots, selected ancestors, and included entries cannot be symlinks, special
files, junctions, or known Windows reparse points. Ordinary system symlinks above
an explicitly supplied source root are resolved once. ZIP loading checks member
metadata, rejects duplicate members and file/directory conflicts, and retains both
explicit empty directories and directories inferred from member paths. Resource adapters must identify a single unambiguous source root. Each
implementation documents which package resource providers it supports.

Filesystem content outside selected skill trees is not copied. ZIP member paths
under the selected resource, and its archive ancestors, are validated even when
they are not selected skills. `flyrail.json` itself is metadata, not installed
content, unless a skill independently contains a file with that name.

## Content digest, encoding 1

The content digest is the lowercase hexadecimal SHA-256 of the following byte
stream. Every integer is an **unsigned 64-bit big-endian** integer. Lengths count
bytes, never Unicode code points. No delimiters or padding are added except those
specified here.

1. The 19-byte ASCII prefix `flyrail-content-v1` followed by one NUL byte
   (18 printable bytes and NUL).
2. The number of entries, as one integer.
3. Every entry in ascending lexicographic order of its UTF-8 encoded installed
   relative path, each encoded as follows:
   - One ASCII type byte: `D` for a directory or `F` for a file.
   - UTF-8 path byte length, as one integer, followed by the UTF-8 path bytes.
   - For a file only: one executable byte (`00` or `01`), its data byte length as
     one integer, then its exact data bytes.

Every skill root is a directory entry. Every nested directory is an entry, so
adding or removing an empty directory changes the digest. File content, installed
path spelling, skill names, and explicit executable intent change the digest.
Directory executable bits are not represented. The bundle ID, version, source
paths, manifest field/skill/executable order, timestamps, and filesystem modes are
excluded. Moving source trees or changing a version label alone preserves the
digest. A skill subset can use the same framing, with its own count and unchanged
installed paths, to produce an independently comparable inventory digest.

`spec/fixtures/bundles.json` is a language-neutral fixture collection.
Each case provides a manifest, source directories and files (`data_hex`), expected
installed entries, and expected SHA-256. Tiny cases also contain complete,
independently assembled `framing_hex` streams. Implementations should validate
those bytes as well as the general vectors.
