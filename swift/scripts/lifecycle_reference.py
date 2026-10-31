import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "python/scripts"))

from differential_driver import (
    emit,
    lifecycle_bundle,
    lifecycle_rendered,
    lifecycle_result,
    rows,
    run_lifecycle,
    stable_json,
)
from flyrail._codec import decode, encode
from flyrail._resource_models import Index
from flyrail._state import index_key
from flyrail.configuration import InstallationTarget
from flyrail.lifecycle import (
    apply_preview,
    inspect_installation,
    preview,
    recover_installation,
    remove,
    sync,
)
from flyrail.stores import DirectoryStore
from harness import tree_snapshot


def store_snapshot(store):
    return [(key, store.read(key)) for key in store.keys("")]


def edge_records(root):
    case_root = root / "public-boundaries"
    case_root.mkdir()
    store = DirectoryStore(root / ".state" / "public-boundaries")
    target = InstallationTarget(store)
    rendered, ids = lifecycle_rendered(case_root, "shared", "one")
    bundle = lifecycle_bundle("1", ids)
    assert sync(bundle, rendered, target).status.value == "applied"
    observed = inspect_installation("team", target)
    changed, _ = lifecycle_rendered(case_root, "shared", "two")
    before = tree_snapshot(case_root)
    stored = store_snapshot(store)
    matches = [
        observed.matches(bundle, rendered),
        observed.matches(lifecycle_bundle("2", ids), rendered),
        observed.matches(bundle, changed),
    ]
    pure = tree_snapshot(case_root) == before and store_snapshot(store) == stored
    emit("lifecycle", "public-boundaries", "desired-generation", stable_json({"matches": matches, "pure": pure}))
    proposed = preview(lifecycle_bundle("2", ids), changed, target)
    key = index_key("team", target.context_digest)
    raw = store.read(key)
    current = decode(raw)
    pending = Index("team", target.context_digest, pending=current.current, residual=current.resources)
    assert store.write(key, encode(pending), expected=raw)
    stale = apply_preview(proposed)
    emit("lifecycle", "public-boundaries", "stale-preview", stable_json({
        "status": stale.status.value,
        "error": None if stale.error is None else stale.error.code.value,
        "retained": store.read(key) == encode(pending),
    }))
    lifecycle_result("public-boundaries", "source-free-recover", recover_installation("team", target), case_root, target)
    partial = inspect_installation("team", target)
    emit("lifecycle", "public-boundaries", "partial-inspect", stable_json({"current": partial.is_current, "pending": partial.pending}))
    lifecycle_result("public-boundaries", "partial-sync", sync(bundle, rendered, target), case_root, target)
    lifecycle_result("public-boundaries", "source-free-remove", remove("team", target), case_root, target)


def lifecycle_records(corpus, root):
    run_lifecycle(rows(corpus), root)
    edge_records(root)
    for child in root.iterdir():
        shutil.rmtree(child)


if __name__ == "__main__":
    boundary_root = Path(sys.argv[1])
    boundary_root.mkdir()
    edge_records(boundary_root)
