"""Unit tests for the Collatz launcher and its ``collatz`` console-script entry point.

These exercise only the *description* of the serving stack and the ``main()`` wiring: no npm,
docker, or live API is touched. The harness itself (spawning, teardown, platform ensure) is the
SDK's responsibility and is tested there; here we mock :func:`freeagent.sdk.run`.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest
from freeagent.app.collatz import launch
from freeagent.app.collatz.launch import (
    VIEWER_URL,
    CollatzLauncher,
    main,
)
from freeagent.sdk import Launcher, Service
from freeagent.sdk.launch import DockerUnavailableError


def test_launcher_satisfies_the_launcher_protocol() -> None:
    """A :class:`CollatzLauncher` is a structural :class:`~freeagent.sdk.Launcher`."""
    assert isinstance(CollatzLauncher(), Launcher)


def test_launcher_name_is_collatz() -> None:
    """The launcher's name is the bare application name used in the harness's output."""
    assert CollatzLauncher().name == "collatz"


def test_services_returns_exactly_one_viewer_service() -> None:
    """The Collatz serving stack is a single service named ``viewer``."""
    services = CollatzLauncher().services()

    assert len(services) == 1
    assert isinstance(services[0], Service)
    assert services[0].name == "viewer"


def test_viewer_service_serves_the_stdlib_http_server_on_8080() -> None:
    """``serve`` runs this interpreter's ``http.server`` on the viewer port."""
    (service,) = CollatzLauncher().services()

    assert service.serve == [sys.executable, "-m", "http.server", "8080"]


def test_viewer_service_url_is_the_viewer_url() -> None:
    """The printed URL is :data:`VIEWER_URL`, matching the server's port."""
    (service,) = CollatzLauncher().services()

    assert service.url == VIEWER_URL == "http://localhost:8080"


def test_viewer_service_cwd_is_the_real_viewer_directory() -> None:
    """``cwd`` points at the checked-in viewer package directory (holds ``package.json``)."""
    (service,) = CollatzLauncher().services()

    cwd = Path(service.cwd)  # type: ignore[arg-type]
    assert cwd.is_dir()
    assert (cwd / "package.json").is_file()
    assert cwd.name == "viewer"


def test_prepare_omits_npm_install_when_node_modules_exists(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """A warm checkout (``node_modules`` present) prepares with only ``npm run build``."""
    (tmp_path / "node_modules").mkdir()
    monkeypatch.setattr(launch, "_VIEWER_DIR", tmp_path)

    (service,) = CollatzLauncher().services()

    assert [list(step) for step in service.prepare] == [["npm", "run", "build"]]


def test_prepare_includes_npm_install_when_node_modules_missing(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """A cold checkout (``node_modules`` absent) installs first, then builds — in that order."""
    monkeypatch.setattr(launch, "_VIEWER_DIR", tmp_path)

    (service,) = CollatzLauncher().services()

    assert [list(step) for step in service.prepare] == [
        ["npm", "install"],
        ["npm", "run", "build"],
    ]


def test_main_runs_the_launcher_and_exits_with_its_code(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """``main()`` hands a launcher to :func:`run` and exits with the code it returns."""
    captured: list[Launcher] = []

    def fake_run(launcher: Launcher) -> int:
        captured.append(launcher)
        return 3

    monkeypatch.setattr(launch, "run", fake_run)

    with pytest.raises(SystemExit) as excinfo:
        main()

    assert excinfo.value.code == 3
    assert len(captured) == 1
    assert isinstance(captured[0], CollatzLauncher)


def test_main_exits_zero_on_clean_session(monkeypatch: pytest.MonkeyPatch) -> None:
    """A session that returns ``0`` exits the process with ``0``."""
    monkeypatch.setattr(launch, "run", lambda launcher: 0)

    with pytest.raises(SystemExit) as excinfo:
        main()

    assert excinfo.value.code == 0


def test_main_renders_docker_unavailable_as_a_clean_message(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A :class:`DockerUnavailableError` becomes a string exit message, not a traceback."""

    def fake_run(launcher: Launcher) -> int:
        raise DockerUnavailableError("docker is not running")

    monkeypatch.setattr(launch, "run", fake_run)

    with pytest.raises(SystemExit) as excinfo:
        main()

    assert excinfo.value.code == "docker is not running"
