"""Unit tests for the ``free-agent replay`` command: no NATS, no network.

``replay_episode`` is stubbed so these exercise the command wiring -- argument
validation, option resolution, exit codes -- without a server. One test drives
the real root app to prove ``replay`` is mounted as a top-level command,
alongside the per-app sub-commands.
"""

from __future__ import annotations

from datetime import UTC, datetime
from typing import TYPE_CHECKING, Any

import pytest
import typer
from typer.testing import CliRunner

import freeagent.cli as cli
import freeagent.cli.replay as replay_module
from freeagent import Envelope, ReplayerError, make_record, write_parquet

if TYPE_CHECKING:
    from pathlib import Path

ROOT = "demoapp.episode.ep1"


def _replay_app() -> typer.Typer:
    """A single-command Typer app wrapping ``replay`` for direct invocation."""
    app = typer.Typer()
    app.command()(replay_module.replay)
    return app


def _write_log(path: Path) -> None:
    """Write a tiny but valid one-message episode log."""
    envelope = Envelope(episode_id="ep1", sender="alice", payload="hi")
    record = make_record(
        stream_seq=1,
        subject=f"{ROOT}.public",
        received_at=datetime(2026, 6, 11, 12, 0, 0, tzinfo=UTC),
        data=envelope.to_bytes(),
        fallback_episode_id="ep1",
    )
    write_parquet([record], path)


def _stub_replay(monkeypatch: pytest.MonkeyPatch, calls: dict[str, Any], result: int = 1) -> None:
    async def fake_replay_episode(**kwargs: Any) -> int:
        calls.update(kwargs)
        return result

    monkeypatch.setattr(replay_module, "replay_episode", fake_replay_episode)


def test_replay_is_mounted_on_the_root_app(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    log = tmp_path / "episode.parquet"
    _write_log(log)
    calls: dict[str, Any] = {}
    _stub_replay(monkeypatch, calls, result=1)
    monkeypatch.setattr(cli, "_discover_apps", dict)  # no apps; replay must still be there

    result = CliRunner().invoke(cli.build_root_app(), ["replay", str(log), "--as-fast-as-possible"])

    assert result.exit_code == 0, result.output
    assert "replayed 1 message" in result.output
    assert calls["parquet_path"] == log
    assert calls["as_fast_as_possible"] is True
    assert calls["speed"] == 1.0


def test_replay_defaults_nats_url(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    log = tmp_path / "episode.parquet"
    _write_log(log)
    calls: dict[str, Any] = {}
    _stub_replay(monkeypatch, calls)
    monkeypatch.setenv("FREEAGENT_NATS_URL", "nats://example:9999")

    result = CliRunner().invoke(_replay_app(), [str(log)])

    assert result.exit_code == 0, result.output
    assert calls["nats_url"] == "nats://example:9999"


def test_replay_passes_explicit_nats_url_and_speed(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    log = tmp_path / "episode.parquet"
    _write_log(log)
    calls: dict[str, Any] = {}
    _stub_replay(monkeypatch, calls)

    result = CliRunner().invoke(
        _replay_app(),
        [str(log), "--nats-url", "nats://here:4222", "--speed", "2.5"],
    )

    assert result.exit_code == 0, result.output
    assert calls["nats_url"] == "nats://here:4222"
    assert calls["speed"] == 2.5


def test_replay_rejects_a_missing_file() -> None:
    result = CliRunner().invoke(_replay_app(), ["does-not-exist.parquet"])
    assert result.exit_code == 2  # Typer usage error: Argument(exists=True)


def test_replay_rejects_a_non_positive_speed(tmp_path: Path) -> None:
    log = tmp_path / "episode.parquet"
    _write_log(log)
    result = CliRunner().invoke(_replay_app(), [str(log), "--speed", "0"])
    assert result.exit_code == 2  # usage error from the speed callback
    assert "greater than 0" in result.output


def test_replay_reports_replayer_errors(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    log = tmp_path / "episode.parquet"
    _write_log(log)

    async def boom(**_kwargs: Any) -> int:
        raise ReplayerError("the log is broken")

    monkeypatch.setattr(replay_module, "replay_episode", boom)

    result = CliRunner().invoke(_replay_app(), [str(log)])
    assert result.exit_code == 1
    assert "the log is broken" in result.output
