"""Unit tests for the loguru setup helper.

The key regression: the stderr sink must resolve ``sys.stderr`` at write time,
not bind it at configure time. Pytest's output capture swaps stderr (and closes
the old stream) between tests, so a bound stream raises "I/O operation on closed
file" when an app logs mid-test. A late-bound write always lands on the live
stream.
"""

from __future__ import annotations

import io
import sys

from loguru import logger

from freeagent import configure_logging


def test_log_level_from_env(monkeypatch) -> None:  # noqa: ANN001
    from freeagent import log_level

    monkeypatch.setenv("FREEAGENT_LOG_LEVEL", "debug")
    assert log_level() == "DEBUG"
    monkeypatch.delenv("FREEAGENT_LOG_LEVEL", raising=False)
    assert log_level() == "WARNING"  # the default


def test_sink_writes_to_current_stderr_after_swap_and_close(monkeypatch) -> None:  # noqa: ANN001
    """Reproduces the pytest-capture failure: stderr swapped and the old one closed."""
    monkeypatch.setenv("FREEAGENT_LOG_LEVEL", "DEBUG")
    configure_logging()

    first = io.StringIO()
    monkeypatch.setattr(sys, "stderr", first)
    logger.debug("during-first-stream")

    # Swap to a new stream and close the previous one, exactly as pytest's
    # capture does between tests.
    second = io.StringIO()
    monkeypatch.setattr(sys, "stderr", second)
    first.close()

    # Must not raise ValueError: I/O operation on closed file.
    logger.debug("after-swap-and-close")

    assert "after-swap-and-close" in second.getvalue()

    # Restore loguru to a sane default for the rest of the session.
    configure_logging()
