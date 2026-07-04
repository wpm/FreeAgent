"""Start and stop a real ``nats-server`` subprocess for integration tests.

The test strategy (ADR-0008) runs integration tests against a *real* NATS server rather than a mock:
a mock ends up reimplementing subject matching and async delivery badly, and ``nats-server`` is a
single static Go binary that starts in milliseconds, so the real thing is cheap enough to be the
only integration dependency.

This module owns the mechanics — locate the binary, pick a free port, launch the subprocess, wait
for it to accept connections, and tear it down — so the pytest fixtures in ``conftest.py`` stay
declarative.
"""

from __future__ import annotations

import shutil
import socket
import subprocess
import time
from collections.abc import Iterator
from contextlib import closing, contextmanager

NATS_SERVER_BINARY = "nats-server"
"""Name of the server binary looked up on ``PATH``.

Installed locally with ``brew install nats-server`` and in CI as a pinned release binary; either
way it must be on ``PATH``.
"""

STARTUP_TIMEOUT = 10.0
"""Seconds to wait for the freshly launched server to start accepting connections before giving up.

``nats-server`` starts in milliseconds; this is a generous ceiling that only matters when something
is wrong (e.g. a loaded CI runner), at which point failing beats hanging.
"""

_POLL_INTERVAL = 0.02
"""Seconds between connection attempts while waiting for the server to come up."""


def _free_port() -> int:
    """Reserve and return a currently-free TCP port on the loopback interface.

    Binds to port 0 to let the OS choose a free port, reads it back, then releases it. This is
    the standard "ask the OS for a free port" trick; a brief race remains between releasing the
    port and ``nats-server`` binding it, but the window is tiny and a collision surfaces as a
    startup failure rather than silent misbehavior.

    :return: A port number that was free at the moment it was checked.
    """
    with closing(socket.socket(socket.AF_INET, socket.SOCK_STREAM)) as sock:
        sock.bind(("127.0.0.1", 0))
        port: int = sock.getsockname()[1]
        return port


def _wait_until_accepting(process: subprocess.Popen[bytes], port: int, deadline: float) -> None:
    """Block until ``port`` accepts a TCP connection, or raise if the server dies or ``deadline``
    passes.

    Checks the subprocess between connection attempts: if ``nats-server`` exits before it starts
    listening — the port was stolen after :func:`_free_port` released it, a bad flag, the port was
    already bound — this fails immediately with the exit code instead of spinning until the full
    startup timeout on a port that will never come up.

    :param process: The ``nats-server`` subprocess being waited on.
    :param port: The loopback port the server is expected to listen on.
    :param deadline: Monotonic-clock time by which the server must be accepting connections.
    :raises RuntimeError: If the server process exits before it accepts a connection.
    :raises TimeoutError: If the server is not accepting connections by ``deadline``.
    """
    while True:
        exit_code = process.poll()
        if exit_code is not None:
            raise RuntimeError(
                f"{NATS_SERVER_BINARY} exited with code {exit_code} before accepting connections "
                f"on port {port}"
            )
        try:
            with closing(socket.create_connection(("127.0.0.1", port), timeout=_POLL_INTERVAL)):
                return
        except OSError:
            if time.monotonic() >= deadline:
                raise TimeoutError(
                    f"{NATS_SERVER_BINARY} did not start accepting connections on port {port} "
                    f"within {STARTUP_TIMEOUT}s"
                ) from None
            time.sleep(_POLL_INTERVAL)


@contextmanager
def running_nats_server() -> Iterator[str]:
    """Run a ``nats-server`` subprocess on a random free port for the duration of the context.

    Locates the binary on ``PATH``, launches it bound to loopback on a random free port, waits for
    it to accept connections, and yields the client URL. On exit the subprocess is terminated and
    reaped, even if the body raised.

    :yield: The ``nats://127.0.0.1:{port}`` URL clients should connect to.
    :raises FileNotFoundError: If ``nats-server`` is not on ``PATH``.
    :raises TimeoutError: If the server does not start within :data:`STARTUP_TIMEOUT`.
    """
    binary = shutil.which(NATS_SERVER_BINARY)
    if binary is None:
        raise FileNotFoundError(
            f"{NATS_SERVER_BINARY} not found on PATH; install it (e.g. `brew install nats-server`) "
            f"or run only unit tests with `pytest -m 'not integration'`"
        )
    port = _free_port()
    # `-a 127.0.0.1` keeps the server off external interfaces; `-p` pins the port we probed.
    process = subprocess.Popen(
        [binary, "-a", "127.0.0.1", "-p", str(port)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        _wait_until_accepting(process, port, time.monotonic() + STARTUP_TIMEOUT)
        yield f"nats://127.0.0.1:{port}"
    finally:
        process.terminate()
        try:
            process.wait(timeout=STARTUP_TIMEOUT)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
