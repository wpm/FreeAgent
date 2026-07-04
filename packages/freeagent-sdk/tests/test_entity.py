"""Tests for :class:`freeagent.sdk.entity.Entity`'s request/timeout contract.

Under the ack-then-work pattern a request timeout is a *feature*: it bounds how long a caller waits
for an entity to acknowledge before it starts working inside its handler. These pin that the
timeout is explicit and configurable — a constructor-level default, overridable per call — rather
than nats-py's inherited 0.5 s default, which no call path may rely on.
"""

from __future__ import annotations

from collections.abc import Callable

from fixtures import FakeClient, Ping
from freeagent.sdk.entity import _UNBOUNDED_TIMEOUT, DEFAULT_REQUEST_TIMEOUT, Entity


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
    assert client.request_timeouts == [_UNBOUNDED_TIMEOUT]
