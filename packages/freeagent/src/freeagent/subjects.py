"""NATS subject layout for FreeAgent episodes.

Subject roots are prefixed with the application name, which both namespaces
applications sharing a NATS server and keeps concurrently running episodes
isolated::

    <app>.episode.<id>.public            broadcast channel -- the room
    <app>.episode.<id>.agent.<name>      per-agent inbox
    <app>.episode.<id>.control           lifecycle: start / shutdown (environment -> agents only)
    <app>.episode.<id>.env               environment's inbox (agent mgmt; operator-abort)
    <app>.episode.<id>.reply.<req-id>    replies to environment-originated requests
    <app>.episode.<id>.log.<name>        optional log-only telemetry; no agent subscribes

Application code never touches raw subjects: agents address messages to sets
of agent IDs (default broadcast) and the runtime maps IDs to subjects. This
module is the single place where that mapping lives.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

NAME_PATTERN: re.Pattern[str] = re.compile(r"^[A-Za-z0-9_-]+$")
"""Names usable inside a NATS subject: agent IDs, app names, and episode IDs."""

ENV_NAME: str = "env"
"""Reserved sender name for the environment; no agent may use it."""


def validate_name(name: str, *, kind: str = "name") -> str:
    """Validate a NATS-subject-safe name (``[A-Za-z0-9_-]+``) and return it.

    Raises ``ValueError`` with a message naming *kind* when invalid.
    """
    if not isinstance(name, str) or not NAME_PATTERN.fullmatch(name):
        raise ValueError(f"invalid {kind} {name!r}: must match [A-Za-z0-9_-]+")
    return name


def subject_root(app: str, episode_id: str) -> str:
    """The subject root for one episode of an application: ``<app>.episode.<id>``."""
    validate_name(app, kind="application name")
    validate_name(episode_id, kind="episode id")
    return f"{app}.episode.{episode_id}"


def stream_name(app: str, episode_id: str) -> str:
    """JetStream stream name for an episode: ``<app>_episode_<id>``.

    Dots are not allowed in JetStream stream names, hence the underscores.
    The stream captures ``<app>.episode.<id>.>``.
    """
    validate_name(app, kind="application name")
    validate_name(episode_id, kind="episode id")
    return f"{app}_episode_{episode_id}"


#: The normalized logical channels of an episode subject, in the order they
#: appear after the ``<app>.episode.<id>`` root. ``agent`` and ``reply`` and
#: ``log`` carry a further token (the agent/request/participant name).
CHANNELS: frozenset[str] = frozenset({"public", "control", "env", "reply", "agent", "log"})


def channel_of(subject: str) -> str:
    """The normalized logical channel of an episode *subject*.

    Maps a concrete NATS subject (``<app>.episode.<id>.<channel>[. ...]``) onto
    the channel token a UI cares about (``public``, ``control``, ``env``,
    ``reply``, ``agent``, ``log``), or ``"other"`` for anything that does not fit
    the layout. This is the single place the service turns a wire subject into
    the normalized ``channel`` the feed carries, so the UI never parses subjects.
    """
    parts = subject.split(".")
    if len(parts) < 4 or parts[1] != "episode":
        return "other"
    channel = parts[3]
    return channel if channel in CHANNELS else "other"


@dataclass(frozen=True, slots=True)
class EpisodeSubjects:
    """Helper mapping one episode's logical channels to concrete NATS subjects.

    Used by the runtime only -- application code never sees raw subjects.
    """

    app: str
    episode_id: str

    def __post_init__(self) -> None:
        validate_name(self.app, kind="application name")
        validate_name(self.episode_id, kind="episode id")

    @classmethod
    def from_root(cls, root: str) -> EpisodeSubjects:
        """Parse a ``<app>.episode.<id>`` subject root."""
        parts = root.split(".")
        if len(parts) != 3 or parts[1] != "episode":
            raise ValueError(f"invalid subject root {root!r}: expected '<app>.episode.<id>'")
        return cls(app=parts[0], episode_id=parts[2])

    @property
    def root(self) -> str:
        """The episode's subject root: ``<app>.episode.<id>``."""
        return f"{self.app}.episode.{self.episode_id}"

    @property
    def public(self) -> str:
        """Broadcast channel -- the room."""
        return f"{self.root}.public"

    @property
    def control(self) -> str:
        """Lifecycle subject: ``start`` / ``shutdown``, environment -> agents only."""
        return f"{self.root}.control"

    @property
    def env(self) -> str:
        """The environment's inbox: app-defined agent -> environment messages.

        Also carries the framework's one service -> environment message, the
        operator-abort request (see :mod:`freeagent.control`).
        """
        return f"{self.root}.{ENV_NAME}"

    @property
    def all_subjects(self) -> str:
        """Wildcard covering every subject in the episode: the stream's capture."""
        return f"{self.root}.>"

    @property
    def stream(self) -> str:
        """The episode's JetStream stream name."""
        return stream_name(self.app, self.episode_id)

    def agent(self, name: str) -> str:
        """A specific agent's inbox subject."""
        return f"{self.root}.agent.{validate_name(name, kind='agent id')}"

    def reply(self, request_id: str) -> str:
        """Episode-scoped reply subject for one environment-originated request.

        Replies must use these subjects, never NATS's default ``_INBOX.>`` --
        otherwise replies escape the episode's JetStream stream and break the
        wire-is-the-log invariant.
        """
        return f"{self.root}.reply.{validate_name(request_id, kind='request id')}"

    @property
    def reply_wildcard(self) -> str:
        """Wildcard over all reply subjects (the environment subscribes here)."""
        return f"{self.root}.reply.>"

    def log(self, name: str) -> str:
        """Log-only telemetry subject for one participant; no agent subscribes."""
        return f"{self.root}.log.{validate_name(name, kind='log name')}"
