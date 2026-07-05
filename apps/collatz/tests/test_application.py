"""Unit tests for :mod:`freeagent.app.collatz.application`.

Cover the config validation, the two entity factories, and that the ``collatz`` entry point actually
resolves to a working :class:`~freeagent.sdk.Application` in this installed environment -- the
ADR-0006 loading story Collatz exists to validate.
"""

from __future__ import annotations

import pytest
from freeagent.app.collatz import application
from freeagent.app.collatz.application import CollatzApplication, CollatzConfig
from freeagent.app.collatz.entity import CollatzAgent, CollatzEnvironment
from freeagent.app.collatz.message import Chain
from freeagent.sdk import Application, EpisodeSpec, load_application
from pydantic import ValidationError


def spec(config: dict[str, object]) -> EpisodeSpec:
    """An :class:`EpisodeSpec` with the given Collatz ``config``."""
    return EpisodeSpec(episode_root="episode", episode_id="e1", config=config)


# --- CollatzConfig --------------------------------------------------------------------------------


def test_config_validates_a_start_per_agent() -> None:
    config = CollatzConfig.model_validate({"starts": [6, 27, 2]})

    assert config.starts == [6, 27, 2]


def test_config_maps_starts_to_positional_agent_names() -> None:
    config = CollatzConfig.model_validate({"starts": [6, 27]})

    assert config.agent_starts() == {"agent-0": 6, "agent-1": 27}


def test_config_requires_at_least_one_start() -> None:
    with pytest.raises(ValidationError):
        CollatzConfig.model_validate({"starts": []})


@pytest.mark.parametrize("bad", [[0], [6, -1], [-27]])
def test_config_rejects_non_positive_starts(bad: list[int]) -> None:
    with pytest.raises(ValidationError, match="positive"):
        CollatzConfig.model_validate({"starts": bad})


def test_config_defaults_servers_to_the_sdk_default() -> None:
    from freeagent.sdk.entity import DEFAULT_NATS_SERVER

    config = CollatzConfig.model_validate({"starts": [6]})

    assert config.servers == DEFAULT_NATS_SERVER


# --- CollatzApplication factories -----------------------------------------------------------------


def test_make_environment_builds_a_collatz_environment_seeded_from_config() -> None:
    environment = application.make_environment(spec({"starts": [6, 27]}))

    assert isinstance(environment, CollatzEnvironment)
    assert environment.chains == {
        "agent-0": Chain(numbers=[6]),
        "agent-1": Chain(numbers=[27]),
    }


def test_make_agents_builds_one_named_collatz_agent_per_start() -> None:
    agents = application.make_agents(spec({"starts": [6, 27, 2]}))

    assert all(isinstance(agent, CollatzAgent) for agent in agents)
    names = [agent.name for agent in agents if isinstance(agent, CollatzAgent)]
    assert names == ["agent-0", "agent-1", "agent-2"]


def test_make_environment_rejects_a_malformed_config() -> None:
    with pytest.raises(ValidationError):
        application.make_environment(spec({"starts": []}))


def test_make_agents_rejects_a_malformed_config() -> None:
    with pytest.raises(ValidationError):
        application.make_agents(spec({"starts": [0]}))


def test_application_satisfies_the_protocol_structurally() -> None:
    assert isinstance(application, Application)
    assert isinstance(CollatzApplication(), Application)


def test_application_name_is_collatz() -> None:
    assert application.name == "collatz"


# --- Entry-point loading (ADR-0006) ---------------------------------------------------------------


def test_load_application_resolves_the_installed_collatz_entry_point() -> None:
    """``load_application("collatz")`` returns the registered object in this installed venv.

    This is the acceptance criterion that the ``collatz`` entry point in ``pyproject.toml``
    resolves, exercised against the *real* installed metadata rather than a fixture -- so it only
    passes when the package is actually installed (as ``uv sync`` / editable install makes it).
    """
    loaded = load_application("collatz")

    assert loaded is application
    assert isinstance(loaded, Application)
