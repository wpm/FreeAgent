"""Loguru setup helper -- opt-in, for applications and tests only.

The core classes never log through loguru and never call this; per the
project convention, debug logging lives in applications, where each app
decides its own level. This module is the one shared piece: it reads the
``FREEAGENT_LOG_LEVEL`` environment variable (``INFO``, ``DEBUG``, ...) and
points loguru's sink at stderr accordingly. An app or test suite calls
:func:`configure_logging` once at startup; everything that wants debug output
then imports ``loguru.logger`` and logs against it.
"""

from __future__ import annotations

import os
import sys

from loguru import logger

LOG_LEVEL_ENV_VAR = "FREEAGENT_LOG_LEVEL"
DEFAULT_LOG_LEVEL = "WARNING"


def log_level(default: str = DEFAULT_LOG_LEVEL) -> str:
    """The configured level from ``FREEAGENT_LOG_LEVEL`` (upper-cased), or *default*."""
    value = os.environ.get(LOG_LEVEL_ENV_VAR)
    return value.strip().upper() if value and value.strip() else default


def configure_logging(*, level: str | None = None, default: str = DEFAULT_LOG_LEVEL) -> str:
    """Point loguru at stderr at the chosen level; return that level.

    *level*, when given, is used verbatim (upper-cased) -- this is the path a
    CLI uses after resolving its ``--log-level`` option. Otherwise the level
    comes from ``FREEAGENT_LOG_LEVEL``, falling back to *default*. Levels are
    loguru's names (``TRACE``, ``DEBUG``, ``INFO``, ``SUCCESS``, ``WARNING``,
    ``ERROR``, ``CRITICAL``).

    Replaces loguru's default handler so the level (and a terse format) are
    ours. Idempotent enough for app startup: each call resets the single
    stderr sink.

    The sink resolves ``sys.stderr`` at write time rather than binding it now.
    Under pytest's output capture the stderr object is swapped (and the old
    one closed) between tests, so a bound stream would raise "I/O operation on
    closed file"; a late-bound write always lands on the live stream.
    """
    resolved = level.strip().upper() if level else log_level(default)
    logger.remove()
    logger.add(_write_stderr, level=resolved, format=_FORMAT)
    return resolved


def _write_stderr(message: str) -> None:
    """Loguru sink: write to whatever ``sys.stderr`` is current at call time."""
    sys.stderr.write(message)


_FORMAT = (
    "<green>{time:HH:mm:ss.SSS}</green> "
    "<level>{level: <8}</level> "
    "<cyan>{name}</cyan> - <level>{message}</level>"
)
