"""Unit + integration tests for the recruiter (ADR-0005, #53).

Two tiers, mirroring the rest of the suite:

* **Fast** tests on :class:`MemoryTransport`: the recruiter builds ``N + 1``
  well-formed manifests under the episode's work-queue subject, mirrors the
  orchestrator's child specs exactly, ensures a work-queue stream, and stays
  app-agnostic.
* **Slow** tests (skipped without NATS) enqueue against a real JetStream
  work-queue stream and pull the manifests back off it, asserting the
  work-queue retention is genuine.
"""

from __future__ import annotations

import json
import os
import uuid
from typing import TYPE_CHECKING

import pytest
from collatz_app import CollatzAgent, CollatzEnvironment
from noop_app import APP as NOOP_APP
from noop_app import NoopAgent, NoopEnvironment

from freeagent import (
    WORK_QUEUE_STREAM,
    AppSpec,
    Manifest,
    MemoryTransport,
    NatsTransport,
    SettableConfig,
    default_nats_url,
    make_plan,
    work_subject,
)
from freeagent.cli.child import class_ref
from freeagent.cli.config import EpisodeConfig
from freeagent.recruiter import build_manifests, enqueue_episode

if TYPE_CHECKING:
    from freeagent.cli.config import EpisodePlan

_SLOW_TEST_VAR = "FREEAGENT_SLOW_TEST"
_slow = pytest.mark.skipif(
    not os.environ.get(_SLOW_TEST_VAR),
    reason=f"slow test is opt-in: set {_SLOW_TEST_VAR} to run it",
)

#: A two-agent app built from the collatz classes, exercising N > 1 manifests.
COLLATZ_APP = AppSpec(
    name="collatz",
    environment=CollatzEnvironment,
    roster={"even": CollatzAgent, "odd": CollatzAgent},
    settable_config=SettableConfig(),
)


def _plan(app: AppSpec, **config: object) -> EpisodePlan:
    """A resolved plan for *app* with a fixed episode id and optional config."""
    episode_config = EpisodeConfig(episode_id="ep1", **config)  # type: ignore[arg-type]
    return make_plan(episode_config, app=app.name, roster=app.roster)


def test_build_manifests_from_a_manifest_spec_matches_the_appspec() -> None:
    """A :class:`ManifestSpec` (class-ref strings, engine-free) builds the same set.

    This is the slim-service path: the recruiter never touches a class object,
    only the ``module:QualName`` strings the spec carries -- so an engine that is
    not importable in the service still produces well-formed manifests.
    """
    plan = _plan(NOOP_APP)
    from_classes = build_manifests(NOOP_APP, plan)
    from_strings = build_manifests(NOOP_APP.manifest_spec(), plan)
    assert [m.model_dump() for m in from_strings] == [m.model_dump() for m in from_classes]


def test_build_manifests_from_a_manifest_spec_with_unimportable_refs() -> None:
    """The crux: refs to a never-importable engine still build a valid manifest set."""
    from freeagent.cli.apps import ManifestSpec

    spec = ManifestSpec(
        name="ghost",
        environment="ghost_engine.env:GhostEnvironment",
        roster={"solo": "ghost_engine.agent:GhostAgent"},
    )
    plan = make_plan(EpisodeConfig(episode_id="ep1"), app="ghost", roster=["solo"])
    manifests = build_manifests(spec, plan)
    env = next(m for m in manifests if m.role == "environment")
    agent = next(m for m in manifests if m.role == "agent")
    assert env.cls == "ghost_engine.env:GhostEnvironment"
    assert agent.cls == "ghost_engine.agent:GhostAgent"


def test_build_manifests_returns_n_plus_one() -> None:
    manifests = build_manifests(COLLATZ_APP, _plan(COLLATZ_APP))
    # Two agents + one environment.
    assert len(manifests) == len(COLLATZ_APP.roster) + 1
    roles = [m.role for m in manifests]
    assert roles == ["agent", "agent", "environment"]


def test_agent_manifests_mirror_the_orchestrator() -> None:
    plan = _plan(NOOP_APP)
    manifests = build_manifests(NOOP_APP, plan)
    agent = next(m for m in manifests if m.role == "agent")
    assert agent.agent_id == "alpha"
    assert agent.cls == class_ref(NoopAgent)
    assert agent.subject_root == "noopapp.episode.ep1"
    assert agent.config == {}
    assert agent.nats_url == plan.nats_url
    # The agent-role manifest carries no environment-only fields.
    assert agent.app is None
    assert agent.roster is None
    assert agent.episode_id is None


def test_environment_manifest_mirrors_the_orchestrator() -> None:
    plan = _plan(COLLATZ_APP)
    manifests = build_manifests(COLLATZ_APP, plan)
    env = next(m for m in manifests if m.role == "environment")
    assert env.cls == class_ref(CollatzEnvironment)
    assert env.app == "collatz"
    assert env.roster == ["even", "odd"]
    assert env.episode_id == "ep1"
    assert env.subject_root is None
    assert env.agent_id is None


