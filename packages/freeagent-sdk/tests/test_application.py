"""Tests for :mod:`freeagent.sdk.application`.

These exercise :func:`~freeagent.sdk.application.load_application` and
:func:`~freeagent.sdk.application.available_applications` against a test-only entry point registered
through an :func:`~importlib.metadata.entry_points` fixture, so no real distribution is installed;
plus :class:`~freeagent.sdk.application.EpisodeSpec` round-tripping and the
:class:`~freeagent.sdk.application.Application` protocol matching structurally.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator
from importlib.metadata import EntryPoint, EntryPoints

import pytest
import sample_application
from freeagent.sdk import Environment
from freeagent.sdk.application import (
    APPLICATION_ENTRY_POINT_GROUP,
    Application,
    EpisodeSpec,
    UnknownApplication,
    available_applications,
    load_application,
)
from sample_application import SAMPLE_ENTRY_POINT_VALUE

RegisterApplications = Callable[..., None]


@pytest.fixture
def register_applications(monkeypatch: pytest.MonkeyPatch) -> Iterator[RegisterApplications]:
    """Patch :func:`~importlib.metadata.entry_points` to serve a test-only application registry.

    The returned callable takes ``name=value`` pairs and installs them as ``freeagent.applications``
    entry points, so tests register applications without a real distribution. Entry points from
    other groups are omitted, so :func:`~freeagent.sdk.application.available_applications` sees only
    what a test declared.

    :return: A callable that (re)registers the given ``name=value`` entry points.
    """

    def register(**named_values: str) -> None:
        entries = EntryPoints(
            EntryPoint(name=name, value=value, group=APPLICATION_ENTRY_POINT_GROUP)
            for name, value in named_values.items()
        )

        def fake_entry_points(*, group: str, name: str | None = None) -> EntryPoints:
            selected = entries.select(group=group)
            return selected.select(name=name) if name is not None else selected

        monkeypatch.setattr("freeagent.sdk.application.entry_points", fake_entry_points)

    yield register


def test_load_application_returns_the_registered_object(
    register_applications: RegisterApplications,
) -> None:
    register_applications(sample=SAMPLE_ENTRY_POINT_VALUE)

    assert load_application("sample") is sample_application.application


def test_loaded_application_builds_environment_and_agents(
    register_applications: RegisterApplications,
) -> None:
    register_applications(sample=SAMPLE_ENTRY_POINT_VALUE)
    application = load_application("sample")
    episode = EpisodeSpec(episode_root="root", episode_id="e1", config={"agent_count": 3})

    environment = application.make_environment(episode)
    agents = application.make_agents(episode)

    assert isinstance(environment, Environment)
    assert [agent.subjects for agent in agents] == [
        ["root.agents.agent-0"],
        ["root.agents.agent-1"],
        ["root.agents.agent-2"],
    ]


def test_load_application_unknown_name_raises_unknown_application(
    register_applications: RegisterApplications,
) -> None:
    register_applications(sample=SAMPLE_ENTRY_POINT_VALUE)

    with pytest.raises(UnknownApplication, match="nonesuch"):
        load_application("nonesuch")


def test_unknown_application_is_a_lookup_error(
    register_applications: RegisterApplications,
) -> None:
    register_applications()

    with pytest.raises(LookupError):
        load_application("sample")


def test_available_applications_lists_registered_names(
    register_applications: RegisterApplications,
) -> None:
    register_applications(sample=SAMPLE_ENTRY_POINT_VALUE, other="sample_application:application")

    assert sorted(available_applications()) == ["other", "sample"]


def test_available_applications_is_empty_with_none_registered(
    register_applications: RegisterApplications,
) -> None:
    register_applications()

    assert available_applications() == []


def test_episode_spec_round_trips_through_json() -> None:
    spec = EpisodeSpec(episode_root="root", episode_id="e1", config={"n": 27, "label": "start"})

    decoded = EpisodeSpec.model_validate_json(spec.model_dump_json())

    assert decoded == spec
    assert decoded.config == {"n": 27, "label": "start"}


def test_application_protocol_matches_structurally() -> None:
    assert isinstance(sample_application.application, Application)


def test_non_application_fails_the_protocol_check() -> None:
    assert not isinstance(object(), Application)
