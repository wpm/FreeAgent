"""The durable record carries the manifest set + resolved versions, no PID (ADR-0005).

The environment writes "what should be running" (the recruited manifest set) and
"what ran" (the resolved engine versions) onto its stream's metadata when it
creates the stream, alongside the existing name/status/sealed fields. No process
handle is ever persisted.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from freeagent_testing import connected_transport

from freeagent import Environment, EpisodeState
from freeagent.cli.child import class_ref
from freeagent.manifest import Manifest
from freeagent.metadata import KEY_MANIFEST_SET, KEY_RESOLVED_VERSIONS, EpisodeMetadata
from freeagent.recruiter import build_manifests
from freeagent.service.store import EpisodeStore

if TYPE_CHECKING:
    from collections.abc import Mapping

APP = "demo"
EPISODE = "e1"
STREAM = f"{APP}_episode_{EPISODE}"


def _manifest_set() -> list[dict[str, object]]:
    """A two-member set referencing real, installed classes (freeagent ships them)."""
    return [
        Manifest(
            role="agent",
            cls=class_ref(Environment),  # a real, installed class -> stampable
            subject_root=f"{APP}.episode.{EPISODE}",
            agent_id="alice",
            nats_url="nats://x",
        ).model_dump(),
        Manifest(
            role="environment",
            cls=class_ref(Environment),
            app=APP,
            roster=["alice"],
            episode_id=EPISODE,
            nats_url="nats://x",
        ).model_dump(),
    ]


async def test_environment_writes_manifest_set_and_versions_at_stream_creation() -> None:
    transport = await connected_transport()
    env = Environment(APP, ["alice"], episode_id=EPISODE, config={"setup_timeout": 0.05})
    env.set_manifest_set(_manifest_set())
    # serve() will time out in setup (no agent) and abort; we only need the
    # stream-creation write, which happens first.
    await env.serve(transport)

    raw = await transport.stream_metadata(STREAM)
    assert KEY_MANIFEST_SET in raw
    assert KEY_RESOLVED_VERSIONS in raw
    meta = EpisodeMetadata.from_stream_metadata(raw)
    assert meta is not None
    assert [m["role"] for m in meta.manifest_set] == ["agent", "environment"]
    # The class is freeagent's, so it resolves to a real distribution stamp.
    ref = class_ref(Environment)
    assert meta.resolved_versions[ref].startswith("freeagent==")


async def test_durable_record_persists_no_pid() -> None:
    transport = await connected_transport()
    env = Environment(APP, ["alice"], episode_id=EPISODE, config={"setup_timeout": 0.05})
    env.set_manifest_set(_manifest_set())
    await env.serve(transport)

    raw = await transport.stream_metadata(STREAM)
    # No key, and no manifest field, ever carries a process handle.
    assert not any("pid" in key.lower() for key in raw)
    for value in raw.values():
        assert "pid" not in value.lower()


async def test_set_manifest_set_skips_unstampable_class() -> None:
    env = Environment(APP, ["alice"], episode_id=EPISODE)
    env.set_manifest_set(
        [
            {"role": "agent", "class": "no.such.module:Ghost"},
            {"role": "agent", "class": "also.missing:Other"},
        ]
    )
    # Unimportable refs leave no stamp; the set itself is still recorded verbatim.
    assert env.resolved_versions == {}
    assert [m["class"] for m in env.manifest_set] == [
        "no.such.module:Ghost",
        "also.missing:Other",
    ]


async def test_store_exposes_manifest_set_and_versions() -> None:
    transport = await connected_transport()
    env = Environment(APP, ["alice"], episode_id=EPISODE, config={"setup_timeout": 0.05})
    env.set_manifest_set(_manifest_set())
    await env.serve(transport)

    store = EpisodeStore(transport, application_of=lambda app: app)
    record = await store.get(APP, EPISODE)
    assert record is not None
    assert [m["role"] for m in record.manifest_set] == ["agent", "environment"]
    assert record.resolved_versions[class_ref(Environment)].startswith("freeagent==")


def test_recruiter_stamps_environment_manifest_with_the_whole_set() -> None:
    from collatz_app import CollatzAgent, CollatzEnvironment

    from freeagent import AppSpec, make_plan
    from freeagent.cli.config import EpisodeConfig

    spec = AppSpec(name="collatz", environment=CollatzEnvironment, roster={"even": CollatzAgent})
    plan = make_plan(EpisodeConfig(episode_id="ep1"), app="collatz", roster=spec.roster)
    manifests = build_manifests(spec, plan)
    env = next(m for m in manifests if m.role == "environment")
    assert env.manifest_set is not None
    # The set is the whole roster + the environment, stored as plain dicts; the
    # agent manifests carry none.
    assert [m["role"] for m in env.manifest_set] == ["agent", "environment"]
    for agent in (m for m in manifests if m.role == "agent"):
        assert agent.manifest_set is None
    # The environment copy inside the set does not nest its own set (no recursion),
    # and carries the ``class`` alias on the wire (not ``cls``).
    nested_env = next(m for m in env.manifest_set if m["role"] == "environment")
    assert "manifest_set" not in nested_env
    assert "class" in nested_env
    assert "cls" not in nested_env


def test_environment_child_spec_carries_manifest_set() -> None:
    from collatz_app import CollatzAgent, CollatzEnvironment

    from freeagent import AppSpec, make_plan
    from freeagent.cli.config import EpisodeConfig

    spec = AppSpec(name="collatz", environment=CollatzEnvironment, roster={"even": CollatzAgent})
    plan = make_plan(EpisodeConfig(episode_id="ep1"), app="collatz", roster=spec.roster)
    env_manifest = next(m for m in build_manifests(spec, plan) if m.role == "environment")
    child_spec = env_manifest.to_child_spec()
    assert "manifest_set" in child_spec
    assert [m["role"] for m in child_spec["manifest_set"]] == ["agent", "environment"]


def test_agent_child_spec_omits_manifest_set() -> None:
    agent = Manifest(
        role="agent",
        cls="pkg:Agent",
        subject_root="a.episode.b",
        agent_id="alice",
        nats_url="nats://x",
    )
    assert "manifest_set" not in agent.to_child_spec()


def _config(**kw: object) -> Mapping[str, object]:
    return kw


async def test_serve_settles_normally_with_a_manifest_set() -> None:
    """Carrying a manifest set does not perturb the normal lifecycle."""
    transport = await connected_transport()
    env = Environment(APP, [], episode_id=EPISODE, config={"episode_timeout": 0.05})
    env.set_manifest_set(_manifest_set())
    state = await env.serve(transport)
    assert state is EpisodeState.ENDED
