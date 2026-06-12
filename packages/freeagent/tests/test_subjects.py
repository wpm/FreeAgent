"""Subject layout, stream naming, and name validation."""

from __future__ import annotations

import pytest

from freeagent import EpisodeSubjects, stream_name, subject_root, validate_name


def test_subject_layout_exactly_as_specified() -> None:
    subjects = EpisodeSubjects(app="twentyq", episode_id="ep42")
    assert subjects.root == "twentyq.episode.ep42"
    assert subjects.public == "twentyq.episode.ep42.public"
    assert subjects.agent("alice") == "twentyq.episode.ep42.agent.alice"
    assert subjects.control == "twentyq.episode.ep42.control"
    assert subjects.env == "twentyq.episode.ep42.env"
    assert subjects.reply("req1") == "twentyq.episode.ep42.reply.req1"
    assert subjects.reply_wildcard == "twentyq.episode.ep42.reply.>"
    assert subjects.log("alice") == "twentyq.episode.ep42.log.alice"
    assert subjects.all_subjects == "twentyq.episode.ep42.>"


def test_subject_root_helper() -> None:
    assert subject_root("app", "ep") == "app.episode.ep"


def test_stream_name_uses_underscores() -> None:
    assert stream_name("twentyq", "ep42") == "twentyq_episode_ep42"
    subjects = EpisodeSubjects(app="twentyq", episode_id="ep42")
    assert subjects.stream == "twentyq_episode_ep42"


def test_from_root_round_trips() -> None:
    subjects = EpisodeSubjects.from_root("app.episode.ep")
    assert subjects.app == "app"
    assert subjects.episode_id == "ep"
    assert subjects.root == "app.episode.ep"


@pytest.mark.parametrize("root", ["app.ep", "app.episode", "app.session.ep", "a.episode.b.c"])
def test_from_root_rejects_malformed_roots(root: str) -> None:
    with pytest.raises(ValueError, match="subject root"):
        EpisodeSubjects.from_root(root)


@pytest.mark.parametrize("name", ["alice", "Agent_7", "a-b-c", "0", "A"])
def test_validate_name_accepts_subject_safe_names(name: str) -> None:
    assert validate_name(name) == name


@pytest.mark.parametrize("name", ["", "a b", "a.b", "café", "a/b", "a>", "*"])
def test_validate_name_rejects_unsafe_names(name: str) -> None:
    with pytest.raises(ValueError, match="must match"):
        validate_name(name)
