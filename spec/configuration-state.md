# Resource ownership and installation indexes

Every supported bundle input uses the
[schema-2 state encoding](receipt-format.md). One physical document, tree or
skill container is the authority, lock, receipt and journal domain. The caller's
logical installation index is separate membership bookkeeping.

## Ownership

A claim identifies canonical destination, ownership kind and selector. Its bundle
ID identifies the owner; artifact ID is attribution. Changing the artifact label
at the same claim retains the original baseline. Moving a selector or destination
requires a fresh acquisition and retirement of the old claim.

Created claims have no original content. Takeover retains the first selected
baseline and original array-position anchors; subsequent updates retain it.
Adoption requires a matching selection and deliberately records no baseline.
Uninstall removes intact created/adopted units and restores intact takeovers.

Changed or missing retirement content remains owned and indexed until reconciled.
It never becomes an empty receipt merely because removal failed. Another bundle
cannot acquire an overlapping claim, including an absent one. Shared-document
creation provenance survives while related claims remain; foreign content can
prevent pruning and relinquish that provenance after the last claim retires.
Skill-container receipts similarly retain created root/namespace directory evidence
across owners and prune only empty unchanged directories after their last use.

The [document editor](document-editing.md) defines syntax ownership, typed member
identity and conditional array restoration. Full-file metadata remains outside a
partial section/key claim: update/restoration preserves the currently observed
security metadata, including later restrictive mode changes.

## Logical generations

Before any resource content is published, the index records desired `pending`
membership, the completed `current` generation as `previous`, and resource
references needed for interrupted or retired claims. Source-free inspection and
removal traverse the union of all generations and `residual`.
Complete prepared index metadata contributes its resource references to this
union until a locked mutation recognizes and reconciles the preparation.

A completed generation records version, bundle digest, render digest and exact
resource/claim membership. Current state requires all expected claims with those
identities and intact content, no extra same-owner claims and no pending membership
or resource recovery. An index may have no completed generation while an interrupted
first install already owns resources.

A comparison against an application's desired generation requires this intact
recorded state, supported rendering and exact equality of bundle ID, opaque
version, bundle digest and render digest. This is a pure comparison of snapshots;
it does not discover a newer bundle, re-read resources or prove host readiness.

Successful resource receipts remain authoritative after a later resource or index
write fails. Pending references retain their discoverability. The next mutation
re-observes receipts and reconciles membership; only completion replaces the index
with the new current generation. An empty completed removal has no active bundle
generation.

Current and previous committed generation resources, including a recognized
prepared completion of the pending generation, require a valid resource receipt.
A missing receipt is invalid ownership state: inspection reports the
resource error, and mutation preserves the index, payload and remaining recovery
evidence. The index never supplies deletion or baseline-restoration authority.
Pending first-install resources can lack receipts before committing. A valid
retained receipt with no matching claims can reflect completed retirement by
another logical index and does not constitute a missing receipt.

The index's permanent lock is acquired before resource locks, which use a
deterministic authority-path order. Its context fingerprint binds explicit
installation identity separately from the route selected for an individual call.
Equivalent skill routes bind the same physical container identity; their agent,
scope, home and routing variables remain immutable preview context without
changing the index fingerprint.
An index cannot relocate authority or grant ownership by pointing to an arbitrary
metadata file.

## Preview and preflight

A preview binds immutable bundle/render content, context, dependency graph,
complete preimages, receipts, security metadata, ancestors, index and recovery
state. Foreign-only edits stale it. Applying a preview never refreshes its
preconditions or silently recovers and substitutes a new observation.

Convenience mutation calls recover recognized pending work under locks, then plan
afresh. All known target conflicts are checked before content publication.
An explicit source-free recovery boundary may perform only that recovery phase.
It retains pending/residual membership for the next mutation and does not select
or publish new desired content. Hosts re-observe their selection conditions and
create a fresh immutable preview afterward.
Admission fences can remain after failed preflight. Creating the operation's own
fence does not invalidate its preview; ancestor identity/safety is bound without
treating an unrelated directory mtime change as replacement.

Required-existing resources must exist both at planning and publication. Compare
actual outputs and metadata with retained bundle source roots; a project can
contain a bundle source and still edit a different document safely.

## Dependencies and partial outcomes

Resource grouping must preserve dependency order and reject cycles introduced by
grouping several artifacts into one resource. Publish required assets before
references; retire references before their old required assets. A partial failure
must retain every old resource still required by an active or pending claim.
An updated referenced asset therefore needs a distinct revision destination;
overwriting its existing path cannot preserve the old reference's required bytes.

`PARTIAL` is an aggregate outcome: some resource commits succeeded while another
step failed. `INCOMPLETE` identifies unresolved recovery within a resource.
A resource receipt commit is permanent even when later cleanup fails.

Configuration-current describes stored ownership/content, not host activation,
trust or runtime prerequisites. There is no cross-resource atomicity or simultaneous
multi-file visibility. [Transactions](transaction-protocol.md) specify exact
process-recovery behavior.

## Conformance

`fixtures/configurations.json` contains complete tagged receipts, journals and
observed transitions. Consumers exercise real receipt validation, removal and
recovery using these records; loading fixtures or comparing hashes alone is
insufficient. Synthetic identity tokens are mapped to real isolated filesystem
identities before invoking native transaction code.

Hook dependency and schema refinements are specified in [hook-protocol.md](hook-protocol.md).
Owned claims persist `stable_requirements` as a unique subset of their ordinary
`requirements` pairs, plus `schema_requirements` of top-level `DocumentSchema`
records. Resource `DocumentProvenance` carries created `schema_fields` selection
evidence and optional whitespace `schema_empty_document` preimage. These fields
are part of the closed receipt/journal encoding and share its transaction boundary.
