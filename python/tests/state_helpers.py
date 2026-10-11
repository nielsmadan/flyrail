import json
from pathlib import Path

from flyrail import (
    Bundle,
    BundleIdentity,
    DirectoryStore,
    Family,
    InstallationTarget,
    InstructionArtifact,
    RenderedArtifact,
    RenderedBundle,
    SectionContent,
    StateStore,
)
from flyrail._codec import decode, encode
from flyrail._resource_transaction import Journal, transaction_directory
from flyrail._state import index_key, read_record, resource_key
from flyrail.authority import _management_name


def target(
    root: Path, context: tuple[tuple[str, str], ...] = (), **options: object
) -> InstallationTarget:
    return InstallationTarget(DirectoryStore(root / "state"), context, **options)  # type: ignore[arg-type]


def bundle(version: str = "1", identifier: str = "team") -> Bundle:
    return Bundle.from_artifacts(
        BundleIdentity(identifier, version), [InstructionArtifact("guide", "text")]
    )


def rendered(path: Path, text: str = "generated") -> RenderedBundle:
    return RenderedBundle(
        [RenderedArtifact("guide", Family.INSTRUCTIONS, path, SectionContent("guide", text))]
    )


def put(store: StateStore, key: str, data: bytes) -> None:
    assert store.write(key, data, expected=store.read(key))


def index_record(selected: InstallationTarget, bundle_id: str) -> object:
    data = selected.store.read(index_key(bundle_id, selected.context_digest))
    return None if data is None else decode(data)


def put_index(selected: InstallationTarget, bundle_id: str, value: object) -> None:
    put(selected.store, index_key(bundle_id, selected.context_digest), encode(value))


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
    ).encode()


def transaction_directories(destination: Path) -> list[Path]:
    return sorted(path for path in destination.parent.iterdir() if _management_name(path.name))


def journal_directory(store: StateStore, destination: Path) -> Path:
    journal = read_record(store, resource_key(destination, "journal"), Journal)
    assert journal is not None
    return transaction_directory(journal)
