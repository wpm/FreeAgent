"""Unit tests for NatsTransport's connection error handling.

No server needed: connecting to a closed port fails fast with connection
refused, which the transport must turn into a clear, operator-facing
TransportError rather than nats-py's raw connection traceback.
"""

from __future__ import annotations

import os
from collections.abc import Sequence

import pytest
from nats.errors import TimeoutError as NatsTimeoutError

from freeagent import NatsTransport, TransportError
from freeagent.transport import PulledMessage, _NatsPullSubscription

# A port nothing is listening on -- connect() should fail fast (refused), not
# hang. 4299 is just outside the usual NATS 4222.
DEAD_URL = "nats://127.0.0.1:4299"

_SLOW_TEST_VAR = "FREEAGENT_SLOW_TEST"
_slow = pytest.mark.skipif(
    not os.environ.get(_SLOW_TEST_VAR),
    reason=f"slow test is opt-in: set {_SLOW_TEST_VAR} to run it",
)


@_slow
async def test_connect_to_missing_server_raises_transport_error() -> None:
    transport = NatsTransport(DEAD_URL)
    with pytest.raises(TransportError) as excinfo:
        await transport.connect()
    message = str(excinfo.value)
    # The message names the URL and tells the operator how to start the server.
    assert DEAD_URL in message
    assert "docker compose" in message
    # The original nats-py error is chained for debugging, not shown by default.
    assert excinfo.value.__cause__ is not None


class _RaisingInner:
    """A fake nats-py PullSubscription whose ``fetch`` always times out.

    nats-py raises different timeout types depending on the batch path: the
    ``batch == 1`` path raises ``nats.errors.TimeoutError`` while the batched
    ``_fetch_n`` path raises stdlib ``asyncio.TimeoutError`` (an alias of the
    builtin ``TimeoutError``). The shim must treat both as an ordinary empty poll,
    not a crash.
    """

    def __init__(self, exc: BaseException) -> None:
        self._exc = exc

    async def fetch(self, batch: int, timeout: float) -> Sequence[PulledMessage]:  # noqa: ASYNC109
        raise self._exc


@pytest.mark.parametrize(
    "exc",
    [NatsTimeoutError(), TimeoutError()],
    ids=["nats-timeout", "asyncio-timeout"],
)
async def test_fetch_translates_any_timeout_to_empty_poll(exc: BaseException) -> None:
    """An idle queue must yield an empty list, whichever timeout nats-py raises.

    Regression: the batched fetch path raises ``asyncio.TimeoutError``, which the
    shim did not catch, so an idle worker crashed instead of looping.
    """
    sub = _NatsPullSubscription(_RaisingInner(exc))
    assert await sub.fetch(10, 0.05) == []
