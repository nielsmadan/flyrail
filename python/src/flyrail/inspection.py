from collections.abc import Iterable
from dataclasses import dataclass, replace
from pathlib import Path

from flyrail._inventory import inventory
from flyrail._lifecycle import _references, error_detail, inspect_installation, preview
from flyrail._observation import ObservationFailure
from flyrail._resource_io import read
from flyrail._resource_models import Receipt
from flyrail.artifacts import Family, SkillArtifact
from flyrail.authority import ResourceAuthority, _UnsafeDestination, state_path
from flyrail.bundle import Bundle
from flyrail.configuration import InstallationObservation, InstallationTarget
from flyrail.content import TreeContent
from flyrail.destinations import TRANSLATABLE_AGENTS, untranslatable_message
from flyrail.editors import EditStatus
from flyrail.models import BundleEntry
from flyrail.observations import (
    Conflict,
    ErrorCode,
    Installation,
    Modification,
    ModificationKind,
    Observation,
    ObservationState,
    TargetError,
    TargetInspection,
)
from flyrail.rendered import Notice, NoticeKind, RenderedArtifact, RenderedBundle
from flyrail.targets import Target


@dataclass(frozen=True, slots=True)
class SkillTarget:
    target: Target
    installation: InstallationTarget | TargetError
    root: Path
    alias_of: int | None

    @property
    def state_root(self) -> Path:
        return state_path(self.root)


def skill_targets(identifier: str, targets: Iterable[Target]) -> tuple[SkillTarget, ...]:
    values = tuple(targets)
    if not values:
        raise ValueError("at least one target is required")
    if any(type(target) is not Target for target in values):
        raise TypeError("targets must contain Target values")
    result: list[SkillTarget] = []
    known: dict[Path, int] = {}
    for target in values:
        root = target.root
        agent = target.agent
        if agent is not None and agent not in TRANSLATABLE_AGENTS:
            refusal = TargetError(ErrorCode.UNSUPPORTED, untranslatable_message(agent), root)
            result.append(SkillTarget(target, refusal, root, None))
            continue
        try:
            root = ResourceAuthority(root).destination
            index = root.with_name(f".{root.name}.flyrail-index-{identifier}")
            context = (
                ("destination", str(root)),
                ("agent", target.agent.value if target.agent else ""),
                ("scope", target.scope.value),
                ("home", str(target.home) if target.home else ""),
                *(("env:" + key, value) for key, value in target.environment),
            )
            installation = InstallationTarget(
                index, (("destination", str(root)),), routing_context=context
            )
        except (OSError, ObservationFailure, _UnsafeDestination) as error:
            result.append(SkillTarget(target, error_detail(error), root, None))
            continue
        result.append(SkillTarget(target, installation, root, known.get(root)))
        known.setdefault(root, len(result) - 1)
    return tuple(result)


def _validate_skill_bundle(bundle: Bundle) -> None:
    if any(not isinstance(artifact, SkillArtifact) for artifact in bundle.artifacts):
        raise ValueError("use explicit rendering for non-skill artifacts")
    if bundle.assets or bundle.dependencies:
        raise ValueError("use explicit rendering for skill bundles with assets or dependencies")


def render_skills(bundle: Bundle, target: Target) -> RenderedBundle:
    if not isinstance(bundle, Bundle) or type(target) is not Target:
        raise TypeError("skill rendering requires a Bundle and Target")
    _validate_skill_bundle(bundle)
    agent = target.agent
    if agent is not None and agent not in TRANSLATABLE_AGENTS:
        return RenderedBundle(
            (),
            (),
            [
                Notice(
                    skill.name,
                    NoticeKind.UNSUPPORTED,
                    untranslatable_message(agent),
                    "agent-unsupported",
                )
                for skill in bundle.skills
            ],
        )
    destination = ResourceAuthority(target.root).destination
    rendered = []
    for skill in bundle.skills:
        prefix = skill.name + "/"
        content = TreeContent(
            BundleEntry(entry.path[len(prefix) :], entry.data, entry.executable)
            for entry in bundle.entries
            if entry.path.startswith(prefix)
        )
        rendered.append(
            RenderedArtifact(skill.name, Family.SKILLS, destination, content, subtree=skill.name)
        )
    if not rendered:
        for artifact in bundle.artifacts:
            if isinstance(artifact, SkillArtifact):
                rendered.append(
                    RenderedArtifact(
                        artifact.id,
                        Family.SKILLS,
                        destination,
                        artifact.content,
                        subtree=artifact.name,
                    )
                )
    return RenderedBundle(rendered)


