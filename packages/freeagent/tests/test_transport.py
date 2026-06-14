"""Unit tests for NatsTransport's connection error handling.

No server needed: connecting to a closed port fails fast with connection
refused, which the transport must turn into a clear, operator-facing
TransportError rather than nats-py's raw connection traceback.
"""

from __future__ import annotations

import pytest

from freeagent import NatsTransport, TransportError

# A port nothing is listening on -- connect() should fail fast (refused), not
# hang. 4299 is just outside the usual NATS 4222.
DEAD_URL = "nats://127.0.0.1:4299"


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
