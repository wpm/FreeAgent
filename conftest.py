"""Workspace-root pytest config: wire loguru to FREEAGENT_LOG_LEVEL.

Logging is application-level in FreeAgent, never in the core classes; the test
suite is one such application. Configuring loguru once here means
``FREEAGENT_LOG_LEVEL=DEBUG pytest ...`` surfaces the apps' debug logging
(e.g. Twenty Questions' per-utterance lines) during a test run.
"""

from __future__ import annotations

from freeagent import configure_logging

configure_logging()
