from __future__ import annotations

from collections.abc import Callable, Iterator

import pytest
from fixtures import FakeClient
from freeagent.sdk.message import Message


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
