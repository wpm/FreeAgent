"""Recruiter: roster assembly *before* the episode exists (ADR-0005).

A recruiter assembles the roster -- lobby, matchmaking, open enrollment,
waiting for human players to wander in -- and then hands a fixed roster to a
freshly created :class:`freeagent.environment.Environment`. This keeps the
environment's startup protocol singular and simple: known roster, everyone
shows up in time or the episode aborts.

In v1 the roster is still static (assigned top-down by configuration, per
DESIGN.md and the project non-goals), but ADR-0005 promotes the recruiter from
a stub into the thing that turns a static roster into *work*: from an app's
:class:`~freeagent.cli.apps.AppSpec` and a per-episode
:class:`~freeagent.cli.config.EpisodePlan` it builds the episode's **manifest
set** -- one ``environment`` manifest plus one ``agent`` manifest per roster
member -- and enqueues that set onto the shared work queue
(:mod:`freeagent.workqueue`). A worker (#52) later pulls each manifest and
launches it. The recruiter therefore produces exactly the launch information
:func:`freeagent.cli.orchestrate.start_episode` produces today, but as
wire-safe :class:`~freeagent.manifest.Manifest` objects instead of spawning the
processes itself.

Kept strictly app-agnostic: every fact comes from what the app advertised on
its :class:`AppSpec` (name, environment class, roster) and from the resolved
plan; the recruiter never imports an application by name.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from freeagent.cli.apps import AppSpec, ManifestSpec
from freeagent.manifest import Manifest
from freeagent.subjects import subject_root
from freeagent.workqueue import WORK_QUEUE_STREAM, WORK_QUEUE_SUBJECTS, work_subject

if TYPE_CHECKING:
    from freeagent.cli.config import EpisodePlan
    from freeagent.transport import Transport


def _as_manifest_spec(spec: AppSpec | ManifestSpec) -> ManifestSpec:
    """Normalize either spec form to an engine-free :class:`ManifestSpec`.

    A fat caller passes an :class:`AppSpec` (live classes); the slim service
    passes a :class:`ManifestSpec` (class-ref strings it resolved without
    importing the engine). Either way the recruiter works from strings only.
    """
    return spec.manifest_spec() if isinstance(spec, AppSpec) else spec


def build_manifests(spec: AppSpec | ManifestSpec, plan: EpisodePlan) -> list[Manifest]:
    """Build the episode's manifest set from an app and a resolved plan.

    Returns ``N + 1`` manifests: one ``agent`` manifest per roster member
    (every name in *plan*'s ``agent_configs``) followed by one ``environment``
    manifest. Mirrors :func:`freeagent.cli.orchestrate.start_episode` exactly --
    same classes, subjects, ids, and config -- but produces
    :class:`~freeagent.manifest.Manifest` objects rather than launching: an
    agent carries the episode's ``subject_root`` and its roster ``agent_id``;
    the environment carries the app name, the roster, and the episode id.

    *spec* is either an :class:`~freeagent.cli.apps.AppSpec` (live classes, the
    CLI/fat path) or a :class:`~freeagent.cli.apps.ManifestSpec` (class-ref
    strings, the slim worker-pool service path). Both yield identical manifests,
    because the manifest carries the class only as a ``module:QualName`` *string*
    -- so building a manifest set never imports an engine. The roster names and
    per-agent config come from the plan, so an operator's overrides flow through
    unchanged. Pure: it touches no transport (see :func:`enqueue_episode` for the
    side-effecting front door).
    """
    manifest_spec = _as_manifest_spec(spec)
    root = subject_root(plan.app, plan.episode_id)
    manifests: list[Manifest] = [
        Manifest(
            role="agent",
            cls=manifest_spec.roster[name],
            subject_root=root,
            agent_id=name,
            config=config,
            nats_url=plan.nats_url,
        )
        for name, config in plan.agent_configs.items()
    ]
    environment = Manifest(
        role="environment",
        cls=manifest_spec.environment,
        app=plan.app,
        roster=list(plan.agent_configs),
        episode_id=plan.episode_id,
        config=plan.environment_config,
        nats_url=plan.nats_url,
    )
    manifests.append(environment)
    # Stamp the environment manifest with the *whole* recruited set (agents +
    # the environment itself) so the environment child can write "what should be
    # running" into the durable record at stream creation (ADR-0005). The
    # environment manifest carries no PID and no process handle -- only the
    # manifests that define the roster. set_manifest_set drops each member's own
    # ``manifest_set`` so the durable record is not nested recursively.
    environment.set_manifest_set(manifests)
    return manifests


async def enqueue_episode(
    transport: Transport, *, spec: AppSpec | ManifestSpec, plan: EpisodePlan
) -> list[Manifest]:
    """Build *and enqueue* the episode's manifest set; return what was enqueued.

    Ensures the shared work-queue stream exists (idempotent;
    :data:`freeagent.workqueue.WORK_QUEUE_STREAM`), then publishes each manifest
    from :func:`build_manifests` as JSON to this episode's work-queue subject
    (:func:`freeagent.workqueue.work_subject`). Returns the manifests in the
    order they were enqueued (agents first, then the environment) so a caller --
    the control service's ``create`` (#54) -- has the exact set it scheduled.

    *spec* is an :class:`~freeagent.cli.apps.AppSpec` or a
    :class:`~freeagent.cli.apps.ManifestSpec` (see :func:`build_manifests`); the
    slim service passes the latter so enqueuing imports no engine.

    *transport* must already be connected. The recruiter does not own the
    transport's lifecycle; the caller connects and closes it.
    """
    manifests = build_manifests(spec, plan)
    await transport.ensure_work_queue_stream(WORK_QUEUE_STREAM, WORK_QUEUE_SUBJECTS)
    subject = work_subject(plan.app, plan.episode_id)
    for manifest in manifests:
        await transport.publish(subject, manifest.model_dump_json().encode())
    return manifests
