# Durable state

Schema-2 state records preserve complete resource ownership, revision and recovery
evidence. ``ResourceRef`` locates a physical authority; ``Receipt`` grants claims
at that boundary. ``Index`` records logical installation membership and does not
grant ownership. ``Journal`` binds a transaction's complete before, staged,
published and backup revisions.

Values are immutable and `Sendable`. ``Count`` retains nonnegative decimal
metadata tokens, including values beyond `UInt64`. Semantic integers remain
signed 64-bit values. Text and path identities preserve exact UTF-8 bytes rather
than Unicode-equivalent spelling.

The package-internal codec implements the closed tags in
`spec/receipt-format.md`. Encoders sort object keys, use compact UTF-8 JSON and
append one LF. Decoders accept object-key order and JSON whitespace independently
of value identity, and reject duplicate or undeclared fields, missing fields,
invalid types, noncanonical semantic/byte/path values and trailing input.
JSON input is bounded to 16 MiB, strings to 8 MiB, nesting to 512 levels, tokens
to one million and decimal integers to 4300 digits.

`FlyrailInterop encode` and `decode` canonicalize one state value on standard
input. Optional `--type RecordTag` checks its type. `records` accepts an array of
`{"path":"/absolute/path","bytes":"base64"}` entries and reports their raw and
canonical base64 bytes and record tags. These commands do not access managed
resources; native authority admission, publication and recovery are separate
layers.
