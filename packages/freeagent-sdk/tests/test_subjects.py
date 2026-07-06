"""Tests for :mod:`freeagent.sdk.subjects`: the one definition of a valid NATS subject token."""

from __future__ import annotations

import pytest
from freeagent.sdk.subjects import valid_subject, valid_subject_token


@pytest.mark.parametrize("token", ["collatz", "ep-1", "under_score", "ABC123", "0"])
def test_valid_subject_tokens_are_accepted(token: str) -> None:
    assert valid_subject_token(token)


@pytest.mark.parametrize(
    "token",
    [
        "",  # empty: would collapse two dots together in a subject
        "a.b",  # a dot makes it two tokens, not one
        "x.*",  # NATS single-level wildcard: would overlap other episodes' subjects
        ">",  # NATS full wildcard
        "has space",
        "émoji",
    ],
)
def test_invalid_subject_tokens_are_rejected(token: str) -> None:
    assert not valid_subject_token(token)


@pytest.mark.parametrize("subject", ["episode", "episode.collatz.ep-1", "a.b_c.d-e"])
def test_valid_dotted_subjects_are_accepted(subject: str) -> None:
    assert valid_subject(subject)


@pytest.mark.parametrize(
    "subject",
    [
        "",
        ".",  # empty tokens on both sides of the dot
        "episode.",  # trailing empty token
        ".episode",  # leading empty token
        "episode..x",  # empty token in the middle
        "episode.x.*",  # wildcard token
        "episode.>",  # full wildcard token
        "episode. x",  # whitespace token
    ],
)
def test_invalid_dotted_subjects_are_rejected(subject: str) -> None:
    assert not valid_subject(subject)
