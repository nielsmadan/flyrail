# Flyrail

Flyrail is a multi-language library project. Language implementations belong in
language directories; shared formats and conformance fixtures belong in spec/.
Read the implementation's AGENTS.md before changing its code or tooling.

Keep repository files useful to contributors and package consumers. Commit source,
meaningful tests, synthetic fixtures, examples, public documentation and build/CI
configuration. Store session plans, handoffs, reviews, QA transcripts, run reports
and machine-specific evidence only under ignored .cache/. Never package those files.

Use relative or computed paths. Do not commit developer home directories, temporary
run identifiers or local environment details. Keep generated outputs and caches
ignored. Do not alter HOME, global tools, hooks or real agent installations.

The root Justfile delegates to implemented languages. Run the affected language's
checks and package verification after changes; test shared fixtures in each
available implementation when their contract changes.
