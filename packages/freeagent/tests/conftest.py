"""Test fixtures for the freeagent unit tests."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING

import pytest

from freeagent import FakeLLM

if TYPE_CHECKING:
    from collections.abc import Iterator

NATS_URL = "nats://localhost:4222"
NATS_SKIP_MESSAGE = (
    "NATS is not running; start it with: docker compose -f docker/nats/docker-compose.yml up -d"
)


@pytest.fixture(autouse=True)
def reset_fake_llm() -> Iterator[None]:
    """Keep the FakeLLM shared script isolated between tests."""
    FakeLLM.reset()
    yield
    FakeLLM.reset()


async def _probe_nats(url: str) -> bool:
    """Short-timeout connect probe; True when NATS is reachable."""
    import nats

    try:
        client = await asyncio.wait_for(
            nats.connect(url, connect_timeout=1, max_reconnect_attempts=1), timeout=3
        )
    except Exception:
        return False
    await client.close()
    return True


@pytest.fixture(scope="session")
def nats_url() -> str:
    """The NATS URL, or a clean skip when the server is unreachable."""
    if not asyncio.run(_probe_nats(NATS_URL)):
        pytest.skip(NATS_SKIP_MESSAGE)
    return NATS_URL
