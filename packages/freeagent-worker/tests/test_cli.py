"""Tests for the Free Agent worker CLI.

These exercise the CLI's argument handling, config parsing, and exit codes without a NATS server:
the one call that would touch the wire, :func:`~freeagent.worker.runner.run_episode`, is patched to
a recorder. The full over-the-wire run is the integration test in ``test_integration.py``. The
``collatz`` application is a real installed workspace member, so the loader resolves it here for
real; the unknown-application path is checked against that live registry.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from freeagent.sdk import EpisodeSpec
from freeagent.worker import cli
from freeagent.worker.cli import app
from typer.testing import CliRunner

runner = CliRunner()


@pytest.fixture
def recorded_run(monkeypatch: pytest.MonkeyPatch) -> list[dict[str, Any]]:
    """Patch :func:`~freeagent.worker.runner.run_episode` to record its arguments instead of
    running.

    Lets the CLI tests drive the whole command -- application resolution, config parsing, spec
    construction -- and assert on what *would* have been run, without a live server. The CLI imports
    ``run_episode`` by name, so the patch is applied where the CLI looks it up.

    :return: A list that each invocation appends its captured call to.
    """
    calls: list[dict[str, Any]] = []

    async def fake_run_episode(
        application: Any, episode: EpisodeSpec, servers: str, *, timeout: float | None = None
    ) -> None:
        calls.append(
            {
                "application": application,
                "episode": episode,
                "servers": servers,
                "timeout": timeout,
            }
        )

    monkeypatch.setattr(cli, "run_episode", fake_run_episode)
    return calls


def test_run_rejects_an_episode_id_that_is_not_a_subject_token(
    recorded_run: list[dict[str, Any]],
) -> None:
    # An unvalidated ID is interpolated into NATS subjects: "x.*" would subscribe the observer to
    # a wildcard overlapping every other episode's environment subject, so another episode's
    # EpisodeComplete would end this one early.
    result = runner.invoke(app, ["run", "collatz", "--episode-id", "x.*"])

    assert result.exit_code == 1
    assert "not a single NATS subject token" in result.output
    assert recorded_run == []


def test_run_rejects_an_episode_root_that_is_not_a_subject(
    recorded_run: list[dict[str, Any]],
) -> None:
    result = runner.invoke(
        app, ["run", "collatz", "--episode-id", "ok", "--episode-root", "episode..bad"]
    )

    assert result.exit_code == 1
    assert "Episode root" in result.output
    assert recorded_run == []


def test_run_accepts_a_valid_episode_root(recorded_run: list[dict[str, Any]]) -> None:
    result = runner.invoke(
        app, ["run", "collatz", "--episode-id", "ep1", "--episode-root", "episode.custom.ep1"]
    )

    assert result.exit_code == 0
    [call] = recorded_run
    assert call["episode"].episode_root == "episode.custom.ep1"


def test_run_completes_and_builds_the_expected_spec(recorded_run: list[dict[str, Any]]) -> None:
    result = runner.invoke(
        app,
        [
            "run",
            "collatz",
            "--episode-id",
            "57",
            "--nats-url",
            "nats://example:4222",
            "--config",
            '{"starts": [6, 7, 9]}',
        ],
    )

    assert result.exit_code == 0
    assert "complete" in result.stdout
    [call] = recorded_run
    assert call["application"].name == "collatz"
    assert call["servers"] == "nats://example:4222"
    # --nats-url is pinned into the config's `servers` key so entities and the observer share it.
    assert call["episode"] == EpisodeSpec(
        episode_root="episode.57",
        episode_id="57",
        config={"starts": [6, 7, 9], "servers": "nats://example:4222"},
    )


def test_nats_url_is_pinned_into_the_config_servers_key(
    recorded_run: list[dict[str, Any]],
) -> None:
    # The observer's server and the entities' server must be one and the same; the CLI guarantees
    # this by writing --nats-url into config["servers"], overriding any servers the config carried.
    result = runner.invoke(
        app,
        [
            "run",
            "collatz",
            "--episode-id",
            "1",
            "--nats-url",
            "nats://pinned:4222",
            "--config",
            '{"starts": [2], "servers": "nats://ignored:4222"}',
        ],
    )

    assert result.exit_code == 0
    [call] = recorded_run
    assert call["servers"] == "nats://pinned:4222"
    assert call["episode"].config["servers"] == "nats://pinned:4222"


def test_run_defaults_episode_root_to_the_episode_id(recorded_run: list[dict[str, Any]]) -> None:
    result = runner.invoke(app, ["run", "collatz", "--episode-id", "abc", "--config", "{}"])

    assert result.exit_code == 0
    [call] = recorded_run
    assert call["episode"].episode_root == "episode.abc"


def test_run_honors_an_explicit_episode_root(recorded_run: list[dict[str, Any]]) -> None:
    result = runner.invoke(
        app,
        [
            "run",
            "collatz",
            "--episode-id",
            "abc",
            "--episode-root",
            "custom.root",
            "--config",
            "{}",
        ],
    )

    assert result.exit_code == 0
    [call] = recorded_run
    assert call["episode"].episode_root == "custom.root"


def test_run_passes_the_timeout_through(recorded_run: list[dict[str, Any]]) -> None:
    result = runner.invoke(
        app, ["run", "collatz", "--episode-id", "1", "--config", "{}", "--timeout", "12.5"]
    )

    assert result.exit_code == 0
    assert recorded_run[0]["timeout"] == 12.5


def test_run_with_no_config_uses_only_the_pinned_server(
    recorded_run: list[dict[str, Any]],
) -> None:
    result = runner.invoke(app, ["run", "collatz", "--episode-id", "1"])

    assert result.exit_code == 0
    # No --config, so the only key is the server the CLI pins in (defaulting to the SDK default).
    assert recorded_run[0]["episode"].config == {"servers": "nats://localhost:4222"}


def test_config_can_be_read_from_a_file(recorded_run: list[dict[str, Any]], tmp_path: Path) -> None:
    config_file = tmp_path / "config.json"
    config_file.write_text(json.dumps({"starts": [4, 8]}))

    result = runner.invoke(
        app, ["run", "collatz", "--episode-id", "1", "--config", str(config_file)]
    )

    assert result.exit_code == 0
    assert recorded_run[0]["episode"].config == {
        "starts": [4, 8],
        "servers": "nats://localhost:4222",
    }


def test_unknown_application_exits_nonzero_and_lists_available(
    recorded_run: list[dict[str, Any]],
) -> None:
    result = runner.invoke(app, ["run", "nonesuch", "--episode-id", "1"])

    assert result.exit_code == 1
    assert "Unknown application" in result.output
    # The live registry has collatz installed, so it is offered as an available application.
    assert "collatz" in result.output
    assert recorded_run == []  # never reached the runner


def test_malformed_config_exits_nonzero(recorded_run: list[dict[str, Any]]) -> None:
    result = runner.invoke(app, ["run", "collatz", "--episode-id", "1", "--config", "{not json"])

    assert result.exit_code == 1
    assert "config" in result.output.lower()
    assert recorded_run == []


def test_non_object_config_exits_nonzero(recorded_run: list[dict[str, Any]]) -> None:
    result = runner.invoke(app, ["run", "collatz", "--episode-id", "1", "--config", "[1, 2, 3]"])

    assert result.exit_code == 1
    assert "config" in result.output.lower()
    assert recorded_run == []


def test_episode_failure_exits_nonzero(monkeypatch: pytest.MonkeyPatch) -> None:
    async def failing_run_episode(*_: object, **__: object) -> None:
        raise RuntimeError("boom")

    monkeypatch.setattr(cli, "run_episode", failing_run_episode)

    result = runner.invoke(app, ["run", "collatz", "--episode-id", "1", "--config", "{}"])

    assert result.exit_code == 1
    assert "boom" in result.output


def test_bare_invocation_shows_help() -> None:
    result = runner.invoke(app, [])

    # `no_args_is_help` turns a bare invocation into help rather than an error, and Typer stays in
    # multi-command mode so the subcommand is spelled `run`.
    assert "run" in result.output
