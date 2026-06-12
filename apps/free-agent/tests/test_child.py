"""Unit tests for the child-process protocol (no NATS)."""

from __future__ import annotations

import json

import pytest

from freeagent import EpisodeState
from freeagent_runner.child import (
    EXIT_ABORTED,
    EXIT_ENDED,
    agent_spec,
    environment_spec,
    exit_code_for_state,
    run_spec,
)


def test_agent_spec_round_trips_through_json() -> None:
    spec = agent_spec(
        class_ref="noop_app:NoopAgent",
        subject_root="noopapp.episode.ep1",
        agent_id="alpha",
        config={"grace_period": 0.3, "model": "fake:script.yml"},
        nats_url="nats://localhost:4222",
    )
    assert json.loads(json.dumps(spec)) == spec
    assert spec["role"] == "agent"
    assert spec["class"] == "noop_app:NoopAgent"


def test_environment_spec_round_trips_through_json() -> None:
    spec = environment_spec(
        class_ref="noop_app:NoopEnvironment",
        app="noopapp",
        roster=["alpha", "beta"],
        episode_id="ep1",
        config={"setup_timeout": 1.0, "episode_timeout": 5, "grace_period": 0.2},
        nats_url="nats://localhost:4222",
    )
    assert json.loads(json.dumps(spec)) == spec
    assert spec["role"] == "environment"
    assert spec["roster"] == ["alpha", "beta"]


@pytest.mark.parametrize(
    ("state", "code"),
    [
        (EpisodeState.ENDED, EXIT_ENDED),
        (EpisodeState.ABORTED, EXIT_ABORTED),
        # Defensive: any non-ended final state maps to the aborted code.
        (EpisodeState.STOPPING, EXIT_ABORTED),
    ],
)
def test_exit_code_for_state(state: EpisodeState, code: int) -> None:
    assert exit_code_for_state(state) == code


def test_run_spec_rejects_unknown_role() -> None:
    with pytest.raises(ValueError, match="unknown child role"):
        run_spec({"role": "referee", "class": "noop_app:NoopAgent"})
