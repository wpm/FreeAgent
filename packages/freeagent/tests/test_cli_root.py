"""Unit tests for the root CLI: app discovery, mounting, class validation, options."""

from __future__ import annotations

from pathlib import Path

import pytest
import typer
from noop_app import NoopAgent, NoopEnvironment, NotAnAgent
from typer.testing import CliRunner

import freeagent.cli as cli
from freeagent import (
    ConfigError,
    EpisodeConfig,
    EpisodeStatus,
    ParquetLogOption,
    make_plan,
    run_episode,
)
from freeagent.cli import orchestrate


def _app_with_parquet_log() -> typer.Typer:
    """A minimal app whose command uses the shared ``--parquet-log`` option."""
    app = typer.Typer()

    @app.command()
    def run(parquet_log: ParquetLogOption = None) -> None:
        typer.echo(f"log={parquet_log}")

    return app


def _spec(cli_app: typer.Typer | None = None) -> cli.AppSpec:
    """A minimal AppSpec for the noop test app, optionally carrying a CLI."""
    return cli.AppSpec(
        name="demoapp",
        environment=NoopEnvironment,
        roster={"alpha": NoopAgent},
        cli=cli_app,
    )


def test_build_root_app_mounts_discovered_apps(monkeypatch: pytest.MonkeyPatch) -> None:
    sub = typer.Typer(name="demo")

    @sub.command()
    def hello() -> None:
        typer.echo("hi from demo")

    monkeypatch.setattr(cli, "load_apps", lambda: {"demo": _spec(sub)})
    root = cli.build_root_app()
    result = CliRunner().invoke(root, ["demo", "hello"])
    assert result.exit_code == 0
    assert "hi from demo" in result.stdout


def test_build_root_app_skips_apps_without_a_cli(monkeypatch: pytest.MonkeyPatch) -> None:
    """An app registered to be launched but with no CLI mounts nothing, no crash."""
    monkeypatch.setattr(cli, "load_apps", lambda: {"headless": _spec(None)})
    root = cli.build_root_app()
    # The replay command is always there; the headless app contributes no sub-app.
    result = CliRunner().invoke(root, ["headless"])
    assert result.exit_code != 0  # no such command mounted


def test_root_app_no_args_is_help(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli, "load_apps", dict)
    result = CliRunner().invoke(cli.build_root_app(), [])
    # no_args_is_help: exits non-zero with usage, not a crash.
    assert "APP" in result.stdout or "Usage" in result.stdout


def test_run_episode_rejects_bad_environment_class() -> None:
    with pytest.raises(ConfigError, match=r"not a subclass of freeagent\.Environment"):
        run_episode(
            EpisodeConfig(),
            app="noopapp",
            environment=NotAnAgent,  # type: ignore[arg-type]
            agents={"alpha": NoopAgent},
        )


def test_run_episode_rejects_bad_agent_class() -> None:
    with pytest.raises(ConfigError, match=r"not a subclass of freeagent\.Agent"):
        run_episode(
            EpisodeConfig(),
            app="noopapp",
            environment=NoopEnvironment,
            agents={"alpha": NotAnAgent},  # type: ignore[dict-item]
        )


def test_parquet_log_defaults_to_none() -> None:
    result = CliRunner().invoke(_app_with_parquet_log(), [])
    assert result.exit_code == 0
    assert "log=None" in result.stdout


def test_parquet_log_accepts_a_fresh_path(tmp_path: Path) -> None:
    target = tmp_path / "episode.parquet"  # does not exist
    result = CliRunner().invoke(_app_with_parquet_log(), ["--parquet-log", str(target)])
    assert result.exit_code == 0
    assert "episode.parquet" in result.stdout


def test_parquet_log_refuses_to_overwrite(tmp_path: Path) -> None:
    target = tmp_path / "episode.parquet"
    target.write_text("already here", encoding="utf-8")
    result = CliRunner().invoke(_app_with_parquet_log(), ["--parquet-log", str(target)])
    assert result.exit_code == 2  # Typer usage error
    assert "already exists" in result.output


class _FinishedProc:
    """A stand-in process that is already gone, so cleanup is a no-op."""

    returncode = 0

    async def wait(self) -> int:
        return 0


async def test_handle_records_error_when_supervision_fails(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """An unexpected supervision failure latches an ERROR outcome and still raises.

    No NATS or child processes: the failure is injected into the supervisor, and
    the point is that ``state`` becomes ERROR (not a wedged RUNNING) for a poller
    while ``wait()`` still re-raises for an awaiter.
    """
    boom = RuntimeError("supervision blew up")

    async def _raise(*_args: object, **_kwargs: object) -> int:
        raise boom

    monkeypatch.setattr(orchestrate, "_wait_for_environment", _raise)
    plan = make_plan(EpisodeConfig(), app="noopapp", roster=["alpha"])
    env_proc = _FinishedProc()
    handle = orchestrate.EpisodeHandle(
        plan=plan,
        agent_procs=[],
        env_proc=env_proc,  # type: ignore[arg-type]
        recorder_proc=None,
        children=[env_proc],  # type: ignore[list-item]
    )

    with pytest.raises(RuntimeError, match="supervision blew up"):
        await handle.wait()

    assert handle.state is EpisodeStatus.ERROR
    assert handle.outcome is not None
    assert handle.outcome.status is EpisodeStatus.ERROR
    assert handle.outcome.exit_code == 1
    assert "supervision blew up" in handle.outcome.summary
