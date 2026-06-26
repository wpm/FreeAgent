"""Unit tests for the episode manifest -- the serializable unit of work (#51)."""

from __future__ import annotations

import json

import pytest
from pydantic import ValidationError

from freeagent import Agent
from freeagent.cli.child import class_ref, run_spec
from freeagent.manifest import (
    MANIFEST_VERSION,
    Manifest,
    resolved_version_for,
)


def _agent_manifest() -> Manifest:
    return Manifest(
        role="agent",
        cls="noop_app:NoopAgent",
        subject_root="noopapp.episode.ep1",
        agent_id="alpha",
        config={"grace_period": 0.3, "model": "fake:script.yml"},
        nats_url="nats://localhost:4222",
    )


def _environment_manifest() -> Manifest:
    return Manifest(
        role="environment",
        cls="noop_app:NoopEnvironment",
        app="noopapp",
        roster=["alpha", "beta"],
        episode_id="ep1",
        config={"setup_timeout": 1.0, "episode_timeout": 5},
        nats_url="nats://localhost:4222",
    )


# ---------------------------------------------------------------------------
# Construction and validation
# ---------------------------------------------------------------------------


def test_agent_manifest_carries_its_fields() -> None:
    manifest = _agent_manifest()
    assert manifest.role == "agent"
    assert manifest.cls == "noop_app:NoopAgent"
    assert manifest.agent_id == "alpha"
    assert manifest.subject_root == "noopapp.episode.ep1"
    assert manifest.manifest_version == MANIFEST_VERSION
    assert manifest.resolved_version is None


def test_environment_manifest_carries_its_fields() -> None:
    manifest = _environment_manifest()
    assert manifest.role == "environment"
    assert manifest.app == "noopapp"
    assert manifest.roster == ["alpha", "beta"]
    assert manifest.episode_id == "ep1"


def test_agent_manifest_requires_agent_fields() -> None:
    with pytest.raises(ValidationError, match="agent manifest"):
        Manifest(
            role="agent",
            cls="noop_app:NoopAgent",
            config={},
            nats_url="nats://localhost:4222",
        )


def test_environment_manifest_requires_environment_fields() -> None:
    with pytest.raises(ValidationError, match="environment manifest"):
        Manifest(
            role="environment",
            cls="noop_app:NoopEnvironment",
            config={},
            nats_url="nats://localhost:4222",
        )


def test_unknown_role_is_rejected() -> None:
    with pytest.raises(ValidationError):
        Manifest(
            role="referee",  # type: ignore[arg-type]
            cls="noop_app:NoopAgent",
            nats_url="nats://localhost:4222",
        )


def test_extra_fields_are_forbidden() -> None:
    with pytest.raises(ValidationError):
        Manifest.model_validate(
            {
                "role": "agent",
                "class": "noop_app:NoopAgent",
                "subject_root": "noopapp.episode.ep1",
                "agent_id": "alpha",
                "nats_url": "nats://localhost:4222",
                "surprise": 1,
            }
        )


# ---------------------------------------------------------------------------
# Wire safety: JSON round-trip + the ``class`` field alias
# ---------------------------------------------------------------------------


def test_agent_manifest_round_trips_through_json() -> None:
    manifest = _agent_manifest()
    wire = manifest.model_dump_json()
    assert Manifest.model_validate_json(wire) == manifest


def test_environment_manifest_round_trips_through_json() -> None:
    manifest = _environment_manifest()
    wire = manifest.model_dump_json()
    assert Manifest.model_validate_json(wire) == manifest


def test_wire_uses_class_keyword_not_cls() -> None:
    wire = json.loads(_agent_manifest().model_dump_json())
    assert wire["class"] == "noop_app:NoopAgent"
    assert "cls" not in wire


def test_manifest_validates_from_class_keyword() -> None:
    manifest = Manifest.model_validate(
        {
            "role": "agent",
            "class": "noop_app:NoopAgent",
            "subject_root": "noopapp.episode.ep1",
            "agent_id": "alpha",
            "nats_url": "nats://localhost:4222",
        }
    )
    assert manifest.cls == "noop_app:NoopAgent"


# ---------------------------------------------------------------------------
# child.py consumes a manifest unchanged
# ---------------------------------------------------------------------------


def test_agent_manifest_to_child_spec_matches_legacy_agent_spec() -> None:
    from freeagent.cli.child import agent_spec

    manifest = _agent_manifest()
    expected = agent_spec(
        class_ref="noop_app:NoopAgent",
        subject_root="noopapp.episode.ep1",
        agent_id="alpha",
        config={"grace_period": 0.3, "model": "fake:script.yml"},
        nats_url="nats://localhost:4222",
    )
    assert manifest.to_child_spec() == expected


def test_environment_manifest_to_child_spec_matches_legacy_environment_spec() -> None:
    from freeagent.cli.child import environment_spec

    manifest = _environment_manifest()
    expected = environment_spec(
        class_ref="noop_app:NoopEnvironment",
        app="noopapp",
        roster=["alpha", "beta"],
        episode_id="ep1",
        config={"setup_timeout": 1.0, "episode_timeout": 5},
        nats_url="nats://localhost:4222",
    )
    assert manifest.to_child_spec() == expected


def test_child_spec_is_json_serializable_and_launchable_shape() -> None:
    # run_spec only needs role/class plus the role's fields; a manifest's
    # child spec is exactly that, JSON round-tripped.
    spec = _agent_manifest().to_child_spec()
    assert json.loads(json.dumps(spec)) == spec
    # An unknown role still raises in run_spec, proving the spec drives it.
    with pytest.raises(ValueError, match="unknown child role"):
        run_spec({**spec, "role": "referee"})


# ---------------------------------------------------------------------------
# Resolved engine version: write-only provenance stamped after import
# ---------------------------------------------------------------------------


def test_resolved_version_for_a_framework_class() -> None:
    # Agent lives in the installed ``freeagent`` distribution.
    stamp = resolved_version_for(Agent)
    assert stamp is not None
    assert stamp.startswith("freeagent==")


def test_resolved_version_for_class_without_distribution_is_none() -> None:
    # A class defined in this test module has no installed distribution.
    class Loose:
        pass

    assert resolved_version_for(Loose) is None


def test_with_resolved_version_returns_a_stamped_copy() -> None:
    manifest = _agent_manifest()
    stamped = manifest.with_resolved_version("noop_app==9.9.9")
    assert stamped.resolved_version == "noop_app==9.9.9"
    # Write-only provenance: it does not mutate the original.
    assert manifest.resolved_version is None
    # And everything else is preserved.
    assert stamped.model_copy(update={"resolved_version": None}) == manifest


def test_stamp_from_imported_class_uses_the_distribution_version() -> None:
    manifest = Manifest(
        role="agent",
        cls=class_ref(Agent),
        subject_root="freeagent.episode.ep1",
        agent_id="alpha",
        nats_url="nats://localhost:4222",
    )
    stamped = manifest.stamp_resolved_version(Agent)
    assert stamped.resolved_version is not None
    assert stamped.resolved_version.startswith("freeagent==")


def test_environment_round_trips_with_resolved_version() -> None:
    stamped = _environment_manifest().with_resolved_version("twentyquestions==1.2.3")
    assert Manifest.model_validate_json(stamped.model_dump_json()) == stamped
