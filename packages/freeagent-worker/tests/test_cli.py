"""Tests for the Free Agent worker CLI."""

from freeagent.worker.cli import app
from typer.testing import CliRunner

runner = CliRunner()


def test_run() -> None:
    result = runner.invoke(app)
    assert result.exit_code == 0
    assert "running" in result.stdout
