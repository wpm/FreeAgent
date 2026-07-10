"""The one definition of what counts as a valid NATS subject token in Free Agent.

Application names, episode IDs, agent names, and episode roots all become tokens of NATS subjects. A
token containing a ``.`` would splice extra levels into the subject hierarchy; a wildcard character
(``*`` or ``>``) would make one episode's subscription overlap another's; whitespace or control
characters are rejected by the server after entities have already partially started. Every boundary
that accepts such a value — the API's REST models, the worker's command line — validates against
this module, so the definitions cannot drift apart.
"""

from __future__ import annotations

import re

SUBJECT_TOKEN_PATTERN = r"[A-Za-z0-9_-]+"
"""Regex source for a single NATS subject token: letters, digits, hyphen, underscore.

Deliberately narrower than what NATS itself allows, matching the platform's naming conventions. Kept
as a raw pattern (not only a compiled regex) so request models can embed it in their own anchored
``pattern=`` constraints.
"""

_SUBJECT_TOKEN = re.compile(SUBJECT_TOKEN_PATTERN)
"""Compiled :data:`SUBJECT_TOKEN_PATTERN`, for full-match validation."""

_SUBJECT = re.compile(rf"{SUBJECT_TOKEN_PATTERN}(\.{SUBJECT_TOKEN_PATTERN})*")
"""A whole subject: one or more tokens joined by dots, with no wildcards."""


def valid_subject_token(token: str) -> bool:
    """Whether ``token`` is a single valid NATS subject token.

    :param token: The candidate token, e.g. an episode ID or application name.
    :return: Whether the whole string is one token of letters, digits, hyphen, or underscore.
    """
    return _SUBJECT_TOKEN.fullmatch(token) is not None


def valid_subject(subject: str) -> bool:
    """Whether ``subject`` is a valid literal (wildcard-free) NATS subject.

    :param subject: The candidate subject, e.g. an episode root like ``episode.collatz.ep-1``.
    :return: Whether the whole string is dot-joined valid tokens.
    """
    return _SUBJECT.fullmatch(subject) is not None
