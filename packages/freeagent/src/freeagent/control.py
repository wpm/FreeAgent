"""Service -> environment control protocol: the graceful operator abort.

A running episode is the environment's to end: it owns the lifecycle state
machine and the ``shutdown`` broadcast. An external operator -- the control
service, or an in-process supervisor holding an :class:`~freeagent.EpisodeHandle`
-- cannot end an episode *well* by killing the environment process: that skips
the ``shutdown`` broadcast and the grace period, stranding agents mid-goodbye.

Instead the operator sends a **request**. One management message
(``{"type": "freeagent.abort"}``) on the environment's inbox subject asks the
environment to begin its normal ``stopping -> aborted`` path. Control authority
stays with the environment: the message is a request, the broadcast is the
decision, and the environment ignores it unless the episode is running.

**Subject choice.** The request rides the environment's existing inbox,
``<root>.env`` (:attr:`~freeagent.EpisodeSubjects.env`), rather than a dedicated
out-of-band subject. That inbox already carries agent -> environment management
traffic (an agent telling the env "the game is over"), so an operator-abort
*request* is a natural fit beside it. Reusing it keeps the abort inside the
episode's subject root, where the JetStream stream captures it like every other
message -- the wire-is-the-log invariant holds, with no side channel to reason
about separately. Any acks (none are needed here -- the ``shutdown`` broadcast
is the observable acknowledgement) would use episode-scoped reply subjects,
never NATS's ``_INBOX.>``.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from .envelope import Envelope
from .environment import OPERATOR_ABORT_TYPE
from .subjects import EpisodeSubjects, validate_name
from .transport import NatsTransport

if TYPE_CHECKING:
    from .transport import Transport

OPERATOR_SENDER = "operator"
"""Default ``sender`` on an operator-abort request -- distinct from any roster agent."""


def operator_abort_message(reason: str | None = None) -> dict[str, Any]:
    """Build the management payload that asks an environment to abort gracefully."""
    message: dict[str, Any] = {"type": OPERATOR_ABORT_TYPE}
    if reason is not None:
        message["reason"] = reason
    return message


async def publish_operator_abort(
    transport: Transport,
    app: str,
    episode_id: str,
    *,
    reason: str | None = None,
    sender: str = OPERATOR_SENDER,
) -> None:
    """Publish an operator-abort request on one episode's environment inbox.

    Uses an already-connected *transport* (the caller owns its lifetime); the
    request lands on ``<root>.env`` and the environment, if running, broadcasts
    ``shutdown``, runs the grace period, and exits with the aborted code.
    """
    subjects = EpisodeSubjects(app=app, episode_id=episode_id)
    envelope = Envelope(
        episode_id=episode_id,
        sender=validate_name(sender, kind="sender"),
        payload=operator_abort_message(reason),
    )
    await transport.publish(subjects.env, envelope.to_bytes())


async def abort_episode(
    nats_url: str,
    app: str,
    episode_id: str,
    *,
    reason: str | None = None,
    sender: str = OPERATOR_SENDER,
) -> None:
    """Connect to NATS, send an operator-abort request, then disconnect.

    A self-contained one-shot for callers that hold no transport of their own
    (the control service, an in-process supervisor): it does not wait for the
    episode to finish, only for the request to be persisted. The environment
    drives the graceful ``stopping -> aborted`` shutdown from there.
    """
    transport = NatsTransport(nats_url)
    await transport.connect()
    try:
        await publish_operator_abort(transport, app, episode_id, reason=reason, sender=sender)
    finally:
        await transport.close()
