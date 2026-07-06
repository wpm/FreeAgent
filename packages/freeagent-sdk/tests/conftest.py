from __future__ import annotations

import hashlib
import sys
from collections.abc import Callable, Iterator
from pathlib import Path

# Under pytest's `importlib` import mode (set repo-wide so same-named test modules across workspace
# members don't collide), pytest no longer prepends each test file's directory to `sys.path`. Put
# this directory back on the path so the bare sibling-helper imports below (`fixtures`,
# `nats_server`, and the tests' `sample_application`) resolve.
sys.path.insert(0, str(Path(__file__).parent))

import pytest
from fixtures import FakeClient
from freeagent.sdk.message import Message
from nats_server import running_nats_server


@pytest.fixture
def isolated_message_registry() -> Iterator[None]:
    """Snapshot and restore :attr:`Message._by_type` around a test.

    Registering a :class:`~freeagent.sdk.message.Message` subclass mutates the process-global
    registry, and defining one (or importing a module that does) inside a test would otherwise leak
    that entry into every later test. Tests that define or import :class:`Message` subclasses use
    this to keep the registry pristine.
    """
    snapshot = dict(Message._by_type)
    try:
        yield
    finally:
        Message._by_type.clear()
        Message._by_type.update(snapshot)


@pytest.fixture
def created_clients(monkeypatch: pytest.MonkeyPatch) -> list[FakeClient]:
    """Patch nats.connect to hand out a FakeClient, and record every one created."""
    created: list[FakeClient] = []

    async def _connect(_: str | list[str], **__: object) -> FakeClient:
        client = FakeClient()
        created.append(client)
        return client

    monkeypatch.setattr("freeagent.sdk.entity.nats.connect", _connect)
    return created


@pytest.fixture
def fake_connect(created_clients: list[FakeClient]) -> Callable[[], FakeClient]:
    """Expose the most recently created FakeClient."""
    return lambda: created_clients[-1]


@pytest.fixture(scope="session")
def nats_server() -> Iterator[str]:
    """Run one real ``nats-server`` subprocess for the whole test session and yield its client URL.

    Session-scoped because ``nats-server`` startup, though fast, is pure overhead to repeat: one
    server serves every integration test, with per-test isolation coming from :func:`episode_root`
    instead (a shared server plus per-test subject roots is deliberate, so that
    cross-test leakage through the subject namespace is *observable* as a failure rather than hidden
    by a fresh server each time).

    :yield: The ``nats://127.0.0.1:{port}`` URL to connect entities to.
    """
    with running_nats_server() as url:
        yield url


@pytest.fixture
def episode_root(request: pytest.FixtureRequest) -> str:
    """Derive a unique episode-root subject for this test from its node id.

    Every integration test roots its entities under a subject unique to that test, so two tests
    sharing the one session server never collide in the subject namespace. The node id is a stable,
    per-test string; NATS subject tokens can't contain the ``.``/``/``/whitespace that node ids
    carry, so those are mapped to ``_`` for a readable subject. Because that mapping is lossy (two
    node ids differing only in punctuation would sanitize identically), a short digest of the raw
    node id is appended so distinct tests always get distinct roots.

    :param request: The active pytest request, whose ``node.nodeid`` names the running test.
    :return: A subject-safe episode root unique to the current test.
    """
    node_id = request.node.nodeid
    readable = "".join(c if c.isalnum() or c in "-_" else "_" for c in node_id)
    digest = hashlib.sha1(node_id.encode(), usedforsecurity=False).hexdigest()[:8]
    return f"episode.{readable}.{digest}"
