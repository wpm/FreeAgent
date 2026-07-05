"""Test configuration and fixtures for the Free Agent API's tests.

Under pytest's ``importlib`` import mode (set repo-wide so same-named test modules across workspace
members don't collide), pytest no longer prepends each test file's directory to ``sys.path``. This
puts *this* directory back on the path so a test's bare sibling-helper imports
(``api_fakes``) resolve.

It also provides the ``nats_server`` fixture the integration tests need: a real ``nats-server``
subprocess, session-scoped so its (already fast) startup is paid once. The mechanics mirror the
SDK's and worker's own ``nats_server`` fixtures (ADR-0008: integration tests run against a real
server, not a mock), deliberately re-implemented here as a small, self-contained helper rather than
reaching into another package's test directory, keeping this package's test suite independent.
"""

from __future__ import annotations

import shutil
import socket
import subprocess
import sys
import time
from collections.abc import Iterator
from contextlib import closing
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent))

_NATS_SERVER_BINARY = "nats-server"
_STARTUP_TIMEOUT = 10.0
_POLL_INTERVAL = 0.02


def _free_port() -> int:
    """Reserve and return a currently-free loopback TCP port.

    Binds port 0 to let the OS pick a free port, reads it back, and releases it; a tiny race remains
    before ``nats-server`` binds it, which would surface as a startup failure rather than silent
    misbehavior.

    :return: A port number free at the moment it was checked.
    """
    with closing(socket.socket(socket.AF_INET, socket.SOCK_STREAM)) as sock:
        sock.bind(("127.0.0.1", 0))
        port: int = sock.getsockname()[1]
        return port


def _wait_until_accepting(process: subprocess.Popen[bytes], port: int, deadline: float) -> None:
    """Block until ``port`` accepts a connection, or raise if the server dies or ``deadline``
    passes.

    :param process: The ``nats-server`` subprocess being waited on.
    :param port: The loopback port the server should listen on.
    :param deadline: Monotonic-clock time by which the server must be accepting connections.
    :raises RuntimeError: If the server exits before accepting a connection.
    :raises TimeoutError: If the server is not accepting connections by ``deadline``.
    """
    while True:
        exit_code = process.poll()
        if exit_code is not None:
            raise RuntimeError(
                f"{_NATS_SERVER_BINARY} exited with code {exit_code} before accepting connections"
            )
        try:
            with closing(socket.create_connection(("127.0.0.1", port), timeout=_POLL_INTERVAL)):
                return
        except OSError:
            if time.monotonic() >= deadline:
                raise TimeoutError(
                    f"{_NATS_SERVER_BINARY} did not accept connections on port {port} within "
                    f"{_STARTUP_TIMEOUT}s"
                ) from None
            time.sleep(_POLL_INTERVAL)


@pytest.fixture(scope="session")
def nats_server() -> Iterator[str]:
    """Run one real ``nats-server`` subprocess for the whole session and yield its client URL.

    Locates the binary on ``PATH``, launches it bound to loopback on a random free port, waits for
    it to accept connections, and yields the client URL; the subprocess is terminated and reaped on
    exit. Session-scoped because startup is pure overhead to repeat.

    :yield: The ``nats://127.0.0.1:{port}`` URL to connect to.
    :raises FileNotFoundError: If ``nats-server`` is not on ``PATH``.
    """
    binary = shutil.which(_NATS_SERVER_BINARY)
    if binary is None:
        raise FileNotFoundError(
            f"{_NATS_SERVER_BINARY} not found on PATH; install it "
            f"(e.g. `brew install nats-server`) or run only unit tests with "
            f"`pytest -m 'not integration'`"
        )
    port = _free_port()
    process = subprocess.Popen(
        [binary, "-a", "127.0.0.1", "-p", str(port)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        _wait_until_accepting(process, port, time.monotonic() + _STARTUP_TIMEOUT)
        yield f"nats://127.0.0.1:{port}"
    finally:
        process.terminate()
        try:
            process.wait(timeout=_STARTUP_TIMEOUT)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