def test_build_manifests_carries_config_overrides_through() -> None:
    plan = _plan(
        NOOP_APP,
        environment={"config": {"episode_timeout": 5}},
        agents={"alpha": {"config": {"grace_period": 0.5}}},
    )
    manifests = build_manifests(NOOP_APP, plan)
    agent = next(m for m in manifests if m.role == "agent")
    env = next(m for m in manifests if m.role == "environment")
    assert agent.config == {"grace_period": 0.5}
    assert env.config == {"episode_timeout": 5}


async def test_enqueue_publishes_one_message_per_manifest() -> None:
    transport = MemoryTransport()
    await transport.connect()
    plan = _plan(COLLATZ_APP)

    enqueued = await enqueue_episode(transport, spec=COLLATZ_APP, plan=plan)

    subject = work_subject("collatz", "ep1")
    on_wire = [data for s, data in transport.history if s == subject]
    assert len(on_wire) == len(enqueued) == len(COLLATZ_APP.roster) + 1
    # Every message is a well-formed manifest, round-tripping from the wire.
    decoded = [Manifest.model_validate_json(data) for data in on_wire]
    assert [m.model_dump() for m in decoded] == [m.model_dump() for m in enqueued]


async def test_enqueue_uses_the_class_alias_on_the_wire() -> None:
    transport = MemoryTransport()
    await transport.connect()
    await enqueue_episode(transport, spec=NOOP_APP, plan=_plan(NOOP_APP))

    subject = work_subject("noopapp", "ep1")
    payloads = [json.loads(data) for s, data in transport.history if s == subject]
    # The wire shape carries ``class`` (matching today's child spec), not ``cls``.
    assert all("class" in payload and "cls" not in payload for payload in payloads)


async def test_enqueue_ensures_the_shared_work_queue_stream() -> None:
    transport = MemoryTransport()
    await transport.connect()
    await enqueue_episode(transport, spec=NOOP_APP, plan=_plan(NOOP_APP))

    assert WORK_QUEUE_STREAM in transport.streams
    assert WORK_QUEUE_STREAM in transport.work_queue_streams


async def test_enqueue_returns_manifests_agents_first_then_environment() -> None:
    transport = MemoryTransport()
    await transport.connect()
    enqueued = await enqueue_episode(transport, spec=COLLATZ_APP, plan=_plan(COLLATZ_APP))
    assert [m.role for m in enqueued] == ["agent", "agent", "environment"]


async def test_two_episodes_share_one_stream_on_distinct_subjects() -> None:
    transport = MemoryTransport()
    await transport.connect()
    await enqueue_episode(transport, spec=NOOP_APP, plan=_plan(NOOP_APP))
    plan2 = make_plan(EpisodeConfig(episode_id="ep2"), app=NOOP_APP.name, roster=NOOP_APP.roster)
    await enqueue_episode(transport, spec=NOOP_APP, plan=plan2)

    # One shared work-queue stream; each episode on its own subject token.
    assert list(transport.streams) == [WORK_QUEUE_STREAM]
    subjects = {s for s, _ in transport.history}
    assert subjects == {work_subject("noopapp", "ep1"), work_subject("noopapp", "ep2")}


def test_recruiter_imports_no_application_by_name() -> None:
    # The recruiter reads only what the app advertises; it must not import any
    # application module by name. (A regression guard for app-agnosticism.)
    from pathlib import Path

    import freeagent.recruiter as recruiter

    source = recruiter.__file__
    assert source is not None
    text = Path(source).read_text(encoding="utf-8")
    for app_name in ("collatz", "noop", "twentyquestions"):
        assert app_name not in text


# ---------------------------------------------------------------------------
# Slow: real JetStream work-queue stream.
# ---------------------------------------------------------------------------


@_slow
async def test_enqueue_against_real_jetstream_work_queue() -> None:
    transport = NatsTransport(default_nats_url())
    await transport.connect()
    # A unique app so the per-episode subjects don't collide across test runs;
    # the shared work-queue stream is created once and reused.
    app_name = "rtest" + uuid.uuid4().hex[:8]
    spec = AppSpec(name=app_name, environment=NoopEnvironment, roster={"alpha": NoopAgent})
    plan = make_plan(EpisodeConfig(episode_id="ep1"), app=app_name, roster=spec.roster)
    try:
        enqueued = await enqueue_episode(transport, spec=spec, plan=plan)
        # Re-enqueuing a second episode reuses the shared stream idempotently
        # (the "stream already exists" branch of ensure_work_queue_stream).
        plan2 = make_plan(EpisodeConfig(episode_id="ep2"), app=app_name, roster=spec.roster)
        await enqueue_episode(transport, spec=spec, plan=plan2)

        # The stream exists with work-queue retention (a message is removed once
        # acked), proving ensure_work_queue_stream built the right kind.
        from nats.js.api import RetentionPolicy

        js = transport._jetstream()
        info = await js.stream_info(WORK_QUEUE_STREAM)
        assert info.config.retention == RetentionPolicy.WORK_QUEUE

        # Pull every manifest back off the queue for this episode's subject.
        subject = work_subject(app_name, "ep1")
        sub = await js.pull_subscribe(subject, durable=None)
        msgs = await sub.fetch(len(enqueued), timeout=5)
        pulled = [Manifest.model_validate_json(msg.data) for msg in msgs]
        for msg in msgs:
            await msg.ack()
        assert {m.model_dump_json() for m in pulled} == {m.model_dump_json() for m in enqueued}
    finally:
        await transport.close()
