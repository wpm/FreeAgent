"""Tests for :class:`freeagent.sdk.entity.Entity`'s request/timeout contract.

Under the ack-then-work pattern a request timeout is a *feature*: it bounds how long a caller waits
for an entity to acknowledge before it starts working inside its handler. These pin that the timeout
is explicit and configurable — a constructor-level default, overridable per call — rather than the
0.5 s default inherited from nats-py, which no call path may rely on.
"""

from __future__ import annotations

from collections.abc import Callable

from fixtures import FakeClient, Ping
from freeagent.sdk.entity import DEFAULT_REQUEST_TIMEOUT, UNBOUNDED_TIMEOUT, Entity
from freeagent.sdk.message import Message


async def test_request_passes_the_constructor_default_timeout(
    fake_connect: Callable[[], FakeClient],
) -> None:
    entity = Entity("nats://localhost:4222", "episode-root", timeout=2.5)

    await entity.request("episode-root.somewhere", Ping())

    client = fake_connect()
    assert client.request_timeouts == [2.5]


async def test_request_timeout_defaults_to_the_module_default(
    fake_connect: Callable[[], FakeClient],
) -> None:
    entity = Entity("nats://localhost:4222", "episode-root")

    await entity.request("episode-root.somewhere", Ping())

    client = fake_connect()
    assert client.request_timeouts == [DEFAULT_REQUEST_TIMEOUT]


async def test_request_per_call_timeout_overrides_the_constructor_default(
    fake_connect: Callable[[], FakeClient],
) -> None:
    entity = Entity("nats://localhost:4222", "episode-root", timeout=2.5)

    await entity.request("episode-root.somewhere", Ping(), timeout=0.1)

    client = fake_connect()
    assert client.request_timeouts == [0.1]


async def test_request_with_explicit_none_waits_effectively_unbounded(
    fake_connect: Callable[[], FakeClient],
) -> None:
    # nats-py can't take None; an explicit None means "wait indefinitely", translated at the
    # boundary to a very large finite timeout rather than the entity default.
    entity = Entity("nats://localhost:4222", "episode-root", timeout=2.5)

    await entity.request("episode-root.somewhere", Ping(), timeout=None)

    client = fake_connect()
    assert client.request_timeouts == [UNBOUNDED_TIMEOUT]


async def test_publish_connects_first_when_not_already_connected(
    fake_connect: Callable[[], FakeClient],
) -> None:
    # Like request, publish must connect on demand: an entity that was never started can still
    # publish. This covers the connect-if-None branch.
    entity = Entity("nats://localhost:4222", "episode-root")
    assert entity.client is None

    await entity.publish("episode-root.somewhere", Ping(label="hi"))

    client = fake_connect()
    assert entity.client is not None
    assert [subject for subject, _ in client.published] == ["episode-root.somewhere"]
    ((_, payload),) = client.published
    assert Message.model_validate_json(payload) == Ping(label="hi")


async def test_publish_reuses_an_existing_connection(
    created_clients: list[FakeClient],
) -> None:
    entity = Entity("nats://localhost:4222", "episode-root")
    await entity.start()
    client = created_clients[-1]

    await entity.publish("episode-root.somewhere", Ping())

    # No reconnect: the already-connected client is the one that published.
    assert entity.client is not None
    assert [subject for subject, _ in client.published] == ["episode-root.somewhere"]
    # Only one client was ever created — publish didn't open a second connection.
    assert len(created_clients) == 1