def prepare_skills(
    bundle: Bundle | None, identifier: str, targets: Iterable[Target]
) -> tuple[tuple[SkillTarget, RenderedBundle | None], ...]:
    if bundle is not None:
        _validate_skill_bundle(bundle)
    selected = skill_targets(identifier, targets)
    result: list[tuple[SkillTarget, RenderedBundle | None]] = []
    for item in selected:
        if item.alias_of is not None:
            result.append((item, result[item.alias_of][1]))
            continue
        rendered = None
        if bundle is not None and isinstance(item.installation, InstallationTarget):
            try:
                rendered = render_skills(bundle, item.target)
                _references(rendered, None, item.installation, bundle)
            except (OSError, ObservationFailure, _UnsafeDestination) as error:
                item = replace(item, installation=error_detail(error))
        result.append((item, rendered))
    return tuple(result)


def skill_observation(
    observation: InstallationObservation,
    bundle: Bundle | None,
    rendered: RenderedBundle | None = None,
) -> Observation:
    installations = []
    metadata_error = None
    for observed in observation.resources:
        try:
            stored = read(observed.state_root / "receipt.json", Receipt)
        except (OSError, ObservationFailure, ValueError) as caught:
            metadata_error = error_detail(caught)
            continue
        if stored is None:
            continue
        for identifier in sorted({claim.bundle_id for claim in stored.claims}):
            claims = [claim for claim in stored.claims if claim.bundle_id == identifier]
            entries = []
            for claim in claims:
                if claim.installed is not None and isinstance(claim.claim.selector, str):
                    for node in claim.installed.nodes:
                        name = claim.claim.selector + ("/" + node.path if node.path else "")
                        entries.append(
                            BundleEntry(
                                name, node.data, node.data is not None and bool(node.mode & 0o111)
                            )
                        )
            installations.append(
                Installation(
                    identifier,
                    claims[0].version,
                    claims[0].bundle_digest,
                    stored.transaction_id,
                    inventory(entries),
                )
            )
    installed = next(
        (item for item in installations if item.bundle_id == observation.bundle_id), None
    )
    modifications = tuple(
        Modification(claim.bundle_id, claim.artifact_id, ModificationKind.CONTENT_CHANGED)
        for resource in observation.resources
        for claim in resource.claims
        if claim.status is not EditStatus.CURRENT
    )
    recovery = tuple(path for resource in observation.resources for path in resource.recovery_paths)
    error = (
        observation.error
        or metadata_error
        or next((resource.error for resource in observation.resources if resource.error), None)
    )
    state = (
        ObservationState.RECOVERY_NEEDED
        if recovery
        else (
            ObservationState.UNKNOWN
            if error
            else ObservationState.INSTALLED
            if installed
            else ObservationState.ABSENT
        )
    )
    return Observation(
        state,
        installed,
        tuple(installations),
        None if bundle is None or installed is None else installed.version == bundle.version,
        None
        if bundle is None or installed is None
        else installed.content_digest == bundle.content_digest,
        None
        if bundle is None
        else observation.is_current
        and observation.bundle_digest == bundle.content_digest
        and (rendered is None or observation.render_digest == rendered.content_digest),
        modifications,
        (),
        error,
        recovery,
    )


def inspect(bundle: Bundle, targets: Iterable[Target]) -> tuple[TargetInspection, ...]:
    if not isinstance(bundle, Bundle):
        raise TypeError("bundle must be a Bundle snapshot")
    result: list[TargetInspection] = []
    for item, rendered in prepare_skills(bundle, bundle.id, targets):
        if item.alias_of is not None:
            result.append(
                replace(result[item.alias_of], target=item.target, alias_of=item.alias_of)
            )
            continue
        if isinstance(item.installation, TargetError):
            observation = Observation(ObservationState.UNKNOWN, error=item.installation)
        else:
            observation = skill_observation(
                inspect_installation(bundle.id, item.installation), bundle, rendered
            )
            if rendered is None:
                raise AssertionError("skill inspection requires rendered content")
            planned = preview(bundle, rendered, item.installation)
            conflicts = tuple(
                Conflict(str(plan.resource.destination), None)
                for plan in planned.resources
                if plan.error and plan.error.code is ErrorCode.CONFLICT
            )
            error = (
                observation.error
                or planned.error
                or next(
                    (
                        plan.error
                        for plan in planned.resources
                        if plan.error and plan.error.code is not ErrorCode.CONFLICT
                    ),
                    None,
                )
            )
            observation = replace(
                observation,
                state=ObservationState.UNKNOWN
                if error and observation.state is not ObservationState.RECOVERY_NEEDED
                else observation.state,
                error=error,
                conflicts=conflicts,
            )
        result.append(
            TargetInspection(
                item.target,
                item.root,
                item.state_root,
                item.alias_of,
                observation,
            )
        )
    return tuple(result)
