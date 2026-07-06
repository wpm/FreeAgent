"""Run one episode of an application to completion, as a dumb host.

The worker's job is
deliberately small: given an :class:`~freeagent.sdk.Application` and an
:class:`~freeagent.sdk.EpisodeSpec`, build the episode's one
:class:`~freeagent.sdk.Environment` and its :class:`~freeagent.sdk.Agent` instances, run them until
the episode is over, and tear everything down. All application intelligence stays behind the
:class:`~freeagent.sdk.Application` protocol; the runner here knows nothing about any app's messages
or config.

The one lifecycle fact the runner relies on is an SDK guarantee, not an application one: an
:class:`~freeagent.sdk.Environment` publishes exactly one
:class:`~freeagent.sdk.message.EpisodeComplete` on ``{episode_root}.environment`` as the last
message of an episode (see :meth:`~freeagent.sdk.entity.Environment.stop`). The runner subscribes an
observer to that subject and treats the arrival of :class:`~freeagent.sdk.message.EpisodeComplete`
as the signal the episode is done. Because :class:`~freeagent.sdk.message.EpisodeComplete` is a
plain SDK :class:`~freeagent.sdk.message.Message`, watching for it keeps the runner app-agnostic.

Startup ordering matters: every agent's run loop must be live before the environment broadcasts its
opening :class:`~freeagent.sdk.message.StartEntity`, so the runner starts all agents first, then the
environment. The observer subscribes before either, so the terminal
:class:`~freeagent.sdk.message.EpisodeComplete` cannot be missed by a late subscription.
"""

from __future__ import annotations

import asyncio
import logging

from freeagent.sdk import Application, EpisodeSpec
from freeagent.sdk.entity import ENVIRONMENT, Entity
from freeagent.sdk.message import EpisodeComplete, Message
from nats.aio.msg import Msg

_logger = logging.getLogger(__name__)
"""Logger for teardown failures the runner deliberately swallows on the force-stop path; see
:func:`run_episode`."""


async def _stop_quietly(entity: Entity) -> None:
    """Stop ``entity``, logging and swallowing any error instead of letting it propagate.

    Used on :func:`run_episode`'s force-stop path, where several entities are torn down after an
    episode has already failed. A teardown that raises there -- e.g. an
    :class:`~nats.errors.NoRespondersError` from broadcasting to an agent that has already
    unsubscribed, or an ``unsubscribe``/``close`` erroring over a severed connection -- must not
    abort the remaining teardowns or mask the original failure the caller is about to re-raise.

    :param entity: The entity to stop.
    """
    try:
        await entity.stop()
    except Exception as error:
        _logger.warning("Error stopping %s during force teardown: %r", type(entity).__name__, error)


class _EpisodeObserver(Entity):
    """A passive :class:`~freeagent.sdk.entity.Entity` that waits for the episode's end marker.

    Subscribes to the environment's own ``{episode_root}.environment`` subject and sets an
    :class:`asyncio.Event` the first time an :class:`~freeagent.sdk.message.EpisodeComplete` arrives
    there. It never publishes, drives no game state, and takes part in no lifecycle broadcast -- the
    environment addresses agents, not observers -- so it is invisible to the episode it watches.

    :param servers: NATS server URL(s) to connect to; the same server the episode runs on.
    :param episode_root: The root NATS subject for the episode being observed.
    """

    def __init__(self, servers: str | list[str], episode_root: str) -> None:
        super().__init__(servers, episode_root, ENVIRONMENT)
        self.completed = asyncio.Event()

    async def handle_incoming_message(self, msg: Msg) -> None:
        """Set :attr:`completed` when an :class:`~freeagent.sdk.message.EpisodeComplete` arrives.

        Any other message on the environment subject -- the environment's own lifecycle traffic --
        is ignored; only the terminal end marker matters to an observer.

        :param msg: The NATS message that arrived on the environment subject.
        """
        message = Message.try_decode(msg.data)
        if isinstance(message, EpisodeComplete):
            self.completed.set()


async def run_episode(
    application: Application,
    episode: EpisodeSpec,
    servers: str | list[str],
    *,
    timeout: float | None = None,
) -> None:
    """Build an application's episode from its spec, run it to completion, and tear it down.

    Subscribes an observer to the environment subject, starts every agent (so their run loops are
    live before any command reaches them), then starts the environment -- which broadcasts
    :class:`~freeagent.sdk.message.StartEntity` and drives the episode. The call then blocks until
    the environment publishes its terminal :class:`~freeagent.sdk.message.EpisodeComplete`.

    On the normal completing path the *application* owns teardown: by the time
    :class:`~freeagent.sdk.message.EpisodeComplete` is seen, the environment has already stopped
    every finished agent (each with :class:`~freeagent.sdk.message.StopAgent`) and disconnected
    itself (see :meth:`~freeagent.sdk.entity.Environment.stop`). The only connection the runner
    still owns is its own observer, which it then stops. The runner does *not* re-stop the
    environment or agents on this path: doing so would broadcast
    :class:`~freeagent.sdk.message.StopEntity` at agents that have already unsubscribed, which a
    real server rejects with ``no responders``.

    If instead the wait fails -- the ``timeout`` elapses, or ``start`` raised before completion --
    the episode did *not* end cleanly and no application-driven teardown can be assumed, so the
    runner force-stops every agent and the environment before re-raising, leaving no connection
    stranded. Each stop there is best-effort (see :func:`_stop_quietly`): a teardown error must not
    abort the others or mask the original failure being re-raised. Stopping an agent directly also
    cancels its run loop (see :meth:`~freeagent.sdk.entity.Agent.stop`), so no run-loop task is left
    orphaned.

    :param application: The application to run; its factories build the episode's entities.
    :param episode: The episode specification handed to the application's factories.
    :param servers: NATS server URL(s) the observer connects to; the episode's own entities connect
        to whatever their application built them with.
    :param timeout: Seconds to wait for the episode to complete before giving up, or ``None`` to
        wait indefinitely. A timed-out episode raises :class:`TimeoutError` after teardown.
    :raises TimeoutError: If the episode does not complete within ``timeout`` seconds.
    """
    observer = _EpisodeObserver(servers, episode.episode_root)
    environment = application.make_environment(episode)
    agents = application.make_agents(episode)
    try:
        await observer.start()
        for agent in agents:
            await agent.start()
        await environment.start()
        await asyncio.wait_for(observer.completed.wait(), timeout)
    except BaseException:
        # The episode did not complete cleanly; the application's own teardown can't be relied on,
        # so force everything down. Each stop is best-effort (see _stop_quietly): a teardown that
        # raises -- a NoRespondersError from the environment broadcasting StopEntity at an agent
        # this loop already unsubscribed, or an unsubscribe/close erroring over a severed
        # connection -- must not abort the remaining stops or mask the original failure re-raised
        # below. Each entity's own stop is idempotent, so already-stopped entities are no-ops.
        for agent in agents:
            await _stop_quietly(agent)
        await _stop_quietly(environment)
        raise
    finally:
        # The observer is the runner's own; stop it on every path (its stop is idempotent).
        await _stop_quietly(observer)
