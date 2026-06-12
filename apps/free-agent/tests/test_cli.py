"""Unit tests for the CLI's error paths (no NATS, no child processes)."""

from __future__ import annotations

import shutil
from pathlib import Path

import pytest

from freeagent_runner import cli

TESTS_DIR = Path(__file__).parent


def test_missing_config_file_exits_1(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit) as excinfo:
        cli.main(["run", str(tmp_path / "nope.yml")])
    assert excinfo.value.code == 1
    assert "error" in capsys.readouterr().err


def test_invalid_config_exits_1(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    path = tmp_path / "episode.yml"
    path.write_text(
        "app: noopapp\n"
        "environment:\n"
        "  class: noop_app:NoopEnvironment\n"
        "agents:\n"
        "  env:\n"
        "    class: noop_app:NoopAgent\n",
        encoding="utf-8",
    )
    with pytest.raises(SystemExit) as excinfo:
        cli.main(["run", str(path)])
    assert excinfo.value.code == 1
    assert "reserved" in capsys.readouterr().err


def test_recorder_enabled_but_executable_missing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    # The app module lives next to the yml; the parent must import it from there.
    shutil.copy(TESTS_DIR / "noop_app.py", tmp_path / "noop_app.py")
    path = tmp_path / "episode.yml"
    path.write_text(
        "app: noopapp\n"
        "recorder:\n"
        "  enabled: true\n"
        "environment:\n"
        "  class: noop_app:NoopEnvironment\n"
        "agents:\n"
        "  alpha:\n"
        "    class: noop_app:NoopAgent\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(cli.shutil, "which", lambda _name: None)
    with pytest.raises(SystemExit) as excinfo:
        cli.main(["run", str(path)])
    assert excinfo.value.code == 1
    assert "freeagent-recorder" in capsys.readouterr().err
