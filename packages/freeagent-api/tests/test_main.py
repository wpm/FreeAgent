"""Unit tests for the ``freeagent-api`` entry point in :mod:`freeagent.api.__main__`."""

from __future__ import annotations

from typing import Any
from unittest.mock import patch

import pytest
from freeagent.api.__main__ import main


def run_kwargs(monkeypatch: pytest.MonkeyPatch, env: dict[str, str]) -> dict[str, Any]:
    """Invoke ``main`` with the given environment and capture uvicorn.run's keyword arguments.

    :param monkeypatch: The fixture used to set the environment variables.
    :param env: Environment variables to set before invoking.
    :return: The keyword arguments ``main`` passed to ``uvicorn.run``.
    """
    for name in ("FREEAGENT_API_HOST", "FREEAGENT_API_PORT", "FREEAGENT_API_RELOAD"):
        monkeypatch.delenv(name, raising=False)
    for name, value in env.items():
        monkeypatch.setenv(name, value)
    with patch("freeagent.api.__main__.uvicorn.run") as run:
        main()
    (target,), options = run.call_args
    assert target == "freeagent.api.app:app"
    kwargs: dict[str, Any] = options
    return kwargs


def test_main_serves_localhost_without_reload_by_default(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # reload is uvicorn's development mode: it restarts the server whenever a source file
    # changes, dropping the in-memory EpisodeManager state and orphaning worker subprocesses,
    # so it must be off unless explicitly requested.
    kwargs = run_kwargs(monkeypatch, {})

    assert kwargs["host"] == "127.0.0.1"
    assert kwargs["port"] == 8000
    assert kwargs["reload"] is False


def test_main_reads_host_port_and_reload_from_the_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    kwargs = run_kwargs(
        monkeypatch,
        {
            "FREEAGENT_API_HOST": "0.0.0.0",
            "FREEAGENT_API_PORT": "9000",
            "FREEAGENT_API_RELOAD": "1",
        },
    )

    assert kwargs["host"] == "0.0.0.0"
    assert kwargs["port"] == 9000
    assert kwargs["reload"] is True


def test_main_treats_reload_values_other_than_1_as_off(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    kwargs = run_kwargs(monkeypatch, {"FREEAGENT_API_RELOAD": "0"})

    assert kwargs["reload"] is False
