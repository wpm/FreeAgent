"""Test fixtures for the freeagent unit tests."""

from __future__ import annotations

from typing import TYPE_CHECKING

import pytest

from freeagent import FakeLLM

if TYPE_CHECKING:
    from collections.abc import Iterator


@pytest.fixture(autouse=True)
def reset_fake_llm() -> Iterator[None]:
    """Keep the FakeLLM shared script isolated between tests."""
    FakeLLM.reset()
    yield
    FakeLLM.reset()
