"""Shared helpers for the freeagent unit tests (no network, no NATS)."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any

from freeagent import Envelope, MemoryTransport

if TYPE_CHECKING:
    from collections.abc import Callable

EPISODE_ID = "ep1"
APP = "demo"
ROOT = f"{APP}.episode.{EPISODE_ID}"


async def wait_for(
    predicate: Callable[[], bool],
    deadline_s: float = 2.0,
    message: str = "condition",
) -> None:
    """Poll *predicate* until true or fail the test after *deadline_s* seconds."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + deadline_s
    while not predicate():
        if loop.time() >= deadline:
            raise AssertionError(f"timed out waiting for {message}")
        await asyncio.sleep(0.005)


def decoded(transport: MemoryTransport) -> list[tuple[str, Envelope]]:
    """The transport's publish history as (subject, Envelope) pairs."""
    return [(subject, Envelope.from_bytes(data)) for subject, data in transport.history]


def published(transport: MemoryTransport, subject: str) -> list[Envelope]:
    """All envelopes published to exactly *subject*, in order."""
    return [envelope for s, envelope in decoded(transport) if s == subject]


async def publish(
    transport: MemoryTransport,
    subject: str,
    sender: str,
    payload: Any,
    episode_id: str = EPISODE_ID,
) -> None:
    """Publish one envelope onto the in-memory wire."""
    envelope = Envelope(episode_id=episode_id, sender=sender, payload=payload)
    await transport.publish(subject, envelope.to_bytes())


async def connected_transport() -> MemoryTransport:
    transport = MemoryTransport()
    await transport.connect()
    return transport
