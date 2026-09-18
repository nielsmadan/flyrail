import re
from dataclasses import replace

from flyrail._json_document import JsonDocument
from flyrail.content import DocumentFormat, DocumentSchema, Selector
from flyrail.editors import (
    DocumentEdit,
    DocumentProvenance,
    DocumentRequest,
    EditObservation,
    EditStatus,
    OwnedSelection,
    _structured_snapshot,
    content_claim,
    edit_document,
)


def matches(data: bytes | None, requirement: DocumentSchema) -> bool:
    if data is None or not data.strip():
        return False
    backend = JsonDocument(
        data.decode("utf-8-sig"), comments=requirement.format is DocumentFormat.JSONC
    )
    snapshot = _structured_snapshot(backend, requirement.content.selector)
    return snapshot is not None and snapshot.value == requirement.value


def edit_schema_document(
    data: bytes | None,
    requests: list[DocumentRequest],
    *,
    ownership: tuple[OwnedSelection, ...],
    provenance: DocumentProvenance,
    active_schemas: tuple[DocumentSchema, ...],
    validate: bool,
) -> DocumentEdit:
    requirements = {schema.key: schema for schema in active_schemas}
    if len(set(active_schemas)) != len(requirements):
        raise ValueError("incompatible document schema requirements")
    schema_claims = {content_claim(schema.content) for schema in active_schemas}
    if any(
        claim.overlaps(other)
        for claim in schema_claims
        for other in [*(item.claim for item in requests), *(item.claim for item in ownership)]
    ):
        raise ValueError("document schema overlaps an app-owned field")
    if (
        not active_schemas
        and not provenance.schema_fields
        and provenance.schema_empty_document is None
    ):
        return edit_document(data, requests, ownership=ownership, provenance=provenance)
    normalized = b"{}" if data is not None and not data.strip() else data
    empty_source = data if normalized != data else provenance.schema_empty_document
    backend = JsonDocument((normalized or b"{}").decode("utf-8-sig"), comments=True)
    additions = []
    if validate:
        for schema in requirements.values():
            current = _structured_snapshot(backend, schema.content.selector)
            if current is None:
                additions.append(DocumentRequest.set(schema.content))
            elif current.value != schema.value:
                return DocumentEdit(
                    data,
                    data,
                    ownership,
                    provenance,
                    tuple(
                        EditObservation(
                            item.claim, EditStatus.CONFLICT, None, "document schema differs"
                        )
                        for item in requests
                    ),
                )
    created = tuple(
        item
        for item in provenance.schema_fields
        if isinstance(item.claim.selector, Selector)
        and _structured_snapshot(backend, item.claim.selector) == item.installed
        and not any(
            request.content is not None and request.claim.overlaps(item.claim)
            for request in requests
        )
    )
    created_ids = {item.claim for item in created} | {item.claim for item in additions}
    ordinary = replace(provenance, schema_fields=(), schema_empty_document=None)
    edit = edit_document(
        normalized, [*requests, *additions], ownership=(*ownership, *created), provenance=ordinary
    )
    if not edit.applicable:
        return replace(edit, before=data, after=data, ownership=ownership, provenance=provenance)
    fields = tuple(item for item in edit.ownership if item.claim in created_ids)
    owned = tuple(item for item in edit.ownership if item.claim not in created_ids)
    observations = tuple(item for item in edit.observations if item.claim not in created_ids)
    if not active_schemas:
        candidate = edit_document(
            edit.after,
            [DocumentRequest.retire(item.claim) for item in fields],
            ownership=(*owned, *fields),
            provenance=edit.provenance,
        )
        if (
            candidate.applicable
            and not owned
            and (candidate.after is None or re.fullmatch(rb"\s*\{\s*\}\s*", candidate.after))
        ):
            return replace(
                candidate,
                before=data,
                after=empty_source if empty_source is not None else candidate.after,
                observations=observations,
            )
        return replace(
            edit,
            before=data,
            ownership=owned,
            provenance=replace(
                edit.provenance,
                schema_fields=(),
                schema_empty_document=empty_source if owned else None,
            ),
            observations=observations,
        )
    return replace(
        edit,
        before=data,
        ownership=owned,
        provenance=replace(
            edit.provenance, schema_fields=fields, schema_empty_document=empty_source
        ),
        observations=observations,
    )
