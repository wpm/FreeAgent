"""Debug logging for Twenty Questions -- application-level, never in core.

Per the project convention, the framework's core classes stay quiet and each
application decides its own logging. Here we want one debug line every time an
agent says something, laid out in 80 columns: the speaker's name centered, the
utterance beneath it (wrapped to the same width), and two blank lines between
one speaker and the next.

Wiring this up is :func:`freeagent.configure_logging` (driven by
``FREEAGENT_LOG_LEVEL``) plus calls to :func:`log_utterance` from the app's
say-paths.
"""

from __future__ import annotations

import textwrap

from loguru import logger

WIDTH = 80


def format_utterance(agent_id: str, message: str) -> str:
    """Render one utterance: centered name, wrapped body, trailing blank lines.

    The two trailing blank lines provide the gap before the next speaker, so
    consecutive utterances read as separate blocks even though each is logged
    on its own.
    """
    name = agent_id.center(WIDTH).rstrip()
    body = "\n".join(
        textwrap.fill(line, width=WIDTH) if line else "" for line in message.splitlines() or [""]
    )
    return f"{name}\n{body}\n\n"


def log_utterance(agent_id: str, message: str) -> None:
    """Emit a DEBUG log for one thing *agent_id* said, formatted in 80 columns."""
    logger.opt(raw=False).debug("\n{}", format_utterance(agent_id, message))
